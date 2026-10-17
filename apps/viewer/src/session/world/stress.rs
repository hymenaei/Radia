use std::{
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use base::UUID;
use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div};
use math::{DVec3, Quat, Vec3};
use network::{CoverageLossReason, CoverageSource, CoverageWatermark, SimulatorId};
use protocol::messages::RequestAgentData;
use tokio::sync::oneshot;
use world::{
    Kind, LocalAddress, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_PAGE_ITEMS, MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_WORLD_QUERY_RESULTS,
    MAX_WORLD_QUERY_WORK, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, Pose, QueryCompleteness, QueryIncompleteReason,
    Region, RegionInfo, UpdateSource, WorldQuery, WorldQueryBudget,
};

use crate::devtools::{DeveloperFloater, WorldSource};
use crate::session::{
    coverage::{RecoveryKey, RecoveryKind},
    inbound::{ObjectBatch, ObjectOperation},
    recovery::{MAX_PENDING_RECOVERY_BYTES, MAX_PENDING_RECOVERY_TARGETS, RecoveryRetryQueue},
};

use super::{
    SessionWorld, WORLD_READ_QUEUE_CAPACITY, WorldReadPriority, WorldReadRequest, WorldReader, WorldReaderError, viewer_world_limits,
};

const PER_REGION: usize = 10_000;
const LINKSET_SIZE: usize = 64;
const RECOVERY_STRIDE: usize = 512;
const INSPECTOR_BATCH: usize = MAX_OBJECT_DETAIL_BATCH_ITEMS;
const LOAD_SEED: u64 = 0x5eed_0207;
const LOAD_DURATION: std::time::Duration = std::time::Duration::from_secs(10);
const ORDER_STRIDE: u64 = 100_000;
const SAMPLE_CAPACITY: usize = 4_096;
const FLOATER_PROFILE_OBJECTS: usize = 256;
const FLOATER_PROFILE_ROUNDS: usize = 3;

struct FloaterProfileHost {
    floater: Entity<DeveloperFloater>,
}

impl Render for FloaterProfileHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.floater.clone())
    }
}

struct RegionWorkload {
    churn: Vec<Option<ObjectOperation>>,
    recover: Vec<Option<ObjectOperation>>,
    final_moves: Vec<Option<ObjectOperation>>,
    parent_operations: usize,
}

#[derive(Default)]
struct QueueMetrics {
    renderer_depth: AtomicUsize,
    inspector_depth: AtomicUsize,
}

#[derive(Default)]
struct ReadSamples {
    summaries: Vec<u64>,
    queries: Vec<u64>,
    details: Vec<u64>,
    renderer_busy: Vec<u64>,
    inspector_busy: Vec<u64>,
}

impl ReadSamples {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            summaries: Vec::with_capacity(capacity),
            queries: Vec::with_capacity(capacity),
            details: Vec::with_capacity(capacity),
            renderer_busy: Vec::with_capacity(capacity),
            inspector_busy: Vec::with_capacity(capacity),
        }
    }

    fn append(&mut self, mut other: Self) {
        self.summaries.append(&mut other.summaries);
        self.queries.append(&mut other.queries);
        self.details.append(&mut other.details);
        self.renderer_busy.append(&mut other.renderer_busy);
        self.inspector_busy.append(&mut other.inspector_busy);
    }
}

