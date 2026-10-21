use crate::chat::ChatScreen;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::inventory::{
    FavoriteLocation,
    favorite_store::{self, FavoriteStore},
};
use gpui::{Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};
#[cfg(target_os = "linux")]
use gpui_component::StyledExt;
use gpui_component::{
    Disableable, Icon, IconName, IndexPath, Theme,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use network::{AgentSession, CaptureConfig, Grid, LoginCredentials, MfaChallenge, SessionHandle, StartLocation};

#[cfg(target_os = "linux")]
use crate::window_chrome::linux_titlebar;
#[cfg(target_os = "windows")]
use crate::window_chrome::windows_titlebar;
use crate::{
    asset_coordinator::AssetHandle,
    remembered_login::{self, RememberedLogin},
    session::{
        AuthenticationUpdate, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, RegionLookup, Session, SessionReadyUpdate,
        ViewerUpdate, WriteLock, drain_session, save_favorite_store, write_lock,
    },
    window_geometry::observe,
    world_reader::WorldReader,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum SpawnChoice {
    Last,
    Home,
    Region,
    Favorite(FavoriteLocation),
}

#[derive(Clone)]
struct SpawnOption {
    title: SharedString,
    choice: SpawnChoice,
}

impl SelectItem for SpawnOption {
    type Value = SpawnChoice;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.choice
    }
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
    favorite_storage_warning: Option<String>,
    favorite_store_loaded: bool,
    storage_loading: bool,
    grid: Grid,
    remember_login: bool,
    capture_on_login: Arc<AtomicBool>,
    remembered_login: Option<RememberedLogin>,
    chat_screen: gpui::Entity<ChatScreen>,
    session_handle: Option<SessionHandle>,
    world_reader: Option<WorldReader>,
    assets: Option<AssetHandle>,
    status: Option<String>,
    session: Option<AgentSession>,
    is_connecting: bool,
    awaiting_mfa: bool,
    mfa_challenge: Option<MfaChallenge>,
    credential_warning: Option<String>,
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
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        capture_on_login: Arc<AtomicBool>,
        shutdown_session: Arc<Mutex<Option<SessionHandle>>>,
    ) -> Self {
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
            favorite_store_loaded: false,
            storage_loading: true,
            grid,
            remember_login: false,
            capture_on_login,
            remembered_login: None,
            chat_screen,
            session_handle: None,
            world_reader: None,
            assets: None,
            status: Some("Loading saved login…".into()),
            session: None,
            is_connecting: false,
            awaiting_mfa: false,
            mfa_challenge: None,
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
            let _ = this.update_in(cx, |screen, window, cx| {
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

                screen.favorite_store = favorite_store;
                screen.favorite_store_loaded = favorite_warning.is_none();
                screen.favorite_storage_warning = favorite_warning.clone();
                screen.set_remembered_login(login, favorites, window, cx);
                screen.storage_loading = false;
                screen.status = login_warning.or(favorite_warning);
                cx.notify();
            });
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
            let _ = this.update_in(cx, |screen, window, cx| {
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
                screen.status = login_warning.or_else(|| screen.favorite_storage_warning.clone());
                cx.notify();
            });
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

        if self.awaiting_mfa {
            self.submit_mfa_code(window, cx);
            return;
        }

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

        let remembered_for_user =
            self.remembered_login.as_ref().filter(|saved| saved.username.eq_ignore_ascii_case(username.trim())).cloned();
        let start_location = match &self.spawn_choice {
            SpawnChoice::Last => StartLocation::Last,
            SpawnChoice::Home => StartLocation::Home,
            SpawnChoice::Region => {
                let name = self.region_name.read(cx).value();
                if name.trim().is_empty() {
                    self.status = Some("Enter a region name.".into());
                    cx.notify();
                    return;
                }
                StartLocation::Region { name: name.trim().to_owned(), x: 128, y: 128, z: 0 }
            }
            SpawnChoice::Favorite(favorite) => {
                StartLocation::Region { name: favorite.region_name.clone(), x: favorite.x, y: favorite.y, z: favorite.z }
            }
        };

        if let Some(cancel) = self.session_cancel.take() {
            let _ = cancel.send(());
        }
        self.session_generation = self.session_generation.wrapping_add(1);
        self.presentation_mailbox = None;
        self.chat_screen.update(cx, |chat, cx| chat.clear_session(cx));
        self.session = None;
        self.is_connecting = true;
        self.awaiting_mfa = false;
        self.mfa_challenge = None;
        self.session_handle = None;
        *self.shutdown_session.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.current_region_name = None;
        self.region_lookup = None;
        let capture_enabled = self.capture_on_login.swap(false, Ordering::AcqRel);
        self.status = capture_enabled.then(|| "Capture enabled. Raw data will be saved locally when capture stops.".into());

        let credentials = LoginCredentials {
            first_name,
            last_name,
            password: password.to_string(),
            grid: self.grid,
            start_location,
            mfa_hash: remembered_for_user.as_ref().map(|saved| saved.mfa_hash.clone()).unwrap_or_default(),
        };
        let remember_login = self.remember_login;
        let remembered_username = username.trim().to_owned();
        let remembered_password = password.to_string();
        let network_session = match if capture_enabled {
            capture_config().and_then(|config| Session::start_with_capture(credentials, config).map_err(|error| error.to_string()))
        } else {
            Session::start(credentials).map_err(|error| error.to_string())
        } {
            Ok(session) => session,
            Err(error) => {
                self.is_connecting = false;
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
        let background = crate::app::service_runtime(cx);
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
            grid,
            remembered_username,
            remembered_password,
            remember_login,
            credential_write_lock,
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

        if let Some(authentication) = authentication {
            match authentication {
                AuthenticationUpdate::MfaRequired(challenge) => {
                    self.is_connecting = false;
                    self.awaiting_mfa = true;
                    self.mfa_challenge = Some(challenge);
                    self.status = Some("Enter the code from your authenticator.".into());
                }
                AuthenticationUpdate::CredentialsUpdated { remembered_login, warning } => {
                    if let Some(login) = remembered_login {
                        self.remembered_login = Some(login);
                    } else if warning.is_none() {
                        self.remembered_login = None;
                    }
                    if let Some(warning) = &warning
                        && self.session.is_some()
                    {
                        let status = self.status.take().unwrap_or_default();
                        self.status = Some(format!("{status}\n{warning}"));
                    }
                    self.credential_warning = warning;
                }
            }
        }

        if let Some(ready) = session_ready {
            let SessionReadyUpdate { session, region_lookup, region_name, world_reader, assets, camera, environments } = ready;
            self.assets = Some(assets.clone());
            self.world_reader = Some(world_reader.clone());
            self.is_connecting = false;
            self.awaiting_mfa = false;
            self.mfa_challenge = None;
            tracing::info!(grid = %session.grid.label(), simulator = %session.simulator_address, "connected to simulator");
            let mut warnings = Vec::new();
            if let Some(warning) = self.credential_warning.take() {
                warnings.push(warning);
            }
            let identity_changed = self.favorite_store.remember_agent(session.grid, session.agent_id, &self.username.read(cx).value());
            self.favorite_locations = self.favorite_store.favorites_for(session.grid, session.agent_id).to_vec();
            if identity_changed && self.favorite_store_loaded {
                self.favorite_storage_warning = None;
                self.schedule_favorite_store_save(self.favorite_store.clone(), cx);
            }
            if let Some(warning) = self.favorite_storage_warning.clone() {
                warnings.push(warning);
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
            if crate::app::start_scene(self.session_generation, cx) {
                crate::app::connect_world(
                    self.session_generation,
                    crate::scene_feed::SceneSources { reader: world_reader, environments, assets },
                    session.agent_id,
                    camera,
                    cx,
                );
            }
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

        if let Some((generation, result)) = favorite_store_saved
            && generation == self.favorite_store_save_generation.load(Ordering::Acquire)
        {
            match result {
                Ok(()) => self.favorite_storage_warning = None,
                Err(error) => {
                    let warning = format!("Could not save favorite landmarks: {error}");
                    self.favorite_storage_warning = Some(warning);
                    tracing::warn!(%error, "could not save favorite landmarks");
                }
            }
        }

        if let Some(error) = failure {
            self.end_session(error, cx);
        }

        cx.notify();
    }

    fn end_session(&mut self, error: String, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        crate::app::end_scene(self.session_generation, cx);
        let was_connected = self.session.is_some();
        tracing::warn!(was_connected, %error, "Viewer session ended after a failure");
        self.is_connecting = false;
        self.awaiting_mfa = false;
        self.mfa_challenge = None;
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
            return;
        }

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

        let Some(challenge) = self.mfa_challenge.as_ref() else {
            self.awaiting_mfa = false;
            self.status = Some("The login request has ended. Sign in again.".into());
            cx.notify();
            return;
        };

        self.is_connecting = true;
        self.status = None;
        let result = challenge.complete(&code);
        self.mfa_code.update(cx, |input, cx| input.set_value("", window, cx));
        match result {
            Ok(()) => self.mfa_challenge = None,
            Err(error) => {
                self.is_connecting = false;
                self.status = Some(error.to_string());
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

fn spawn_options(favorites: &[FavoriteLocation]) -> Vec<SpawnOption> {
    let mut options = vec![
        SpawnOption { title: "Last location".into(), choice: SpawnChoice::Last },
        SpawnOption { title: "Home".into(), choice: SpawnChoice::Home },
    ];
    options.extend(
        favorites
            .iter()
            .cloned()
            .map(|favorite| SpawnOption { title: format!("★ {}", favorite.name).into(), choice: SpawnChoice::Favorite(favorite) }),
    );
    options.push(SpawnOption { title: "Enter region name…".into(), choice: SpawnChoice::Region });
    options
}

fn favorites_for_username(favorites: &FavoriteStore, grid: Grid, username: &str) -> Vec<FavoriteLocation> {
    favorites.agent_id_for_username(grid, username).map(|agent_id| favorites.favorites_for(grid, agent_id).to_vec()).unwrap_or_default()
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
        let can_submit = if self.awaiting_mfa {
            !self.storage_loading && !self.is_connecting && !mfa_code.trim().is_empty()
        } else {
            !self.storage_loading && !self.is_connecting && !username.trim().is_empty() && !password.is_empty()
        };
        let form_disabled = self.storage_loading || self.is_connecting || self.awaiting_mfa;

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
                    .child(
                        div().flex().items_center().gap_3().child(Icon::new(IconName::Globe).size_5().text_color(theme.primary)).child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(22.)).text_color(theme.foreground).child("Sign in"))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme.muted_foreground)
                                        .child("Connect to your Second Life account."),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(div().text_size(px(13.)).text_color(theme.foreground).child("Username"))
                                    .child(Input::new(&self.username).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(div().text_size(px(13.)).text_color(theme.foreground).child("Password"))
                                    .child(Input::new(&self.password).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
                            )
                            .when(self.awaiting_mfa, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(div().text_size(px(13.)).text_color(theme.foreground).child("Authentication code"))
                                        .child(
                                            Input::new(&self.mfa_code)
                                                .w_full()
                                                .h(px(42.))
                                                .focus_bordered(false)
                                                .disabled(self.is_connecting),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(div().text_size(px(13.)).text_color(theme.foreground).child("Start at"))
                                    .child(Select::new(&self.spawn_location).w_full().h(px(40.)).disabled(form_disabled)),
                            )
                            .when(show_region_input, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(div().text_size(px(13.)).text_color(theme.foreground).child("Region name"))
                                        .child(
                                            Input::new(&self.region_name).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().text_size(px(13.)).text_color(theme.foreground).child("Grid"))
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                Button::new("grid-agni")
                                                    .label("Main grid")
                                                    .with_variant(if self.grid == Grid::Agni {
                                                        ButtonVariant::Primary
                                                    } else {
                                                        ButtonVariant::Secondary
                                                    })
                                                    .h(px(34.))
                                                    .disabled(form_disabled)
                                                    .on_click(cx.listener(|this, _event, window, cx| {
                                                        this.select_grid(Grid::Agni, window, cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("grid-aditi")
                                                    .label("Beta grid")
                                                    .with_variant(if self.grid == Grid::Aditi {
                                                        ButtonVariant::Primary
                                                    } else {
                                                        ButtonVariant::Secondary
                                                    })
                                                    .h(px(34.))
                                                    .disabled(form_disabled)
                                                    .on_click(cx.listener(|this, _event, window, cx| {
                                                        this.select_grid(Grid::Aditi, window, cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                Checkbox::new("remember-login")
                                    .checked(self.remember_login)
                                    .label("Remember username, password and MFA")
                                    .items_center()
                                    .disabled(form_disabled)
                                    .on_click({
                                        let screen = cx.entity().downgrade();
                                        move |checked, window, app| {
                                            let _ = screen.update(app, |this, cx| {
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
                                                        if let Err(error) = result {
                                                            let _ = this.update(cx, |view, cx| {
                                                                if view.session_generation == generation {
                                                                    view.status = Some(error);
                                                                    cx.notify();
                                                                }
                                                            });
                                                        }
                                                    })
                                                    .detach();
                                                }
                                                cx.notify();
                                            });
                                        }
                                    }),
                            ),
                    )
                    .when_some(self.status.clone(), |this, status| {
                        let color = if self.session.is_some() {
                            theme.success
                        } else if self.storage_loading || self.is_connecting || self.awaiting_mfa {
                            theme.muted_foreground
                        } else {
                            theme.danger
                        };
                        this.child(div().text_size(px(13.)).text_color(color).child(status))
                    })
                    .child(
                        Button::new("login")
                            .label(if self.is_connecting {
                                if self.awaiting_mfa { "Verifying…" } else { "Connecting…" }
                            } else if self.awaiting_mfa {
                                "Verify"
                            } else {
                                "Log in"
                            })
                            .primary()
                            .w_full()
                            .h(px(42.))
                            .disabled(!can_submit)
                            .on_click(cx.listener(|this, _event, window, cx| this.submit(window, cx))),
                    ),
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

pub(super) fn capture_config() -> Result<CaptureConfig, String> {
    let local_data = dirs::data_local_dir().ok_or_else(|| "Could not find local application data directory.".to_string())?;
    let root = local_data.join("Radia").join("captures");
    Ok(CaptureConfig::new(root))
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, Render, TestAppContext, Window, point, px};

    use super::*;

    struct CheckboxHarness {
        checked: bool,
    }

    impl Render for CheckboxHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let screen = cx.entity().downgrade();
            div().debug_selector(|| "remember-login-checkbox".into()).child(
                Checkbox::new("remember-login-test").checked(self.checked).label("Remember me").on_click(move |checked, _, app| {
                    let _ = screen.update(app, |this, cx| {
                        this.checked = *checked;
                        cx.notify();
                    });
                }),
            )
        }
    }

    #[gpui::test]
    fn remember_checkbox_toggles(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (checkbox, cx) = cx.add_window_view(|_, _| CheckboxHarness { checked: true });
        cx.update(|window, cx| _ = window.draw(cx));

        let bounds = cx.debug_bounds("remember-login-checkbox").expect("remember checkbox should render");
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        cx.simulate_click(point(bounds.origin.x + px(8.), bounds.origin.y + px(8.)), Default::default());

        assert!(!cx.update(|_, app| checkbox.read(app).checked));
    }
}
