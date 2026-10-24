#[path = "weights.rs"]
mod weights;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
    task::{self, spawn},
    time::{Instant, advance, timeout},
};

use super::{
    Bucket, BudgetLease, Command, ConnectionRole, ConnectionState, MAX_BURST_BYTES, NANOS_PER_SECOND, OutboundBudget, PendingRequest,
    TrafficClass, dispatch_next, next_dispatch_delay, queue_request,
};
use crate::circuit::MAX_NEIGHBOR_SIMULATORS;
use crate::circuit::UdpTransport;
use crate::fixtures::circuit::{new_send_gate, set_send_gate};

fn dispatch(
    bucket: &Arc<Mutex<super::Bucket>>,
    connections: &mut HashMap<u64, super::ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
) {
    while dispatch_next(bucket, connections, order, pending, class_cursor, connection_cursor, virtual_time) {}
}

async fn queue_worker_request(pacer: &super::PacerHandle, bytes: usize) -> oneshot::Receiver<super::BudgetLease> {
    let (completed, receiver) = oneshot::channel();
    pacer
        .commands
        .send(super::Command::Request { id: pacer.id, bytes, class: TrafficClass::Fair, completed })
        .await
        .expect("pacing worker command channel");
    receiver
}

fn consume_ready(receiver: &mut oneshot::Receiver<super::BudgetLease>) -> bool {
    match receiver.try_recv() {
        Ok(lease) => {
            lease.consume();
            true
        }
        Err(_) => false,
    }
}

fn connection(role: ConnectionRole) -> super::ConnectionState {
    super::ConnectionState { role, finish: [0; 3], active: Arc::new(AtomicBool::new(true)) }
}

async fn sustained_priority_service(primary_class: TrafficClass) {
    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));

    let mut connections = HashMap::new();
    let mut order = Vec::new();
    let mut pending = HashMap::new();
    let mut receivers = Vec::new();
    for id in 1..=5 {
        let role = if id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
        let class = if id == 1 { primary_class } else { TrafficClass::Fair };
        order.push(id);
        connections.insert(id, connection(role));
        let (completed, receiver) = oneshot::channel();
        receivers.push(receiver);
        pending.insert(id, PendingRequest::new(1_000, class, 0, completed));
    }

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    let mut service = Vec::new();
    let mut neighbor_grants = [0; 4];

    for _ in 0..12 {
        advance(Duration::from_millis(125)).await;
        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        let mut granted_ids = Vec::new();
        for (index, receiver) in receivers.iter_mut().enumerate() {
            if let Ok(lease) = receiver.try_recv() {
                lease.consume();
                let id = index + 1;
                service.push(id);
                if id > 1 {
                    neighbor_grants[index - 1] += 1;
                }
                granted_ids.push(id);
            }
        }
        assert_eq!(granted_ids.len(), 1, "each refill grants one 1 KB datagram");

        for id in granted_ids {
            let (completed, receiver) = oneshot::channel();
            receivers[id - 1] = receiver;
            let class = if id == 1 { primary_class } else { TrafficClass::Fair };
            pending.insert(id as u64, PendingRequest::new(1_000, class, 0, completed));
        }
    }

    match primary_class {
        TrafficClass::Control => assert_eq!(&service[..4], &[1, 1, 2, 3], "Control leads, then Fair reaches neighbors"),
        TrafficClass::Deadline => assert_eq!(&service[..3], &[1, 2, 3], "Deadline work precedes Fair service without starving it"),
        TrafficClass::Fair => unreachable!("test covers only primary priority classes"),
    }
    assert!(neighbor_grants.iter().all(|grants| *grants > 0), "every queued neighbor receives service: {neighbor_grants:?}");
    assert_eq!(pending.len(), 5, "the sustained load remains queued after the measured rounds");
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_or_cancelling_shutdown_stops_a_saturated_worker() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };

    for cancel_shutdown in [false, true] {
        let budget = OutboundBudget::new(1);
        let handle = budget.handle();
        for _ in 0..handle.commands.capacity() {
            handle.commands.try_send(Command::Unregister { id: 1 }).expect("fill worker queue before it runs");
        }
        if cancel_shutdown {
            let mut shutdown = pin!(budget.shutdown());
            assert!(shutdown.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        } else {
            drop(budget);
        }
        timeout(Duration::from_secs(1), handle.commands.closed())
            .await
            .unwrap_or_else(|_| panic!("worker outlived its owner; cancel_shutdown={cancel_shutdown}"));
    }
}