fn report_read_samples(label: &str, count: usize, samples: &ReadSamples) {
    eprintln!(
        "{label} {count}: renderer barriers={} p50/p95/p99={}us/{}us/{}us queries={} p50/p95/p99={}us/{}us/{}us busy={} reject-p50/p95/p99={}us/{}us/{}us; inspector details={} p50/p95/p99={}us/{}us/{}us busy={} reject-p50/p95/p99={}us/{}us/{}us",
        samples.summaries.len(),
        percentile_us(&samples.summaries, 50),
        percentile_us(&samples.summaries, 95),
        percentile_us(&samples.summaries, 99),
        samples.queries.len(),
        percentile_us(&samples.queries, 50),
        percentile_us(&samples.queries, 95),
        percentile_us(&samples.queries, 99),
        samples.renderer_busy.len(),
        percentile_us(&samples.renderer_busy, 50),
        percentile_us(&samples.renderer_busy, 95),
        percentile_us(&samples.renderer_busy, 99),
        samples.details.len(),
        percentile_us(&samples.details, 50),
        percentile_us(&samples.details, 95),
        percentile_us(&samples.details, 99),
        samples.inspector_busy.len(),
        percentile_us(&samples.inspector_busy, 50),
        percentile_us(&samples.inspector_busy, 95),
        percentile_us(&samples.inspector_busy, 99),
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run manually to collect the sustained SessionWorld baseline"]
async fn sustained_world_load() {
    for count in [40_000_usize, 65_536] {
        measure_session_load(count).await;
    }
}

#[gpui::test]
fn developer_floater_scene_load_profile(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);

    let mut session = SessionWorld::new(viewer_world_limits());
    let simulator = network::testing::simulator_id();
    let region = session.register_simulator(simulator, None).expect("register profile simulator");
    session
        .update_region_metadata(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() })
        .expect("set profile region origin");

    let mut initial = Vec::with_capacity(FLOATER_PROFILE_OBJECTS);
    for offset in 0..FLOATER_PROFILE_OBJECTS {
        let id = local_id(offset);
        let mut object = observation(region, offset, id, id, 100_000 + offset as u64);
        object.pose = Some(Pose { position: Some(root_position(offset)), rotation: Some(Quat::IDENTITY) });
        initial.push(Some(ObjectOperation::Observe(object)));
    }
    let seed = session
        .apply_object_batch(simulator, ObjectBatch { region, region_handle: None, blocks: initial, invalid_blocks: 0, invalid_fields: 0 })
        .expect("seed profile scene");
    assert!(seed.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));
    repair(&mut session);

    let mut order = 1_000_000;
    let mut closed_samples = Vec::with_capacity(FLOATER_PROFILE_ROUNDS);
    for round in 0..FLOATER_PROFILE_ROUNDS {
        closed_samples.push(profile_scene_round(&mut session, simulator, region, round, &mut order));
    }
    assert!(session.try_read_requests().is_empty(), "closed floater should not queue World reads");

    let source = WorldSource::new();
    source.set(Some(session.reader()));
    let opening = Instant::now();
    let (host, visual) = cx.add_window_view(|window, cx| {
        let floater = cx.new(|cx| DeveloperFloater::new(source, window, cx));
        FloaterProfileHost { floater }
    });
    let floater = visual.update(|_, app| host.read(app).floater.clone());
    let mut open_batches = service_profile_reads(&mut session, visual);
    let initial_open_elapsed = opening.elapsed();
    assert!(open_batches > 0, "open floater should capture World data");
    assert_captured_world(&floater, visual, &session, FLOATER_PROFILE_OBJECTS);

    let mut open_samples = Vec::with_capacity(FLOATER_PROFILE_ROUNDS);
    let mut open_cycle_samples = Vec::with_capacity(FLOATER_PROFILE_ROUNDS);
    for round in 0..FLOATER_PROFILE_ROUNDS {
        let cycle_started = Instant::now();
        open_samples.push(profile_scene_round(&mut session, simulator, region, round + FLOATER_PROFILE_ROUNDS, &mut order));
        visual.executor().advance_clock(Duration::from_secs(1));
        open_batches += service_profile_reads(&mut session, visual);
        open_cycle_samples.push(cycle_started.elapsed());
        assert_captured_world(&floater, visual, &session, FLOATER_PROFILE_OBJECTS);
    }

    pause_profile_updates(&floater, visual);
    let mut paused_samples = Vec::with_capacity(FLOATER_PROFILE_ROUNDS);
    let mut paused_cycle_samples = Vec::with_capacity(FLOATER_PROFILE_ROUNDS);
    for round in 0..FLOATER_PROFILE_ROUNDS {
        let cycle_started = Instant::now();
        paused_samples.push(profile_scene_round(&mut session, simulator, region, round + 2 * FLOATER_PROFILE_ROUNDS, &mut order));
        session.coverage.record_source_loss(
            CoverageWatermark {
                simulator_id: simulator,
                source: CoverageSource::Lludp,
                generation: round as u64 + 1,
                overflow_generation: 0,
            },
            CoverageLossReason::EventAdmission,
            false,
        );
        visual.executor().advance_clock(Duration::from_secs(1));
        assert_eq!(service_profile_reads(&mut session, visual), 0, "paused floater should not refresh after coverage changes");
        paused_cycle_samples.push(cycle_started.elapsed());
        let (captured_revision, captured_count) = captured_world_state(&floater, visual);
        assert_eq!(captured_count, FLOATER_PROFILE_OBJECTS);
        assert_ne!(captured_revision, Some(session.world().revision()), "pause should retain its last captured snapshot");
    }

    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
    let after_close = profile_scene_round(&mut session, simulator, region, 3 * FLOATER_PROFILE_ROUNDS, &mut order);
    visual.executor().advance_clock(Duration::from_secs(1));
    assert!(session.try_read_requests().is_empty(), "closed floater should stop World reads");

    let closed_ns = durations_ns(&closed_samples);
    let open_ns = durations_ns(&open_samples);
    let open_cycle_ns = durations_ns(&open_cycle_samples);
    let paused_ns = durations_ns(&paused_samples);
    let paused_cycle_ns = durations_ns(&paused_cycle_samples);
    eprintln!(
        "Developer Tools changing-scene profile (GPUI test platform; UI layout exercised, native frame timing omitted): objects={FLOATER_PROFILE_OBJECTS}, rounds={FLOATER_PROFILE_ROUNDS}; closed apply+repair p50/max={}us/{}us; initial open+capture={}us; open inspector request batches={open_batches}, apply+repair p50/max={}us/{}us, apply+capture+read p50/max={}us/{}us; paused inspector request batches=0, apply+repair p50/max={}us/{}us, apply+poll p50/max={}us/{}us; after-close apply+repair={}us",
        percentile_us(&closed_ns, 50),
        percentile_us(&closed_ns, 100),
        initial_open_elapsed.as_nanos() / 1_000,
        percentile_us(&open_ns, 50),
        percentile_us(&open_ns, 100),
        percentile_us(&open_cycle_ns, 50),
        percentile_us(&open_cycle_ns, 100),
        percentile_us(&paused_ns, 50),
        percentile_us(&paused_ns, 100),
        percentile_us(&paused_cycle_ns, 50),
        percentile_us(&paused_cycle_ns, 100),
        after_close.as_nanos() / 1_000,
    );
}

