use std::time::Duration;

use ::inventory::{
    CategoryKey, FolderType, InventoryDelta, InventoryNodeId, InventoryNodeKey, InventoryOwner, InventorySnapshot, ItemKey,
    MAX_INVENTORY_NAME_BYTES,
};
use futures::future::{BoxFuture, Shared};
use llsd::{Map, Value};
use network::{
    AgentSession, Delivery, SessionHandle,
    capabilities::{CapabilityUrl, Client, HttpErrorKind},
};
use protocol::{
    inventory::{self, AisEditResponse},
    messages::{MoveInventoryFolder, MoveInventoryFolderBlock, MoveInventoryItem, MoveInventoryItemBlock},
};
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time,
};
use uuid::Uuid;

use super::{
    delta::{delta_from_ais_response, trash_folder},
    handle::InventoryEditHandle,
    types::{EditOperation, EditRequest, InventoryEditOutcome, InventoryEditRejection, MoveExpectation},
};

const REQUEST_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 4;
const MAX_EDIT_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MOVE_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);

type Cancellation = Shared<BoxFuture<'static, ()>>;

pub(crate) enum InventoryEditEvent {
    Apply { delta: InventoryDelta, applied: oneshot::Sender<bool> },
    RefreshGone { folder: CategoryKey, refreshed: oneshot::Sender<bool> },
    MoveStarted { expectation: MoveExpectation, confirmed: oneshot::Sender<()>, registered: oneshot::Sender<()> },
    MoveAborted { expectation: MoveExpectation, removed: oneshot::Sender<()> },
    MoveTimedOut { expectation: MoveExpectation, refreshed: oneshot::Sender<bool> },
}

#[derive(Clone, Copy)]
pub(super) struct EditSession {
    pub(super) agent_id: Uuid,
    pub(super) session_id: Uuid,
}

pub(super) enum InventoryMove {
    Item(MoveInventoryItem),
    Folder(MoveInventoryFolder),
}

pub(super) trait MoveTransport: Send + Sync {
    fn send(&self, movement: InventoryMove) -> Result<(), network::Error>;
}

struct SessionMoveTransport(SessionHandle);

impl MoveTransport for SessionMoveTransport {
    fn send(&self, movement: InventoryMove) -> Result<(), network::Error> {
        match movement {
            InventoryMove::Item(message) => self.0.send(message, Delivery::Reliable),
            InventoryMove::Folder(message) => self.0.send(message, Delivery::Reliable),
        }
    }
}

