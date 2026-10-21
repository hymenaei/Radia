use super::*;
use crate::session::world::{SessionWorld, viewer_world_limits};
use futures::{channel::oneshot, future::ready};
use network::testing::simulator_id;
use protocol::lludp::{BlockValues, SchemaMessage};
use world::{OrderEvidence, Region};

fn source(order: u32) -> UpdateSource {
    UpdateSource::new(Region::new(1), order, OrderEvidence::Ordered(u64::from(order)), 0)
}

fn flat() -> Vec<u8> {
    vec![8, 1, 16, b'L', 0, 0, 0, 32, 65, 20, 0, 101, 38, 16]
}

fn message(data: Vec<u8>) -> Message {
    Message::Schema(
        SchemaMessage::new(
            SchemaMessage::schema_by_name("LayerData").unwrap(),
            vec![BlockValues::single(vec![FieldValue::U8(b'L')]), BlockValues::single(vec![FieldValue::Bytes(data)])],
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn ordered_jobs_cancel_safely() {
    let mut work = TerrainDecodeQueue::default();
    let mut release = Vec::new();
    for order in 1..=MAX_TERRAIN_JOBS as u32 {
        let (send, receive) = oneshot::channel();
        release.push(send);
        work.jobs.push_back(
            async move {
                receive.await.unwrap();
                DecodedTerrain { source: source(order), patches: Vec::new(), rejected: 0 }
            }
            .boxed(),
        );
    }
    assert!(work.is_full());
    release.pop().unwrap().send(()).unwrap();
    assert!(matches!(work.select(ready(7)).await, Ok(7)));
    for send in release {
        send.send(()).unwrap();
    }
    for order in 1..=MAX_TERRAIN_JOBS as u32 {
        let Err(decoded) = work.select(pending::<()>()).await else { unreachable!() };
        assert_eq!(decoded.source.sequence(), order);
        assert!(!work.is_full());
    }
    let cancelled = Arc::clone(&work.cancelled);
    drop(work);
    let decoded = decode_terrain_patches(source(1), &flat(), &cancelled);
    assert!(decoded.patches.is_empty());
}

#[tokio::test]
async fn intake_rejects_retired_regions() {
    let mut world = SessionWorld::new(viewer_world_limits());
    let simulator = simulator_id();
    let region = world.register_simulator(simulator, None).unwrap();
    let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
    let mut work = TerrainDecodeQueue::default();
    work.enqueue(&message(flat()), source, &Handle::current());
    let Err(decoded) = work.select(pending::<()>()).await else { unreachable!() };
    assert_eq!(decoded.rejected, 0);
    world.apply_terrain(decoded);
    let page = world.world().terrain_page(region, None, 8).unwrap().unwrap();
    assert_eq!(page.known_patches, 1);
    assert_eq!(page.patches[0].heights(), &[20.0; 256]);
    let mut truncated = flat();
    truncated.pop();
    let partial = decode_terrain_patches(source, &truncated, &AtomicBool::new(false));
    assert_eq!(partial.patches.len(), 1);
    assert_eq!(partial.rejected, 1);
    let late = decode_terrain_patches(source, &flat(), &AtomicBool::new(false));
    world.retire_all();
    world.apply_terrain(late);
    assert_eq!(world.world().terrain_page(region, None, 8), Ok(None));
}
