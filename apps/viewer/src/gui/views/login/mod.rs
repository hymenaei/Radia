mod favorites;
mod form;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod scene;
mod spawn;

use std::{
    fmt::{self, Display, Formatter},
    sync::{Arc, Mutex, PoisonError},
};

use gpui::{AppContext, Context, Window};
use gpui_component::{
    IndexPath,
    input::{InputEvent, InputState},
    select::{SelectEvent, SelectState},
};
use network::{AgentSession, Grid, LoginCredentials, MfaChallenge, SessionHandle, StartLocation};

use self::{
    favorites::Favorites,
    spawn::{SpawnChoice, SpawnOption, spawn_options},
};
use crate::gui::views::chat::ChatScreen;
use crate::inventory::{FavoriteLocation, favorite_store};
use crate::{
    gui::window_geometry::observe,
    remembered_login::{self, CredentialStoreError, RememberedLogin},
    session::{
        AuthenticationUpdate, CredentialPersistence, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, Session,
        SessionReadyUpdate, WriteLock, drain_session, write_lock,
    },
};

enum LoginPhase {
    Idle,
    Connecting,
    MfaRequired(MfaChallenge),
    VerifyingMfa,
}

enum LoginMessage {
    LoadingSavedLogin,
    EnterUsername,
    EnterCredentials,
    EnterRegionName,
    MfaPrompt,
    EnterMfaCode,
    Detail(String),
}

impl Display for LoginMessage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadingSavedLogin => formatter.write_str("Loading saved login…"),
            Self::EnterUsername => formatter.write_str("Enter your username."),
            Self::EnterCredentials => formatter.write_str("Enter your username and password."),
            Self::EnterRegionName => formatter.write_str("Enter a region name."),
            Self::MfaPrompt => formatter.write_str("Enter the code from your authenticator."),
            Self::EnterMfaCode => formatter.write_str("Enter your authentication code."),
            Self::Detail(message) => formatter.write_str(message),
        }
    }
}

enum CredentialWarning {
    Deferred(CredentialStoreError),
    Visible(CredentialStoreError),
}

#[derive(Default)]
struct LoginStatus {
    message: Option<LoginMessage>,
    session_failure: Option<String>,
    credential_warning: Option<CredentialWarning>,
    load_warning: Option<CredentialStoreError>,
}

impl LoginStatus {
    fn displayed(&self, favorite_messages: Vec<String>, session_active: bool) -> Option<String> {
        if let Some(message) = &self.message {
            return Some(message.to_string());
        }
        if let Some(failure) = &self.session_failure {
            return Some(failure.clone());
        }

        let credential_warning = match &self.credential_warning {
            Some(CredentialWarning::Visible(warning)) => Some(warning),
            Some(CredentialWarning::Deferred(warning)) if session_active => Some(warning),
            _ => None,
        };
        let mut messages = Vec::new();
        if let Some(warning) = credential_warning {
            messages.push(warning.to_string());
        }
        messages.extend(favorite_messages);
        if !session_active && let Some(warning) = &self.load_warning {
            messages.push(warning.to_string());
        }
        (!messages.is_empty()).then(|| messages.join("\n"))
    }

    fn clear_for_attempt(&mut self) {
        self.message = None;
        self.session_failure = None;
        self.credential_warning = None;
        self.load_warning = None;
    }

    fn begin_grid_change(&mut self) {
        self.message = Some(LoginMessage::LoadingSavedLogin);
        self.session_failure = None;
    }

    fn finish_credential_load(&mut self, warning: Option<CredentialStoreError>) {
        self.message = None;
        self.load_warning = warning;
    }

    fn show_credential_warning(&mut self, warning: Option<CredentialStoreError>, session_active: bool) {
        self.credential_warning =
            warning.map(|warning| if session_active { CredentialWarning::Visible(warning) } else { CredentialWarning::Deferred(warning) });
    }

    fn reveal_credential_warning(&mut self) {
        self.credential_warning = self.credential_warning.take().map(|warning| match warning {
            CredentialWarning::Deferred(warning) => CredentialWarning::Visible(warning),
            warning => warning,
        });
    }
}

