use std::time::{Duration, Instant};

use super::{Evaluated, blend_sky, blend_water};

const ALTITUDE_TRANSITION: Duration = Duration::from_secs(5);
const FRAME_CADENCE: Duration = Duration::from_millis(16);
const CYCLE_STEPS: f64 = 240.0;

#[derive(Default)]
pub struct Transition {
    identity: Option<u64>,
    sky_track: Option<u8>,
    output: Option<Evaluated>,
    from: Option<Evaluated>,
    target: Option<Evaluated>,
    started: Option<Instant>,
    duration: Duration,
}

impl Transition {
    pub fn update(&mut self, identity: u64, target: Evaluated, duration: Duration, now: Instant) -> (Evaluated, Option<Instant>) {
        let Some(previous) = self.output.as_ref() else {
            self.identity = Some(identity);
            self.sky_track = target.sky_track;
            self.output = Some(target.clone());
            return (target.clone(), next_cycle_redraw(&target, now));
        };
        let identity_changed = self.identity != Some(identity);
        let track_changed = self.sky_track != target.sky_track;
        if identity_changed || track_changed {
            self.from = Some(previous.clone());
            self.target = Some(target.clone());
            self.started = Some(now);
            self.duration = if track_changed && !identity_changed { ALTITUDE_TRANSITION } else { duration };
            self.identity = Some(identity);
            self.sky_track = target.sky_track;
        } else if self.started.is_some() {
            self.target = Some(target.clone());
        } else {
            self.output = Some(target.clone());
            self.identity = Some(identity);
            self.sky_track = target.sky_track;
            return (target.clone(), next_cycle_redraw(&target, now));
        }

        if self.duration.is_zero() {
            self.clear_transition();
            self.output = Some(target.clone());
            return (target.clone(), next_cycle_redraw(&target, now));
        }

        let from = self.from.as_ref().unwrap_or(previous);
        let end = self.target.as_ref().unwrap_or(&target);
        let started = self.started.unwrap_or(now);
        let progress = (now.saturating_duration_since(started).as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0);
        let output = if progress >= 1.0 { end.clone() } else { blend_evaluated(from, end, progress as f32) };
        self.output = Some(output.clone());
        if progress >= 1.0 {
            self.clear_transition();
            return (output.clone(), next_cycle_redraw(&output, now));
        }
        let frame = now.checked_add(FRAME_CADENCE);
        let finish = started.checked_add(self.duration);
        let next = match (frame, finish) {
            (Some(frame), Some(finish)) => Some(frame.min(finish)),
            (Some(frame), None) => Some(frame),
            (None, Some(finish)) => Some(finish),
            (None, None) => None,
        };
        (output, next)
    }

    fn clear_transition(&mut self) {
        self.from = None;
        self.target = None;
        self.started = None;
        self.duration = Duration::ZERO;
    }
}

fn blend_evaluated(start: &Evaluated, end: &Evaluated, factor: f32) -> Evaluated {
    Evaluated {
        sky: blend_sky(&start.sky, &end.sky, factor),
        water: blend_water(&start.water, &end.water, factor),
        sky_track: end.sky_track,
        day_position: end.day_position,
        cycle_length: end.cycle_length,
    }
}

fn next_cycle_redraw(value: &Evaluated, now: Instant) -> Option<Instant> {
    let seconds = value.cycle_length? / CYCLE_STEPS;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    now.checked_add(Duration::from_secs_f64(seconds))
}