pub(crate) struct InventoryEditController {
    handle: InventoryEditHandle,
    requests: Option<mpsc::Receiver<EditRequest>>,
    events_tx: mpsc::Sender<InventoryEditEvent>,
    events_rx: mpsc::Receiver<InventoryEditEvent>,
    stop_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl InventoryEditController {
    pub(crate) fn new() -> Self {
        let (requests_tx, requests_rx) = mpsc::channel(REQUEST_CAPACITY);
        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        Self { handle: InventoryEditHandle::new(requests_tx), requests: Some(requests_rx), events_tx, events_rx, stop_tx: None, task: None }
    }

    pub(crate) fn handle(&self) -> InventoryEditHandle {
        self.handle.clone()
    }

    pub(crate) fn start(
        &mut self,
        runtime: &Handle,
        client: Client,
        network: SessionHandle,
        session: AgentSession,
        model: watch::Receiver<Option<InventorySnapshot>>,
        cancellation: Cancellation,
    ) {
        self.start_with_transport(
            runtime,
            client,
            EditSession { agent_id: session.agent_id, session_id: session.session_id },
            Arc::new(SessionMoveTransport(network)),
            model,
            cancellation,
        );
    }

    pub(super) fn start_with_transport(
        &mut self,
        runtime: &Handle,
        client: Client,
        session: EditSession,
        transport: Arc<dyn MoveTransport>,
        model: watch::Receiver<Option<InventorySnapshot>>,
        cancellation: Cancellation,
    ) {
        if self.task.is_some() {
            return;
        }
        let Some(requests) = self.requests.take() else { return };
        let (stop_tx, stop_rx) = oneshot::channel();
        self.stop_tx = Some(stop_tx);
        self.task =
            Some(runtime.spawn(run_edits(client, session, transport, model, requests, self.events_tx.clone(), cancellation, stop_rx)));
    }

    pub(crate) async fn next_event(&mut self) -> Option<InventoryEditEvent> {
        self.events_rx.recv().await
    }

    pub(crate) async fn stop(&mut self) {
        if let Some(stop) = self.stop_tx.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take()
            && let Err(error) = task.await
        {
            tracing::debug!(%error, "inventory edit worker ended during session shutdown");
        }
        self.requests.take();
    }
}

#[derive(Clone)]
struct EditCapabilities {
    ais: Option<CapabilityUrl>,
    legacy_create: Option<CapabilityUrl>,
}

enum PreparedEdit {
    Noop,
    CreateFolder { parent: CategoryKey, name: String, preferred_type: FolderType },
    RenameCategory { category: CategoryKey, name: String },
    RenameItem { item: ItemKey, name: String },
    Move(MoveExpectation),
    EmptyTrash(CategoryKey),
}

async fn run_edits(
    client: Client,
    session: EditSession,
    transport: Arc<dyn MoveTransport>,
    model: watch::Receiver<Option<InventorySnapshot>>,
    mut requests: mpsc::Receiver<EditRequest>,
    events: mpsc::Sender<InventoryEditEvent>,
    cancellation: Cancellation,
    mut stop: oneshot::Receiver<()>,
) {
    let mut capabilities = None;
    loop {
        let request = tokio::select! {
            _ = &mut stop => break,
            _ = cancellation.clone() => break,
            request = requests.recv() => request,
        };
        let Some(request) = request else { break };
        let process = process_request(&client, &session, transport.as_ref(), &model, &events, &mut capabilities, request.operation);
        tokio::pin!(process);
        let outcome = tokio::select! {
            _ = &mut stop => break,
            _ = cancellation.clone() => break,
            outcome = &mut process => outcome,
        };
        let _ = request.response.send(outcome);
    }
}

async fn process_request(
    client: &Client,
    session: &EditSession,
    transport: &dyn MoveTransport,
    model: &watch::Receiver<Option<InventorySnapshot>>,
    events: &mpsc::Sender<InventoryEditEvent>,
    capabilities: &mut Option<EditCapabilities>,
    operation: EditOperation,
) -> InventoryEditOutcome {
    let Some(snapshot) = model.borrow().clone() else { return InventoryEditOutcome::ServerError };
    let owner = snapshot.roots().personal().owner();
    if owner.id() != session.agent_id {
        return InventoryEditOutcome::ServerError;
    }
    let prepared = match prepare_edit(&operation, &snapshot, owner) {
        Ok(prepared) => prepared,
        Err(rejection) => return InventoryEditOutcome::Rejected(rejection),
    };
    match prepared {
        PreparedEdit::Noop => InventoryEditOutcome::Applied,
        prepared => execute_edit(client, session, transport, events, capabilities, snapshot, owner, operation, prepared).await,
    }
}

fn prepare_edit(
    operation: &EditOperation,
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
) -> Result<PreparedEdit, InventoryEditRejection> {
    match operation {
        EditOperation::CreateFolder { parent, name, preferred_type } => {
            validate_name(name)?;
            validate_owner(parent.owner(), owner)?;
            if snapshot.category(*parent).is_none() {
                return Err(InventoryEditRejection::MissingCategory);
            }
            Ok(PreparedEdit::CreateFolder { parent: *parent, name: name.clone(), preferred_type: *preferred_type })
        }
        EditOperation::RenameCategory { category, name } => {
            validate_name(name)?;
            validate_owner(category.owner(), owner)?;
            let category_data = snapshot.category(*category).ok_or(InventoryEditRejection::MissingCategory)?;
            validate_category_operation(*category, category_data.preferred_folder_type, snapshot)?;
            Ok(PreparedEdit::RenameCategory { category: *category, name: name.clone() })
        }
        EditOperation::RenameItem { item, name } => {
            validate_name(name)?;
            validate_owner(item.owner(), owner)?;
            snapshot.item(*item).ok_or(InventoryEditRejection::MissingItem)?;
            Ok(PreparedEdit::RenameItem { item: *item, name: name.clone() })
        }
        EditOperation::MoveItem { item, destination } => {
            validate_owner(item.owner(), owner)?;
            validate_owner(destination.owner(), owner)?;
            let current = snapshot.item(*item).ok_or(InventoryEditRejection::MissingItem)?;
            require_destination(*destination, snapshot)?;
            if current.parent == *destination {
                return Ok(PreparedEdit::Noop);
            }
            Ok(PreparedEdit::Move(MoveExpectation {
                node: InventoryNodeKey::new(owner, InventoryNodeId::Item(item.id())),
                from: current.parent,
                to: *destination,
            }))
        }
        EditOperation::MoveFolder { folder, destination } => {
            validate_owner(folder.owner(), owner)?;
            validate_owner(destination.owner(), owner)?;
            let current = snapshot.category(*folder).ok_or(InventoryEditRejection::MissingCategory)?;
            validate_category_operation(*folder, current.preferred_folder_type, snapshot)?;
            require_destination(*destination, snapshot)?;
            let from = current.parent.ok_or(InventoryEditRejection::RootFolder)?;
            if from == *destination {
                return Ok(PreparedEdit::Noop);
            }
            if folder_would_cycle(*folder, *destination, snapshot) {
                return Err(InventoryEditRejection::FolderCycle);
            }
            Ok(PreparedEdit::Move(MoveExpectation {
                node: InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id())),
                from,
                to: *destination,
            }))
        }
        EditOperation::DeleteItem(item) => {
            validate_owner(item.owner(), owner)?;
            let current = snapshot.item(*item).ok_or(InventoryEditRejection::MissingItem)?;
            let trash = trash_folder(snapshot, owner).ok_or(InventoryEditRejection::MissingTrash)?;
            if current.parent == trash {
                return Ok(PreparedEdit::Noop);
            }
            Ok(PreparedEdit::Move(MoveExpectation {
                node: InventoryNodeKey::new(owner, InventoryNodeId::Item(item.id())),
                from: current.parent,
                to: trash,
            }))
        }
        EditOperation::DeleteFolder(folder) => {
            validate_owner(folder.owner(), owner)?;
            let current = snapshot.category(*folder).ok_or(InventoryEditRejection::MissingCategory)?;
            validate_category_operation(*folder, current.preferred_folder_type, snapshot)?;
            let trash = trash_folder(snapshot, owner).ok_or(InventoryEditRejection::MissingTrash)?;
            let from = current.parent.ok_or(InventoryEditRejection::RootFolder)?;
            if from == trash {
                return Ok(PreparedEdit::Noop);
            }
            if folder_would_cycle(*folder, trash, snapshot) {
                return Err(InventoryEditRejection::FolderCycle);
            }
            Ok(PreparedEdit::Move(MoveExpectation {
                node: InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id())),
                from,
                to: trash,
            }))
        }
        EditOperation::EmptyTrash => {
            let trash = trash_folder(snapshot, owner).ok_or(InventoryEditRejection::MissingTrash)?;
            Ok(PreparedEdit::EmptyTrash(trash))
        }
    }
}