#[gpui::test]
fn pause_stops_paging_until_resume(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);

    let mut session = SessionWorld::new(viewer_world_limits());
    let simulator = network::testing::simulator_id();
    let region = session.register_simulator(simulator, None).expect("register simulator");
    let blocks = (0..=MAX_OBJECT_PAGE_ITEMS)
        .map(|offset| {
            let id = local_id(offset);
            Some(ObjectOperation::Observe(observation(region, offset, id, id, 100_000 + offset as u64)))
        })
        .collect();
    session
        .apply_object_batch(simulator, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0, invalid_fields: 0 })
        .expect("seed paged World snapshot");
    repair(&mut session);

    let source = WorldSource::new();
    source.set(Some(session.reader()));
    let (host, visual) = cx.add_window_view(|window, cx| {
        let floater = cx.new(|cx| DeveloperFloater::new(source, window, cx));
        FloaterProfileHost { floater }
    });
    let floater = visual.update(|_, app| host.read(app).floater.clone());

    let mut paused_before_page = false;
    for _ in 0..10_000 {
        visual.run_until_parked();
        let requests = session.try_read_requests();
        if requests.is_empty() {
            continue;
        }
        for request in requests.into_iter() {
            if matches!(&request, WorldReadRequest::ObjectListPage { cursor: None, .. }) {
                pause_profile_updates(&floater, visual);
                session.service_read_request(request);
                paused_before_page = true;
                break;
            }
            session.service_read_request(request);
        }
        if paused_before_page {
            break;
        }
    }
    assert!(paused_before_page, "initial capture should reach its first object page");

    visual.run_until_parked();
    assert!(session.try_read_requests().is_empty(), "pausing should cancel the capture before it requests another page");
    assert_eq!(captured_world_state(&floater, visual), (None, 0));

    pause_profile_updates(&floater, visual);
    visual.executor().advance_clock(Duration::from_secs(1));
    assert!(service_profile_reads(&mut session, visual) > 0, "resuming should request a fresh snapshot");
    assert_eq!(captured_world_state(&floater, visual), (Some(session.world().revision()), MAX_OBJECT_PAGE_ITEMS + 1));

    visual.update(|window, _| window.remove_window());
}

fn captured_world_state(
    floater: &gpui::Entity<DeveloperFloater>,
    cx: &mut gpui::VisualTestContext,
) -> (Option<world::WorldRevision>, usize) {
    cx.update(|_, app| floater.read(app).captured_world_state())
}

fn assert_captured_world(
    floater: &gpui::Entity<DeveloperFloater>,
    cx: &mut gpui::VisualTestContext,
    session: &SessionWorld,
    expected_objects: usize,
) {
    let (revision, object_count) = captured_world_state(floater, cx);
    assert_eq!(revision, Some(session.world().revision()), "open floater should publish the latest scene revision");
    assert_eq!(object_count, expected_objects, "open floater should publish the scene object count");
}

fn pause_profile_updates(floater: &gpui::Entity<DeveloperFloater>, cx: &mut gpui::VisualTestContext) {
    cx.update(|_, app| floater.update(app, |view, cx| view.toggle_pause_for_test(cx)));
}

fn service_profile_reads(session: &mut SessionWorld, cx: &mut gpui::VisualTestContext) -> usize {
    let mut serviced = 0;
    for _ in 0..10_000 {
        cx.run_until_parked();
        let requests = session.try_read_requests();
        if requests.is_empty() {
            cx.update(|window, app| _ = window.draw(app));
            return serviced;
        }
        serviced += usize::from(requests.inspector.is_some());
        for request in requests.into_iter() {
            session.service_read_request(request);
        }
    }
    panic!("Developer Tools profile did not drain the bounded read queue");
}

