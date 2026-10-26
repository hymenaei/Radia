use std::{
    collections::VecDeque,
    mem,
    sync::{Arc, Mutex, PoisonError},
};

use ::inventory::InventorySnapshot;
use futures::{channel::mpsc, stream::StreamExt};
use network::{AgentSession, Grid, MfaChallenge};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use render::scene::Camera;
use tokio::sync::watch;
use uuid::Uuid;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::{asset_coordinator::AssetHandle, session::experience_environment::EnvironmentInbox, world_reader::WorldReader};
use crate::{
    inventory::{FavoriteLocation, favorite_store::FavoriteStoreError, favorites::FavoritesError},
    remembered_login::{CredentialStoreError, RememberedLogin},
    session::ChatMessage,
};

const MAX_PENDING_CHAT_MESSAGES: usize = 64;
const MAX_PENDING_CHAT_BYTES: usize = 256 * 1024;
const PRESENTATION_WAKE_BUFFER: usize = 0;

#[derive(Clone)]
pub(crate) struct PresentationMailbox {
    state: Arc<Mutex<PendingPresentation>>,
    wake_sender: Arc<Mutex<mpsc::Sender<()>>>,
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
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match update {
                ViewerUpdate::Authentication(update) => state.authentication = Some(update),
                ViewerUpdate::SessionReady(update) => {
                    state.region_name = None;
                    state.session_ready = Some(update);
                }
                ViewerUpdate::RegionName(name) => state.region_name = Some(name),
                ViewerUpdate::ChatMessage(message) => state.push_chat(message),
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

        self.wake();
    }
}

impl PresentationMailbox {
    fn wake(&self) {
        let mut wake_sender = self.wake_sender.lock().unwrap_or_else(PoisonError::into_inner);
        match wake_sender.try_send(()) {
            Ok(()) => {}
            Err(error) if error.is_full() => {}
            Err(error) => tracing::trace!(%error, "presentation receiver closed"),
        }
    }
}

pub(crate) struct PresentationReceiver {
    generation: u64,
    state: Arc<Mutex<PendingPresentation>>,
    wake_receiver: mpsc::Receiver<()>,
}

impl PresentationReceiver {
    pub(crate) async fn next(&mut self) -> Option<PresentationSnapshot> {
        self.wake_receiver.next().await?;
        Some(self.take_snapshot())
    }

    fn take_snapshot(&mut self) -> PresentationSnapshot {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            mem::take(&mut *state)
        };
        PresentationSnapshot::from_pending(self.generation, pending)
    }
}

pub(crate) struct PresentationSnapshot {
    pub(crate) generation: u64,
    pub(crate) authentication: Option<AuthenticationUpdate>,
    pub(crate) session_ready: Option<SessionReadyUpdate>,
    pub(crate) region_name: Option<String>,
    pub(crate) chat_messages: VecDeque<ChatMessage>,
    pub(crate) failure: Option<String>,
    pub(crate) favorites: Option<FavoritesUpdate>,
    pub(crate) favorite_store_saved: Option<(u64, Result<(), FavoriteStoreError>)>,
}

impl PresentationSnapshot {
    fn from_pending(generation: u64, pending: PendingPresentation) -> Self {
        let PendingPresentation {
            authentication, session_ready, region_name, chat_messages, failure, favorites, favorite_store_saved, ..
        } = pending;
        Self { generation, authentication, session_ready, region_name, chat_messages, failure, favorites, favorite_store_saved }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.authentication.is_none()
            && self.session_ready.is_none()
            && self.region_name.is_none()
            && self.chat_messages.is_empty()
            && self.failure.is_none()
            && self.favorites.is_none()
            && self.favorite_store_saved.is_none()
    }
}

pub(crate) enum AuthenticationUpdate {
    MfaRequired(MfaChallenge),
    CredentialsUpdated { remembered_login: Option<RememberedLogin>, warning: Option<CredentialStoreError> },
}

pub(crate) struct SessionReadyUpdate {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) environments: EnvironmentInbox,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) camera: watch::Sender<Option<Camera>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) assets: AssetHandle,
    pub(crate) session: AgentSession,
    pub(crate) region_name: Option<String>,
    pub(crate) inventory_updates: watch::Receiver<Option<InventorySnapshot>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) world_reader: WorldReader,
}

pub(crate) struct FavoritesUpdate {
    pub(crate) grid: Grid,
    pub(crate) agent_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) username: String,
    pub(crate) result: Result<Vec<FavoriteLocation>, FavoritesError>,
}