fn validate_name(name: &str) -> Result<(), InventoryEditRejection> {
    if name.len() > MAX_INVENTORY_NAME_BYTES { Err(InventoryEditRejection::InvalidName) } else { Ok(()) }
}

fn validate_owner(actual: InventoryOwner, expected: InventoryOwner) -> Result<(), InventoryEditRejection> {
    if actual == expected { Ok(()) } else { Err(InventoryEditRejection::DifferentOwner) }
}

fn validate_category_operation(
    key: CategoryKey,
    folder_type: FolderType,
    snapshot: &InventorySnapshot,
) -> Result<(), InventoryEditRejection> {
    if snapshot.roots().contains(key) {
        return Err(InventoryEditRejection::RootFolder);
    }
    if folder_type.is_protected() {
        return Err(InventoryEditRejection::ProtectedFolder);
    }
    Ok(())
}

fn require_destination(destination: CategoryKey, snapshot: &InventorySnapshot) -> Result<(), InventoryEditRejection> {
    snapshot.category(destination).map(|_| ()).ok_or(InventoryEditRejection::MissingCategory)
}

fn folder_would_cycle(folder: CategoryKey, destination: CategoryKey, snapshot: &InventorySnapshot) -> bool {
    let mut current = Some(destination);
    let mut visited = std::collections::HashSet::new();
    while let Some(key) = current {
        if key == folder || !visited.insert(key) {
            return true;
        }
        current = snapshot.category(key).and_then(|category| category.parent);
    }
    false
}

