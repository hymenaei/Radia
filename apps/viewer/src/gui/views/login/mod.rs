mod spawn;

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};
#[cfg(target_os = "linux")]
use gpui_component::StyledExt;
use gpui_component::{
    Disableable, Icon, IconName, IndexPath, Theme,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
};
use network::{AgentSession, Grid, LoginCredentials, MfaChallenge, SessionHandle, StartLocation};

use self::spawn::{SpawnChoice, SpawnOption, favorites_for_username, spawn_options};
#[cfg(target_os = "linux")]
use crate::gui::components::window_chrome::linux_titlebar;
#[cfg(target_os = "windows")]
use crate::gui::components::window_chrome::windows_titlebar;
use crate::gui::views::chat::ChatScreen;
use crate::inventory::{
    FavoriteLocation,
    favorite_store::{self, FavoriteStore, FavoriteStoreError},
};
use crate::{
    asset_coordinator::AssetHandle,
    gui::window_geometry::observe,
    remembered_login::{self, CredentialStoreError, RememberedLogin},
    session::{
        AuthenticationUpdate, CredentialPersistence, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, RegionLookup, Session,
        SessionReadyUpdate, ViewerUpdate, WriteLock, drain_session, save_favorite_store, write_lock,
    },
    world_reader::WorldReader,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum FavoriteWarning {
    Storage(FavoriteStoreError),
    Save(FavoriteStoreError),
    Refresh(FavoriteStoreError),
}

impl std::fmt::Display for FavoriteWarning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => std::fmt::Display::fmt(error, formatter),
            Self::Save(error) => write!(formatter, "Could not save favorite landmarks: {error}"),
            Self::Refresh(error) => write!(formatter, "Could not refresh favorite landmarks: {error}"),
        }
    }
}

enum LoginPhase {
    Idle,
    Connecting,
    MfaRequired(MfaChallenge),
    VerifyingMfa,
}

pub struct LoginScreen {
    username: gpui::Entity<InputState>,
    password: gpui::Entity<InputState>,
    mfa_code: gpui::Entity<InputState>,
    region_name: gpui::Entity<InputState>,
    spawn_location: gpui::Entity<SelectState<Vec<SpawnOption>>>,
    spawn_choice: SpawnChoice,
    favorite_locations: Vec<FavoriteLocation>,
    displayed_favorites: Vec<FavoriteLocation>,
    favorite_store: FavoriteStore,
    favorite_storage_warning: Option<FavoriteWarning>,
    favorite_refresh_warning: Option<FavoriteWarning>,
    favorite_store_loaded: bool,
    storage_loading: bool,
    grid: Grid,
    remember_login: bool,
    remembered_login: Option<RememberedLogin>,
    chat_screen: gpui::Entity<ChatScreen>,
    session_handle: Option<SessionHandle>,
    world_reader: Option<WorldReader>,
    assets: Option<AssetHandle>,
    status: Option<String>,
    session: Option<AgentSession>,
    login_phase: LoginPhase,
    credential_warning: Option<CredentialStoreError>,
    current_region_name: Option<String>,
    region_lookup: Option<RegionLookup>,
    session_generation: u64,
    credential_load_generation: u64,
    session_cancel: Option<futures::channel::oneshot::Sender<()>>,
    presentation_mailbox: Option<PresentationMailbox>,
    credential_write_lock: WriteLock,
    favorite_store_write_lock: WriteLock,
    favorite_store_save_generation: Arc<AtomicU64>,
    shutdown_session: Arc<Mutex<Option<SessionHandle>>>,
    _spawn_subscription: gpui::Subscription,
    _username_subscription: gpui::Subscription,
    _window_geometry_subscription: gpui::Subscription,
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
            displayed_favorites: favorite_locations.clone(),
            favorite_locations,
            favorite_store: FavoriteStore::default(),
            favorite_storage_warning: None,
            favorite_refresh_warning: None,
            favorite_store_loaded: false,
            storage_loading: true,
            grid,
            remember_login: false,
            remembered_login: None,
            chat_screen,
            session_handle: None,
            world_reader: None,
            assets: None,
            status: Some("Loading saved login…".into()),
            session: None,
            login_phase: LoginPhase::Idle,
            credential_warning: None,
            current_region_name: None,
            region_lookup: None,
            session_generation: 0,
            credential_load_generation: 0,
            session_cancel: None,
            presentation_mailbox: None,
            credential_write_lock: write_lock(),
            favorite_store_write_lock: write_lock(),
            favorite_store_save_generation: Arc::new(AtomicU64::new(0)),
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
                    let (favorite_store, favorite_warning) = match favorite_store {
                        Ok(favorites) => (favorites, None),
                        Err(error) => (FavoriteStore::default(), Some(error)),
                    };
                    let username = login.as_ref().map(|login| login.username.as_str()).unwrap_or("");
                    let favorites = favorites_for_username(&favorite_store, grid, username);
                    let favorite_storage_status = favorite_warning.as_ref().map(ToString::to_string);

