use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures::{
    FutureExt, StreamExt,
    future::{BoxFuture, pending},
    pin_mut,
    stream::FuturesUnordered,
};
use math::motion as kinematics;
use network::CircuitMotionStatus;
use tokio::sync::watch;
use world::Region;

#[derive(Default)]
struct MotionCircuitLiveness {
    statuses: HashMap<Region, CircuitMotionStatus>,
    active: HashMap<Region, bool>,
}

impl MotionCircuitLiveness {
    fn replace(&mut self, statuses: HashMap<Region, CircuitMotionStatus>, now: Instant) -> bool {
        self.statuses = statuses;
        self.update(now)
    }

    fn update(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for (&region, status) in &self.statuses {
            let active = kinematics::circuit_active(now, status.last_packet_received_at, status.connected);
            changed |= self.active.insert(region, active).unwrap_or(false) != active;
        }
        self.active.retain(|region, active| {
            if self.statuses.contains_key(region) {
                true
            } else {
                changed |= *active;
                false
            }
        });
        changed
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.statuses
            .iter()
            .filter(|(region, _)| self.active.get(region).copied().unwrap_or(false))
            .filter_map(|(_, status)| {
                status
                    .last_packet_received_at
                    .checked_add(Duration::from_secs_f64(kinematics::PHASE_OUT_TIME))?
                    .checked_add(Duration::from_nanos(1))
            })
            .min()
    }
}

pub(super) struct MotionCircuitWake {
    receiver: watch::Receiver<Vec<(Region, watch::Receiver<CircuitMotionStatus>)>>,
    receivers: HashMap<Region, watch::Receiver<CircuitMotionStatus>>,
    watchers: FuturesUnordered<BoxFuture<'static, (Region, watch::Receiver<CircuitMotionStatus>, bool)>>,
    liveness: MotionCircuitLiveness,
    list_closed: bool,
}

impl MotionCircuitWake {
    pub(super) fn new(receiver: watch::Receiver<Vec<(Region, watch::Receiver<CircuitMotionStatus>)>>) -> Self {
        let mut state = Self {
            receiver,
            receivers: HashMap::new(),
            watchers: FuturesUnordered::new(),
            liveness: MotionCircuitLiveness::default(),
            list_closed: false,
        };
        let circuits = state.receiver.borrow_and_update().clone();
        let _ = state.replace(circuits, tokio::time::Instant::now().into_std());
        state
    }

    fn push(&mut self, region: Region, mut receiver: watch::Receiver<CircuitMotionStatus>) {
        self.watchers.push(
            async move {
                let changed = receiver.changed().await.is_ok();
                (region, receiver, changed)
            }
            .boxed(),
        );
    }

    fn replace(&mut self, circuits: Vec<(Region, watch::Receiver<CircuitMotionStatus>)>, now: Instant) -> bool {
        self.watchers.clear();
        self.receivers.clear();
        for (region, mut receiver) in circuits {
            let _ = receiver.borrow_and_update();
            self.receivers.insert(region, receiver.clone());
            self.push(region, receiver);
        }
        self.update_liveness(now)
    }

    fn update_liveness(&mut self, now: Instant) -> bool {
        let statuses = self.receivers.iter().map(|(region, receiver)| (*region, *receiver.borrow())).collect();
        self.liveness.replace(statuses, now)
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.liveness.next_deadline()
    }

