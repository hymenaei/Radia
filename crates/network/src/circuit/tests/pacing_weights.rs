use super::*;
use tokio::time::advance;

async fn dispatch_one_under_pressure(
    active: &[u64],
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, super::ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    receivers: &mut HashMap<u64, oneshot::Receiver<super::BudgetLease>>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
) -> u64 {
    advance(Duration::from_millis(200)).await;
    dispatch(bucket, connections, order, pending, class_cursor, connection_cursor, virtual_time);

    let granted = active
        .iter()
        .filter_map(|id| match receivers.get_mut(id).expect("active request receiver").try_recv() {
            Ok(lease) => {
                lease.consume();
                Some(*id)
            }
            Err(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(granted.len(), 1, "one packet should use each refill of the shared bucket");
    let id = granted[0];
    assert_eq!(bucket.lock().unwrap().credit_byte_nanos, 0);

    let (completed, receiver) = oneshot::channel();
    receivers.insert(id, receiver);
    queue_request(id, 1_000, TrafficClass::Fair, completed, connections, pending, virtual_time);
    id
}

async fn assert_service_window(
    active: &[u64],
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, super::ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    receivers: &mut HashMap<u64, oneshot::Receiver<super::BudgetLease>>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
) {
    let mut served = HashSet::new();
    for _ in 0..active.len() * 2 {
        let id = dispatch_one_under_pressure(
            active,
            bucket,
            connections,
            order,
            pending,
            receivers,
            class_cursor,
            connection_cursor,
            virtual_time,
        )
        .await;
        served.insert(id);
        if served.len() == active.len() {
            break;
        }
    }
    assert_eq!(served, active.iter().copied().collect(), "every active connection should receive service within a bounded window");
}

#[tokio::test(start_paused = true)]
async fn small_packets_follow_role_weights() {
    let bucket = Arc::new(Mutex::new(Bucket::new(5_000, Instant::now())));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, Instant::now()));
    let order = (1..=5).collect::<Vec<_>>();
    let mut connections = HashMap::new();
    let mut pending = HashMap::new();
    let mut receivers = HashMap::new();
    let mut virtual_time = [0; 3];
    for id in &order {
        connections.insert(*id, connection(if *id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
        let (completed, receiver) = oneshot::channel();
        receivers.insert(*id, receiver);
        queue_request(*id, 500, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
    }

    let mut counts = HashMap::<u64, usize>::new();
    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    for _ in 0..90 {
        advance(Duration::from_millis(100)).await;
        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        let granted = order
            .iter()
            .filter_map(|id| match receivers.get_mut(id).expect("active connection receiver").try_recv() {
                Ok(lease) => {
                    lease.consume();
                    *counts.entry(*id).or_default() += 1;
                    Some(*id)
                }
                Err(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(granted.len(), 1, "one 500-byte packet should use each refill");

        for id in granted {
            let (completed, receiver) = oneshot::channel();
            receivers.insert(id, receiver);
            queue_request(id, 500, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
        }
    }

    assert_eq!(counts.get(&1), Some(&30), "the primary should receive twice each neighbor's bytes: {counts:?}");
    for id in 2..=5 {
        assert_eq!(counts.get(&id), Some(&15), "neighbor {id} should receive its weighted share");
    }
}

#[tokio::test(start_paused = true)]
async fn shared_pressure_services_at_configured_limit() {
    let simulator_count = MAX_NEIGHBOR_SIMULATORS + 1;
    let bucket = Arc::new(Mutex::new(Bucket::new(5_000, Instant::now())));
    assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, Instant::now()));

    let mut order = (1..=simulator_count as u64).collect::<Vec<_>>();
    let mut connections = HashMap::new();
    let mut pending = HashMap::new();
    let mut receivers = HashMap::new();
    for id in &order {
        connections.insert(*id, connection(if *id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
        let (completed, receiver) = oneshot::channel();
        receivers.insert(*id, receiver);
        queue_request(*id, 1_000, TrafficClass::Fair, completed, &connections, &mut pending, &[0; 3]);
    }

    let mut class_cursor = 0;
    let mut connection_cursor = [0; 3];
    let mut virtual_time = [0; 3];
    for _ in 0..2 {
        assert_service_window(
            &order,
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut receivers,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
        )
        .await;
    }

    connections.get_mut(&1).expect("original primary").role = ConnectionRole::Neighbor;
    connections.get_mut(&1).expect("original primary").finish = virtual_time;
    connections.get_mut(&3).expect("promoted neighbor").role = ConnectionRole::Primary;
    connections.get_mut(&3).expect("promoted neighbor").finish = virtual_time;
    assert_service_window(
        &order,
        &bucket,
        &mut connections,
        &order,
        &mut pending,
        &mut receivers,
        &mut class_cursor,
        &mut connection_cursor,
        &mut virtual_time,
    )
    .await;

    let leaving_id = simulator_count as u64;
    connections.remove(&leaving_id);
    pending.remove(&leaving_id);
    receivers.remove(&leaving_id);
    order.retain(|id| *id != leaving_id);
    connection_cursor = [0; 3];
    let joining_id = leaving_id + 1;
    order.push(joining_id);
    connections.insert(
        joining_id,
        super::ConnectionState { role: ConnectionRole::Neighbor, finish: virtual_time, active: Arc::new(AtomicBool::new(true)) },
    );
    let (completed, receiver) = oneshot::channel();
    receivers.insert(joining_id, receiver);
    queue_request(joining_id, 1_000, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
    assert_service_window(
        &order,
        &bucket,
        &mut connections,
        &order,
        &mut pending,
        &mut receivers,
        &mut class_cursor,
        &mut connection_cursor,
        &mut virtual_time,
    )
    .await;
}
