use super::support::{
    Appearance, FieldUpdate, Introduced, Kind, ObjectPatch, Region, Root, TextureAnimation, TextureAnimationError, Updated, local_id,
    object, source, surface, with_region,
};

const ON: u8 = 0x01;
const LOOP: u8 = 0x02;
const SMOOTH: u8 = 0x10;

fn animation(mode: u8, face: u8, size: [u8; 2], start: f32, length: f32, rate: f32) -> TextureAnimation {
    let mut bytes = vec![mode, face, size[0], size[1]];
    bytes.extend_from_slice(&start.to_le_bytes());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    TextureAnimation::from_wire(&bytes).expect("valid texture animation block")
}

#[test]
fn parses_animation_wire_data_and_validates_boundaries() {
    let value = animation(ON | LOOP, 3, [2, 4], 1.5, 6.0, -2.0);
    assert_eq!(value.mode(), ON | LOOP);
    assert_eq!(value.face(), 3);
    assert_eq!(value.size(), [2, 4]);
    assert_eq!(value.start(), 1.5);
    assert_eq!(value.length(), 6.0);
    assert_eq!(value.rate(), -2.0);

    let mut malformed = vec![ON, 0, 2, 2];
    malformed.extend_from_slice(&0.0_f32.to_le_bytes());
    malformed.extend_from_slice(&f32::NAN.to_le_bytes());
    malformed.extend_from_slice(&1.0_f32.to_le_bytes());
    assert_eq!(TextureAnimation::from_wire(&malformed), Err(TextureAnimationError::NonFiniteValue));
    assert_eq!(TextureAnimation::from_wire(&malformed[..15]), Err(TextureAnimationError::InvalidLength));
    malformed[1] = 45;
    assert_eq!(TextureAnimation::from_wire(&malformed), Err(TextureAnimationError::InvalidFace));

    let mut negative_length = vec![ON, 0, 2, 2];
    negative_length.extend_from_slice(&0.0_f32.to_le_bytes());
    negative_length.extend_from_slice(&(-1.0_f32).to_le_bytes());
    negative_length.extend_from_slice(&1.0_f32.to_le_bytes());
    assert_eq!(TextureAnimation::from_wire(&negative_length), Err(TextureAnimationError::InvalidFrameCount));
    assert_eq!(animation(ON, 0, [0, 0], 0.0, 0.0, 1.0).size(), [1, 1]);
    assert_eq!(animation(ON | SMOOTH, 0, [0, 0], 0.0, 0.0, 1.0).size(), [0, 0]);
}

#[test]
fn animation_targets_a_face_or_every_face() {
    let all = animation(ON, 0xff, [2, 2], 0.0, 0.0, 1.0);
    assert!(all.targets_face(0));
    assert!(all.targets_face(44));
    let one = animation(ON, 7, [2, 2], 0.0, 0.0, 1.0);
    assert!(one.targets_face(7));
    assert!(!one.targets_face(8));
}

#[test]
fn live_animation_add_clear_and_patch_retention() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let value = animation(ON | LOOP, 3, [2, 2], 0.0, 0.0, 1.0);
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
