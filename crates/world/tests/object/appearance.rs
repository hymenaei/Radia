use super::support::{
    Appearance, AssetId, Avatar, AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, FaceAppearance,
    FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate, IgnoredStale, Introduced, MAX_AVATAR_TEXTURE_SLOTS,
    MAX_PBR_MATERIAL_BINDINGS, MediaEntry, Object, ObjectMediaUpdate, ObjectPatch, PbrMaterialAssignments, PbrMaterialAssignmentsError,
    PropertiesPatch, Quat, Region, Root, Shape, SitTarget, SurfaceAppearance, SurfaceAppearanceError, Unchanged, Unknown, UntrustedSource,
    Updated, Uuid, Vec3, address, local_id, mesh_shape, object, patch_properties_current, pbr_materials, sculpt_shape, source, surface,
    untracked, uuid, with_region,
};

#[test]
fn shape_and_appearance_are_independent() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 1), 207, 1, Unknown(231), Root);
    observation.shape = FieldUpdate::Set(mesh_shape(0x207));
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    let result = world.observe_object(observation).unwrap();
    let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
    let object = world.object(entity).unwrap();

    assert_eq!(object.kind(), Unknown(231));
    assert_eq!(object.shape(), Some(mesh_shape(0x207)));
    assert_eq!(object.appearance(), Some(Appearance::Surface));
}

#[test]
fn shape_appearance_order_and_clear() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 1), 208, 1, Object, Root);
    observation.shape = FieldUpdate::Set(sculpt_shape(0x208));
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    let result = world.observe_object(observation).unwrap();
    let Introduced(entity) = result.outcome else { panic!("expected object introduction") };

    let stale =
        ObjectPatch { shape: FieldUpdate::Clear, appearance: FieldUpdate::Clear, ..ObjectPatch::new(source(region, 0), local_id(1)) };
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().shape(), Some(sculpt_shape(0x208)));
    assert_eq!(world.object(entity).unwrap().appearance(), Some(Appearance::Surface));

    let clear_appearance = ObjectPatch { appearance: FieldUpdate::Clear, ..ObjectPatch::new(source(region, 2), local_id(1)) };
    assert_eq!(world.patch_object(clear_appearance).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().shape(), Some(sculpt_shape(0x208)));
    assert_eq!(world.object(entity).unwrap().appearance(), None);

    let clear_shape = ObjectPatch { shape: FieldUpdate::Clear, ..ObjectPatch::new(source(region, 3), local_id(1)) };
    assert_eq!(world.patch_object(clear_shape).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().shape(), None);
    assert_eq!(world.object(entity).unwrap().appearance(), None);
}

#[test]
fn shape_appearance_fences_are_independent() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 10), 209, 1, Object, Root);
    observation.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object introduction") };

    let newer_appearance =
        ObjectPatch { appearance: FieldUpdate::Set(Appearance::Avatar), ..ObjectPatch::new(source(region, 20), local_id(1)) };
    assert_eq!(world.patch_object(newer_appearance).unwrap().outcome, Updated(entity));

    let mixed_freshness = ObjectPatch {
        shape: FieldUpdate::Set(mesh_shape(0x209)),
        appearance: FieldUpdate::Set(Appearance::Surface),
        ..ObjectPatch::new(source(region, 15), local_id(1))
    };
    assert_eq!(world.patch_object(mixed_freshness).unwrap().outcome, Updated(entity));
    let current = world.object(entity).unwrap();
    assert_eq!(current.shape(), Some(mesh_shape(0x209)));
    assert_eq!(current.appearance(), Some(Appearance::Avatar));
    let clear =
        ObjectPatch { shape: FieldUpdate::Clear, appearance: FieldUpdate::Clear, ..ObjectPatch::new(source(region, 30), local_id(1)) };
    assert_eq!(world.patch_object(clear).unwrap().outcome, Updated(entity));
    assert_eq!(world.objects().iter().map(|object| object.entity()).collect::<Vec<_>>(), [entity]);
    assert_eq!(world.object(entity).unwrap().shape(), None);
    assert_eq!(world.object(entity).unwrap().appearance(), None);

    let stale = ObjectPatch {
        shape: FieldUpdate::Set(mesh_shape(0x209)),
        appearance: FieldUpdate::Set(Appearance::Surface),
        ..ObjectPatch::new(source(region, 25), local_id(1))
    };
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().shape(), None);
    assert_eq!(world.entity_at(address(region, 1)), Some(entity));
}