fn profile_scene_round(
    session: &mut SessionWorld,
    simulator: network::SimulatorId,
    region: Region,
    round: usize,
    order: &mut u64,
) -> std::time::Duration {
    let blocks = (0..FLOATER_PROFILE_OBJECTS)
        .map(|offset| {
            let id = local_id(offset);
            let mut patch = ObjectPatch::new(source(region, order), id);
            patch.pose = Some(Pose {
                position: Some(root_position(offset) + Vec3::new(round as f32 + 1.0, 0.0, 0.0)),
                rotation: Some(Quat::IDENTITY),
            });
            Some(ObjectOperation::Patch(patch))
        })
        .collect();
    let started = Instant::now();
    let summary = session
        .apply_object_batch(simulator, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0, invalid_fields: 0 })
        .expect("apply changing-scene profile batch");
    assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));
    repair(session);
    started.elapsed()
}

fn durations_ns(samples: &[std::time::Duration]) -> Vec<u64> {
    samples.iter().map(|sample| sample.as_nanos() as u64).collect()
}

async fn measure_session_load(count: usize) {
    let mut session = SessionWorld::new(viewer_world_limits());
    let region_count = count.div_ceil(PER_REGION);
    let mut regions = Vec::with_capacity(region_count);
    let mut total_roots = 0;
    for region_index in 0..region_count {
        let item_count = (count - region_index * PER_REGION).min(PER_REGION);
        let simulator = network::testing::simulator_id();
        let region = session.register_simulator(simulator, None).expect("register simulator");
        session
            .update_region_metadata(region, RegionInfo { origin: Some(region_origin(region_index)), ..RegionInfo::default() })
            .expect("set region origin");
        let mut initial = Vec::with_capacity(item_count);
        for offset in 0..item_count {
            let local_id = local_id(offset);
            let parent = parent_local_id(offset);
            let position = if parent == local_id { root_position(offset / LINKSET_SIZE) } else { Vec3::ZERO };
            let global_offset = region_index * PER_REGION + offset;
            let mut object = observation(region, global_offset, local_id, parent, 100_000 + offset as u64);
            object.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
            initial.push(Some(ObjectOperation::Observe(object)));
            total_roots += usize::from(parent == local_id);
        }
        let summary = session
            .apply_object_batch(
                simulator,
                ObjectBatch { region, region_handle: None, blocks: initial, invalid_blocks: 0, invalid_fields: 0 },
            )
            .expect("apply initial scene");
        assert_eq!(summary.invalid_blocks, 0);
        assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));

        regions.push((simulator, region, item_count));
    }

    repair(&mut session);
    assert_eq!(session.world().object_count(), count);

    let detail_entities =
        (0..INSPECTOR_BATCH).map(|index| session.world().entity(object_uuid(index)).expect("seed inspector entity")).collect::<Vec<_>>();
    assert_eq!(detail_entities.len(), INSPECTOR_BATCH);

    let renderer = session.reader();
    let inspector = renderer.inspector();
    let queues = Arc::new(QueueMetrics::default());
    let source_loss =
        CoverageWatermark { simulator_id: regions[0].0, source: CoverageSource::Lludp, generation: 1, overflow_generation: 0 };
    session.coverage.record_source_loss(source_loss, CoverageLossReason::EventAdmission, false);

    let mut recovery_queue = RecoveryRetryQueue::new(MAX_PENDING_RECOVERY_TARGETS, MAX_PENDING_RECOVERY_BYTES);
    let agent_data = RequestAgentData { agent_id: UUID::from_u128(0xA61), session_id: UUID::from_u128(0xB72) };
    let mut recovery_targets = 0;
    let mut parent_operations = 0;
    let mut repair_calls = 0;
    let mut repair_work = 0;
    let mut max_dirty_scopes = 0;
    let mut repair_renderer_reads = 0;
    let mut repair_inspector_reads = 0;
    let mut rounds = 0;
    let mut final_revision = None;
    let mut index_lag_samples = Vec::new();
    let mut active_elapsed = std::time::Duration::ZERO;
    let mut fixture_build_elapsed = std::time::Duration::ZERO;
    let mut cooperative_samples = ReadSamples::default();
    let mut deferred_samples = ReadSamples::default();

    while rounds == 0 || active_elapsed < LOAD_DURATION {
        let mut deferred_moves = Vec::with_capacity(regions.len());
        for (simulator, region, item_count) in regions.iter().copied() {
            let fixture_started = Instant::now();
            let workload = build_workload(region, item_count, rounds);
            parent_operations += workload.parent_operations;
            fixture_build_elapsed += fixture_started.elapsed();

            let (stop, readers) = start_readers(&renderer, &inspector, &detail_entities, &queues);
            let region_started = Instant::now();
            let coverage = session.coverage.clone();
            let _summary = apply_cooperative_batch(&mut session, simulator, region, workload.churn, |part, targets, updated| {
                part.record_coverage_losses(&coverage, simulator);
                for address in updated {
                    recovery_queue.cancel(address);
                    coverage.cancel_recovery(RecoveryKey { address, kind: RecoveryKind::Object });
                }
                for (target_simulator, address) in targets {
                    let target_simulator = target_simulator.expect("route recovery to its region");
                    let key = RecoveryKey { address, kind: RecoveryKind::Object };
                    let token =
                        recovery_queue.schedule(target_simulator, agent_data, address, Instant::now()).expect("recovery queue capacity");
                    coverage.schedule_recovery(key, target_simulator, token);
                }
            })
            .await;
            loop {
                let due = recovery_queue.take_due(Instant::now());
                if due.is_empty() {
                    break;
                }
                session.coverage.start_recoveries(due.iter().map(|recovery| (recovery.key, recovery.token)));
                recovery_targets += due.len();
            }

            let coverage = session.coverage.clone();
            let _recovered = apply_cooperative_batch(&mut session, simulator, region, workload.recover, |part, targets, updated| {
                part.record_coverage_losses(&coverage, simulator);
                for address in updated {
                    recovery_queue.cancel(address);
                    coverage.cancel_recovery(RecoveryKey { address, kind: RecoveryKind::Object });
                }
                for (target_simulator, address) in targets {
                    let target_simulator = target_simulator.expect("route recovery to its region");
                    let key = RecoveryKey { address, kind: RecoveryKind::Object };
                    let token =
                        recovery_queue.schedule(target_simulator, agent_data, address, Instant::now()).expect("recovery queue capacity");
                    coverage.schedule_recovery(key, target_simulator, token);
                }
            })
            .await;
            assert!(recovery_queue.pending.is_empty());

            deferred_moves.push((simulator, region, workload.final_moves));
            active_elapsed += region_started.elapsed();
            cooperative_samples.append(stop_readers(&mut session, stop, readers).await);
        }

        let (stop, readers) = start_readers(&renderer, &inspector, &detail_entities, &queues);
        let repair_started = Instant::now();
        let index_lag_started = Instant::now();
        for (simulator, region, moves) in deferred_moves {
            let _summary = apply_deferred_batch(&mut session, simulator, region, moves).await;
        }
        for (region_index, (_, region, _)) in regions.iter().copied().enumerate() {
            let query = WorldQuery::live().in_region(region).within(region_origin(region_index), 0.25).expect("valid spatial query");
            let pending = session
                .world()
                .query(query, None, WorldQueryBudget { work: 4_096, results: MAX_WORLD_QUERY_RESULTS })
                .expect("bounded query while spatial repair is pending");
            assert_eq!(pending.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
            assert!(!pending.unresolved.complete);
        }

        loop {
            let requests = session.try_read_requests();
            repair_renderer_reads += usize::from(requests.renderer.is_some());
            repair_inspector_reads += usize::from(requests.inspector.is_some());
            for request in requests.into_iter() {
                session.service_read_request(request);
            }
            let step = session.world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("maintain query state");
            repair_calls += 1;
            repair_work += step.work_used;
            max_dirty_scopes = max_dirty_scopes.max(step.dirty_scopes);
            if step.dirty_scopes == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        index_lag_samples.push(index_lag_started.elapsed().as_nanos() as u64);
        active_elapsed += repair_started.elapsed();
        deferred_samples.append(stop_readers(&mut session, stop, readers).await);

        for (region_index, (_, region, _)) in regions.iter().copied().enumerate() {
            let query = WorldQuery::live().in_region(region).within(region_origin(region_index), 0.25).expect("valid spatial query");
            let exact = session
                .world()
                .query(query, None, WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS })
                .expect("exact query after repair");
            assert!(exact.items.is_empty());
            assert_eq!(exact.completeness, QueryCompleteness::Exact);
            assert_eq!(exact.unresolved.objects, 0);
            assert!(exact.unresolved.complete);
            final_revision = Some(exact.observed_revision);
        }

        rounds += 1;
    }

    assert!(active_elapsed >= LOAD_DURATION);
    assert!(rounds > 0);
    assert!(parent_operations >= count);
    assert!(recovery_targets > 0);
    assert!(repair_calls > 0);
    assert!(repair_work > 0);
    assert!(!index_lag_samples.is_empty());
    assert!(max_dirty_scopes > 0);
    assert!(repair_renderer_reads > 0, "renderer reads were serviced during deferred repair");
    assert!(repair_inspector_reads > 0, "inspector reads were serviced during deferred repair");
    for (phase, samples) in [("cooperative intake", &cooperative_samples), ("forced deferred repair", &deferred_samples)] {
        assert!(!samples.summaries.is_empty(), "renderer barrier reads progressed during {phase}");
        assert!(!samples.queries.is_empty(), "renderer queries progressed during {phase}");
        assert!(!samples.details.is_empty(), "inspector reads progressed during {phase}");
    }

    let reader = session.reader();
    let barrier = read_barrier(&mut session, &reader);
    let final_revision = final_revision.expect("at least one complete load round ran");
    assert_eq!(barrier.object_count, count);
    assert_eq!(barrier.revision, final_revision);
    assert_eq!(barrier.coverage.world_revision, final_revision);
    assert_eq!(barrier.coverage.recovery.pending, 0);
    assert_eq!(barrier.coverage.recovery.in_flight, 0);
    assert_eq!(barrier.coverage.recovery.degraded, 0);
    assert_eq!(barrier.coverage.losses.len(), 1);

    let renderer_depth = queues.renderer_depth.load(Ordering::Relaxed);
    let inspector_depth = queues.inspector_depth.load(Ordering::Relaxed);
    assert!(renderer_depth <= WORLD_READ_QUEUE_CAPACITY);
    assert!(inspector_depth <= WORLD_READ_QUEUE_CAPACITY);

    eprintln!(
        "SessionWorld sustained {count}: target={:.1}s fixture-build={:.3}s seed={LOAD_SEED:#x}, linkset={LINKSET_SIZE}, regions={region_count}, roots={total_roots}, rounds={rounds}, cooperative-parent-ops={parent_operations}, cooperative-root-moves={}, forced-deferred-root-moves={}, recoveries={recovery_targets}, injected-coverage-losses=1(EventAdmission); active={:.3}s; forced-repair serviced-read-batches renderer={} inspector={}; max-queue-depth renderer={} inspector={}; forced-repair-work={repair_work} forced-repair-calls={repair_calls} max-remaining-dirty-scopes={max_dirty_scopes}",
        LOAD_DURATION.as_secs_f64(),
        fixture_build_elapsed.as_secs_f64(),
        total_roots * rounds,
        total_roots * rounds,
        active_elapsed.as_secs_f64(),
        repair_renderer_reads,
        repair_inspector_reads,
        renderer_depth,
        inspector_depth,
    );
    eprintln!(
        "forced-deferred index lag {count}: rounds={} p50/p95/p99={:.3}ms/{:.3}ms/{:.3}ms",
        index_lag_samples.len(),
        percentile_us(&index_lag_samples, 50) / 1_000.0,
        percentile_us(&index_lag_samples, 95) / 1_000.0,
        percentile_us(&index_lag_samples, 99) / 1_000.0,
    );
    report_read_samples("cooperative-intake", count, &cooperative_samples);
    report_read_samples("forced-deferred-repair", count, &deferred_samples);
}

