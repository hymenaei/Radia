//! Drains session events away from GPUI and coalesces viewer state updates.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use base::UUID;
use futures::{FutureExt, channel::mpsc, pin_mut, stream::StreamExt};
use gpui::BackgroundExecutor;
use network::{AgentSession, Event, Grid, Session, SessionHandle};
use protocol::udp::Message;

use crate::{
    chat::{self, ChatMessage},
    inventory::{FavoriteLocation, favorite_store, favorites::load_favorites},
    remembered_login::{self, RememberedLogin},
    world::{RegionLookup, agent_update},
};

const MAX_PENDING_CHAT_MESSAGES: usize = 64;
const MAX_PENDING_CHAT_BYTES: usize = 256 * 1024;
const PRESENTATION_WAKE_BUFFER: usize = 0;

pub(crate) type WriteLock = Arc<futures::lock::Mutex<()>>;

#[derive(Clone)]
pub(crate) struct PresentationMailbox {
    state: Arc<Mutex<PendingPresentation>>,
    wake_sender: Arc<Mutex<mpsc::Sender<()>>>,
}

pub(crate) struct PresentationReceiver {
    generation: u64,
    state: Arc<Mutex<PendingPresentation>>,
    wake_receiver: mpsc::Receiver<()>,
}

pub(crate) struct PresentationSnapshot {
    pub(crate) generation: u64,
    pub(crate) authentication: Option<AuthenticationUpdate>,
    pub(crate) session_ready: Option<SessionReadyUpdate>,
    pub(crate) region_name: Option<String>,
    pub(crate) chat_messages: VecDeque<ChatMessage>,
    pub(crate) capture_notice: Option<String>,
    pub(crate) send_error: Option<String>,
    pub(crate) failure: Option<String>,
    pub(crate) favorites: Option<FavoritesUpdate>,
    pub(crate) favorite_store_saved: Option<(u64, Result<(), String>)>,
}

pub(crate) enum AuthenticationUpdate {
    MfaRequired(network::MfaChallenge),
    CredentialsUpdated { remembered_login: Option<RememberedLogin>, warning: Option<String> },
}

pub(crate) struct SessionReadyUpdate {
    pub(crate) session: AgentSession,
    pub(crate) region_lookup: RegionLookup,
}

pub(crate) struct FavoritesUpdate {
    pub(crate) grid: Grid,
    pub(crate) agent_id: UUID,
    pub(crate) session_id: UUID,
    pub(crate) username: String,
    pub(crate) result: Result<Vec<FavoriteLocation>, String>,
}

#[derive(Default)]
struct PendingPresentation {
    authentication: Option<AuthenticationUpdate>,
    session_ready: Option<SessionReadyUpdate>,
    region_name: Option<String>,
    chat_messages: VecDeque<ChatMessage>,
    chat_bytes: usize,
    capture_notice: Option<String>,
    send_error: Option<String>,
    failure: Option<String>,
    favorites: Option<FavoritesUpdate>,
    favorite_store_saved: Option<(u64, Result<(), String>)>,
}

pub(crate) enum ViewerUpdate {
    Authentication(AuthenticationUpdate),
    SessionReady(SessionReadyUpdate),
    RegionName(String),
    ChatMessage(ChatMessage),
    CaptureNotice(String),
    SendError(String),
    Failed(String),
    Favorites(FavoritesUpdate),
    FavoriteStoreSaved { generation: u64, result: Result<(), String> },
}

#[derive(Clone, Copy)]
enum SimulatorScope {
    Primary,
    Neighbor,
}

impl PresentationMailbox {
    pub(crate) fn new(generation: u64) -> (Self, PresentationReceiver) {
        let state = Arc::new(Mutex::new(PendingPresentation::default()));
        let (wake_sender, wake_receiver) = mpsc::channel(PRESENTATION_WAKE_BUFFER);
        (
            Self { state: state.clone(), wake_sender: Arc::new(Mutex::new(wake_sender)) },
            PresentationReceiver { generation, state, wake_receiver },
        )
    }