#[test]
fn texture_entry_has_independent_freshness() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 10), 211, 1, Object, Root);
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    observation.surface_appearance = Some(surface(0x211, 0x244));
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object introduction") };

    let category = ObjectPatch { appearance: FieldUpdate::Set(Appearance::Avatar), ..ObjectPatch::new(source(region, 20), local_id(1)) };
    assert_eq!(world.patch_object(category).unwrap().outcome, Updated(entity));
    let entry = ObjectPatch { surface_appearance: Some(surface(0x212, 0x245)), ..ObjectPatch::new(source(region, 15), local_id(1)) };
    assert_eq!(world.patch_object(entry).unwrap().outcome, Updated(entity));
    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.appearance(), Some(Appearance::Avatar));
        assert_eq!(current.surface_appearance().unwrap().default_face().texture, Some(AssetId::new(uuid(0x212))));
        assert_eq!(current.surface_appearance().unwrap().face(44).unwrap().texture, Some(AssetId::new(uuid(0x245))));
    }

    let stale = ObjectPatch { surface_appearance: Some(surface(0x213, 0x246)), ..ObjectPatch::new(source(region, 14), local_id(1)) };
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().surface_appearance().unwrap().default_face().texture, Some(AssetId::new(uuid(0x212))));
}

#[test]
fn avatar_values_merge() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut avatar = object(source(region, 1), 214, 1, Avatar, Root);
    avatar.appearance = FieldUpdate::Set(Appearance::Avatar);
    let Introduced(entity) = world.observe_object(avatar).unwrap().outcome else { panic!("expected avatar introduction") };

    let mut textures = [None; MAX_AVATAR_TEXTURE_SLOTS];
    textures[usize::from(AvatarTextureSlot::HEAD_BAKED.index())] = Some(AssetId::new(uuid(0x214)));
    let first = AvatarAppearanceUpdate::new(
        Some(1),
        Some(10),
        vec![(AvatarVisualParamIndex::new(0).unwrap(), 10), (AvatarVisualParamIndex::new(2).unwrap(), 30)],
        Some(AvatarTextureSlots::new(textures).unwrap()),
    )
    .unwrap();
    let first_patch = ObjectPatch { avatar_appearance: Some(first), ..ObjectPatch::new(source(region, 2), local_id(1)) };
    assert_eq!(world.patch_object(first_patch).unwrap().outcome, Updated(entity));

    let second = AvatarAppearanceUpdate::new(None, Some(11), vec![(AvatarVisualParamIndex::new(0).unwrap(), 20)], None).unwrap();
    let second_patch = ObjectPatch { avatar_appearance: Some(second), ..ObjectPatch::new(source(region, 3), local_id(1)) };
    assert_eq!(world.patch_object(second_patch).unwrap().outcome, Updated(entity));

    let avatar = world.object(entity).unwrap();
    let appearance = avatar.avatar_appearance().unwrap();
    assert_eq!(appearance.cof_version(), Some(11));
    assert_eq!(appearance.visual_param(AvatarVisualParamIndex::new(0).unwrap()), Some(20));
    assert_eq!(appearance.visual_param(AvatarVisualParamIndex::new(2).unwrap()), Some(30));
    assert_eq!(appearance.texture_slots().and_then(|slots| slots.texture(AvatarTextureSlot::HEAD_BAKED)), Some(AssetId::new(uuid(0x214))));
    assert_eq!(avatar.surface_appearance(), None);
}

#[test]
fn self_cof_is_stale() {
    let region = Region::new(25);
    let mut world = with_region(region);
    world.set_self_agent(uuid(215)).unwrap();
    for (id, local_id_value) in [(215, 1), (216, 2)] {
        let mut avatar = object(source(region, 1), id, local_id_value, Avatar, Root);
        avatar.appearance = FieldUpdate::Set(Appearance::Avatar);
        assert!(matches!(world.observe_object(avatar).unwrap().outcome, Introduced(_)));
    }
    let param = AvatarVisualParamIndex::new(0).unwrap();
    let initial = AvatarAppearanceUpdate::new(Some(1), Some(10), vec![(param, 10)], None).unwrap();
    let _ = world
        .patch_object(ObjectPatch { avatar_appearance: Some(initial.clone()), ..ObjectPatch::new(source(region, 2), local_id(1)) })
        .unwrap();
    let _ =
        world.patch_object(ObjectPatch { avatar_appearance: Some(initial), ..ObjectPatch::new(source(region, 2), local_id(2)) }).unwrap();

    let stale = AvatarAppearanceUpdate::new(None, Some(10), vec![(param, 20)], None).unwrap();
    let rejected = world
        .patch_object(ObjectPatch { avatar_appearance: Some(stale.clone()), ..ObjectPatch::new(source(region, 3), local_id(1)) })
        .unwrap();
    assert_eq!(rejected.outcome, Unchanged);
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::AvatarAppearance, reason: FieldRejectionReason::StaleVersion }]
    );
    assert_eq!(world.object(world.entity(uuid(215)).unwrap()).unwrap().avatar_appearance().unwrap().visual_param(param), Some(10));

    let remote =
        world.patch_object(ObjectPatch { avatar_appearance: Some(stale), ..ObjectPatch::new(source(region, 3), local_id(2)) }).unwrap();
    assert_eq!(remote.outcome, Updated(world.entity(uuid(216)).unwrap()));
    assert_eq!(world.object(world.entity(uuid(216)).unwrap()).unwrap().avatar_appearance().unwrap().visual_param(param), Some(20));
}