                    screen.favorite_store = favorite_store;
                    screen.favorite_store_loaded = favorite_warning.is_none();
                    screen.favorite_storage_warning = favorite_warning.map(FavoriteWarning::Storage);
                    screen.set_remembered_login(login, favorites, window, cx);
                    screen.storage_loading = false;
                    screen.status = login_warning.map(|warning| warning.to_string()).or(favorite_storage_status);
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
        self.status = Some("Loading saved login…".into());
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
                    let favorites = favorites_for_username(&screen.favorite_store, grid, username);
                    screen.set_remembered_login(login, favorites, window, cx);
                    screen.storage_loading = false;
                    screen.status = login_warning
                        .map(|warning| warning.to_string())
                        .or_else(|| screen.favorite_storage_warning.as_ref().map(ToString::to_string))
                        .or_else(|| screen.favorite_refresh_warning.as_ref().map(ToString::to_string));
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
        let favorites = favorites_for_username(&self.favorite_store, self.grid, &username);
        if self.favorite_locations != favorites {
            self.favorite_locations = favorites;
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
        self.displayed_favorites = favorites.clone();
        self.favorite_locations = favorites;
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
            self.status = Some("Enter your username.".into());
            cx.notify();
            return;
        };

        if password.is_empty() {
            self.status = Some("Enter your username and password.".into());
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
                    self.status = Some("Enter a region name.".into());
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
        if let Some(cancel) = self.session_cancel.take()
            && cancel.send(()).is_err()
        {
            tracing::trace!("previous login task was already stopped");
        }
        self.session_generation = self.session_generation.wrapping_add(1);
        self.presentation_mailbox = None;
        self.chat_screen.update(cx, |chat, cx| chat.clear_session(cx));
        self.session = None;
        self.login_phase = LoginPhase::Connecting;
        self.session_handle = None;
        *self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.current_region_name = None;
        self.region_lookup = None;
        self.status = None;
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
                self.status = Some(error);
                cx.notify();
                return;
            }
        };
        let session_handle = network_session.handle();
        self.session_handle = Some(session_handle.clone());
        *self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner) = Some(session_handle.clone());
        self.chat_screen.update(cx, |chat, cx| chat.set_session_handle(session_handle.clone(), cx));

        let generation = self.session_generation;
        let (mailbox, mut presentation) = PresentationMailbox::new(generation);
        self.presentation_mailbox = Some(mailbox.clone());
        let (cancel_sender, cancel_receiver) = futures::channel::oneshot::channel();
        self.session_cancel = Some(cancel_sender);
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
            self.current_region_name = Some(region_name.clone());
            self.chat_screen.update(cx, |chat, cx| chat.set_region_name(region_name, cx));
        }

        if !chat_messages.is_empty() {
            self.chat_screen.update(cx, |chat, cx| chat.add_chat_messages(chat_messages, cx));
        }

        if let Some(update) = favorites {
            self.apply_favorites(update, cx);
        }

        self.apply_favorite_store_saved(favorite_store_saved);

        if let Some(error) = failure {
            self.end_session(error, cx);
        }

        cx.notify();
    }

    fn apply_authentication(&mut self, update: Option<AuthenticationUpdate>) {
        match update {
            Some(AuthenticationUpdate::MfaRequired(challenge)) => {
                self.login_phase = LoginPhase::MfaRequired(challenge);
                self.status = Some("Enter the code from your authenticator.".into());
            }
            Some(AuthenticationUpdate::CredentialsUpdated { remembered_login, warning }) => {
                if let Some(login) = remembered_login {
                    self.remembered_login = Some(login);
                } else if warning.is_none() {
                    self.remembered_login = None;
                }
                if let Some(message) = warning.as_ref().map(ToString::to_string)
                    && self.session.is_some()
                {
                    let status = self.status.take().unwrap_or_default();
                    self.status = Some(format!("{status}\n{message}"));
                }
                self.credential_warning = warning;
            }
            None => {}
        }
    }

    fn apply_session_ready(&mut self, ready: SessionReadyUpdate, cx: &mut Context<Self>) {
        let SessionReadyUpdate { session, region_lookup, region_name, world_reader, assets, camera, environments } = ready;
        self.login_phase = LoginPhase::Idle;
        tracing::info!(grid = %session.grid.label(), simulator = %session.simulator_address, "connected to simulator");

        let mut warnings = self.credential_warning.take().map(|warning| warning.to_string()).into_iter().collect::<Vec<_>>();
        let identity_changed = self.favorite_store.remember_agent(session.grid, session.agent_id, &self.username.read(cx).value());
        self.favorite_locations = self.favorite_store.favorites_for(session.grid, session.agent_id).to_vec();
        if identity_changed && self.favorite_store_loaded {
            self.favorite_storage_warning = None;
            self.schedule_favorite_store_save(self.favorite_store.clone(), cx);
        }
        self.favorite_refresh_warning = None;
        if let Some(warning) = &self.favorite_storage_warning {
            warnings.push(warning.to_string());
        }
        self.status = (!warnings.is_empty()).then(|| warnings.join("\n"));
        self.region_lookup = Some(region_lookup);
        self.session = Some(session.clone());
        self.current_region_name = region_name;
        let initial_region_name = self.current_region_name.clone();
        self.chat_screen.update(cx, |chat, cx| {
            chat.set_session(session.clone(), cx);
            if let Some(region_name) = initial_region_name {
                chat.set_region_name(region_name, cx);
            }
        });

        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if crate::gui::start_scene(self.session_generation, cx) {
            crate::gui::connect_world(
                self.session_generation,
                crate::scene_feed::SceneSources { reader: world_reader.clone(), environments, assets: assets.clone() },
                session.agent_id,
                camera,
                cx,
            );
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let _ = (camera, environments);

        self.assets = Some(assets);
        self.world_reader = Some(world_reader);
    }

    fn apply_favorite_store_saved(&mut self, saved: Option<(u64, Result<(), FavoriteStoreError>)>) {
        let Some((generation, result)) = saved else {
            return;
        };
        if generation != self.favorite_store_save_generation.load(Ordering::Acquire) {
            return;
        }

        match result {
            Ok(()) => {
                let current_warning = self.favorite_storage_warning.as_ref().map(ToString::to_string);
                if current_warning.as_ref().is_some_and(|warning| self.status.as_ref() == Some(warning)) {
                    self.status = None;
                }
                self.favorite_storage_warning = None;
            }
            Err(error) => {
                tracing::warn!(%error, "could not save favorite landmarks");
                let warning = FavoriteWarning::Save(error);
                if self.status.is_none() {
                    self.status = Some(warning.to_string());
                }
                self.favorite_storage_warning = Some(warning);
            }
        }
    }

    fn end_session(&mut self, error: String, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        crate::gui::end_scene(self.session_generation, cx);
        tracing::warn!(%error, "viewer session ended after a failure");
        self.login_phase = LoginPhase::Idle;
        self.session = None;
        self.session_handle = None;
        self.world_reader = None;
        self.assets = None;
        self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner).take();
        self.region_lookup = None;
        self.current_region_name = None;
        self.session_cancel = None;
        self.presentation_mailbox = None;
        self.session_generation = self.session_generation.wrapping_add(1);
        self.status = Some(error);
        self.chat_screen.update(cx, |chat, cx| chat.clear_session(cx));
        cx.notify();
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn scene_failed(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::error!(%error, "scene renderer stopped");
        self.end_session("Unable to display the world.".into(), cx);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._window_geometry_subscription = observe("login", window, cx);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn chat_view(&self) -> gpui::Entity<ChatScreen> {
        self.chat_screen.clone()
    }

    fn apply_favorites(&mut self, update: FavoritesUpdate, cx: &mut Context<Self>) {
        if !self.session.as_ref().is_some_and(|session| {
            session.agent_id == update.agent_id && session.session_id == update.session_id && session.grid == update.grid
        }) {
            return;
        }

        if let Err(error) = favorite_store::apply_favorites_refresh(
            &mut self.favorite_locations,
            &mut self.favorite_store,
            update.grid,
            update.agent_id,
            &update.username,
            update.result,
        ) {
            tracing::warn!(%error, "could not apply favorite inventory refresh");
            let warning = FavoriteWarning::Refresh(error);
            self.status = Some(warning.to_string());
            self.favorite_refresh_warning = Some(warning);
            return;
        }

        let warning_text = self.favorite_refresh_warning.as_ref().map(ToString::to_string);
        if warning_text.as_ref().is_some_and(|warning| self.status.as_ref() == Some(warning)) {
            self.status = None;
        }
        self.favorite_refresh_warning = None;
        if self.favorite_store_loaded {
            self.favorite_storage_warning = None;
            self.schedule_favorite_store_save(self.favorite_store.clone(), cx);
        }
    }

    fn schedule_favorite_store_save(&self, store: FavoriteStore, cx: &mut Context<Self>) {
        let Some(mailbox) = self.presentation_mailbox.clone() else {
            return;
        };
        let write_lock = self.favorite_store_write_lock.clone();
        let save_generation = Arc::clone(&self.favorite_store_save_generation);
        let generation = save_generation.fetch_add(1, Ordering::AcqRel) + 1;
        cx.background_executor()
            .spawn(async move {
                if let Some(result) = save_favorite_store(store, write_lock, save_generation, generation).await {
                    mailbox.publish(ViewerUpdate::FavoriteStoreSaved { generation, result });
                }
            })
            .detach();
    }

    fn submit_mfa_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.mfa_code.read(cx).value();
        if code.trim().is_empty() {
            self.status = Some("Enter your authentication code.".into());
            cx.notify();
            return;
        }

        let LoginPhase::MfaRequired(challenge) = &self.login_phase else { return };

        self.status = None;
        let result = challenge.complete(&code);
        self.mfa_code.update(cx, |input, cx| input.set_value("", window, cx));
        match result {
            Ok(()) => self.login_phase = LoginPhase::VerifyingMfa,
            Err(error) => {
                self.status = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn render_heading(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        div().flex().items_center().gap_3().child(Icon::new(IconName::Globe).size_5().text_color(theme.primary)).child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(22.)).text_color(theme.foreground).child("Sign in"))
                .child(div().text_size(px(13.)).text_color(theme.muted_foreground).child("Connect to your Second Life account.")),
        )
    }

    fn render_credentials_form(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Username"))
                    .child(Input::new(&self.username).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Password"))
                    .child(Input::new(&self.password).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
            )
    }

    fn render_mfa_prompt(&self, show_mfa: bool, connecting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div().when(show_mfa, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Authentication code"))
                    .child(Input::new(&self.mfa_code).w_full().h(px(42.)).focus_bordered(false).disabled(connecting)),
            )
        })
    }

    fn render_spawn_selection(&self, form_disabled: bool, show_region_input: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_size(px(13.)).text_color(foreground).child("Start at"))
            .child(Select::new(&self.spawn_location).w_full().h(px(40.)).disabled(form_disabled))
            .when(show_region_input, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(13.)).text_color(foreground).child("Region name"))
                        .child(Input::new(&self.region_name).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
                )
            })
    }

    fn render_grid_selection(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div().flex().items_center().justify_between().child(div().text_size(px(13.)).text_color(foreground).child("Grid")).child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("grid-agni")
                        .label("Main grid")
                        .with_variant(if self.grid == Grid::Agni { ButtonVariant::Primary } else { ButtonVariant::Secondary })
                        .h(px(34.))
                        .disabled(form_disabled)
                        .on_click(cx.listener(|this, _event, window, cx| this.select_grid(Grid::Agni, window, cx))),
                )
                .child(
                    Button::new("grid-aditi")
                        .label("Beta grid")
                        .with_variant(if self.grid == Grid::Aditi { ButtonVariant::Primary } else { ButtonVariant::Secondary })
                        .h(px(34.))
                        .disabled(form_disabled)
                        .on_click(cx.listener(|this, _event, window, cx| this.select_grid(Grid::Aditi, window, cx))),
                ),
        )
    }

    fn render_remember_control(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        Checkbox::new("remember-login")
            .checked(self.remember_login)
            .label("Remember username, password and MFA")
            .items_center()
            .disabled(form_disabled)
            .on_click({
                let screen = cx.entity().downgrade();
                move |checked, window, app| {
                    if screen
                        .update(app, |this, cx| {
                            this.remember_login = *checked;
                            if !*checked {
                                this.remembered_login = None;
                                this.spawn_choice = SpawnChoice::Last;
                                this.spawn_location.update(cx, |state, cx| {
                                    state.set_items(spawn_options(&[]), window, cx);
                                    state.set_selected_value(&SpawnChoice::Last, window, cx);
                                });
                                let grid = this.grid;
                                let generation = this.session_generation;
                                let write_lock = this.credential_write_lock.clone();
                                let background = cx.background_executor().clone();
                                cx.spawn(async move |this, cx| {
                                    let result = background
                                        .spawn(async move {
                                            let _guard = write_lock.lock().await;
                                            remembered_login::clear(grid)
                                        })
                                        .await;
                                    if let Err(error) = result
                                        && this
                                            .update(cx, |view, cx| {
                                                if view.session_generation == generation {
                                                    view.status = Some(error.to_string());
                                                    cx.notify();
                                                }
                                            })
                                            .is_err()
                                    {
                                        tracing::trace!("login screen was already unavailable after clearing saved credentials");
                                    }
                                })
                                .detach();
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        tracing::trace!("login screen was already unavailable after changing the remember setting");
                    }
                }
            })
    }

    fn render_status(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        div().when_some(self.status.clone(), |this, status| {
            let color = if self.session.is_some() {
                theme.success
            } else if form_disabled {
                theme.muted_foreground
            } else {
                theme.danger
            };
            this.child(div().text_size(px(13.)).text_color(color).child(status))
        })
    }

    fn render_submit_button(&self, can_submit: bool, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("login")
            .label(match self.login_phase {
                LoginPhase::Idle => "Log in",
                LoginPhase::Connecting => "Connecting…",
                LoginPhase::MfaRequired(_) => "Verify",
                LoginPhase::VerifyingMfa => "Verifying…",
            })
            .primary()
            .w_full()
            .h(px(42.))
            .disabled(!can_submit)
            .on_click(cx.listener(|this, _event, window, cx| this.submit(window, cx)))
    }
}

impl Render for LoginScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.displayed_favorites != self.favorite_locations {
            let options = spawn_options(&self.favorite_locations);
            self.spawn_location.update(cx, |state, cx| {
                state.set_items(options, window, cx);
                state.set_selected_value(&SpawnChoice::Last, window, cx);
            });
            self.spawn_choice = SpawnChoice::Last;
            self.displayed_favorites = self.favorite_locations.clone();
        }

        let username = self.username.read(cx).value();
        let password = self.password.read(cx).value();
        let mfa_code = self.mfa_code.read(cx).value();
        let show_region_input = self.spawn_choice == SpawnChoice::Region;
        let connecting = matches!(self.login_phase, LoginPhase::Connecting | LoginPhase::VerifyingMfa);
        let show_mfa = matches!(self.login_phase, LoginPhase::MfaRequired(_) | LoginPhase::VerifyingMfa);
        let can_submit = !self.storage_loading
            && match self.login_phase {
                LoginPhase::Idle => !username.trim().is_empty() && !password.is_empty(),
                LoginPhase::MfaRequired(_) => !mfa_code.trim().is_empty(),
                LoginPhase::Connecting | LoginPhase::VerifyingMfa => false,
            };
        let form_disabled = self.storage_loading || !matches!(self.login_phase, LoginPhase::Idle);

        let theme = Theme::global(cx).colors;
        let content = div().size_full().flex().flex_col().bg(theme.background).text_color(theme.foreground);

        #[cfg(target_os = "windows")]
        let content = content.child(windows_titlebar(window.is_maximized(), div().into_any_element(), div().into_any_element(), theme));

        #[cfg(target_os = "linux")]
        let content = {
            let titlebar_content = div().text_sm().font_semibold().text_color(theme.foreground).child("Radia").into_any_element();
            let titlebar_action = div().into_any_element();
            if let Some(titlebar) = linux_titlebar(window, titlebar_content, titlebar_action) { content.child(titlebar) } else { content }
        };

        let login_body = div().id("login-scroll").flex_1().min_h_0().overflow_y_scroll().child(
            div().w_full().flex().justify_center().px_4().py_6().child(
                div()
                    .w_full()
                    .max_w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .p_5()
                    .child(self.render_heading(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.render_credentials_form(form_disabled, cx))
                            .child(self.render_mfa_prompt(show_mfa, connecting, cx))
                            .child(self.render_spawn_selection(form_disabled, show_region_input, cx))
                            .child(self.render_grid_selection(form_disabled, cx))
                            .child(self.render_remember_control(form_disabled, cx)),
                    )
                    .child(self.render_status(form_disabled, cx))
                    .child(self.render_submit_button(can_submit, cx)),
            ),
        );

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        if self.session.is_some() {
            content.child(div().flex_1().min_h_0().child(self.chat_screen.clone()))
        } else {
            content.child(login_body)
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        content.child(login_body)
    }
}
fn split_username(username: &str) -> Option<(String, String)> {
    let mut names = username.split_whitespace();
    let first_name = names.next()?.to_owned();
    let last_name = names.collect::<Vec<_>>().join(" ");
    let last_name = if last_name.is_empty() { "Resident".to_owned() } else { last_name };

    Some((first_name, last_name))
}
