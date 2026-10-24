use std::{future::Future, time::Duration};

use futures::{FutureExt, pin_mut};
use network::Event;

use crate::{
    session::{environment::Completion, object_media::ObjectMediaResult, terrain::DecodedTerrain},
    world_reader::{ReadRequest, WorldReadHost},
};

const CAMERA_TICK_INTERVAL: Duration = Duration::from_millis(25);

pub(super) enum Input {
    Cancelled,
    Event(Option<Event>),
    Media(Option<ObjectMediaResult>),
    Terrain(DecodedTerrain),
    Environment(Option<Box<Completion>>),
    WorldRead(ReadRequest),
    CameraTick,
}

pub(super) fn next_camera_tick_delay(elapsed: Duration) -> Duration {
    let elapsed_in_interval = Duration::from_nanos((elapsed.as_nanos() % CAMERA_TICK_INTERVAL.as_nanos()) as u64);
    CAMERA_TICK_INTERVAL - elapsed_in_interval
}

pub(super) async fn select_drain_input<E, M, T, N, C, Q>(
    reads: &mut WorldReadHost,
    event: E,
    media: M,
    terrain: T,
    environment: N,
    cancellation: C,
    camera_tick: Q,
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
    let camera_tick = camera_tick.fuse();
    let input = {
        let read = reads.next_request().fuse();
        pin_mut!(event, media, terrain, environment, cancellation, camera_tick, read);
        futures::select_biased! {
            _ = cancellation => Input::Cancelled,
            _ = camera_tick => Input::CameraTick,
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