    pub(crate) fn publish(&self, update: ViewerUpdate) {
        {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match update {
                ViewerUpdate::Authentication(update) => state.authentication = Some(update),
                ViewerUpdate::SessionReady(update) => state.session_ready = Some(update),
                ViewerUpdate::RegionName(name) => state.region_name = Some(name),
                ViewerUpdate::ChatMessage(message) => state.push_chat(message),
                ViewerUpdate::CaptureNotice(notice) => state.capture_notice = Some(notice),
                ViewerUpdate::SendError(error) => state.send_error = Some(error),
                ViewerUpdate::Failed(error) => {
                    state.chat_messages.clear();
                    state.chat_bytes = 0;
                    state.failure = Some(error);
                }
                ViewerUpdate::Favorites(update) => state.favorites = Some(update),
                ViewerUpdate::FavoriteStoreSaved { generation, result } => {
                    if state.favorite_store_saved.as_ref().is_none_or(|(current, _)| generation >= *current) {
                        state.favorite_store_saved = Some((generation, result));
                    }
                }
            }
        }

        let mut wake_sender = self.wake_sender.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = wake_sender.try_send(());
    }
}

impl PresentationReceiver {
    pub(crate) async fn next(&mut self) -> Option<PresentationSnapshot> {
        self.wake_receiver.next().await?;
        Some(self.take_snapshot())
    }

    fn take_snapshot(&mut self) -> PresentationSnapshot {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *state)
        };
        PresentationSnapshot::from_pending(self.generation, pending)
    }
}

impl PendingPresentation {
    fn push_chat(&mut self, message: ChatMessage) {
        let message_bytes = message.from_name.len().saturating_add(message.message.len());
        if message_bytes > MAX_PENDING_CHAT_BYTES {
            return;
        }

        while self.chat_messages.len() >= MAX_PENDING_CHAT_MESSAGES
            || self.chat_bytes.saturating_add(message_bytes) > MAX_PENDING_CHAT_BYTES
        {
            let Some(removed) = self.chat_messages.pop_front() else {
                break;
            };
            self.chat_bytes = self.chat_bytes.saturating_sub(removed.from_name.len().saturating_add(removed.message.len()));
        }

        self.chat_bytes += message_bytes;
        self.chat_messages.push_back(message);
    }
}

