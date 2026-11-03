use std::time::{Duration, Instant};

use super::{Evaluated, blend_sky, blend_water};

const ALTITUDE_TRANSITION: Duration = Duration::from_secs(5);
const FRAME_CADENCE: Duration = Duration::from_millis(16);
const CYCLE_REDRAW_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Transition {
    identity: Option<u64>,
    sky_track: Option<u8>,
    output: Option<Evaluated>,
    blend: Option<Blend>,
}

struct Blend {
    from: Evaluated,
    started: Instant,
    duration: Duration,
}

impl Transition {
    pub(super) fn is_blending(&self) -> bool {
        self.blend.is_some()
    }

    pub fn update(&mut self, identity: u64, target: Evaluated, duration: Duration, now: Instant) -> (Evaluated, Option<Instant>) {
        let Some(previous) = self.output.as_ref() else {
            self.identity = Some(identity);
            self.sky_track = target.sky_track;
            self.output = Some(target.clone());
            let next = next_cycle_redraw(&target, now);
            return (target, next);
        };

        let identity_changed = self.identity != Some(identity);
        let track_changed = self.sky_track != target.sky_track;
        if identity_changed || track_changed {
            let duration = if track_changed && !identity_changed { ALTITUDE_TRANSITION } else { duration };
            let duration = if now.checked_add(duration).is_some() { duration } else { Duration::ZERO };
            self.blend = Some(Blend { from: previous.clone(), started: now, duration });
            self.identity = Some(identity);
            self.sky_track = target.sky_track;
        }

        let Some(blend) = self.blend.as_ref() else {
            self.output = Some(target.clone());
            let next = next_cycle_redraw(&target, now);
            return (target, next);
        };
        if blend.duration.is_zero() {
            self.blend = None;
            self.output = Some(target.clone());
            let next = next_cycle_redraw(&target, now);
            return (target, next);
        }

        let progress = (now.saturating_duration_since(blend.started).as_secs_f64() / blend.duration.as_secs_f64()).clamp(0.0, 1.0);
        let output = if progress >= 1.0 { target } else { blend_evaluated(&blend.from, &target, progress as f32) };
        let finish = blend.started.checked_add(blend.duration);
        self.output = Some(output.clone());
        if progress >= 1.0 {
            self.blend = None;
            let next = next_cycle_redraw(&output, now);
            return (output, next);
        }
        let frame = now.checked_add(FRAME_CADENCE);
        let next = match (frame, finish) {
            (Some(frame), Some(finish)) => Some(frame.min(finish)),
            (Some(frame), None) => Some(frame),
            (None, Some(finish)) => Some(finish),
            (None, None) => None,
        };
        (output, next)
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
    let cycle_length = value.cycle_length?;
    if !cycle_length.is_finite() || cycle_length <= 0.0 {
        return None;
    }
    now.checked_add(CYCLE_REDRAW_INTERVAL)
}