#[derive(Default)]
struct PendingPresentation {
    authentication: Option<AuthenticationUpdate>,
    session_ready: Option<SessionReadyUpdate>,
    region_name: Option<String>,
    chat_messages: VecDeque<ChatMessage>,
    chat_bytes: usize,
    failure: Option<String>,
    favorites: Option<FavoritesUpdate>,
    favorite_store_saved: Option<(u64, Result<(), FavoriteStoreError>)>,
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

pub(crate) enum ViewerUpdate {
    Authentication(AuthenticationUpdate),
    SessionReady(SessionReadyUpdate),
    RegionName(String),
    ChatMessage(ChatMessage),
    Failed(String),
    Favorites(FavoritesUpdate),
    FavoriteStoreSaved { generation: u64, result: Result<(), FavoriteStoreError> },
}

#[cfg(test)]
mod tests {
    use network::PacketOrder::Ordered;

    use crate::test_helpers::simulator_id;

    use crate::session::coordinator::events::{SimulatorScope, decode_simulator_world_update, handle_simulator_message};
    use crate::session::coordinator::tests::{chat_message, region_handshake};
    use crate::session::{
        ChatMessage,
        world::{SessionWorld, viewer_world_limits},
    };

    use super::*;

    #[test]
    fn updates_coalesce_and_bound_chat() {
        let (mailbox, mut receiver) = PresentationMailbox::new(7);
        for index in 0..1000 {
            mailbox.publish(ViewerUpdate::RegionName(format!("Region {index}")));
        }
        for index in 0..MAX_PENDING_CHAT_MESSAGES + 20 {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: format!("Agent {index}"), message: "Hello".into() }));
        }

        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.region_name.as_deref(), Some("Region 999"));
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        assert!(snapshot.chat_messages.front().is_some_and(|message| message.from_name == "Agent 20"));
        let bytes = snapshot.chat_messages.iter().map(|message| message.from_name.len() + message.message.len()).sum::<usize>();
        assert!(bytes <= MAX_PENDING_CHAT_BYTES);
        assert!(receiver.wake_receiver.try_recv().is_err());
    }

    #[test]
    fn failure_discards_pending_chat() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        for _ in 0..MAX_PENDING_CHAT_MESSAGES {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: "Agent".into(), message: "Hello".into() }));
        }
        mailbox.publish(ViewerUpdate::Failed("connection closed".into()));

        let snapshot = futures::executor::block_on(receiver.next()).expect("failure wake");
        assert!(snapshot.chat_messages.is_empty());
        assert_eq!(snapshot.failure.as_deref(), Some("connection closed"));
    }

    #[test]
    fn region_name_follows_world() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        world.register_simulator(simulator, None).unwrap();
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        let address = "127.0.0.1:13000".parse().unwrap();
        for (order, name) in [(10, "Initial"), (20, "Renamed"), (15, "Stale")] {
            let message = region_handshake(name);
            decode_simulator_world_update(&mut world, simulator, order, Ordered(u64::from(order)), &message);
            handle_simulator_message(SimulatorScope::Primary, address, message, None, None, world.region_name(simulator), &mailbox);
        }
        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.region_name.as_deref(), Some("Renamed"));
        assert_eq!(world.region_name(simulator), Some("Renamed"));
    }

    #[test]
    fn neighbor_chat_survives_primary_updates() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        let source = "127.0.0.1:13000".parse().expect("simulator address");

        for index in 0..100 {
            handle_simulator_message(
                SimulatorScope::Neighbor,
                source,
                region_handshake(&format!("Neighbor {index}")),
                None,
                None,
                Some("Neighbor"),
                &mailbox,
            );
            handle_simulator_message(SimulatorScope::Neighbor, source, chat_message("Neighbor"), None, None, None, &mailbox);
            handle_simulator_message(
                SimulatorScope::Primary,
                source,
                region_handshake("Unaccepted"),
                None,
                None,
                Some(&format!("Primary {index}")),
                &mailbox,
            );
            handle_simulator_message(SimulatorScope::Primary, source, chat_message("Primary"), None, None, None, &mailbox);
        }

        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.region_name.as_deref(), Some("Primary 99"));
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        let names = snapshot.chat_messages.iter().map(|message| message.from_name.as_str()).collect::<Vec<_>>();
        let expected = (0..MAX_PENDING_CHAT_MESSAGES / 2).flat_map(|_| ["Neighbor", "Primary"]).collect::<Vec<_>>();
        assert_eq!(names, expected);
        assert!(receiver.wake_receiver.try_recv().is_err());
    }
}