#[test]
fn self_missing_cof_is_stale() {
    let region = Region::new(25);
    let mut world = with_region(region);
    world.set_self_agent(uuid(218)).unwrap();
    let mut avatar = object(source(region, 1), 218, 1, Avatar, Root);
    avatar.appearance = FieldUpdate::Set(Appearance::Avatar);
    assert!(matches!(world.observe_object(avatar).unwrap().outcome, Introduced(_)));

    let param = AvatarVisualParamIndex::new(0).unwrap();
    let missing = AvatarAppearanceUpdate::new(None, None, vec![(param, 20)], None).unwrap();
    let rejected = world
        .patch_object(ObjectPatch { avatar_appearance: Some(missing.clone()), ..ObjectPatch::new(source(region, 2), local_id(1)) })
        .unwrap();
    assert_eq!(rejected.outcome, Unchanged);
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::AvatarAppearance, reason: FieldRejectionReason::StaleVersion }]
    );
    assert_eq!(world.object(world.entity(uuid(218)).unwrap()).unwrap().avatar_appearance(), None);

    let initial = AvatarAppearanceUpdate::new(Some(1), Some(10), vec![(param, 10)], None).unwrap();
    assert_eq!(
        world
            .patch_object(ObjectPatch { avatar_appearance: Some(initial), ..ObjectPatch::new(source(region, 3), local_id(1)) })
            .unwrap()
            .outcome,
        Updated(world.entity(uuid(218)).unwrap())
    );

    let stale =
        world.patch_object(ObjectPatch { avatar_appearance: Some(missing), ..ObjectPatch::new(source(region, 4), local_id(1)) }).unwrap();
    assert_eq!(stale.outcome, Unchanged);
    assert_eq!(world.object(world.entity(uuid(218)).unwrap()).unwrap().avatar_appearance().unwrap().visual_param(param), Some(10));
}

#[test]
fn avatar_update_rejects_object() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let Introduced(entity) = world.observe_object(object(source(region, 1), 217, 1, Object, Root)).unwrap().outcome else {
        panic!("expected object introduction")
    };
    let update = AvatarAppearanceUpdate::default();
    let patch = ObjectPatch { avatar_appearance: Some(update), ..ObjectPatch::new(source(region, 2), local_id(1)) };
    let applied = world.patch_object(patch).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::AvatarAppearance, reason: FieldRejectionReason::InvalidValue { field: "avatar_appearance" } }]
    );
    assert_eq!(world.object(entity).unwrap().avatar_appearance(), None);
}

#[test]
fn untracked_texture_entry_cannot_replace() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 1), 212, 1, Object, Root);
    observation.surface_appearance = Some(surface(0x214, 0x244));
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object introduction") };

    let patch = ObjectPatch { surface_appearance: Some(surface(0x215, 0x245)), ..ObjectPatch::new(untracked(region, 1_000), local_id(1)) };
    let applied = world.patch_object(patch).unwrap();
    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::TextureEntry, reason: UntrustedSource }]
    );
    assert_eq!(world.object(entity).unwrap().surface_appearance().unwrap().default_face().texture, Some(AssetId::new(uuid(0x214))));
}