pub struct LoginScreen {
    username: gpui::Entity<InputState>,
    password: gpui::Entity<InputState>,
    mfa_code: gpui::Entity<InputState>,
    region_name: gpui::Entity<InputState>,
    spawn_location: gpui::Entity<SelectState<Vec<SpawnOption>>>,
    spawn_choice: SpawnChoice,
    favorites: Favorites,
    storage_loading: bool,
    grid: Grid,
    remember_login: bool,
    remembered_login: Option<RememberedLogin>,
    chat_screen: gpui::Entity<ChatScreen>,
    active_session: Option<ActiveSession>,
    status: LoginStatus,
    login_phase: LoginPhase,
    session_generation: u64,
    credential_load_generation: u64,
    credential_write_lock: WriteLock,
    shutdown_session: Arc<Mutex<Option<SessionHandle>>>,
    _spawn_subscription: gpui::Subscription,
    _username_subscription: gpui::Subscription,
    _window_geometry_subscription: gpui::Subscription,
}

struct ActiveSession {
    _session_handle: SessionHandle,
    _cancel: futures::channel::oneshot::Sender<()>,
    presentation_mailbox: PresentationMailbox,
    ready: Option<ReadySession>,
}

struct ReadySession {
    session: AgentSession,
}

impl ActiveSession {
    fn is_ready(&self) -> bool {
        self.ready.is_some()
    }

    fn ready(&self) -> Option<&ReadySession> {
        self.ready.as_ref()
    }
}