#[test]
fn dropped_registration_is_retired_when_command_queue_is_full() {
    let (commands, mut receiver) = mpsc::channel(1);
    let (completed, _) = oneshot::channel();
    assert!(commands.try_send(super::Command::Shutdown(completed)).is_ok());

    let active = Arc::new(AtomicBool::new(true));
    let pacer = super::ConnectionPacer {
        id: 7,
        role: ConnectionRole::Neighbor,
        commands: commands.clone(),
        bucket: Arc::new(Mutex::new(Bucket::new(1, Instant::now()))),
        active: Arc::clone(&active),
    };
    let mut connections =
        HashMap::from([(7, super::ConnectionState { role: ConnectionRole::Neighbor, finish: [0; 3], active: Arc::clone(&active) })]);
    let mut order = vec![7];
    let (completed, mut response) = oneshot::channel();
    let mut pending = HashMap::from([(7, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed))]);
    let mut connection_cursor = [0; 3];

    drop(pacer);
    assert!(!active.load(Ordering::Acquire));
    assert_eq!(commands.capacity(), 0, "the unregister command could not enter the full queue");

    super::retire_inactive_connections(&mut connections, &mut order, &mut pending, &mut connection_cursor);

    assert!(connections.is_empty());
    assert!(order.is_empty());
    assert!(pending.is_empty());
    assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
    assert!(matches!(receiver.try_recv(), Ok(super::Command::Shutdown(_))));
}

#[tokio::test(start_paused = true)]
async fn bucket_charges_refunds_and_caps_bursts() {
    let now = Instant::now();
    let mut bucket = Bucket::new(8_000, now);
    assert!(bucket.take(MAX_BURST_BYTES as usize, now));
    assert!(!bucket.take(1, now));

    advance(Duration::from_millis(125)).await;
    assert!(bucket.reserve(1_000, Instant::now()));
    bucket.settle_reservation(1_000, true, Instant::now());
    assert!(bucket.take(1_000, Instant::now()));
    assert!(!bucket.take(1, Instant::now()));
}

#[test]
fn bucket_keeps_a_packet_sized_bounded_burst_at_low_rates() {
    let bucket = Bucket::new(1, Instant::now());
    assert_eq!(bucket.capacity_bytes, MAX_BURST_BYTES);
}

#[test]
fn reliable_retry_reduces_rate_and_quiet_period_recovers_toward_ceiling() {
    let start = Instant::now();
    let mut bucket = Bucket::new(20_000, start);

    bucket.report_loss(start + Duration::from_secs(1));
    assert_eq!(bucket.rate_bytes_per_second, 10_000);

    bucket.refill(start + Duration::from_secs(2));
    assert_eq!(bucket.rate_bytes_per_second, 11_000, "recovery adds five percent of the configured ceiling per second");

    for _ in 0..8 {
        bucket.report_loss(start + Duration::from_secs(2));
    }
    assert_eq!(bucket.rate_bytes_per_second, 5_500, "one retry burst should produce one decrease");

    bucket.report_loss(start + Duration::from_secs(3));
    assert_eq!(bucket.rate_bytes_per_second, 2_750, "a retry at the recovery boundary should still reduce the rate");
}