async fn apply_cooperative_batch(
    session: &mut SessionWorld,
    simulator: network::SimulatorId,
    region: Region,
    blocks: Vec<Option<ObjectOperation>>,
    on_chunk: impl FnMut(&super::IngestSummary, Vec<(Option<SimulatorId>, LocalAddress)>, Vec<LocalAddress>),
) -> super::IngestSummary {
    let expected_blocks = blocks.len();
    let summary = session
        .apply_object_batch_cooperatively(
            simulator,
            ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0, invalid_fields: 0 },
            on_chunk,
        )
        .await
        .expect("registered simulator accepts workload batch");
    assert_eq!(summary.status, super::IngestStatus::Applied);
    assert_eq!(summary.blocks.len(), expected_blocks);
    assert_eq!(summary.invalid_blocks, 0);
    assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_none_or(|result| result.is_ok())));
    summary
}

fn build_workload(region: Region, item_count: usize, round: usize) -> RegionWorkload {
    let child_count = item_count - item_count.div_ceil(LINKSET_SIZE);
    let mut churn = Vec::with_capacity(child_count * 2 + item_count.div_ceil(LINKSET_SIZE) + child_count / RECOVERY_STRIDE);
    let mut recover = Vec::with_capacity(child_count / RECOVERY_STRIDE);
    let mut final_moves = Vec::with_capacity(item_count.div_ceil(LINKSET_SIZE));
    let mut order = 200_000_u64 + round as u64 * ORDER_STRIDE;

    for offset in 0..item_count {
        let local_id = local_id(offset);
        let parent = parent_local_id(offset);
        if parent == local_id {
            let position = moved_root_position(offset / LINKSET_SIZE, 512.0);
            let mut patch = ObjectPatch::new(source(region, &mut order), local_id);
            patch.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
            churn.push(Some(ObjectOperation::Patch(patch)));

            let mut final_move = ObjectPatch::new(source(region, &mut order), local_id);
            final_move.pose =
                Some(Pose { position: Some(moved_root_position(offset / LINKSET_SIZE, 768.0)), rotation: Some(Quat::IDENTITY) });
            final_moves.push(Some(ObjectOperation::Patch(final_move)));
            continue;
        }

        let root_position = moved_root_position(offset / LINKSET_SIZE, 512.0);
        let mut detach = ObjectPatch::new(source(region, &mut order), local_id);
        detach.parent = Some(ParentUpdate::Root);
        detach.pose = Some(Pose { position: Some(root_position), rotation: Some(Quat::IDENTITY) });
        churn.push(Some(ObjectOperation::Patch(detach)));

        let mut attach = ObjectPatch::new(source(region, &mut order), local_id);
        attach.parent = Some(ParentUpdate::Local(parent));
        attach.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        churn.push(Some(ObjectOperation::Patch(attach)));

        if offset % RECOVERY_STRIDE == 1 {
            churn.push(Some(ObjectOperation::Cached { source: source(region, &mut order), local_id }));
            let mut recovered = ObjectPatch::new(source(region, &mut order), local_id);
            recovered.parent = Some(ParentUpdate::Root);
            recovered.pose = Some(Pose { position: Some(root_position), rotation: Some(Quat::IDENTITY) });
            recover.push(Some(ObjectOperation::Patch(recovered)));
        }
    }

    let parent_operations = child_count * 2 + recover.len();
    RegionWorkload { churn, recover, final_moves, parent_operations }
}