impl LoginScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, shutdown_session: Arc<Mutex<Option<SessionHandle>>>) -> Self {
        let grid = Grid::default();
        let favorite_locations = Vec::new();
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let mfa_code = cx.new(|cx| InputState::new(window, cx).placeholder("Code from your authenticator"));
        let region_name = cx.new(|cx| InputState::new(window, cx).placeholder("Region name"));
        let spawn_location =
            cx.new(|cx| SelectState::new(spawn_options(&favorite_locations), Some(IndexPath::default().row(0)), window, cx));
        let spawn_subscription = cx.subscribe(&spawn_location, |screen, _, event: &SelectEvent<Vec<SpawnOption>>, cx| {
            if let SelectEvent::Confirm(Some(choice)) = event {
                screen.spawn_choice = choice.clone();
                cx.notify();
            }
        });
        let username_subscription = cx.subscribe(&username, |screen, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                screen.refresh_favorites(cx);
            }
        });
        let window_geometry_subscription = observe("login", window, cx);
        let chat_screen = cx.new(|cx| ChatScreen::new(window, cx));

        let screen = Self {
            username,
            password,
            mfa_code,
            region_name,
            spawn_location,
            spawn_choice: SpawnChoice::Last,
            favorites: Favorites::new(),
            storage_loading: true,
            grid,
            remember_login: false,
            remembered_login: None,
            chat_screen,
            active_session: None,
            status: LoginStatus { message: Some(LoginMessage::LoadingSavedLogin), ..LoginStatus::default() },
            login_phase: LoginPhase::Idle,
            session_generation: 0,
            credential_load_generation: 0,
            credential_write_lock: write_lock(),
            shutdown_session,
            _spawn_subscription: spawn_subscription,
            _username_subscription: username_subscription,
            _window_geometry_subscription: window_geometry_subscription,
        };

        let background = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let (login, favorite_store) = background.spawn(async move { (remembered_login::load(grid), favorite_store::load()) }).await;
            if this
                .update_in(cx, |screen, window, cx| {
                    let (login, login_warning) = match login {
                        Ok(login) => (login, None),
                        Err(error) => (None, Some(error)),
                    };
                    screen.favorites.load(favorite_store);
                    let username = login.as_ref().map(|login| login.username.as_str()).unwrap_or("");
                    let favorites = screen.favorites.for_username(grid, username);
                    screen.set_remembered_login(login, favorites, window, cx);
                    screen.storage_loading = false;
                    screen.status.finish_credential_load(login_warning);
                    cx.notify();
                })
                .is_err()
            {
                tracing::trace!("login screen was already unavailable after loading saved credentials");
            }
        })
        .detach();

        screen
    }

    fn select_grid(&mut self, grid: Grid, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_loading || self.grid == grid {
            return;
        }

        self.grid = grid;
        self.credential_load_generation = self.credential_load_generation.wrapping_add(1);
        let generation = self.credential_load_generation;
        self.storage_loading = true;
        self.status.begin_grid_change();
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let login = background.spawn(async move { remembered_login::load(grid) }).await;
            if this
                .update_in(cx, |screen, window, cx| {
                    if screen.grid != grid || screen.credential_load_generation != generation {
                        return;
                    }

                    let (login, login_warning) = match login {
                        Ok(login) => (login, None),
                        Err(error) => (None, Some(error)),
                    };
                    let username = login.as_ref().map(|login| login.username.as_str()).unwrap_or("");
                    let favorites = screen.favorites.for_username(grid, username);
                    screen.set_remembered_login(login, favorites, window, cx);
                    screen.storage_loading = false;
                    screen.status.finish_credential_load(login_warning);
                    cx.notify();
                })
                .is_err()
            {
                tracing::trace!("login screen was already unavailable after changing grids");
            }
        })
        .detach();
    }

    fn refresh_favorites(&mut self, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value();
        if self.favorites.refresh_for_username(self.grid, &username) {
            cx.notify();
        }
    }

    fn set_remembered_login(
        &mut self,
        login: Option<RememberedLogin>,
        favorites: Vec<FavoriteLocation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let username = login.as_ref().map(|login| login.username.as_str()).unwrap_or("").to_owned();
        let password = login.as_ref().map(|login| login.password.as_str()).unwrap_or("").to_owned();
        let options = spawn_options(&favorites);

        self.username.update(cx, |input, cx| input.set_value(username, window, cx));
        self.password.update(cx, |input, cx| input.set_value(password, window, cx));
        self.spawn_location.update(cx, |state, cx| {
            state.set_items(options, window, cx);
            state.set_selected_value(&SpawnChoice::Last, window, cx);
        });
        self.spawn_choice = SpawnChoice::Last;
        self.favorites.set_locations(favorites);
        self.remember_login = login.is_some();
        self.remembered_login = login;
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_loading {
            return;
        }

        match self.login_phase {
            LoginPhase::Idle => {}
            LoginPhase::MfaRequired(_) => {
                self.submit_mfa_code(window, cx);
                return;
            }
            LoginPhase::Connecting | LoginPhase::VerifyingMfa => return,
        }

        self.submit_credentials(cx);
    }

    fn submit_credentials(&mut self, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value();
        let password = self.password.read(cx).value();

        let Some((first_name, last_name)) = split_username(&username) else {
            self.status.message = Some(LoginMessage::EnterUsername);
            cx.notify();
            return;
        };

        if password.is_empty() {
            self.status.message = Some(LoginMessage::EnterCredentials);
            cx.notify();
            return;
        }

        let Some(start_location) = self.start_location(cx) else {
            return;
        };
        let remembered_for_user =
            self.remembered_login.as_ref().filter(|saved| saved.username.eq_ignore_ascii_case(username.trim())).cloned();
        let credentials = LoginCredentials {
            first_name,
            last_name,
            password: password.to_string(),
            grid: self.grid,
            start_location,
            mfa_hash: remembered_for_user.as_ref().map(|saved| saved.mfa_hash.clone()).unwrap_or_default(),
        };
        self.start_session(credentials, username.trim().to_owned(), password.to_string(), cx);
    }

    fn start_location(&mut self, cx: &mut Context<Self>) -> Option<StartLocation> {
        Some(match &self.spawn_choice {
            SpawnChoice::Last => StartLocation::Last,
            SpawnChoice::Home => StartLocation::Home,
            SpawnChoice::Region => {
                let name = self.region_name.read(cx).value();
                if name.trim().is_empty() {
                    self.status.message = Some(LoginMessage::EnterRegionName);
                    cx.notify();
                    return None;
                }
                StartLocation::Region { name: name.trim().to_owned(), x: 128, y: 128, z: 0 }
            }
            SpawnChoice::Favorite(favorite) => {
                StartLocation::Region { name: favorite.region_name.clone(), x: favorite.x, y: favorite.y, z: favorite.z }
            }
        })
    }

    fn prepare_session_attempt(&mut self, cx: &mut Context<Self>) {
        self.active_session = None;
        *self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.session_generation = self.session_generation.wrapping_add(1);
        self.chat_screen.update(cx, |chat, cx| chat.clear_session(cx));
        self.login_phase = LoginPhase::Connecting;
        self.status.clear_for_attempt();
    }

    fn start_session(
        &mut self,
        credentials: LoginCredentials,
        remembered_username: String,
        remembered_password: String,
        cx: &mut Context<Self>,
    ) {
        self.prepare_session_attempt(cx);
        let remember_login = self.remember_login;
        let network_session = match Session::start(credentials).map_err(|error| error.to_string()) {
            Ok(session) => session,
            Err(error) => {
                self.login_phase = LoginPhase::Idle;
                self.status.session_failure = Some(error);
                cx.notify();
                return;
            }
        };
        let session_handle = network_session.handle();
        let (mailbox, mut presentation) = PresentationMailbox::new(self.session_generation);
        let (cancel_sender, cancel_receiver) = futures::channel::oneshot::channel();
        let active = ActiveSession {
            _session_handle: session_handle.clone(),
            _cancel: cancel_sender,
            presentation_mailbox: mailbox.clone(),
            ready: None,
        };
        *self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner) = Some(session_handle.clone());
        self.active_session = Some(active);
        self.chat_screen.update(cx, |chat, cx| chat.set_session_handle(session_handle.clone(), cx));

        let background = crate::gui::service_runtime(cx);
        let credential_write_lock = self.credential_write_lock.clone();
        let grid = self.grid;

        cx.spawn(async move |this, cx| {
            while let Some(snapshot) = presentation.next().await {
                if snapshot.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |view, cx| {
                        if view.session_generation == snapshot.generation {
                            view.apply_presentation(snapshot, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        drop(background.spawn(drain_session(
            network_session,
            session_handle,
            background.clone(),
            mailbox,
            cancel_receiver,
            CredentialPersistence {
                grid,
                username: remembered_username,
                password: remembered_password,
                remember_login,
                write_lock: credential_write_lock,
            },
        )));

        cx.notify();
    }

    fn apply_presentation(&mut self, snapshot: PresentationSnapshot, cx: &mut Context<Self>) {
        let PresentationSnapshot {
            authentication,
            session_ready,
            region_name,
            chat_messages,
            failure,
            favorites,
            favorite_store_saved,
            ..
        } = snapshot;

        self.apply_authentication(authentication);
        if let Some(ready) = session_ready {
            self.apply_session_ready(ready, cx);
        }

        if let Some(region_name) = region_name {
            self.chat_screen.update(cx, |chat, cx| chat.set_region_name(region_name, cx));
        }

        if !chat_messages.is_empty() {
            self.chat_screen.update(cx, |chat, cx| chat.add_chat_messages(chat_messages, cx));
        }

        if let Some(update) = favorites {
            self.apply_favorites(update, cx);
        }

        self.favorites.apply_saved(favorite_store_saved);

        if let Some(error) = failure {
            self.end_session(error, cx);
        }

        cx.notify();
    }

    fn apply_authentication(&mut self, update: Option<AuthenticationUpdate>) {
        match update {
            Some(AuthenticationUpdate::MfaRequired(challenge)) => {
                self.login_phase = LoginPhase::MfaRequired(challenge);
                self.status.message = Some(LoginMessage::MfaPrompt);
            }
            Some(AuthenticationUpdate::CredentialsUpdated { remembered_login, warning }) => {
                if let Some(login) = remembered_login {
                    self.remembered_login = Some(login);
                } else if warning.is_none() {
                    self.remembered_login = None;
                }
                let session_active = self.active_session.as_ref().is_some_and(ActiveSession::is_ready);
                self.status.show_credential_warning(warning, session_active);
            }
            None => {}
        }
    }

    fn apply_session_ready(&mut self, ready: SessionReadyUpdate, cx: &mut Context<Self>) {
        let session = ready.session;
        let region_name = ready.region_name;
        let Some(active) = self.active_session.as_ref() else { return };
        let mailbox = active.presentation_mailbox.clone();
        self.login_phase = LoginPhase::Idle;
        self.status.message = None;
        self.status.reveal_credential_warning();
        tracing::info!(grid = %session.grid.label(), simulator = %session.simulator_address, "connected to simulator");

        let username = self.username.read(cx).value();
        self.favorites.prepare_session(session.grid, session.agent_id, &username, Some(mailbox), cx);
        if let Some(active) = &mut self.active_session {
            active.ready = Some(ReadySession { session: session.clone() });
        }
        self.chat_screen.update(cx, |chat, cx| {
            chat.set_session(session.clone(), cx);
            if let Some(region_name) = region_name {
                chat.set_region_name(region_name, cx);
            }
        });

        #[cfg(any(target_os = "windows", target_os = "linux"))]
        crate::gui::scene::start_session(
            self.session_generation,
            &ready.world_reader,
            ready.environments,
            &ready.assets,
            session.agent_id,
            ready.camera,
            cx,
        );
    }

    pub(crate) fn end_session(&mut self, error: String, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        crate::gui::scene::end_session(self.session_generation, cx);
        tracing::warn!(%error, "viewer session ended after a failure");
        self.login_phase = LoginPhase::Idle;
        self.status.message = None;
        self.status.session_failure = Some(error);
        self.active_session = None;
        self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner).take();
        self.session_generation = self.session_generation.wrapping_add(1);
        self.chat_screen.update(cx, |chat, cx| chat.clear_session(cx));
        cx.notify();
    }

    fn apply_favorites(&mut self, update: FavoritesUpdate, cx: &mut Context<Self>) {
        let Some(active) = self.active_session.as_ref() else { return };
        if !active.ready().is_some_and(|ready| {
            let session = &ready.session;
            session.agent_id == update.agent_id && session.session_id == update.session_id && session.grid == update.grid
        }) {
            return;
        }
        let mailbox = Some(active.presentation_mailbox.clone());
        self.favorites.apply_update(update, mailbox, cx);
    }

    fn submit_mfa_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.mfa_code.read(cx).value();
        if code.trim().is_empty() {
            self.status.message = Some(LoginMessage::EnterMfaCode);
            cx.notify();
            return;
        }

        let LoginPhase::MfaRequired(challenge) = &self.login_phase else { return };

        self.status.message = None;
        let result = challenge.complete(&code);
        self.mfa_code.update(cx, |input, cx| input.set_value("", window, cx));
        match result {
            Ok(()) => self.login_phase = LoginPhase::VerifyingMfa,
            Err(error) => {
                self.status.message = Some(LoginMessage::Detail(error.to_string()));
            }
        }
        cx.notify();
    }
}

fn split_username(username: &str) -> Option<(String, String)> {
    let mut names = username.split_whitespace();
    let first_name = names.next()?.to_owned();
    let last_name = names.collect::<Vec<_>>().join(" ");
    let last_name = if last_name.is_empty() { "Resident".to_owned() } else { last_name };

    Some((first_name, last_name))
}

#[cfg(test)]
mod tests {
    use super::LoginStatus;
    use crate::remembered_login::CredentialStoreError;

    fn credential_error(detail: &str) -> CredentialStoreError {
        CredentialStoreError::Open(detail.to_owned())
    }

    #[test]
    fn visible_credential_warning_survives_reveal() {
        let warning = credential_error("keyring unavailable");
        let mut status = LoginStatus::default();
        status.show_credential_warning(Some(warning.clone()), true);

        status.reveal_credential_warning();

        assert_eq!(status.displayed(Vec::new(), true), Some(warning.to_string()));
    }

    #[test]
    fn grid_change_replaces_session_failure() {
        let mut status = LoginStatus { session_failure: Some("previous login failed".into()), ..LoginStatus::default() };

        status.begin_grid_change();

        assert_eq!(status.displayed(Vec::new(), false), Some("Loading saved login…".into()));

        let warning = credential_error("selected grid credential read failed");
        status.finish_credential_load(Some(warning.clone()));

        assert_eq!(status.displayed(Vec::new(), false), Some(warning.to_string()));
    }
}