async fn execute_edit(
    client: &Client,
    session: &EditSession,
    transport: &dyn MoveTransport,
    events: &mpsc::Sender<InventoryEditEvent>,
    capabilities: &mut Option<EditCapabilities>,
    snapshot: InventorySnapshot,
    owner: InventoryOwner,
    operation: EditOperation,
    prepared: PreparedEdit,
) -> InventoryEditOutcome {
    match prepared {
        PreparedEdit::Move(expectation) => execute_move(transport, session, events, expectation).await,
        PreparedEdit::CreateFolder { parent, name, preferred_type } => {
            let capabilities = match get_capabilities(client, capabilities).await {
                Ok(capabilities) => capabilities,
                Err(outcome) => return outcome,
            };
            let response = if let Some(ais) = capabilities.ais.as_ref() {
                let url = match ais.with_path_segments(&["category", &parent.id().uuid().to_string()]) {
                    Ok(url) => url.with_query_pair("tid", &Uuid::new_v4().to_string()),
                    Err(_) => return InventoryEditOutcome::ServerError,
                };
                let body = create_folder_body(&name, preferred_type);
                match client.post_bytes_with_response_limit(&url, &body, MAX_EDIT_RESPONSE_BYTES).await {
                    Ok(body) => match inventory::decode_ais_edit_response(&body, owner.id()) {
                        Ok(response) => response,
                        Err(_) => return InventoryEditOutcome::ServerError,
                    },
                    Err(error) => return request_outcome(error),
                }
            } else {
                let Some(legacy) = capabilities.legacy_create.as_ref() else { return InventoryEditOutcome::ServerError };
                let folder_id = Uuid::new_v4();
                let body = legacy_create_body(folder_id, parent, preferred_type, &name);
                let created = match client.post_bytes_with_response_limit(legacy, &body, MAX_EDIT_RESPONSE_BYTES).await {
                    Ok(body) => match inventory::decode_created_category_response(&body) {
                        Ok(created) => created,
                        Err(_) => return InventoryEditOutcome::ServerError,
                    },
                    Err(error) => return request_outcome(error),
                };
                if created.id != folder_id
                    || created.parent_id != parent.id().uuid()
                    || created.name != name
                    || created.preferred_folder_type != preferred_type.raw()
                {
                    return InventoryEditOutcome::ServerError;
                }
                AisEditResponse {
                    category: None,
                    item: None,
                    created_categories: vec![folder_id],
                    created_items: Vec::new(),
                    updated_categories: Vec::new(),
                    updated_items: Vec::new(),
                    removed_items: Vec::new(),
                    categories_removed: Vec::new(),
                    category_items_removed: Vec::new(),
                    broken_links_removed: Vec::new(),
                    updated_category_versions: Default::default(),
                }
            };
            let confirmation = match delta_from_ais_response(&response, &operation, &snapshot, owner) {
                Ok(confirmation) => confirmation,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            let outcome = confirmation.created.map_or(InventoryEditOutcome::ServerError, InventoryEditOutcome::Created);
            apply_confirmation(events, confirmation.delta, outcome).await
        }
        PreparedEdit::RenameCategory { category, name } => {
            let capabilities = match require_ais(client, capabilities).await {
                Ok(capability) => capability,
                Err(outcome) => return outcome,
            };
            let url = match capabilities.with_path_segments(&["category", &category.id().uuid().to_string()]) {
                Ok(url) => url,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            let body = rename_body(&name);
            let response = match client.patch_bytes_with_response_limit(&url, &body, MAX_EDIT_RESPONSE_BYTES).await {
                Ok(body) => match inventory::decode_ais_edit_response(&body, owner.id()) {
                    Ok(response) => response,
                    Err(_) => return InventoryEditOutcome::ServerError,
                },
                Err(error) => return request_outcome(error),
            };
            let confirmation = match delta_from_ais_response(&response, &operation, &snapshot, owner) {
                Ok(confirmation) => confirmation,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            apply_confirmation(events, confirmation.delta, InventoryEditOutcome::Applied).await
        }
        PreparedEdit::RenameItem { item, name } => {
            let capabilities = match require_ais(client, capabilities).await {
                Ok(capability) => capability,
                Err(outcome) => return outcome,
            };
            let url = match capabilities.with_path_segments(&["item", &item.id().uuid().to_string()]) {
                Ok(url) => url,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            let body = rename_body(&name);
            let response = match client.patch_bytes_with_response_limit(&url, &body, MAX_EDIT_RESPONSE_BYTES).await {
                Ok(body) => match inventory::decode_ais_edit_response(&body, owner.id()) {
                    Ok(response) => response,
                    Err(_) => return InventoryEditOutcome::ServerError,
                },
                Err(error) => return request_outcome(error),
            };
            let confirmation = match delta_from_ais_response(&response, &operation, &snapshot, owner) {
                Ok(confirmation) => confirmation,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            apply_confirmation(events, confirmation.delta, InventoryEditOutcome::Applied).await
        }
        PreparedEdit::EmptyTrash(trash) => {
            let capabilities = match require_ais(client, capabilities).await {
                Ok(capability) => capability,
                Err(outcome) => return outcome,
            };
            let url = match capabilities.with_path_segments(&["category", &trash.id().uuid().to_string(), "children"]) {
                Ok(url) => url,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            let response = match client.delete_bytes_with_response_limit(&url, MAX_EDIT_RESPONSE_BYTES).await {
                Ok(body) => match inventory::decode_ais_edit_response(&body, owner.id()) {
                    Ok(response) => response,
                    Err(_) => return InventoryEditOutcome::ServerError,
                },
                Err(error) if request_is_gone(&error) => {
                    if refresh_gone(events, trash).await {
                        return InventoryEditOutcome::Gone;
                    }
                    return InventoryEditOutcome::ServerError;
                }
                Err(error) => return request_outcome(error),
            };
            let confirmation = match delta_from_ais_response(&response, &operation, &snapshot, owner) {
                Ok(confirmation) => confirmation,
                Err(_) => return InventoryEditOutcome::ServerError,
            };
            apply_confirmation(events, confirmation.delta, InventoryEditOutcome::Applied).await
        }
        PreparedEdit::Noop => InventoryEditOutcome::Applied,
    }
}

async fn get_capabilities(client: &Client, cached: &mut Option<EditCapabilities>) -> Result<EditCapabilities, InventoryEditOutcome> {
    if let Some(capabilities) = cached {
        return Ok(capabilities.clone());
    }
    let discovered = client.request_capabilities(&["InventoryAPIv3", "CreateInventoryCategory"]).await.map_err(request_outcome)?;
    let capabilities = EditCapabilities {
        ais: discovered.get("InventoryAPIv3").cloned(),
        legacy_create: discovered.get("CreateInventoryCategory").cloned(),
    };
    *cached = Some(capabilities.clone());
    Ok(capabilities)
}

async fn require_ais(client: &Client, cached: &mut Option<EditCapabilities>) -> Result<CapabilityUrl, InventoryEditOutcome> {
    get_capabilities(client, cached).await?.ais.ok_or(InventoryEditOutcome::ServerError)
}

fn create_folder_body(name: &str, preferred_type: FolderType) -> Value {
    let mut category = Map::from([("name".into(), Value::String(name.to_owned()))]);
    if preferred_type != FolderType::None {
        category.insert("type_default".into(), Value::Integer(preferred_type.raw()));
    }
    Value::Map(Map::from([("categories".into(), Value::Array(vec![Value::Map(category)]))]))
}

fn legacy_create_body(folder: Uuid, parent: CategoryKey, preferred_type: FolderType, name: &str) -> Value {
    Value::Map(Map::from([
        ("folder_id".into(), Value::Uuid(folder)),
        ("parent_id".into(), Value::Uuid(parent.id().uuid())),
        ("type".into(), Value::Integer(preferred_type.raw())),
        ("name".into(), Value::String(name.to_owned())),
    ]))
}

fn rename_body(name: &str) -> Value {
    Value::Map(Map::from([("name".into(), Value::String(name.to_owned()))]))
}

async fn apply_confirmation(
    events: &mpsc::Sender<InventoryEditEvent>,
    delta: InventoryDelta,
    outcome: InventoryEditOutcome,
) -> InventoryEditOutcome {
    let (applied, result) = oneshot::channel();
    if events.send(InventoryEditEvent::Apply { delta, applied }).await.is_err() {
        return InventoryEditOutcome::ServerError;
    }
    match result.await {
        Ok(true) => outcome,
        Ok(false) | Err(_) => InventoryEditOutcome::ServerError,
    }
}

async fn refresh_gone(events: &mpsc::Sender<InventoryEditEvent>, folder: CategoryKey) -> bool {
    let (refreshed, result) = oneshot::channel();
    if events.send(InventoryEditEvent::RefreshGone { folder, refreshed }).await.is_err() {
        return false;
    }
    matches!(result.await, Ok(true))
}

async fn execute_move(
    transport: &dyn MoveTransport,
    session: &EditSession,
    events: &mpsc::Sender<InventoryEditEvent>,
    expectation: MoveExpectation,
) -> InventoryEditOutcome {
    let (confirmed, confirmation) = oneshot::channel();
    let (registered, registration) = oneshot::channel();
    if events.send(InventoryEditEvent::MoveStarted { expectation, confirmed, registered }).await.is_err() || registration.await.is_err() {
        return InventoryEditOutcome::ServerError;
    }
    let movement = match expectation.node.id() {
        InventoryNodeId::Item(id) => InventoryMove::Item(MoveInventoryItem {
            agent_id: session.agent_id,
            session_id: session.session_id,
            stamp: false,
            inventory_data: vec![MoveInventoryItemBlock {
                item_id: id.uuid(),
                folder_id: expectation.to.id().uuid(),
                new_name: String::new(),
            }],
            invalid_blocks: 0,
        }),
        InventoryNodeId::Category(id) => InventoryMove::Folder(MoveInventoryFolder {
            agent_id: session.agent_id,
            session_id: session.session_id,
            stamp: false,
            inventory_data: vec![MoveInventoryFolderBlock { folder_id: id.uuid(), parent_id: expectation.to.id().uuid() }],
            invalid_blocks: 0,
        }),
    };
    if transport.send(movement).is_err() {
        let (removed, result) = oneshot::channel();
        if events.send(InventoryEditEvent::MoveAborted { expectation, removed }).await.is_ok() {
            let _ = result.await;
        }
        return InventoryEditOutcome::ServerError;
    }

    let confirmation = time::timeout(MOVE_CONFIRMATION_TIMEOUT, confirmation).await;
    match confirmation {
        Ok(Ok(())) => InventoryEditOutcome::Applied,
        Ok(Err(_)) => InventoryEditOutcome::Rejected(InventoryEditRejection::SessionEnded),
        Err(_) => {
            let (refreshed, result) = oneshot::channel();
            if events.send(InventoryEditEvent::MoveTimedOut { expectation, refreshed }).await.is_err() {
                return InventoryEditOutcome::ServerError;
            }
            if matches!(result.await, Ok(true)) { InventoryEditOutcome::TimedOut } else { InventoryEditOutcome::ServerError }
        }
    }
}

fn request_outcome(error: network::Error) -> InventoryEditOutcome {
    if matches!(
        &error,
        network::Error::SessionClosed
            | network::Error::LoginEnded
            | network::Error::CapabilityRequestCanceled
            | network::Error::CapabilitySessionClosed
    ) {
        return InventoryEditOutcome::Rejected(InventoryEditRejection::SessionEnded);
    }
    if request_is_gone(&error) {
        return InventoryEditOutcome::Gone;
    }
    if matches!(error, network::Error::CapabilityHttp(ref http) if http.kind == HttpErrorKind::Timeout) {
        return InventoryEditOutcome::TimedOut;
    }
    InventoryEditOutcome::ServerError
}

fn request_is_gone(error: &network::Error) -> bool {
    matches!(error, network::Error::CapabilityHttp(http) if http.status.is_some_and(|status| status.as_u16() == 410))
}