async fn apply_deferred_batch(
    session: &mut SessionWorld,
    simulator: network::SimulatorId,
    region: Region,
    blocks: Vec<Option<ObjectOperation>>,
) -> super::IngestSummary {
    let expected_blocks = blocks.len();
    let mut remaining = blocks.into_iter();
    let mut summary: Option<super::IngestSummary> = None;

    loop {
        let operations = remaining.by_ref().take(super::MAX_OBJECTS_PER_INTAKE_QUANTUM).collect::<Vec<_>>();
        if operations.is_empty() {
            break;
        }
        let part = session
            .apply_object_batch(
                simulator,
                ObjectBatch { region, region_handle: None, blocks: operations, invalid_blocks: 0, invalid_fields: 0 },
            )
            .expect("registered simulator accepts workload batch");
        assert_eq!(part.status, super::IngestStatus::Applied);
        match &mut summary {
            Some(summary) => summary.append(part),
            None => summary = Some(part),
        }

        for request in session.try_read_requests().into_iter() {
            session.service_read_request(request);
        }
        tokio::task::yield_now().await;
    }

    let summary = summary.expect("load batches are nonempty");
    assert_eq!(summary.blocks.len(), expected_blocks);
    assert_eq!(summary.invalid_blocks, 0);
    assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_none_or(|result| result.is_ok())));
    summary
}