#[tokio::test(start_paused = true)]
async fn worker_shares_configured_rate() {
    let budget = OutboundBudget::new(5_000);
    let primary = budget.handle().register(1, true).await.expect("primary registration");
    let neighbor = budget.handle().register(2, false).await.expect("neighbor registration");
    let primary_handle = primary.handle();
    let neighbor_handle = neighbor.handle();

    queue_worker_request(&primary_handle, MAX_BURST_BYTES as usize).await.await.expect("initial burst lease").consume();

    let mut primary_request = queue_worker_request(&primary_handle, 1_000).await;
    let mut neighbor_request = queue_worker_request(&neighbor_handle, 1_000).await;
    task::yield_now().await;
    assert!(primary_request.try_recv().is_err());
    assert!(neighbor_request.try_recv().is_err());

    advance(Duration::from_millis(199)).await;
    task::yield_now().await;
    assert!(primary_request.try_recv().is_err());
    assert!(neighbor_request.try_recv().is_err());

    advance(Duration::from_millis(1)).await;
    task::yield_now().await;
    let first_grants = usize::from(consume_ready(&mut primary_request)) + usize::from(consume_ready(&mut neighbor_request));
    assert_eq!(first_grants, 1, "the configured 5,000-byte rate yields one 1,000-byte send every 200 ms");

    advance(Duration::from_millis(199)).await;
    task::yield_now().await;
    assert!(primary_request.try_recv().is_err());
    assert!(neighbor_request.try_recv().is_err());

    advance(Duration::from_millis(1)).await;
    task::yield_now().await;
    let second_grants = usize::from(consume_ready(&mut primary_request)) + usize::from(consume_ready(&mut neighbor_request));
    assert_eq!(second_grants, 1, "both simulators share the session's configured byte budget");

    drop(primary_handle);
    drop(neighbor_handle);
    drop(primary);
    drop(neighbor);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn worker_reweights_after_promotion() {
    let budget = OutboundBudget::new(5_000);
    let mut pacers = Vec::new();
    for id in 0..=5 {
        pacers.push(budget.handle().register(id, id == 1).await.expect("register simulator"));
    }
    let handles = pacers.iter().map(super::ConnectionPacer::handle).collect::<Vec<_>>();

    queue_worker_request(&handles[0], MAX_BURST_BYTES as usize).await.await.expect("drain initial burst").consume();
    let mut requests = Vec::new();
    for handle in &handles[1..] {
        requests.push(queue_worker_request(handle, 500).await);
    }

    for phase in 0..2 {
        let mut counts = [0; 5];
        for _ in 0..90 {
            advance(Duration::from_millis(100)).await;
            task::yield_now().await;

            let mut granted = None;
            for (index, request) in requests.iter_mut().enumerate() {
                if let Ok(lease) = request.try_recv() {
                    lease.consume();
                    assert!(granted.replace(index).is_none(), "one 500-byte request should use each refill");
                }
            }
            let index = granted.expect("each 100 ms refill should grant one queued request");
            counts[index] += 1;
            requests[index] = queue_worker_request(&handles[index + 1], 500).await;
        }

        let primary = if phase == 0 { 0 } else { 1 };
        assert_eq!(counts[primary], 30, "the active primary should receive twice a neighbor's share");
        for (index, count) in counts.iter().enumerate() {
            if index != primary {
                assert_eq!(*count, 15, "neighbor {index} should receive its weighted share");
            }
        }

        if phase == 0 {
            pacers[1].set_primary(false).await.expect("demote the former primary");
            pacers[2].set_primary(true).await.expect("promote the selected neighbor");
            task::yield_now().await;
        }
    }

    drop(handles);
    drop(pacers);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn worker_weights_small_bursts() {
    const PACKET_BYTES: usize = 46;

    let budget = OutboundBudget::new(5_000);
    let dummy = budget.handle().register(99, false).await.expect("register burst drain");
    let dummy_handle = dummy.handle();
    queue_worker_request(&dummy_handle, MAX_BURST_BYTES as usize).await.await.expect("drain initial burst").consume();
    drop(dummy_handle);
    drop(dummy);

    let mut pacers = Vec::new();
    for id in 1..=5 {
        pacers.push(budget.handle().register(id, id == 1).await.expect("register simulator"));
    }
    let mut counters = Vec::new();
    let mut tasks = Vec::new();
    for pacer in &pacers {
        let handle = pacer.handle();
        let count = Arc::new(AtomicUsize::new(0));
        let task_count = Arc::clone(&count);
        tasks.push(spawn(async move {
            loop {
                let Ok(lease) = queue_worker_request(&handle, PACKET_BYTES).await.await else {
                    break;
                };
                lease.consume();
                task_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
        counters.push(count);
    }

    for _ in 0..20 {
        advance(Duration::from_millis(100)).await;
        for _ in 0..8 {
            task::yield_now().await;
        }
    }

    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    let primary = counters[0].load(Ordering::Relaxed);
    let neighbor_average = counters[1..].iter().map(|count| count.load(Ordering::Relaxed)).sum::<usize>() as f64 / 4.0;
    assert!(neighbor_average > 0.0);
    assert!(primary as f64 >= neighbor_average * 1.5, "primary={primary}, neighbor average={neighbor_average}");

    drop(pacers);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn scheduler_shares_one_bucket_across_connections() {
    let bucket = Arc::new(Mutex::new(Bucket::new(8_000, Instant::now())));
    let mut connections = HashMap::new();
    let mut order = Vec::new();
    let mut pending = HashMap::new();
    let mut receivers = Vec::new();
    let virtual_time = [0; 3];
    for id in 1..=10 {
        order.push(id);
        connections.insert(id, connection(if id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
        let (completed, receiver) = oneshot::channel();
        receivers.push(receiver);
        pending.insert(id, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed));
    }

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = virtual_time;
    dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

    let mut granted = 0;
    for receiver in &mut receivers {
        if let Ok(lease) = receiver.try_recv() {
            granted += 1;
            let _sent_at = lease.consume();
        }
    }
    assert_eq!(granted, 8);
    assert_eq!(pending.len(), 2);
    let remainder = u128::from(MAX_BURST_BYTES - 8_000) * 1_000_000_000;
    assert_eq!(bucket.lock().unwrap().credit_byte_nanos, remainder);

    for _ in 0..2 {
        advance(Duration::from_millis(125)).await;
        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);
        let mut round_granted = 0;
        for receiver in &mut receivers {
            if let Ok(lease) = receiver.try_recv() {
                round_granted += 1;
                let _sent_at = lease.consume();
            }
        }
        assert_eq!(round_granted, 1);
        assert_eq!(bucket.lock().unwrap().credit_byte_nanos, remainder);
    }
    assert!(pending.is_empty());
}

#[tokio::test(start_paused = true)]
async fn primary_control_and_deadline_survive_neighbor_retries() {
    sustained_priority_service(TrafficClass::Control).await;
    sustained_priority_service(TrafficClass::Deadline).await;
}

#[test]
fn cancelled_waiter_does_not_spend_budget() {
    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
    let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor))]);
    let order = vec![1];
    let (completed, receiver) = oneshot::channel();
    drop(receiver);
    let mut pending = HashMap::from([(1, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed))]);
    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];

    dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

    assert!(pending.is_empty());
    assert_eq!(bucket.lock().unwrap().credit_byte_nanos, u128::from(MAX_BURST_BYTES) * 1_000_000_000);
}

#[test]
fn cancelled_request_does_not_block_its_replacement() {
    let connections = HashMap::from([(1, connection(ConnectionRole::Neighbor))]);
    let mut pending = HashMap::new();
    let (old_completed, old_receiver) = oneshot::channel();
    pending.insert(1, PendingRequest::new(1_000, TrafficClass::Fair, 0, old_completed));
    drop(old_receiver);

    let (completed, _receiver) = oneshot::channel();
    queue_request(1, 500, TrafficClass::Fair, completed, &connections, &mut pending, &[0; 3]);

    assert_eq!(pending.get(&1).expect("replacement request").bytes, 500);
    assert!(!pending.get(&1).expect("replacement request").completed.is_closed());
}

#[test]
fn cancelled_send_refunds_its_budget_lease() {
    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
    assert!(bucket.lock().unwrap().reserve(4_000, now));

    drop(super::BudgetLease::new(Arc::clone(&bucket), 4_000));

    assert_eq!(bucket.lock().unwrap().credit_byte_nanos, u128::from(MAX_BURST_BYTES) * 1_000_000_000);
}

#[tokio::test(start_paused = true)]
async fn blocked_udp_send_refunds_credit_before_waiting_for_writability() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator socket");
    let address = server.local_addr().expect("simulator endpoint");
    let mut blocked_socket = UdpTransport::connect(address).await.expect("connect blocked transport");
    let (gate, gate_state, send_entered) = new_send_gate();
    gate_state.send_replace(true);
    set_send_gate(&mut blocked_socket, gate);
    let flowing_socket = UdpTransport::connect(address).await.expect("connect flowing transport");

    let budget = OutboundBudget::new(8_000);
    let blocked_connection = budget.handle().register(1, true).await.expect("register blocked connection");
    let flowing_connection = budget.handle().register(2, false).await.expect("register flowing connection");
    let blocked_pacer = blocked_connection.handle();
    let flowing_pacer = flowing_connection.handle();
    let blocked = spawn(async move {
        let bytes = [0; MAX_BURST_BYTES as usize];
        blocked_pacer.send(&blocked_socket, &bytes, TrafficClass::Fair).await
    });
    timeout(Duration::from_secs(1), send_entered.notified()).await.expect("blocked socket should report backpressure");

    let flowing = spawn(async move {
        let bytes = [1; 100];
        flowing_pacer.send(&flowing_socket, &bytes, TrafficClass::Fair).await
    });
    let started = Instant::now();
    flowing.await.expect("flowing send task").expect("flowing datagram");
    assert!(started.elapsed() < Duration::from_millis(1), "unblocked connection should use refunded pacing credit");

    let mut received = [0; MAX_BURST_BYTES as usize];
    let (received_bytes, _) = server.recv_from(&mut received).await.expect("read unblocked simulator datagram");
    assert_eq!(&received[..received_bytes], &[1; 100]);

    advance(Duration::from_millis(13)).await;
    gate_state.send_replace(false);
    blocked.await.expect("blocked send task").expect("blocked datagram");
    let (received_bytes, _) = server.recv_from(&mut received).await.expect("read resumed simulator datagram");
    assert_eq!(received_bytes, MAX_BURST_BYTES as usize);

    drop(blocked_connection);
    drop(flowing_connection);
    budget.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn retransmission_feedback_reduces_the_session_rate() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator socket");
    let socket = UdpTransport::connect(server.local_addr().expect("simulator endpoint")).await.expect("connect transport");
    let budget = OutboundBudget::new(20_000);
    let connection = budget.handle().register(1, true).await.expect("register simulator");

    assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, 20_000);
    let handle = connection.handle();
    handle.report_loss();
    handle.send(&socket, &[0; 100], TrafficClass::Fair).await.expect("send retry");
    assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, 10_000);

    drop(connection);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn outstanding_leases_reserve_burst_capacity_until_send() {
    let budget = OutboundBudget::new(8_000);
    let first = budget.handle().register(1, true).await.expect("register first simulator");
    let second = budget.handle().register(2, false).await.expect("register second simulator");
    let first_handle = first.handle();
    let second_handle = second.handle();

    let delayed_request = queue_worker_request(&first_handle, MAX_BURST_BYTES as usize).await;
    let delayed_lease = delayed_request.await.expect("grant the initial burst");
    let mut next_request = queue_worker_request(&second_handle, 1).await;
    task::yield_now().await;

    advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        task::yield_now().await;
    }
    assert!(
        matches!(next_request.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
        "refill must count the delayed lease against the burst ceiling"
    );

    delayed_lease.consume();
    advance(Duration::from_millis(1)).await;
    for _ in 0..8 {
        task::yield_now().await;
    }
    next_request.await.expect("the next request gets newly earned credit").consume();

    drop((first, second));
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn worker_wakes_for_rate_recovery_before_retry_deadline() {
    const MAX_RATE_BYTES_PER_SECOND: u64 = 1_000;

    let budget = OutboundBudget::new(MAX_RATE_BYTES_PER_SECOND);
    let first = budget.handle().register(1, true).await.expect("register loss reporter");
    let second = budget.handle().register(2, false).await.expect("register waiting simulator");
    let first_handle = first.handle();
    let second_handle = second.handle();

    queue_worker_request(&first_handle, MAX_BURST_BYTES as usize).await.await.expect("grant the initial burst").consume();
    first_handle.report_loss();
    for expected_rate in [250, 125, 62] {
        advance(Duration::from_secs(1)).await;
        first_handle.report_loss();
        assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, expected_rate);
    }

    queue_worker_request(&first_handle, 875).await.await.expect("drain credit earned during loss backoff").consume();
    let queued_at = Instant::now();
    let mut pending = queue_worker_request(&second_handle, 500).await;
    let mut granted = None;
    for _ in 0..4 {
        advance(Duration::from_secs(1)).await;
        for _ in 0..8 {
            task::yield_now().await;
        }
        if let Ok(lease) = pending.try_recv() {
            granted = Some(lease);
            break;
        }
    }

    let sent_at = granted.expect("recovery timers wake the worker before the five-second retry deadline").consume();
    assert!(sent_at.saturating_duration_since(queued_at) < Duration::from_secs(5));

    drop((first, second));
    budget.shutdown().await;
}

