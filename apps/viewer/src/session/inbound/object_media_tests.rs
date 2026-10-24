use super::*;

#[test]
fn media_requests_cover_full_and_compressed() {
    let marker = "x-mv:12/00000000-0000-0000-0000-000000000001";
    let id = Uuid::from_u128(0x1234);
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let Message::ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    update.objects[0].id = id;
    update.objects[0].local_id = 7;
    update.objects[0].media_url = marker.into();

    let requests = object_media_requests(REGION, 8, OrderEvidence::Ordered(8), &full);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].object_id, id);
    assert_eq!(requests[0].local_id.get(), 7);
    assert_eq!(requests[0].version, 12);
    assert_eq!(requests[0].source.region(), REGION);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let Message::ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    update.objects[0].id = id;
    update.objects[0].local_id = 7;
    update.objects[0].media_url = Some(marker.into());
    let requests = object_media_requests(REGION, 9, OrderEvidence::Ordered(9), &compressed);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].object_id, id);
    assert_eq!(requests[0].version, 12);
}

#[test]
fn media_requests_require_volume_marker() {
    let mut message = packet(vectors::OBJECT_UPDATE).message().clone();
    {
        let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
        update.objects.truncate(1);
        update.objects[0].pcode = 47;
        update.objects[0].media_url = "x-mv:12/00000000-0000-0000-0000-000000000001".into();
    }
    assert!(object_media_requests(REGION, 1, OrderEvidence::Ordered(1), &message).is_empty());

    {
        let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
        update.objects[0].pcode = 9;
        update.objects[0].media_url = "https://media.test/object".into();
    }
    assert!(object_media_requests(REGION, 2, OrderEvidence::Ordered(2), &message).is_empty());
}

#[test]
fn attachment_state_decodes_wire_nibbles() {
    assert_eq!(attachment_point(0), FieldUpdate::Clear);
    assert_eq!(attachment_point(0x10), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(1).unwrap())));
    assert_eq!(attachment_point(0x18), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(1).unwrap())));
    assert_eq!(attachment_point(0x80), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(8).unwrap())));
    assert_eq!(attachment_point(0x88), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(8).unwrap())));
}

#[test]
fn override_message_targets_local_id() {
    let message = message(0x4175, br"{'id':i7,'te':[i4],'od':[{'mf':r0.5}]}");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    let Some(ObjectOperation::Patch(patch)) = &batch.blocks[0] else { panic!("expected object patch") };
    assert_eq!(patch.local_id.get(), 7);
    assert_eq!(patch.source.region(), Region::new(11));
    assert_eq!(patch.pbr_material_overrides.as_ref().unwrap().face(4).unwrap().metallic, Some(0.5));
}

#[test]
fn malformed_override_is_ignored() {
    let message = message(0x4175, b"not LLSD");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    assert_eq!(batch.invalid_blocks, 1);
    assert!(batch.blocks.iter().all(Option::is_none));
}

#[test]
fn unrelated_streaming_method_is_ignored() {
    let message = message(0x1234, br"{'id':i7,'te':[],'od':[]}");

    assert!(decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).is_none());
}