fn observation(region: Region, global_offset: usize, local_id: NonZeroU32, parent: NonZeroU32, order: u64) -> ObjectObservation {
    ObjectObservation {
        source: UpdateSource::new(region, local_id.get(), OrderEvidence::Ordered(order), 0),
        id: object_uuid(global_offset),
        local_id,
        kind: Kind::Object,
        parent: if parent == local_id { ParentUpdate::Root } else { ParentUpdate::Local(parent) },
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: Some(Vec3::ONE),
        attachment_point: None,
        shape: None,
        appearance: None,
        surface_appearance: None,
        render_materials: None,
    }
}

fn object_uuid(global_offset: usize) -> UUID {
    UUID::from_u128(LOAD_SEED as u128 + global_offset as u128 + 1)
}

fn local_id(offset: usize) -> NonZeroU32 {
    NonZeroU32::new(offset as u32 + 1).expect("local IDs are nonzero")
}

fn parent_local_id(offset: usize) -> NonZeroU32 {
    local_id(offset / LINKSET_SIZE * LINKSET_SIZE)
}

fn root_position(group: usize) -> Vec3 {
    Vec3::new(group as f32 * 8.0, 0.0, 0.0)
}

fn moved_root_position(group: usize, offset: f32) -> Vec3 {
    root_position(group) + Vec3::new(offset, 0.0, 0.0)
}

fn region_origin(index: usize) -> DVec3 {
    DVec3::new(index as f64 * 1_024.0, 0.0, 0.0)
}

fn source(region: Region, order: &mut u64) -> UpdateSource {
    let current = *order;
    *order += 1;
    UpdateSource::new(region, u32::try_from(current).expect("load sequence fits u32"), OrderEvidence::Ordered(current), 0)
}

fn repair(session: &mut SessionWorld) {
    loop {
        let step = session.world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("maintain initial query state");
        if step.dirty_scopes == 0 {
            return;
        }
    }
}

fn start_readers(
    renderer: &WorldReader,
    inspector: &WorldReader,
    entities: &[world::Entity],
    queues: &Arc<QueueMetrics>,
) -> (Arc<AtomicBool>, Vec<tokio::task::JoinHandle<ReadSamples>>) {
    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::with_capacity(4);
    let mut inspector_entities = [entities.to_vec(), entities.to_vec()].into_iter();
    for _ in 0..2 {
        readers.push(tokio::spawn(sample_renderer(
            renderer.clone(),
            ReadSamples::with_capacity(SAMPLE_CAPACITY),
            Arc::clone(&stop),
            Arc::clone(queues),
        )));
        readers.push(tokio::spawn(sample_inspector(
            inspector.clone(),
            inspector_entities.next().expect("one entity fixture per inspector"),
            ReadSamples::with_capacity(SAMPLE_CAPACITY),
            Arc::clone(&stop),
            Arc::clone(queues),
        )));
    }
    (stop, readers)
}