#[test]
fn large_request_does_not_block_smaller_same_class_work() {
    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize - 1_000, now));
    let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
    let order = vec![1, 2];
    let (large_completed, mut large_receiver) = oneshot::channel();
    let (small_completed, mut small_receiver) = oneshot::channel();
    let mut pending = HashMap::from([
        (1, PendingRequest::new(MAX_BURST_BYTES as usize, TrafficClass::Fair, 0, large_completed)),
        (2, PendingRequest::new(500, TrafficClass::Fair, 0, small_completed)),
    ]);
    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];

    dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

    small_receiver.try_recv().expect("smaller same-class send should use available credit").consume();
    assert!(large_receiver.try_recv().is_err());
    assert!(pending.contains_key(&1));
}

#[tokio::test(start_paused = true)]
async fn large_request_reserves_its_service_turn_against_continuous_small_packets() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const SMALL_PACKET_BYTES: usize = 500;
    const REFILL: Duration = Duration::from_nanos(62_500_000);

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
    let order = vec![1, 2];
    let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
    let (large_completed, mut large_receiver) = oneshot::channel();
    let (small_completed, mut small_receiver) = oneshot::channel();
    let mut pending = HashMap::new();
    queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, large_completed, &connections, &mut pending, &[0; 3]);
    queue_request(2, SMALL_PACKET_BYTES, TrafficClass::Fair, small_completed, &connections, &mut pending, &[0; 3]);

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    let mut total_granted_bytes = 0u64;
    let mut small_grants = 0usize;
    let mut large_grant_tick = None;

    for tick in 1..=33 {
        advance(REFILL).await;
        let granted =
            dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        let small_sent = consume_ready(&mut small_receiver);
        let large_sent = consume_ready(&mut large_receiver);
        assert_eq!(granted, small_sent || large_sent, "the arbiter reports each completed lease");
        assert!(!(small_sent && large_sent), "one dispatch grants at most one request");
        if small_sent {
            small_grants += 1;
            total_granted_bytes += SMALL_PACKET_BYTES as u64;
            let (completed, receiver) = oneshot::channel();
            small_receiver = receiver;
            queue_request(2, SMALL_PACKET_BYTES, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
        }
        if large_sent {
            total_granted_bytes += MAX_BURST_BYTES;
            large_grant_tick = Some(tick);
        }

        let elapsed = REFILL * tick;
        let bucket = bucket.lock().unwrap();
        let earned_byte_nanos = elapsed.as_nanos() * u128::from(RATE_BYTES_PER_SECOND);
        let spent_byte_nanos = u128::from(total_granted_bytes) * NANOS_PER_SECOND;
        assert!(bucket.credit_byte_nanos <= u128::from(bucket.capacity_bytes) * NANOS_PER_SECOND);
        assert!(spent_byte_nanos + bucket.credit_byte_nanos <= earned_byte_nanos, "grants must remain within the shared token budget");
    }

    assert_eq!(small_grants, 16, "smaller packets use their fair virtual service before the large request becomes next");
    assert_eq!(large_grant_tick, Some(33), "the reserved large packet is served after finite token accrual");
    assert_eq!(total_granted_bytes, 16 * SMALL_PACKET_BYTES as u64 + MAX_BURST_BYTES);
}

