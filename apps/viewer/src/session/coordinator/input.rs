use std::{future::Future, time::Duration};

use futures::{FutureExt, pin_mut};
use network::Event;

use crate::{
    session::{environment::Completion, object_media::ObjectMediaResult, terrain::DecodedTerrain},
    world_reader::{ReadRequest, WorldReadHost},
};

const QUERY_STATE_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(25);
const QUERY_STATE_MAINTENANCE_RETRY: Duration = Duration::from_millis(1);

pub(super) enum Input {
    Cancelled,
    Event(Option<Event>),
    Media(Option<ObjectMediaResult>),
    Terrain(DecodedTerrain),
    Environment(Option<Box<Completion>>),
    WorldRead(ReadRequest),
    QueryStateMaintenance,
}

pub(super) fn next_query_state_maintenance_delay(elapsed: Duration, has_dirty_pressure: bool) -> Duration {
    if has_dirty_pressure {
        return QUERY_STATE_MAINTENANCE_RETRY;
    }
    let elapsed_in_interval = Duration::from_nanos((elapsed.as_nanos() % QUERY_STATE_MAINTENANCE_INTERVAL.as_nanos()) as u64);
    QUERY_STATE_MAINTENANCE_INTERVAL - elapsed_in_interval
}

pub(super) async fn select_drain_input<E, M, T, N, C, Q>(
    reads: &mut WorldReadHost,
    event: E,
    media: M,
    terrain: T,
    environment: N,
    cancellation: C,
    maintenance: Q,
) -> Input
where
    E: Future<Output = Option<Event>>,
    M: Future<Output = Option<ObjectMediaResult>>,
    T: Future<Output = DecodedTerrain>,
    N: Future<Output = Option<Box<Completion>>>,
    C: Future<Output = ()> + Clone,
    Q: Future<Output = ()>,
{
    let event = event.fuse();
    let media = media.fuse();
    let terrain = terrain.fuse();
    let environment = environment.fuse();
    if cancellation.clone().now_or_never().is_some() {
        return Input::Cancelled;
    }
    if let Some(request) = reads.take_reserved() {
        return Input::WorldRead(request);
    }
    let cancellation = cancellation.fuse();
    let maintenance = maintenance.fuse();
    let input = {
        let read = reads.next_request().fuse();
        pin_mut!(event, media, terrain, environment, cancellation, maintenance, read);
        futures::select_biased! {
            _ = cancellation => Input::Cancelled,
            _ = maintenance => Input::QueryStateMaintenance,
            decoded = terrain => Input::Terrain(decoded),
            result = media => Input::Media(result),
            completion = environment => Input::Environment(completion),
            event = event => Input::Event(event),
            request = read => Input::WorldRead(request),
        }
    };
    if !matches!(&input, Input::WorldRead(_)) {
        reads.record_intake_turn();
    }
    input
}
