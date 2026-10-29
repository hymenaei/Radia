use std::future::Future;

use futures::{FutureExt, pin_mut};
use network::Event;

use crate::session::teleport::TeleportInput;
use crate::{
    inventory::InventoryFetchEvent,
    session::{environment::Completion, object_media::ObjectMediaResult, recovery::RecoveryCompletion, terrain::DecodedTerrain},
    world_reader::{ReadRequest, WorldReadHost},
};

pub(super) enum Input {
    Cancelled,
    Event(Option<Event>),
    Media(Option<ObjectMediaResult>),
    Terrain(DecodedTerrain),
    Environment(Option<Box<Completion>>),
    WorldRead(ReadRequest),
    CameraTick,
    Recovery(RecoveryCompletion),
    InventoryFetch(Option<InventoryFetchEvent>),
    Teleport(TeleportInput),
}

pub(super) async fn select_drain_input<E, M, T, N, C, Q, R, I, P>(
    reads: &mut WorldReadHost,
    event: E,
    media: M,
    terrain: T,
    environment: N,
    cancellation: C,
    camera_tick: Q,
    recovery: R,
    inventory_fetch: I,
    teleport: P,
) -> Input
where
    E: Future<Output = Option<Event>>,
    M: Future<Output = Option<ObjectMediaResult>>,
    T: Future<Output = DecodedTerrain>,
    N: Future<Output = Option<Box<Completion>>>,
    C: Future<Output = ()> + Clone,
    Q: Future<Output = tokio::time::Instant>,
    R: Future<Output = RecoveryCompletion>,
    I: Future<Output = Option<InventoryFetchEvent>>,
    P: Future<Output = TeleportInput>,
{
    let event = event.fuse();
    let media = media.fuse();
    let terrain = terrain.fuse();
    let environment = environment.fuse();
    let recovery = recovery.fuse();
    let inventory_fetch = inventory_fetch.fuse();
    let teleport = teleport.fuse();
    if cancellation.clone().now_or_never().is_some() {
        return Input::Cancelled;
    }
    if let Some(request) = reads.take_reserved() {
        return Input::WorldRead(request);
    }
    let cancellation = cancellation.fuse();
    let camera_tick = camera_tick.fuse();
    let input = {
        let read = reads.next_request().fuse();
        pin_mut!(event, media, terrain, environment, cancellation, camera_tick, read, recovery, inventory_fetch, teleport);
        futures::select_biased! {
            _ = cancellation => Input::Cancelled,
            teleport = teleport => Input::Teleport(teleport),
            _ = camera_tick => Input::CameraTick,
            decoded = terrain => Input::Terrain(decoded),
            result = media => Input::Media(result),
            completion = environment => Input::Environment(completion),
            recovery = recovery => Input::Recovery(recovery),
            event = event => Input::Event(event),
            inventory_fetch = inventory_fetch => Input::InventoryFetch(inventory_fetch),
            request = read => Input::WorldRead(request),
        }
    };
    if !matches!(&input, Input::WorldRead(_)) {
        reads.record_intake_turn();
    }
    input
}
