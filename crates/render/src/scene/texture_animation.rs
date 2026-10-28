use math::UvTransform;
use world::TextureAnimation;

const ON: u8 = 0x01;
const LOOP: u8 = 0x02;
const REVERSE: u8 = 0x04;
const PING_PONG: u8 = 0x08;
const SMOOTH: u8 = 0x10;
const ROTATE: u8 = 0x20;
const SCALE: u8 = 0x40;

pub(crate) const fn is_enabled(animation: TextureAnimation) -> bool {
    animation.mode() & ON != 0
}

pub(crate) const fn is_smooth(animation: TextureAnimation) -> bool {
    animation.mode() & SMOOTH != 0
}

fn num_frames(animation: TextureAnimation) -> f32 {
    if animation.length() != 0.0 {
        animation.length()
    } else {
        let [size_x, size_y] = animation.size();
        (f32::from(size_x) * f32::from(size_y)).max(1.0)
    }
}

fn full_length(animation: TextureAnimation) -> f32 {
    let num_frames = num_frames(animation);
    if animation.mode() & PING_PONG != 0 {
        if is_smooth(animation) {
            2.0 * num_frames
        } else if animation.mode() & LOOP != 0 {
            (2.0 * num_frames - 2.0).max(1.0)
        } else {
            (2.0 * num_frames - 1.0).max(1.0)
        }
    } else {
        num_frames
    }
}

pub(crate) fn transform_at(animation: TextureAnimation, elapsed_seconds: f64, smooth_frame_offset: f64, base: UvTransform) -> UvTransform {
    if !is_enabled(animation) || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 || !smooth_frame_offset.is_finite() {
        return base;
    }

    let num_frames = num_frames(animation);
    let full_length = full_length(animation);
    if !full_length.is_finite() || full_length <= 0.0 {
        return base;
    }

    let elapsed = elapsed_seconds as f32;
    let frame_offset = if is_smooth(animation) { smooth_frame_offset as f32 } else { 0.0 };
    let mut frame = elapsed * animation.rate() + frame_offset;
    if !frame.is_finite() {
        return base;
    }
    if animation.mode() & LOOP != 0 {
        frame %= full_length;
    } else {
        frame = frame.min(full_length - 1.0);
    }
    if !is_smooth(animation) {
        frame = (frame + 0.01).floor().min(full_length - 1.0);
    }
    if animation.mode() & PING_PONG != 0 && frame >= num_frames {
        frame = if is_smooth(animation) { num_frames - (frame - num_frames) } else { (num_frames - 1.99) - (frame - num_frames) };
    }
    if animation.mode() & REVERSE != 0 {
        frame = if is_smooth(animation) { num_frames - frame } else { (num_frames - 0.99) - frame };
    }
    frame += animation.start();
    if !is_smooth(animation) {
        frame = (frame + 0.5).floor();
    }
    if !frame.is_finite() {
        return base;
    }

    let mut result = base;
    let [size_x, size_y] = animation.size();
    if animation.mode() & ROTATE != 0 {
        result.rotation = frame;
    } else if animation.mode() & SCALE != 0 {
        result.scale = [frame; 2];
    } else if size_x != 0 && size_y != 0 {
        let size_x = f32::from(size_x);
        let size_y = f32::from(size_y);
        let scale = [1.0 / size_x, 1.0 / size_y];
        let x_frame = frame % size_x;
        let y_frame = (frame / size_x).trunc().clamp(i32::MIN as f32, i32::MAX as f32);
        result.scale = scale;
        result.offset = [(-0.5 + 0.5 * scale[0]) + x_frame * scale[0], (0.5 - 0.5 * scale[1]) - y_frame * scale[1]];
    } else {
        result.scale = [1.0; 2];
        result.offset = [-0.5 + 0.5 + frame, 0.0];
    }
    if result.is_finite() { result } else { base }
}

pub(crate) fn smooth_frame_counter(animation: TextureAnimation, elapsed_seconds: f64, smooth_frame_offset: f64) -> Option<f64> {
    if !is_smooth(animation) || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 || !smooth_frame_offset.is_finite() {
        return None;
    }
    let frame = elapsed_seconds as f32 * animation.rate() + smooth_frame_offset as f32;
    frame.is_finite().then_some(f64::from(frame))
}

pub(crate) fn is_changing_at(animation: TextureAnimation, elapsed_seconds: f64, smooth_frame_offset: f64) -> bool {
    if !is_enabled(animation) || animation.rate() == 0.0 || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
        return false;
    }
    let full_length = full_length(animation);
    if !full_length.is_finite() || full_length <= 0.0 {
        return false;
    }
    if animation.mode() & LOOP != 0 {
        return is_smooth(animation) || full_length > 1.0;
    }
    if animation.rate() < 0.0 {
        return true;
    }
    let offset = if is_smooth(animation) { smooth_frame_offset as f32 } else { 0.0 };
    let frame = elapsed_seconds as f32 * animation.rate() + offset;
    frame.is_finite() && frame < full_length - 1.0
}