#[test]
fn pbr_materials_freshness_and_clears() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 10), 213, 1, Object, Root);
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    observation.pbr_materials = Some(pbr_materials(0x220, 0x244));
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object introduction") };

    let category = ObjectPatch { appearance: FieldUpdate::Set(Appearance::Avatar), ..ObjectPatch::new(source(region, 20), local_id(1)) };
    assert_eq!(world.patch_object(category).unwrap().outcome, Updated(entity));
    let materials = ObjectPatch { pbr_materials: Some(pbr_materials(0x221, 0x245)), ..ObjectPatch::new(source(region, 15), local_id(1)) };
    assert_eq!(world.patch_object(materials).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().appearance(), Some(Appearance::Avatar));
    assert_eq!(world.object(entity).unwrap().pbr_materials().unwrap().material(44), Some(AssetId::new(uuid(0x245))));

    let stale = ObjectPatch { pbr_materials: Some(pbr_materials(0x222, 0x246)), ..ObjectPatch::new(source(region, 14), local_id(1)) };
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().pbr_materials().unwrap().material(44), Some(AssetId::new(uuid(0x245))));
    let clear = ObjectPatch { pbr_materials: Some(PbrMaterialAssignments::default()), ..ObjectPatch::new(source(region, 21), local_id(1)) };
    assert_eq!(world.patch_object(clear).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().pbr_materials(), None);
}

#[test]
fn pbr_materials_reject_invalid_bindings() {
    let too_many = vec![(0, AssetId::new(uuid(1))); MAX_PBR_MATERIAL_BINDINGS + 1];
    assert_eq!(PbrMaterialAssignments::new(too_many), Err(PbrMaterialAssignmentsError::TooManyBindings));
    assert_eq!(PbrMaterialAssignments::new(vec![(45, AssetId::new(uuid(1)))]), Err(PbrMaterialAssignmentsError::FaceIndexOutOfRange(45)));
    assert_eq!(
        PbrMaterialAssignments::new(vec![(1, AssetId::new(uuid(1))), (1, AssetId::new(uuid(2)))]),
        Err(PbrMaterialAssignmentsError::DuplicateFaceIndex(1))
    );
    assert!(PbrMaterialAssignments::new(vec![(1, AssetId::new(Uuid::nil()))]).unwrap().is_empty());
}

#[test]
fn avatars_ignore_prim_face_data() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 1), 214, 1, Avatar, Root);
    observation.surface_appearance = Some(surface(0x230, 0x244));
    observation.pbr_materials = Some(pbr_materials(0x231, 0x245));
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected avatar introduction") };
    let current = world.object(entity).unwrap();
    assert_eq!(current.surface_appearance(), None);
    assert_eq!(current.pbr_materials(), None);
}

#[test]
fn surface_appearance_rejects_bad_faces() {
    let default = FaceAppearance::default();
    let oversized = vec![(0, FaceAppearance { glow: f32::NAN, ..default }); world::MAX_SURFACE_FACES + 1];
    assert_eq!(SurfaceAppearance::new(default, oversized), Err(SurfaceAppearanceError::TooManyOverrides));
    assert_eq!(SurfaceAppearance::new(default, vec![(45, default)]), Err(SurfaceAppearanceError::FaceIndexOutOfRange(45)));
    assert_eq!(
        SurfaceAppearance::new(default, vec![(1, default), (1, FaceAppearance { texture: Some(AssetId::new(uuid(1))), ..default })]),
        Err(SurfaceAppearanceError::DuplicateFaceIndex(1))
    );
    for transform in [
        math::UvTransform { scale: [f32::NAN, 1.0], ..Default::default() },
        math::UvTransform { scale: [1.0, f32::INFINITY], ..Default::default() },
        math::UvTransform { offset: [f32::NEG_INFINITY, 0.0], ..Default::default() },
        math::UvTransform { offset: [0.0, f32::NAN], ..Default::default() },
        math::UvTransform { rotation: f32::NAN, ..Default::default() },
    ] {
        let face = FaceAppearance { transform, ..default };
        assert_eq!(SurfaceAppearance::new(face, Vec::new()), Err(SurfaceAppearanceError::NonFiniteValue));
        assert_eq!(SurfaceAppearance::new(default, vec![(0, face)]), Err(SurfaceAppearanceError::NonFiniteValue));
    }
    for (face, field) in [
        (FaceAppearance { texture: Some(AssetId::new(Uuid::nil())), ..default }, "texture"),
        (FaceAppearance { texture: Some(AssetId::new(Uuid::max())), ..default }, "texture"),
        (FaceAppearance { material: Some(AssetId::new(Uuid::nil())), ..default }, "material"),
        (FaceAppearance { bump: 32, ..default }, "bump"),
        (FaceAppearance { shiny: 4, ..default }, "shiny"),
        (FaceAppearance { mapping: world::TextureMapping::Unsupported(0), ..default }, "mapping"),
        (FaceAppearance { mapping: world::TextureMapping::Unsupported(1), ..default }, "mapping"),
    ] {
        assert_eq!(SurfaceAppearance::new(face, Vec::new()), Err(SurfaceAppearanceError::InvalidValue(field)));
        assert_eq!(SurfaceAppearance::new(default, vec![(0, face)]), Err(SurfaceAppearanceError::InvalidValue(field)));
    }
}