async fn stop_readers(
    session: &mut SessionWorld,
    stop: Arc<AtomicBool>,
    readers: Vec<tokio::task::JoinHandle<ReadSamples>>,
) -> ReadSamples {
    stop.store(true, Ordering::Release);
    tokio::task::yield_now().await;
    drain_reads(session);

    let mut samples = ReadSamples::default();
    for reader in readers {
        samples.append(reader.await.expect("reader task completes"));
    }
    samples
}

async fn sample_renderer(reader: WorldReader, mut samples: ReadSamples, stop: Arc<AtomicBool>, queues: Arc<QueueMetrics>) -> ReadSamples {
    loop {
        if stop.load(Ordering::Acquire) {
            return samples;
        }
        let (reply, response) = oneshot::channel();
        let started = Instant::now();
        match send_read_request(&reader, super::WorldReadRequest::Barrier(reply), &queues) {
            Ok(()) => match response.await.expect("renderer barrier reply") {
                Ok(_barrier) => samples.summaries.push(started.elapsed().as_nanos() as u64),
                Err(error) => panic!("renderer summary read failed: {error:?}"),
            },
            Err(WorldReaderError::Busy) => {
                samples.renderer_busy.push(started.elapsed().as_nanos() as u64);
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("renderer summary request failed: {error:?}"),
        }

        if stop.load(Ordering::Acquire) {
            return samples;
        }
        let (reply, response) = oneshot::channel();
        let request = super::WorldReadRequest::Query {
            query: WorldQuery::live(),
            cursor: None,
            budget: WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS },
            reply,
        };
        let started = Instant::now();
        match send_read_request(&reader, request, &queues) {
            Ok(()) => match response.await.expect("renderer query reply") {
                Ok(page) => {
                    assert_eq!(page.items.len(), MAX_WORLD_QUERY_RESULTS);
                    assert_eq!(page.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::ResultLimit });
                    samples.queries.push(started.elapsed().as_nanos() as u64);
                }
                Err(error) => panic!("renderer query failed: {error:?}"),
            },
            Err(WorldReaderError::Busy) => {
                samples.renderer_busy.push(started.elapsed().as_nanos() as u64);
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("renderer query request failed: {error:?}"),
        }
    }
}

async fn sample_inspector(
    reader: WorldReader,
    entities: Vec<world::Entity>,
    mut samples: ReadSamples,
    stop: Arc<AtomicBool>,
    queues: Arc<QueueMetrics>,
) -> ReadSamples {
    while !stop.load(Ordering::Acquire) {
        let (reply, response) = oneshot::channel();
        let request = super::WorldReadRequest::ObjectDetails { entities: entities.clone(), reply };
        let started = Instant::now();
        match send_read_request(&reader, request, &queues) {
            Ok(()) => match response.await.expect("inspector detail reply") {
                Ok(_details) => samples.details.push(started.elapsed().as_nanos() as u64),
                Err(error) => panic!("inspector detail read failed: {error:?}"),
            },
            Err(WorldReaderError::Busy) => {
                samples.inspector_busy.push(started.elapsed().as_nanos() as u64);
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("inspector detail request failed: {error:?}"),
        }
    }
    samples
}

fn send_read_request(reader: &WorldReader, request: super::WorldReadRequest, queues: &QueueMetrics) -> Result<(), WorldReaderError> {
    let result = reader.try_send(request);
    let (remaining, maximum) = match reader.priority {
        WorldReadPriority::Renderer => (reader.renderer_requests.capacity(), &queues.renderer_depth),
        WorldReadPriority::Inspector => (reader.inspector_requests.capacity(), &queues.inspector_depth),
    };
    maximum.fetch_max(WORLD_READ_QUEUE_CAPACITY - remaining, Ordering::Relaxed);
    result
}

fn drain_reads(session: &mut SessionWorld) {
    loop {
        let requests = session.try_read_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests.into_iter() {
            session.service_read_request(request);
        }
    }
}

fn read_barrier(session: &mut SessionWorld, reader: &WorldReader) -> super::SessionReadBarrier {
    let (reply, response) = oneshot::channel();
    reader.try_send(super::WorldReadRequest::Barrier(reply)).expect("enqueue final read barrier");
    for request in session.try_read_requests().into_iter() {
        session.service_read_request(request);
    }
    futures::executor::block_on(response).expect("final World response").expect("final World barrier")
}

fn percentile_us(samples: &[u64], percentile: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = sorted.len().saturating_mul(percentile).div_ceil(100).saturating_sub(1);
    sorted.get(index).copied().unwrap_or_default() as f64 / 1_000.0
}