#[tokio::test(start_paused = true)]
async fn control_uses_its_scheduled_turn_while_large_fair_request_waits() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const CONTROL_PACKET_BYTES: usize = 500;
    const REFILL: Duration = Duration::from_nanos(62_500_000);

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
    let order = vec![1, 2];
    let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
    let (large_completed, mut large_receiver) = oneshot::channel();
    let mut pending = HashMap::new();
    queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, large_completed, &connections, &mut pending, &[0; 3]);
    let (competitor_completed, competitor_receiver) = oneshot::channel();
    pending.insert(
        2,
        PendingRequest::new(MAX_BURST_BYTES as usize / 2, TrafficClass::Fair, u128::from(MAX_BURST_BYTES) * 2 + 1, competitor_completed),
    );

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    assert!(!dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    assert!(pending.get(&1).expect("large Fair request remains pending").reserved);
    drop(competitor_receiver);

    let (control_completed, mut control_receiver) = oneshot::channel();
    queue_request(2, CONTROL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);

    let mut total_granted_bytes = 0u64;
    let mut control_grant_tick = None;
    let mut fair_grant_tick = None;
    for tick in 1..=18 {
        advance(REFILL).await;
        let granted =
            dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);
        let control_sent = consume_ready(&mut control_receiver);
        let fair_sent = consume_ready(&mut large_receiver);
        assert_eq!(granted, control_sent || fair_sent);
        assert!(!(control_sent && fair_sent));
        if control_sent {
            total_granted_bytes += CONTROL_PACKET_BYTES as u64;
            control_grant_tick = Some(tick);
        }
        if fair_sent {
            total_granted_bytes += MAX_BURST_BYTES;
            fair_grant_tick = Some(tick);
        }

        let elapsed = REFILL * tick;
        let bucket = bucket.lock().unwrap();
        let earned_byte_nanos = elapsed.as_nanos() * u128::from(RATE_BYTES_PER_SECOND);
        let spent_byte_nanos = u128::from(total_granted_bytes) * NANOS_PER_SECOND;
        assert!(bucket.credit_byte_nanos <= u128::from(bucket.capacity_bytes) * NANOS_PER_SECOND);
        assert!(spent_byte_nanos + bucket.credit_byte_nanos <= earned_byte_nanos, "grants must remain within the shared token budget");
    }

    assert_eq!(control_grant_tick, Some(1), "Control uses its next scheduled opportunity");
    assert_eq!(fair_grant_tick, Some(18), "bounded Control service does not starve the reserved Fair request");
    assert_eq!(total_granted_bytes, CONTROL_PACKET_BYTES as u64 + MAX_BURST_BYTES);
}