#[cfg(test)]
mod tests {
    use math::UvTransform;
    use world::TextureAnimation;

    use super::{is_changing_at, transform_at};

    fn animation(mode: u8, size: [u8; 2], start: f32, length: f32, rate: f32) -> TextureAnimation {
        let mut bytes = vec![mode, 0xff, size[0], size[1]];
        bytes.extend_from_slice(&start.to_le_bytes());
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        TextureAnimation::from_wire(&bytes).expect("valid texture animation block")
    }

    fn assert_transform(actual: UvTransform, expected: UvTransform, mode: u8) {
        assert_eq!(actual.scale, expected.scale, "hand-computed from the pinned llviewertextureanim.cpp, mode {mode:#04x}");
        assert_eq!(actual.offset, expected.offset, "hand-computed from the pinned llviewertextureanim.cpp, mode {mode:#04x}");
        assert_eq!(actual.rotation, expected.rotation, "hand-computed from the pinned llviewertextureanim.cpp, mode {mode:#04x}");
    }

    #[test]
    fn mode_fixtures_match_viewer_math() {
        let base = UvTransform { offset: [0.2, -0.3], scale: [2.0, 3.0], rotation: 0.4 };
        let modes = [
            (0x01, 0.25, [0.5, 0.5], [-0.25, 0.25]),
            (0x03, 2.25, [0.5, 0.5], [-0.25, -0.25]),
            (0x05, 1.25, [0.5, 0.5], [-0.25, -0.25]),
            (0x07, 4.25, [0.5, 0.5], [0.25, -0.25]),
            (0x09, 4.25, [0.5, 0.5], [-0.25, -0.25]),
            (0x0d, 4.25, [0.5, 0.5], [0.25, 0.25]),
            (0x0b, 4.25, [0.5, 0.5], [-0.25, -0.25]),
            (0x0f, 4.25, [0.5, 0.5], [0.25, 0.25]),
            (0x11, 1.5, [0.5, 0.5], [0.5, 0.25]),
            (0x15, 1.5, [0.5, 0.5], [0.0, -0.25]),
            (0x13, 4.5, [0.5, 0.5], [0.0, 0.25]),
            (0x17, 4.5, [0.5, 0.5], [0.5, -0.25]),
            (0x19, 6.5, [0.5, 0.5], [0.5, 0.25]),
            (0x1b, 6.5, [0.5, 0.5], [0.5, 0.25]),
        ];
        for (mode, elapsed, scale, offset) in modes {
            let actual = transform_at(animation(mode, [2, 2], 0.0, 0.0, 1.0), elapsed, 0.0, base);
            assert_transform(actual, UvTransform { offset, scale, rotation: 0.4 }, mode);
        }
        assert_eq!(transform_at(animation(0, [2, 2], 0.0, 0.0, 1.0), 5.0, 0.0, base), base);

        let rotate = animation(0x21, [9, 7], 1.0, 3.0, 2.0);
        assert_transform(transform_at(rotate, 0.5, 0.0, base), UvTransform { rotation: 2.0, ..base }, 0x21);
        let scale = animation(0x41, [9, 7], 1.0, 3.0, 2.0);
        assert_transform(transform_at(scale, 0.5, 0.0, base), UvTransform { scale: [2.0; 2], ..base }, 0x41);
        let rotate_and_scale = animation(0x61, [9, 7], 1.0, 3.0, 2.0);
        assert_transform(transform_at(rotate_and_scale, 0.5, 0.0, base), UvTransform { rotation: 2.0, ..base }, 0x61);
        let window = animation(0x01, [2, 2], 2.0, 2.0, 1.0);
        assert_transform(
            transform_at(window, 2.0, 0.0, base),
            UvTransform { offset: [0.25, -0.25], scale: [0.5, 0.5], rotation: base.rotation },
            0x01,
        );
    }

    #[test]
    fn redraw_predicate_stops_when_non_looping_animation_reaches_its_end() {
        assert!(is_changing_at(animation(0x03, [2, 2], 0.0, 0.0, 1.0), 400.0, 0.0));
        assert!(!is_changing_at(animation(0x01, [2, 2], 0.0, 0.0, 1.0), 4.0, 0.0));
        assert!(!is_changing_at(animation(0x01, [2, 2], 0.0, 0.0, 0.0), 0.0, 0.0));
    }
}
