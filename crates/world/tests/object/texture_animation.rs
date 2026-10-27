use math::UvTransform;

use super::support::{
    Appearance, FieldUpdate, Introduced, Kind, ObjectPatch, Region, Root, TextureAnimation, TextureAnimationError, Updated, local_id,
    object, source, surface, with_region,
};

fn animation(mode: u8, face: u8, size: [u8; 2], start: f32, length: f32, rate: f32) -> TextureAnimation {
    let mut bytes = vec![mode, face, size[0], size[1]];
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
        (TextureAnimation::ON, 0.25, 0.0, 0.0, [0.5, 0.5], [-0.25, 0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::LOOP, 2.25, 0.0, 0.0, [0.5, 0.5], [-0.25, -0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::REVERSE, 1.25, 0.0, 0.0, [0.5, 0.5], [-0.25, -0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::REVERSE, 4.25, 0.0, 0.0, [0.5, 0.5], [0.25, -0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::PING_PONG, 4.25, 0.0, 0.0, [0.5, 0.5], [-0.25, -0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::PING_PONG | TextureAnimation::REVERSE, 4.25, 0.0, 0.0, [0.5, 0.5], [0.25, 0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::PING_PONG, 4.25, 0.0, 0.0, [0.5, 0.5], [-0.25, -0.25], 0.4),
        (
            TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::PING_PONG | TextureAnimation::REVERSE,
            4.25,
            0.0,
            0.0,
            [0.5, 0.5],
            [0.25, 0.25],
            0.4,
        ),
        (TextureAnimation::ON | TextureAnimation::SMOOTH, 1.5, 0.0, 0.0, [0.5, 0.5], [0.5, 0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::SMOOTH | TextureAnimation::REVERSE, 1.5, 0.0, 0.0, [0.5, 0.5], [0.0, -0.25], 0.4),
        (TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::SMOOTH, 4.5, 0.0, 0.0, [0.5, 0.5], [0.0, 0.25], 0.4),
        (
            TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::SMOOTH | TextureAnimation::REVERSE,
            4.5,
            0.0,
            0.0,
            [0.5, 0.5],
            [0.5, -0.25],
            0.4,
        ),
        (TextureAnimation::ON | TextureAnimation::PING_PONG | TextureAnimation::SMOOTH, 6.5, 0.0, 0.0, [0.5, 0.5], [0.5, 0.25], 0.4),
        (
            TextureAnimation::ON | TextureAnimation::LOOP | TextureAnimation::PING_PONG | TextureAnimation::SMOOTH,
            6.5,
            0.0,
            0.0,
            [0.5, 0.5],
            [0.5, 0.25],
            0.4,
        ),
    ];
    for (mode, elapsed, start, length, scale, offset, rotation) in modes {
        let actual = animation(mode, 0xff, [2, 2], start, length, 1.0).transform_at(elapsed, 0.0, base);
        assert_transform(actual, UvTransform { offset, scale, rotation }, mode);
    }
    assert_eq!(animation(0, 0xff, [2, 2], 0.0, 0.0, 1.0).transform_at(5.0, 0.0, base), base);

    let rotate = animation(TextureAnimation::ON | TextureAnimation::ROTATE, 0, [9, 7], 1.0, 3.0, 2.0);
    assert_transform(
        rotate.transform_at(0.5, 0.0, base),
        UvTransform { rotation: 2.0, ..base },
        TextureAnimation::ON | TextureAnimation::ROTATE,
    );
    let scale = animation(TextureAnimation::ON | TextureAnimation::SCALE, 0, [9, 7], 1.0, 3.0, 2.0);
    assert_transform(
        scale.transform_at(0.5, 0.0, base),
        UvTransform { scale: [2.0; 2], ..base },
        TextureAnimation::ON | TextureAnimation::SCALE,
    );
    let rotate_and_scale = animation(TextureAnimation::ON | TextureAnimation::ROTATE | TextureAnimation::SCALE, 0, [9, 7], 1.0, 3.0, 2.0);
    assert_transform(
        rotate_and_scale.transform_at(0.5, 0.0, base),
        UvTransform { rotation: 2.0, ..base },
        TextureAnimation::ON | TextureAnimation::ROTATE | TextureAnimation::SCALE,
    );
    let window = animation(TextureAnimation::ON, 0, [2, 2], 2.0, 2.0, 1.0);
    assert_transform(
        window.transform_at(2.0, 0.0, base),
        UvTransform { offset: [0.25, -0.25], scale: [0.5, 0.5], rotation: base.rotation },
        TextureAnimation::ON,
    );
}

#[test]
fn targeting_validation_and_redraw_state() {
    let all = animation(TextureAnimation::ON, 0xff, [2, 2], 0.0, 0.0, 1.0);
    assert!(all.targets_face(0));
    assert!(all.targets_face(44));
    let one = animation(TextureAnimation::ON, 7, [2, 2], 0.0, 0.0, 1.0);
    assert!(one.targets_face(7));
    assert!(!one.targets_face(8));

    assert!(all.is_changing_at(0.0, 0.0));
    assert!(!animation(TextureAnimation::ON, 0, [2, 2], 0.0, 0.0, 0.0).is_changing_at(0.0, 0.0));
    assert!(!animation(TextureAnimation::ON, 0, [2, 2], 0.0, 0.0, 1.0).is_changing_at(4.0, 0.0));
    assert!(animation(TextureAnimation::ON | TextureAnimation::LOOP, 0, [2, 2], 0.0, 0.0, 1.0).is_changing_at(400.0, 0.0));

    let mut malformed = vec![TextureAnimation::ON, 0, 2, 2];
    malformed.extend_from_slice(&0.0_f32.to_le_bytes());
    malformed.extend_from_slice(&f32::NAN.to_le_bytes());
    malformed.extend_from_slice(&1.0_f32.to_le_bytes());
    assert_eq!(TextureAnimation::from_wire(&malformed), Err(TextureAnimationError::NonFiniteValue));
    assert_eq!(TextureAnimation::from_wire(&malformed[..15]), Err(TextureAnimationError::InvalidLength));
    malformed[1] = 45;
    assert_eq!(TextureAnimation::from_wire(&malformed), Err(TextureAnimationError::InvalidFace));
    let mut negative_length = vec![TextureAnimation::ON, 0, 2, 2];
    negative_length.extend_from_slice(&0.0_f32.to_le_bytes());
    negative_length.extend_from_slice(&(-1.0_f32).to_le_bytes());
    negative_length.extend_from_slice(&1.0_f32.to_le_bytes());
    assert_eq!(TextureAnimation::from_wire(&negative_length), Err(TextureAnimationError::InvalidFrameCount));
    assert_eq!(animation(TextureAnimation::ON, 0, [0, 0], 0.0, 0.0, 1.0).size(), [1, 1]);
    assert_eq!(animation(TextureAnimation::ON | TextureAnimation::SMOOTH, 0, [0, 0], 0.0, 0.0, 1.0).size(), [0, 0]);
}

#[test]
fn live_animation_add_clear_and_patch_retention() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let value = animation(TextureAnimation::ON | TextureAnimation::LOOP, 3, [2, 2], 0.0, 0.0, 1.0);
    let observation = world::ObjectObservation {
        appearance: FieldUpdate::Set(Appearance::Surface),
        texture_animation: FieldUpdate::Set(value),
        ..object(source(region, 1), 0x7283, 1, Kind::Object, Root)
    };
    let Introduced(entity) = world.observe_object(observation).expect("introduce live animated object").outcome else {
        panic!("expected object introduction")
    };
    assert_eq!(world.object(entity).expect("live object").texture_animation(), Some(value));

    let patch = ObjectPatch { surface_appearance: Some(surface(0x7284, 0x7285)), ..ObjectPatch::new(source(region, 2), local_id(1)) };
    assert_eq!(world.patch_object(patch).expect("terse-style appearance patch").outcome, Updated(entity));
    assert_eq!(world.object(entity).expect("live object").texture_animation(), Some(value));

    let clear = ObjectPatch { texture_animation: FieldUpdate::Clear, ..ObjectPatch::new(source(region, 3), local_id(1)) };
    assert_eq!(world.patch_object(clear).expect("clear live animation").outcome, Updated(entity));
    assert_eq!(world.object(entity).expect("live object").texture_animation(), None);
}