#[test]
fn media_prefix_clears_and_preserves_suffix() {
    let region = Region::new(1);
    let mut world = with_region(region);
    let mut observation = object(source(region, 1), 215, 1, Object, Root);
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object") };

    let first = MediaEntry { current_url: "https://media.test/first".into(), ..MediaEntry::default() };
    let second = MediaEntry { current_url: "https://media.test/second".into(), ..MediaEntry::default() };
    let third = MediaEntry { current_url: "https://media.test/third".into(), ..MediaEntry::default() };
    let initial = ObjectMediaUpdate::new(3, vec![Some(first), Some(second), Some(third)]).unwrap();
    assert_eq!(
        world
            .patch_object(ObjectPatch { object_media: Some(initial), ..ObjectPatch::new(source(region, 2), local_id(1)) })
            .unwrap()
            .outcome,
        Updated(entity)
    );

    let clear = ObjectMediaUpdate::new(4, vec![None]).unwrap();
    assert_eq!(
        world.patch_object(ObjectPatch { object_media: Some(clear), ..ObjectPatch::new(source(region, 3), local_id(1)) }).unwrap().outcome,
        Updated(entity)
    );
    {
        let object = world.object(entity).unwrap();
        let current = object.object_media().unwrap();
        assert_eq!(current.version(), 4);
        assert_eq!(current.face(0), None);
        assert_eq!(current.face(1).unwrap().current_url, "https://media.test/second");
        assert_eq!(current.face(2).unwrap().current_url, "https://media.test/third");
    }

    let stale = ObjectMediaUpdate::new(3, vec![Some(MediaEntry::default())]).unwrap();
    assert_eq!(
        world.patch_object(ObjectPatch { object_media: Some(stale), ..ObjectPatch::new(source(region, 4), local_id(1)) }).unwrap().outcome,
        Unchanged
    );
    assert_eq!(world.object(entity).unwrap().object_media().unwrap().version(), 4);
    assert_eq!(world.object(entity).unwrap().object_media().unwrap().face(0), None);

    let replacement = MediaEntry { current_url: "https://media.test/new".into(), ..MediaEntry::default() };
    let replace_and_clear = ObjectMediaUpdate::new(5, vec![Some(replacement), None]).unwrap();
    assert_eq!(
        world
            .patch_object(ObjectPatch { object_media: Some(replace_and_clear), ..ObjectPatch::new(source(region, 5), local_id(1)) })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let object = world.object(entity).unwrap();
    let current = object.object_media().unwrap();
    assert_eq!(current.face(0).unwrap().current_url, "https://media.test/new");
    assert_eq!(current.face(1), None);
    assert_eq!(current.face(2).unwrap().current_url, "https://media.test/third");
}

#[test]
fn media_requires_tracked_surface() {
    let region = Region::new(1);
    let mut world = with_region(region);
    let observation = object(source(region, 1), 216, 1, Object, Root);
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object") };
    let update = ObjectMediaUpdate::new(1, vec![Some(MediaEntry::default())]).unwrap();
    let result =
        world.patch_object(ObjectPatch { object_media: Some(update.clone()), ..ObjectPatch::new(source(region, 2), local_id(1)) }).unwrap();
    assert!(result.rejections.iter().any(|rejection| *rejection
        == FieldRejection { group: FieldGroup::ObjectMedia, reason: FieldRejectionReason::InvalidValue { field: "object_media" } }));
    assert_eq!(world.object(entity).unwrap().object_media(), None);

    let mut surface = object(source(region, 3), 217, 2, Object, Root);
    surface.appearance = FieldUpdate::Set(Appearance::Surface);
    let Introduced(surface_entity) = world.observe_object(surface).unwrap().outcome else { panic!("expected surface object") };
    let result =
        world.patch_object(ObjectPatch { object_media: Some(update), ..ObjectPatch::new(untracked(region, 4), local_id(2)) }).unwrap();
    assert!(
        result.rejections.iter().any(|rejection| *rejection == FieldRejection { group: FieldGroup::ObjectMedia, reason: UntrustedSource })
    );
    assert_eq!(world.object(surface_entity).unwrap().object_media(), None);
}

#[test]
fn untracked_shape_appearance_rejected() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let mut observation = object(source(region, 2), 210, 1, Object, Root);
    observation.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("expected object introduction") };

    let untracked = ObjectPatch {
        shape: FieldUpdate::Set(mesh_shape(0x210)),
        appearance: FieldUpdate::Set(Appearance::Avatar),
        ..ObjectPatch::new(untracked(region, 1_000), local_id(1))
    };
    let applied = world.patch_object(untracked).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::Shape, reason: UntrustedSource },
            FieldRejection { group: FieldGroup::Appearance, reason: UntrustedSource },
        ]
    );
    let current = world.object(entity).unwrap();
    assert_eq!(current.shape(), Some(Shape::Prim(Default::default())));
    assert_eq!(current.appearance(), Some(Appearance::Surface));
}