    pub(super) async fn wait_for_transition(&mut self) -> bool {
        loop {
            enum Event {
                List(Result<(), watch::error::RecvError>),
                Circuit(Option<(Region, watch::Receiver<CircuitMotionStatus>, bool)>),
                Timer,
            }
            let deadline = self.next_deadline();
            let event = {
                let list = async { if self.list_closed { pending().await } else { self.receiver.changed().await } }.fuse();
                let circuit = async { if self.watchers.is_empty() { pending().await } else { self.watchers.next().await } }.fuse();
                let timer = async {
                    match deadline {
                        Some(deadline) if tokio::time::Instant::from_std(deadline) <= tokio::time::Instant::now() => {
                            tokio::task::yield_now().await
                        }
                        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
                        None => pending().await,
                    }
                }
                .fuse();
                pin_mut!(list, circuit, timer);
                futures::select_biased! {
                    result = list => Event::List(result),
                    update = circuit => Event::Circuit(update),
                    _ = timer => Event::Timer,
                }
            };
            let now = tokio::time::Instant::now().into_std();
            match event {
                Event::List(Ok(())) => {
                    let circuits = self.receiver.borrow_and_update().clone();
                    if self.replace(circuits, now) {
                        return true;
                    }
                }
                Event::List(Err(_)) => {
                    self.list_closed = true;
                    if self.replace(Vec::new(), now) {
                        return true;
                    }
                }
                Event::Circuit(Some((region, mut receiver, true))) => {
                    let _ = receiver.borrow_and_update();
                    self.receivers.insert(region, receiver.clone());
                    self.push(region, receiver);
                    if self.update_liveness(now) {
                        return true;
                    }
                }
                Event::Circuit(Some((region, _, false))) => {
                    self.receivers.remove(&region);
                    if self.update_liveness(now) {
                        return true;
                    }
                }
                Event::Circuit(None) => {}
                Event::Timer => {
                    if self.liveness.update(now) {
                        return true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use super::*;

    #[test]
    fn circuit_wakes_only_for_liveness_transitions() {
        let started_at = Instant::now();
        let silence = Duration::from_secs_f64(kinematics::PHASE_OUT_TIME);
        let taper = Duration::from_secs_f64(kinematics::MAX_INTERPOLATION_TIME - kinematics::PHASE_OUT_TIME);
        let maximum_interpolation = silence + taper;
        let region = Region::new(1);
        let mut liveness = MotionCircuitLiveness::default();
        let status = |last_packet_received_at| CircuitMotionStatus { last_packet_received_at, measured_ping_delay: None, connected: true };

        assert!(liveness.replace(HashMap::from([(region, status(started_at))]), started_at));
        let active_packet_at = started_at + silence / 2;
        assert!(!liveness.replace(HashMap::from([(region, status(active_packet_at))]), active_packet_at));
        assert!(liveness.update(active_packet_at + silence + Duration::from_nanos(1)));
        let interpolation_end = active_packet_at + maximum_interpolation;
        assert!(!liveness.update(interpolation_end));
        assert!(liveness.replace(HashMap::from([(region, status(interpolation_end))]), interpolation_end));
        let next_packet_at = interpolation_end + silence / 2;
        assert!(!liveness.replace(HashMap::from([(region, status(next_packet_at))]), next_packet_at));
        let removed_at = interpolation_end + maximum_interpolation;
        assert!(liveness.replace(HashMap::new(), removed_at));
        assert!(liveness.replace(HashMap::from([(region, status(removed_at))]), removed_at));
    }

    #[tokio::test]
    async fn circuit_watcher_wakes_on_state_changes_but_not_active_packets() {
        let region = Region::new(1);
        let status = CircuitMotionStatus { last_packet_received_at: Instant::now(), measured_ping_delay: None, connected: false };
        let (status_sender, status_receiver) = watch::channel(status);
        let (_circuits_sender, circuits_receiver) = watch::channel(vec![(region, status_receiver)]);
        let mut wake = MotionCircuitWake::new(circuits_receiver);

        status_sender.send_modify(|status| {
            status.connected = true;
            status.last_packet_received_at = Instant::now();
        });
        assert!(wake.wait_for_transition().await);

        status_sender.send_modify(|status| status.last_packet_received_at = Instant::now());
        let mut wait = Box::pin(wake.wait_for_transition());
        std::assert_matches!(futures::poll!(wait.as_mut()), Poll::Pending);
        drop(wait);

        status_sender.send_modify(|status| status.connected = false);
        assert!(wake.wait_for_transition().await);

        status_sender.send_modify(|status| {
            status.connected = true;
            status.last_packet_received_at = Instant::now();
        });
        assert!(wake.wait_for_transition().await);
    }

    #[tokio::test(start_paused = true)]
    async fn silence_wakes_once() {
        let region = Region::new(1);
        let last_packet_received_at = tokio::time::Instant::now().into_std();
        let status = CircuitMotionStatus { last_packet_received_at, measured_ping_delay: None, connected: true };
        let (status_sender, status_receiver) = watch::channel(status);
        let (_circuits_sender, circuits_receiver) = watch::channel(vec![(region, status_receiver)]);
        let mut wake = MotionCircuitWake::new(circuits_receiver);
        let mut transition = Box::pin(wake.wait_for_transition());
        let silence = Duration::from_secs_f64(kinematics::PHASE_OUT_TIME);
        let later = Duration::from_secs_f64(kinematics::MAX_INTERPOLATION_TIME);

        std::assert_matches!(futures::poll!(transition.as_mut()), Poll::Pending);
        tokio::time::advance(silence).await;
        std::assert_matches!(futures::poll!(transition.as_mut()), Poll::Pending);
        tokio::time::advance(Duration::from_millis(1)).await;
        std::assert_matches!(futures::poll!(transition.as_mut()), Poll::Ready(true));
        drop(transition);

        let mut pending = Box::pin(wake.wait_for_transition());
        std::assert_matches!(futures::poll!(pending.as_mut()), Poll::Pending);
        tokio::time::advance(later).await;
        std::assert_matches!(futures::poll!(pending.as_mut()), Poll::Pending);
        drop(pending);
        drop(status_sender);
    }
}
