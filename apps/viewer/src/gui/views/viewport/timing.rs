use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct FrameIntervals {
    previous: Option<Instant>,
    samples: VecDeque<(Instant, Duration)>,
    ordered: Vec<Duration>,
}

pub(super) struct Summary {
    pub(super) p50: Duration,
    pub(super) p95: Duration,
    pub(super) max: Duration,
}

impl FrameIntervals {
    pub(super) fn record(&mut self, at: Instant) {
        if let Some(previous) = self.previous.replace(at) {
            let interval = at.saturating_duration_since(previous);
            if interval > Duration::ZERO {
                self.samples.push_back((at, interval));
            }
        }
        self.prune(at);
    }

    pub(super) fn break_sequence(&mut self) {
        self.previous = None;
    }

    pub(super) fn summary(&mut self, now: Instant) -> Option<Summary> {
        self.prune(now);
        self.ordered.clear();
        self.ordered.extend(self.samples.iter().map(|(_, interval)| *interval));
        self.ordered.sort_unstable();
        Some(Summary { p50: percentile(&self.ordered, 50)?, p95: percentile(&self.ordered, 95)?, max: *self.ordered.last()? })
    }

    fn prune(&mut self, now: Instant) {
        while self.samples.front().is_some_and(|(at, _)| now.saturating_duration_since(*at) > WINDOW) {
            self.samples.pop_front();
        }
    }
}

fn percentile(values: &[Duration], percentile: usize) -> Option<Duration> {
    let rank = values.len().checked_mul(percentile)?.div_ceil(100).max(1);
    values.get(rank - 1).copied()
}

pub(super) fn next_frame_deadline(deadline: Instant, completed: Instant, interval: Duration) -> Instant {
    if deadline > completed {
        return deadline;
    }
    let periods = completed.duration_since(deadline).as_nanos() / interval.as_nanos() + 1;
    let advance = Duration::from_nanos(interval.as_nanos().saturating_mul(periods).min(u64::MAX as u128) as u64);
    deadline.checked_add(advance).unwrap_or(completed)
}

pub(super) fn refresh_interval(millihertz: Option<u32>, fallback: Duration) -> Duration {
    millihertz.filter(|rate| *rate > 0).map(|rate| Duration::from_nanos(1_000_000_000_000 / u64::from(rate))).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_intervals_report_nearest_rank_percentiles_for_one_second() {
        let start = Instant::now();
        let mut intervals = FrameIntervals::default();
        intervals.record(start);
        for millis in [16, 33, 53, 83, 183] {
            intervals.record(start + Duration::from_millis(millis));
        }
        let summary = intervals.summary(start + Duration::from_millis(183)).unwrap();
        assert_eq!(summary.p50, Duration::from_millis(20));
        assert_eq!(summary.p95, Duration::from_millis(100));
        assert_eq!(summary.max, Duration::from_millis(100));
        assert!(intervals.summary(start + WINDOW + Duration::from_millis(184)).is_none());
    }

    #[test]
    fn frame_deadlines_keep_cadence_and_skip_missed_refreshes() {
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        assert_eq!(next_frame_deadline(start, start + Duration::from_millis(5), interval), start + interval);
        assert_eq!(next_frame_deadline(start, start + Duration::from_millis(37), interval), start + interval * 3);
        assert_eq!(next_frame_deadline(start + interval, start, interval), start + interval);
    }

    #[test]
    fn frame_intervals_include_active_stalls_but_break_after_idle() {
        let start = Instant::now();
        let mut intervals = FrameIntervals::default();
        intervals.record(start);
        intervals.record(start + Duration::from_millis(1500));
        let summary = intervals.summary(start + Duration::from_millis(1500)).unwrap();
        assert_eq!(summary.max, Duration::from_millis(1500));
        intervals.break_sequence();
        intervals.record(start + Duration::from_secs(4));
        assert!(intervals.summary(start + Duration::from_secs(4)).is_none());
    }

    #[test]
    fn display_refresh_period_uses_millihertz_and_a_fallback() {
        let fallback = Duration::from_micros(16_667);
        assert_eq!(refresh_interval(Some(60_000), fallback), Duration::from_nanos(16_666_666));
        assert_eq!(refresh_interval(Some(144_000), fallback), Duration::from_nanos(6_944_444));
        assert_eq!(refresh_interval(Some(0), fallback), fallback);
        assert_eq!(refresh_interval(None, fallback), fallback);
    }
}