#[test]
fn transfer_scopes_regional_fields() {
    let first = Region::new(38);
    let second = Region::new(39);
    let mut world = with_region(first);
    assert!(matches!(world.adopt_region(second).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    let object_id = uuid(380);
    let owner = uuid(381);
    let creator = uuid(382);
    let first_object = world.observe_object(object(source(first, 1), 380, 1, Object, Root)).unwrap();
    let Introduced(entity) = first_object.outcome else { panic!("expected object introduction") };
    let saved_shape = mesh_shape(0x380);
    assert_eq!(
        world
            .patch_object(ObjectPatch { shape: FieldUpdate::Set(saved_shape), ..ObjectPatch::new(source(first, 200), local_id(1)) })
            .unwrap()
            .outcome,
        Updated(entity)
    );

    assert_eq!(
        patch_properties_current(&mut world, source(first, 200), object_id, PropertiesPatch { owner_id: Some(owner), creator_id: None })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let stale_creator =
        patch_properties_current(&mut world, source(first, 199), object_id, PropertiesPatch { owner_id: None, creator_id: Some(creator) })
            .unwrap();
    assert_eq!(stale_creator.outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().properties().creator_id, Some(creator));

    let target = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(first, 201), object_id, Some(target)).unwrap().outcome, Updated(entity));
    let transfer = world.observe_object(object(source(second, 1), 380, 1, Object, Root)).unwrap();
    assert_eq!(transfer.outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(owner));

    assert_eq!(
        patch_properties_current(&mut world, source(second, 2), object_id, PropertiesPatch { owner_id: None, creator_id: Some(creator) })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let destination_target = SitTarget { position: Vec3::Y, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(second, 2), object_id, Some(destination_target)).unwrap().outcome, Updated(entity));

    assert_eq!(
        patch_properties_current(
            &mut world,
            source(first, 300),
            object_id,
            PropertiesPatch { owner_id: None, creator_id: Some(uuid(383)) }
        )
        .unwrap()
        .outcome,
        IgnoredStale
    );
    assert_eq!(world.patch_sit_target(source(first, 301), object_id, Some(target)).unwrap().outcome, Unchanged);
    let view = world.object(entity).unwrap();
    assert_eq!(view.properties().owner_id, Some(owner));
    assert_eq!(view.properties().creator_id, Some(creator));
    assert_eq!(view.sit_target(), Some(destination_target));
    let returned = world.observe_object(object(source(first, 2), 380, 2, Object, Root)).unwrap();
    assert_eq!(returned.outcome, Updated(entity));
    assert_eq!(
        world
            .patch_object(ObjectPatch { shape: FieldUpdate::Set(mesh_shape(0x383)), ..ObjectPatch::new(source(first, 3), local_id(2)) })
            .unwrap()
            .outcome,
        Unchanged
    );
    let next_creator = uuid(383);
    assert_eq!(
        patch_properties_current(
            &mut world,
            source(first, 3),
            object_id,
            PropertiesPatch { owner_id: Some(uuid(384)), creator_id: Some(next_creator) },
        )
        .unwrap()
        .outcome,
        Updated(entity)
    );
    let view = world.object(entity).unwrap();
    assert_eq!(view.shape(), Some(saved_shape));
    assert_eq!(view.properties().owner_id, Some(owner));
    assert_eq!(view.properties().creator_id, Some(next_creator));
    assert_eq!(view.properties_provenance().owner.unwrap().order, 200);
    assert_eq!(view.properties_provenance().creator.unwrap().region, first);
    assert_eq!(view.properties_provenance().creator.unwrap().order, 3);
}