#[tokio::test(start_paused = true)]
async fn affordable_deadline_uses_its_turn_while_reserved_fair_waits() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const SMALL_PACKET_BYTES: usize = 500;
    const REFILL: Duration = Duration::from_nanos(62_500_000);

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
    let order = vec![1, 2, 3];
    let mut connections = HashMap::from([
        (1, connection(ConnectionRole::Neighbor)),
        (2, connection(ConnectionRole::Neighbor)),
        (3, connection(ConnectionRole::Neighbor)),
    ]);
    let (fair_completed, mut fair_receiver) = oneshot::channel();
    let mut pending = HashMap::new();
    queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, fair_completed, &connections, &mut pending, &[0; 3]);
    let (competitor_completed, competitor_receiver) = oneshot::channel();
    pending.insert(
        2,
        PendingRequest::new(MAX_BURST_BYTES as usize / 2, TrafficClass::Fair, u128::from(MAX_BURST_BYTES) * 2 + 1, competitor_completed),
    );

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    assert!(!dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    drop(competitor_receiver);

    let (control_completed, mut control_receiver) = oneshot::channel();
    queue_request(2, MAX_BURST_BYTES as usize, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
    let (deadline_completed, mut deadline_receiver) = oneshot::channel();
    queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Deadline, deadline_completed, &connections, &mut pending, &virtual_time);

    advance(REFILL).await;
    assert!(dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    assert!(control_receiver.try_recv().is_err(), "an unaffordable Control request should not block Deadline");
    deadline_receiver.try_recv().expect("affordable Deadline request uses its scheduled bypass").consume();
    assert!(fair_receiver.try_recv().is_err(), "the Fair request remains reserved until its full packet is funded");

    drop(control_receiver);
    let mut fair_grant_tick = None;
    for tick in 2..=18 {
        advance(REFILL).await;
        let granted =
            dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);
        let fair_sent = consume_ready(&mut fair_receiver);
        assert_eq!(granted, fair_sent);
        if fair_sent {
            fair_grant_tick = Some(tick);
        }
    }
    assert_eq!(fair_grant_tick, Some(18), "the unaffordable Control request cannot leave the Fair reservation stuck");
}