impl PresentationSnapshot {
    fn from_pending(generation: u64, pending: PendingPresentation) -> Self {
        let PendingPresentation {
            authentication,
            session_ready,
            region_name,
            chat_messages,
            capture_notice,
            send_error,
            failure,
            favorites,
            favorite_store_saved,
            ..
        } = pending;
        Self {
            generation,
            authentication,
            session_ready,
            region_name,
            chat_messages,
            capture_notice,
            send_error,
            failure,
            favorites,
            favorite_store_saved,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.authentication.is_none()
            && self.session_ready.is_none()
            && self.region_name.is_none()
            && self.chat_messages.is_empty()
            && self.capture_notice.is_none()
            && self.send_error.is_none()
            && self.failure.is_none()
            && self.favorites.is_none()
            && self.favorite_store_saved.is_none()
    }
}

pub(crate) async fn drain_session(
    mut session: Session,
    session_handle: SessionHandle,
    background: BackgroundExecutor,
    mailbox: PresentationMailbox,
    cancellation: futures::channel::oneshot::Receiver<()>,
    grid: Grid,
    username: String,
    password: String,
    remember_login: bool,
    credential_write_lock: WriteLock,
) {
    let cancellation = async move {
        let _ = cancellation.await;
    }
    .shared();
    let mut primary_simulator_id = None;
    let mut active_session = None;
    let mut region_lookup = None;

    loop {
        let cancelled = cancellation.clone().fuse();
        pin_mut!(cancelled);
        let event = session.recv_event().fuse();
        pin_mut!(event);
        futures::select! {
            _ = cancelled => break,
            event = event => {
                let Some(event) = event else {
                    mailbox.publish(ViewerUpdate::Failed("The network session ended.".into()));
                    break;
                };

                match event {
                    Event::MfaRequired { challenge } => {
                        mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::MfaRequired(challenge)));
                    }
                    Event::LoginAccepted { mfa_hash } => {
                        let mailbox = mailbox.clone();
                        let write_lock = credential_write_lock.clone();
                        let username = username.clone();
                        let password = password.clone();
                        background.spawn(async move {
                            let _guard = write_lock.lock().await;
                            let mut warning = None;
                            let remembered_login = if remember_login {
                                let saved = RememberedLogin { username, password, mfa_hash: mfa_hash.unwrap_or_default() };
                                match remembered_login::save(grid, &saved) {
                                    Ok(()) => Some(saved),
                                    Err(error) => {
                                        warning = Some(error);
                                        None
                                    }
                                }
                            } else {
                                if let Err(error) = remembered_login::clear(grid) {
                                    warning = Some(error);
                                }
                                None
                            };
                            mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::CredentialsUpdated {
                                remembered_login,
                                warning,
                            }));
                        }).detach();
                    }
                    Event::SessionReady { simulator_id, session, capabilities, login_data } => {
                        let session = *session;
                        primary_simulator_id = Some(simulator_id);
                        active_session = Some(session.clone());
                        let lookup = RegionLookup::new(session_handle.clone(), &session, background.clone());
                        region_lookup = Some(lookup.clone());
                        mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
                            session: session.clone(),
                            region_lookup: lookup.clone(),
                        }));

                        let _ = session_handle.send(
                            agent_update::idle(session.agent_id, session.session_id),
                            network::Delivery::Reliable,
                        );

                        let mailbox = mailbox.clone();
                        let username = username.clone();
                        let background = background.clone();
                        let cancelled = cancellation.clone();
                        background.spawn(async move {
                            let load = load_favorites(&capabilities, &lookup, &session, &login_data).fuse();
                            pin_mut!(load);
                            let cancelled = cancelled.fuse();
                            pin_mut!(cancelled);
                            futures::select! {
                                result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                                    grid: session.grid,
                                    agent_id: session.agent_id,
                                    session_id: session.session_id,
                                    username,
                                    result,
                                })),
                                _ = cancelled => {}
                            }
                        }).detach();
                    }
                    Event::SimulatorConnected { simulator_id, region_handle: None, .. } => {
                        primary_simulator_id = Some(simulator_id);
                    }
                    Event::SimulatorMessage { simulator_id, source, message, .. } => {
                        let scope = if Some(simulator_id) == primary_simulator_id {
                            SimulatorScope::Primary
                        } else {
                            SimulatorScope::Neighbor
                        };
                        handle_simulator_message(scope, source, message, active_session.as_ref(), region_lookup.as_ref(), &mailbox);
                    }
                    Event::CaptureFinished { path, error } => {
                        let notice = match error {
                            Some(error) => format!("Could not complete capture: {error}"),
                            None => format!("Capture saved to {}", path.display()),
                        };
                        mailbox.publish(ViewerUpdate::CaptureNotice(notice));
                    }
                    Event::CaptureInterrupted { path, error } => {
                        let notice = match error {
                            Some(error) => format!("Capture interrupted; saved to {}. {error}", path.display()),
                            None => format!("Capture interrupted and saved to {}.", path.display()),
                        };
                        mailbox.publish(ViewerUpdate::CaptureNotice(notice));
                    }
                    Event::SendFailed(error) => mailbox.publish(ViewerUpdate::SendError(error)),
                    Event::Failed(error) => {
                        mailbox.publish(ViewerUpdate::Failed(error));
                        break;
                    }
                    Event::CapabilityEvent { .. }
                    | Event::CapabilityError { .. }
                    | Event::SimulatorDisconnected { .. }
                    | Event::SimulatorFailed { .. }
                    | Event::SimulatorConnectFailed { .. }
                    | Event::SimulatorCapabilities { .. }
                    | Event::SimulatorConnected { .. } => {}
                }
            }
        }
    }
}

fn handle_simulator_message(
    scope: SimulatorScope,
    source: std::net::SocketAddr,
    message: Message,
    session: Option<&AgentSession>,
    region_lookup: Option<&RegionLookup>,
    mailbox: &PresentationMailbox,
) {
    if matches!(scope, SimulatorScope::Neighbor) {
        return;
    }

    if let Some(region_lookup) = region_lookup {
        region_lookup.handle_message(source, &message);
    }
    if let Message::RegionHandshake(message) = &message {
        mailbox.publish(ViewerUpdate::RegionName(message.region_info1.sim_name.clone()));
    }
    if let Message::ChatFromSimulator(message) = message
        && let Some(chat_message) = chat::display_message(message.chat_data, session)
    {
        mailbox.publish(ViewerUpdate::ChatMessage(chat_message));
    }
}

