use std::time::{Duration, Instant};

const FRAME_WORK_SAMPLES: usize = 120;

pub(super) struct FrameTimes {
    samples: [Duration; FRAME_WORK_SAMPLES],
    next: usize,
    len: usize,
}

impl Default for FrameTimes {
    fn default() -> Self {
        Self { samples: [Duration::ZERO; FRAME_WORK_SAMPLES], next: 0, len: 0 }
    }
}

pub(super) struct Summary {
    pub(super) p50: Duration,
    pub(super) p95: Duration,
}

impl FrameTimes {
    pub(super) fn record(&mut self, frame_work: Duration) {
        self.samples[self.next] = frame_work;
        self.next = (self.next + 1) % FRAME_WORK_SAMPLES;
        self.len = (self.len + 1).min(FRAME_WORK_SAMPLES);
    }

    pub(super) fn summary(&self) -> Option<Summary> {
        if self.len == 0 {
            return None;
        }
        let mut ordered = self.samples;
        ordered[..self.len].sort_unstable();
        Some(Summary { p50: percentile(&ordered[..self.len], 50)?, p95: percentile(&ordered[..self.len], 95)? })
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

pub(super) fn redraw_interval(display: Duration, focused: bool, movement_pending: bool) -> Duration {
    if focused || movement_pending { display } else { display.max(Duration::from_millis(16) + Duration::from_micros(667)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_before_first_frame() {
        assert!(FrameTimes::default().summary().is_none());
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut times = FrameTimes::default();
        for millis in 1..=20 {
            times.record(Duration::from_millis(millis));
        }
        let summary = times.summary().expect("recorded frame work has a summary");
        assert_eq!(summary.p50, Duration::from_millis(10));
        assert_eq!(summary.p95, Duration::from_millis(19));
    }

    #[test]
    fn ring_keeps_latest_120_frames() {
        let mut times = FrameTimes::default();
        for millis in 1..=121 {
            times.record(Duration::from_millis(millis));
        }
        let summary = times.summary().expect("recorded frame work has a summary");
        assert_eq!(summary.p50, Duration::from_millis(61));
        assert_eq!(summary.p95, Duration::from_millis(115));
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
    fn display_refresh_period_uses_millihertz_and_a_fallback() {
        let fallback = Duration::from_micros(16_667);
        assert_eq!(refresh_interval(Some(60_000), fallback), Duration::from_nanos(16_666_666));
        assert_eq!(refresh_interval(Some(144_000), fallback), Duration::from_nanos(6_944_444));
        assert_eq!(refresh_interval(Some(0), fallback), fallback);
        assert_eq!(refresh_interval(None, fallback), fallback);
    }

    #[test]
    fn background_animation_is_capped_without_delaying_movement_stop() {
        let refresh = Duration::from_nanos(6_944_444);
        let slow_refresh = Duration::from_millis(33);
        assert_eq!(redraw_interval(refresh, false, false), Duration::from_micros(16_667));
        assert_eq!(redraw_interval(slow_refresh, false, false), slow_refresh);
        assert_eq!(redraw_interval(refresh, false, true), refresh);
        assert_eq!(redraw_interval(refresh, true, false), refresh);
    }
}