#[tokio::test(start_paused = true)]
async fn cancelling_reserved_request_releases_its_service_slot() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const REFILL: Duration = Duration::from_nanos(62_500_000);

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
    let order = vec![1, 2];
    let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
    let (cancelled_completed, cancelled_receiver) = oneshot::channel();
    let mut pending = HashMap::new();
    queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, cancelled_completed, &connections, &mut pending, &[0; 3]);
    let (competitor_completed, competitor_receiver) = oneshot::channel();
    pending.insert(
        2,
        PendingRequest::new(MAX_BURST_BYTES as usize / 2, TrafficClass::Fair, u128::from(MAX_BURST_BYTES) * 2 + 1, competitor_completed),
    );

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    assert!(!dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    assert!(pending.get(&1).expect("large request is reserved").reserved);
    drop(cancelled_receiver);
    drop(competitor_receiver);

    let (control_completed, mut control_receiver) = oneshot::channel();
    queue_request(2, 500, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
    advance(REFILL).await;
    assert!(dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    control_receiver.try_recv().expect("cancellation removes the pinned reservation").consume();
    assert!(!pending.contains_key(&1));
}

#[tokio::test(start_paused = true)]
async fn reserved_fair_send_consumes_its_service_slot_before_next_control() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const SMALL_PACKET_BYTES: usize = 500;
    const REFILL: Duration = Duration::from_millis(1_024);

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
    let order = vec![1, 2, 3, 4];
    let mut connections = HashMap::from([
        (1, connection(ConnectionRole::Neighbor)),
        (2, connection(ConnectionRole::Neighbor)),
        (3, connection(ConnectionRole::Neighbor)),
        (4, connection(ConnectionRole::Neighbor)),
    ]);
    let (reserved_completed, mut reserved_receiver) = oneshot::channel();
    let (first_fair_completed, mut first_fair_receiver) = oneshot::channel();
    let mut pending = HashMap::from([
        (1, PendingRequest::new(MAX_BURST_BYTES as usize, TrafficClass::Fair, 0, reserved_completed)),
        (2, PendingRequest::new(SMALL_PACKET_BYTES, TrafficClass::Fair, 1, first_fair_completed)),
    ]);
    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];

    assert!(!dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    assert!(pending.get(&1).expect("large Fair request is reserved").reserved);
    assert_eq!(class_cursor, 3, "the reservation pins the selected Fair slot");

    advance(REFILL).await;
    assert!(dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    reserved_receiver.try_recv().expect("reserved Fair request is funded").consume();
    assert_eq!(class_cursor, 4, "the reserved send consumes Fair slot 3 and advances to slot 4");

    let (second_fair_completed, mut second_fair_receiver) = oneshot::channel();
    queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Fair, second_fair_completed, &connections, &mut pending, &virtual_time);
    let (control_completed, mut control_receiver) = oneshot::channel();
    queue_request(4, SMALL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
    advance(Duration::from_millis(250)).await;

    let mut granted = Vec::new();
    for _ in 0..3 {
        assert!(dispatch_next(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
        ));
        if let Ok(lease) = first_fair_receiver.try_recv() {
            lease.consume();
            granted.push("fair");
        } else if let Ok(lease) = control_receiver.try_recv() {
            lease.consume();
            granted.push("control");
        } else if let Ok(lease) = second_fair_receiver.try_recv() {
            lease.consume();
            granted.push("fair");
        } else {
            panic!("each successful dispatch grants exactly one queued request");
        }
    }

    assert_eq!(granted, ["fair", "control", "fair"], "slot 4 Fair may precede the next Control slot only once");
}

#[tokio::test(start_paused = true)]
async fn reserved_wait_ignores_ineligible_small_requests_but_dispatches_new_control() {
    const RATE_BYTES_PER_SECOND: u64 = 8_000;
    const LARGE_PACKET_BYTES: usize = MAX_BURST_BYTES as usize;
    const SMALL_PACKET_BYTES: usize = 500;

    let now = Instant::now();
    let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
    assert!(bucket.lock().unwrap().take(LARGE_PACKET_BYTES, now));
    let order = vec![1, 2, 3];
    let mut connections = HashMap::from([
        (1, connection(ConnectionRole::Neighbor)),
        (2, connection(ConnectionRole::Neighbor)),
        (3, connection(ConnectionRole::Neighbor)),
    ]);
    let (large_completed, mut large_receiver) = oneshot::channel();
    let (small_completed, mut small_receiver) = oneshot::channel();
    let mut pending = HashMap::from([
        (1, PendingRequest::new(LARGE_PACKET_BYTES, TrafficClass::Fair, 0, large_completed)),
        (2, PendingRequest::new(SMALL_PACKET_BYTES, TrafficClass::Fair, 1, small_completed)),
    ]);
    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];

    assert!(!dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    assert!(pending.get(&1).expect("large request remains pending").reserved);

    advance(Duration::from_millis(100)).await;
    let delay = next_dispatch_delay(&bucket, &pending);
    assert_eq!(delay, Duration::from_millis(924), "wait for the reserved 8 KiB request, not affordable 500 B Fair work");

    let (control_completed, mut control_receiver) = oneshot::channel();
    queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
    assert!(dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time,));
    control_receiver.try_recv().expect("new eligible Control work uses accumulated credit immediately").consume();
    assert!(large_receiver.try_recv().is_err(), "Control uses only the reserved Fair turn's bounded bypass");
    assert!(small_receiver.try_recv().is_err(), "same-class small work cannot cause another short timer wake");
    assert!(pending.get(&1).expect("large request remains reserved").reserved);
    assert_eq!(next_dispatch_delay(&bucket, &pending), Duration::from_nanos(986_500_000));
}