pub(crate) fn write_lock() -> WriteLock {
    Arc::new(futures::lock::Mutex::new(()))
}

pub(crate) async fn save_favorite_store(
    store: crate::inventory::favorite_store::FavoriteStore,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
) -> Option<Result<(), String>> {
    save_if_current(store, lock, generation, expected_generation, |store| favorite_store::save(&store)).await
}

async fn save_if_current<T>(
    value: T,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    save: impl FnOnce(T) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let _guard = lock.lock().await;
    (generation.load(Ordering::Acquire) == expected_generation).then(|| save(value))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use futures::stream::StreamExt;

    use base::UUID;
    use network::Grid;
    use protocol::{
        Audible, ChatSourceType, ChatType, Vector3,
        messages::{
            chat_from_simulator::ChatData,
            region_handshake::{RegionHandshake, RegionInfo1},
        },
        udp::Message,
    };

    use crate::chat::ChatMessage;
    use crate::inventory::{FavoriteLocation, favorite_store::FavoriteStore};

    use super::{
        MAX_PENDING_CHAT_BYTES, MAX_PENDING_CHAT_MESSAGES, PresentationMailbox, SimulatorScope, ViewerUpdate, handle_simulator_message,
        save_if_current, write_lock,
    };

    fn region_handshake(name: &str) -> Message {
        Message::RegionHandshake(RegionHandshake {
            region_info1: RegionInfo1 { sim_name: name.into(), ..RegionInfo1::default() },
            region_info2: None,
            region_info3: None,
            region_info4: Vec::new(),
        })
    }

    fn chat_message(from_name: &str) -> Message {
        Message::ChatFromSimulator(protocol::messages::ChatFromSimulator {
            chat_data: ChatData {
                from_name: from_name.into(),
                source_id: UUID::nil(),
                owner_id: UUID::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vector3::default(),
                message: "hello".into(),
            },
        })
    }

    #[test]
    fn mailbox_coalesces_state_behind_one_wake() {
        let (mailbox, mut receiver) = PresentationMailbox::new(7);
        for index in 0..1000 {
            mailbox.publish(ViewerUpdate::RegionName(format!("Region {index}")));
        }

        assert_eq!(futures::executor::block_on(receiver.wake_receiver.next()), Some(()));
        let snapshot = receiver.take_snapshot();
        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.region_name.as_deref(), Some("Region 999"));
        assert!(receiver.wake_receiver.try_recv().is_err());
    }

    #[test]
    fn mailbox_bounds_chat_retention() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        for index in 0..MAX_PENDING_CHAT_MESSAGES + 20 {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: format!("Agent {index}"), message: "Hello".into() }));
        }

        assert_eq!(futures::executor::block_on(receiver.wake_receiver.next()), Some(()));
        let snapshot = receiver.take_snapshot();
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        assert!(snapshot.chat_messages.front().is_some_and(|message| message.from_name == "Agent 20"));
        assert!(snapshot.chat_messages.back().is_some_and(|message| message.from_name == "Agent 83"));
        let bytes = snapshot.chat_messages.iter().map(|message| message.from_name.len() + message.message.len()).sum::<usize>();
        assert!(bytes <= MAX_PENDING_CHAT_BYTES);
    }

    #[test]
    fn mailbox_bounds_chat_bytes() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        for index in 0..MAX_PENDING_CHAT_MESSAGES + 20 {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: format!("Agent {index}"), message: "x".repeat(4096) }));
        }

        assert_eq!(futures::executor::block_on(receiver.wake_receiver.next()), Some(()));
        let snapshot = receiver.take_snapshot();
        let bytes = snapshot.chat_messages.iter().map(|message| message.from_name.len() + message.message.len()).sum::<usize>();
        assert!(bytes <= MAX_PENDING_CHAT_BYTES);
        assert!(snapshot.chat_messages.len() < MAX_PENDING_CHAT_MESSAGES);
    }

    #[test]
    fn mailbox_keeps_failure_after_chat_burst() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        for index in 0..MAX_PENDING_CHAT_MESSAGES + 20 {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: format!("Agent {index}"), message: "Hello".into() }));
        }
        mailbox.publish(ViewerUpdate::Failed("connection closed".into()));

        let snapshot = futures::executor::block_on(receiver.next()).expect("failure wake");
        assert!(snapshot.chat_messages.is_empty());
        assert_eq!(snapshot.failure.as_deref(), Some("connection closed"));
        assert!(receiver.wake_receiver.try_recv().is_err());
    }

    #[test]
    fn mailbox_keeps_newest_favorite_store_save_result() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        mailbox.publish(ViewerUpdate::FavoriteStoreSaved { generation: 2, result: Err("latest failure".into()) });
        mailbox.publish(ViewerUpdate::FavoriteStoreSaved { generation: 1, result: Ok(()) });

        let snapshot = futures::executor::block_on(receiver.next()).expect("save result wake");
        assert!(matches!(snapshot.favorite_store_saved, Some((2, Err(error))) if error == "latest failure"));
    }

    #[test]
    fn stale_favorite_snapshot_cannot_overwrite_newer_snapshot() {
        const AGENT_ID: UUID = UUID::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after epoch").as_nanos();
        let path = std::env::temp_dir().join(format!("radia-favorite-generation-{}-{unique}.json", std::process::id()));

        let mut old_store = FavoriteStore::default();
        old_store.replace_favorites(
            Grid::Agni,
            AGENT_ID,
            "Example Resident",
            vec![FavoriteLocation { name: "Old".into(), region_name: "Old Region".into(), x: 1, y: 2, z: 3 }],
        );
        let mut new_store = FavoriteStore::default();
        new_store.replace_favorites(
            Grid::Agni,
            AGENT_ID,
            "Example Resident",
            vec![FavoriteLocation { name: "Newest".into(), region_name: "New Region".into(), x: 4, y: 5, z: 6 }],
        );

        let (old_result, new_result) = futures::executor::block_on(async {
            let lock = write_lock();
            let guard = lock.lock().await;
            let generation = Arc::new(AtomicU64::new(1));

            let old_path = path.clone();
            let mut old_save = Box::pin(save_if_current(old_store, lock.clone(), Arc::clone(&generation), 1, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(old_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(old_save.as_mut()).is_pending());

            let new_path = path.clone();
            let mut new_save = Box::pin(save_if_current(new_store, lock.clone(), Arc::clone(&generation), 2, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(new_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(new_save.as_mut()).is_pending());

            generation.store(2, Ordering::Release);
            drop(guard);
            futures::join!(old_save, new_save)
        });

        assert!(old_result.is_none());
        assert_eq!(new_result, Some(Ok(())));
        let saved: FavoriteStore =
            serde_json::from_slice(&fs::read(&path).expect("saved latest favorites")).expect("saved snapshot is valid JSON");
        assert_eq!(saved.favorites_for(Grid::Agni, AGENT_ID)[0].name, "Newest");
        fs::remove_file(path).expect("remove temporary favorite snapshot");
    }

    #[test]
    fn primary_burst_survives_neighbor_and_blocked_presentation() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        let source = "127.0.0.1:13000".parse().expect("simulator address");

        for index in 0..100 {
            handle_simulator_message(
                SimulatorScope::Neighbor,
                source,
                region_handshake(&format!("Neighbor {index}")),
                None,
                None,
                &mailbox,
            );
            handle_simulator_message(SimulatorScope::Neighbor, source, chat_message("Neighbor"), None, None, &mailbox);
            handle_simulator_message(SimulatorScope::Primary, source, region_handshake(&format!("Primary {index}")), None, None, &mailbox);
            handle_simulator_message(SimulatorScope::Primary, source, chat_message("Primary"), None, None, &mailbox);
        }

        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.region_name.as_deref(), Some("Primary 99"));
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        assert!(snapshot.chat_messages.iter().all(|message| message.from_name == "Primary"));
        assert!(receiver.wake_receiver.try_recv().is_err());
    }
}
