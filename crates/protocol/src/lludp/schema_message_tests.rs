use super::*;
use crate::schema::{BlockSchema, MessageEncoding, MessageTrust};
use proptest::prelude::*;

static PREFLIGHT_FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
static PREFLIGHT_FIXED_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: PREFLIGHT_FIXED_FIELDS }];
static PREFLIGHT_FIXED_SCHEMA: MessageSchema = MessageSchema {
    name: "RepeatedFixedPreflight",
    id: MessageId::high(0x76),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: PREFLIGHT_FIXED_BLOCKS,
};
static PREFLIGHT_VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Variable(2) }];
static PREFLIGHT_VARIABLE_BLOCKS: &[BlockSchema] =
    &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: PREFLIGHT_VARIABLE_FIELDS }];
static PREFLIGHT_VARIABLE_SCHEMA: MessageSchema = MessageSchema {
    name: "VariablePreflight",
    id: MessageId::high(0x77),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: PREFLIGHT_VARIABLE_BLOCKS,
};

static ALL_FIELD_SCHEMAS: &[FieldSchema] = &[
    FieldSchema { name: "Bool", kind: FieldKind::Bool },
    FieldSchema { name: "F32", kind: FieldKind::F32 },
    FieldSchema { name: "F64", kind: FieldKind::F64 },
    FieldSchema { name: "IpAddr", kind: FieldKind::IpAddr },
    FieldSchema { name: "IpPort", kind: FieldKind::IpPort },
    FieldSchema { name: "Null", kind: FieldKind::Null },
    FieldSchema { name: "Quaternion", kind: FieldKind::Quaternion },
    FieldSchema { name: "Uuid", kind: FieldKind::Uuid },
    FieldSchema { name: "Vector3", kind: FieldKind::Vector3 },
    FieldSchema { name: "Vector3d", kind: FieldKind::Vector3d },
    FieldSchema { name: "Vector4", kind: FieldKind::Vector4 },
    FieldSchema { name: "S8", kind: FieldKind::S8 },
    FieldSchema { name: "S16", kind: FieldKind::S16 },
    FieldSchema { name: "S32", kind: FieldKind::S32 },
    FieldSchema { name: "S64", kind: FieldKind::S64 },
    FieldSchema { name: "U8", kind: FieldKind::U8 },
    FieldSchema { name: "U16", kind: FieldKind::U16 },
    FieldSchema { name: "U32", kind: FieldKind::U32 },
    FieldSchema { name: "U64", kind: FieldKind::U64 },
    FieldSchema { name: "Fixed", kind: FieldKind::Fixed(4) },
    FieldSchema { name: "Variable1", kind: FieldKind::Variable(1) },
    FieldSchema { name: "Variable2", kind: FieldKind::Variable(2) },
];
static ALL_FIELD_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: ALL_FIELD_SCHEMAS }];
static ALL_FIELDS_SCHEMA: MessageSchema = MessageSchema {
    name: "AllFieldKinds",
    id: MessageId::high(0x70),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: ALL_FIELD_BLOCKS,
};

static BLOCK_MODE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U8 }];
static MULTIPLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U16 }];
static VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U32 }];
static BLOCK_MODE_SCHEMAS: &[BlockSchema] = &[
    BlockSchema { name: "Single", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
    BlockSchema { name: "Multiple", mode: BlockMode::Multiple(2), fields: MULTIPLE_FIELDS },
    BlockSchema { name: "Variable", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
];
static BLOCK_MODES_SCHEMA: MessageSchema = MessageSchema {
    name: "BlockModes",
    id: MessageId::high(0x71),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: BLOCK_MODE_SCHEMAS,
};

static COMPAT_BASE_FIELDS: &[FieldSchema] =
    &[FieldSchema { name: "Base", kind: FieldKind::U8 }, FieldSchema { name: "Added", kind: FieldKind::U16 }];
static COMPAT_LATER_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Later", kind: FieldKind::U32 }];
static COMPAT_BLOCKS: &[BlockSchema] = &[
    BlockSchema { name: "Base", mode: BlockMode::Single, fields: COMPAT_BASE_FIELDS },
    BlockSchema { name: "Later", mode: BlockMode::Multiple(2), fields: COMPAT_LATER_FIELDS },
];
static COMPAT_SCHEMA: MessageSchema = MessageSchema {
    name: "AppendedBlocks",
    id: MessageId::high(0x73),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: COMPAT_BLOCKS,
};
static VARIABLE_SUFFIX_BLOCKS: &[BlockSchema] = &[
    BlockSchema { name: "Base", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
    BlockSchema { name: "Optional", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
    BlockSchema { name: "Added", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
];
static VARIABLE_SUFFIX_SCHEMA: MessageSchema = MessageSchema {
    name: "VariableSuffix",
    id: MessageId::high(0x74),
    trust: MessageTrust::NotTrusted,
    encoding: MessageEncoding::Unencoded,
    flags: &[],
    blocks: VARIABLE_SUFFIX_BLOCKS,
};

fn all_field_values() -> Vec<FieldValue> {
    vec![
        FieldValue::Bool(true),
        FieldValue::F32(1.0),
        FieldValue::F64(-1.0),
        FieldValue::IpAddr([1, 2, 3, 4]),
        FieldValue::IpPort([0x1f, 0x90]),
        FieldValue::Null,
        FieldValue::Quaternion(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0)),
        FieldValue::Uuid(Uuid::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
        FieldValue::Vector3(Vec3::new(1.0, -2.0, 0.5)),
        FieldValue::Vector3d(DVec3::new(1.0, -2.0, 0.5)),
        FieldValue::Vector4(Vec4::new(1.0, -2.0, 0.5, 4.0)),
        FieldValue::S8(-2),
        FieldValue::S16(-3),
        FieldValue::S32(-4),
        FieldValue::S64(-5),
        FieldValue::U8(5),
        FieldValue::U16(0x0607),
        FieldValue::U32(0x0809_0a0b),
        FieldValue::U64(0x0809_0a0b_0c0d_0e0f),
        FieldValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
        FieldValue::Bytes(vec![0xaa, 0xbb]),
        FieldValue::Bytes(vec![0xcc, 0xdd, 0xee]),
    ]
}

#[test]
fn field_kinds_match_bytes() {
    let message = SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(all_field_values())]).expect("valid message");
    let mut actual = Vec::new();
    message.encode_payload(&mut Writer::new(&mut actual)).expect("encode all field kinds");

    let expected = vec![
        0x01, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0xbf, 0x01, 0x02, 0x03, 0x04, 0x1f, 0x90, 0x00, 0x00, 0x80,
        0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0,
        0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00,
        0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x40, 0xfe, 0xfd, 0xff, 0xfc, 0xff, 0xff, 0xff, 0xfb, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0x05, 0x07, 0x06, 0x0b, 0x0a, 0x09, 0x08, 0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0xde, 0xad, 0xbe,
        0xef, 0x02, 0xaa, 0xbb, 0x03, 0x00, 0xcc, 0xdd, 0xee,
    ];
    assert_eq!(actual, expected);
    assert_eq!(
        SchemaMessage::decode(&ALL_FIELDS_SCHEMA, &expected).expect("decode all fields").blocks[0].instances[0].fields,
        all_field_values()
    );
}

#[test]
fn variable_2_little_endian() {
    use std::iter::repeat_n;

    let payload = vec![0x02, 0x01].into_iter().chain(repeat_n(0xaa, 258)).collect::<Vec<_>>();
    let schema = FieldSchema { name: "Data", kind: FieldKind::Variable(2) };
    let (decoded, complete) = decode_field(&mut Reader::new(&payload), &schema).expect("decode 258-byte Variable(2)");
    assert!(complete);
    assert_eq!(decoded, FieldValue::Bytes(vec![0xaa; 258]));
}

#[test]
fn block_modes_match_bytes() {
    let message = SchemaMessage::new(
        &BLOCK_MODES_SCHEMA,
        vec![
            BlockValues::single(vec![FieldValue::U8(0x11)]),
            BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U16(0x2222)]), BlockInstance::new(vec![FieldValue::U16(0x3333)])]),
            BlockValues::new(vec![
                BlockInstance::new(vec![FieldValue::U32(0x4444_4444)]),
                BlockInstance::new(vec![FieldValue::U32(0x5555_5555)]),
            ]),
        ],
    )
    .expect("valid block counts");
    let mut actual = Vec::new();
    message.encode_payload(&mut Writer::new(&mut actual)).expect("encode block modes");
    assert_eq!(actual, [0x11, 0x22, 0x22, 0x33, 0x33, 0x02, 0x44, 0x44, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55]);
}

#[test]
fn empty_final_block_count() {
    let payload = [0x11, 0x22, 0x22, 0x33, 0x33];
    let decoded = SchemaMessage::decode(&BLOCK_MODES_SCHEMA, &payload).expect("legacy empty final block");
    assert!(decoded.blocks[2].instances.is_empty());
    let mut encoded = Vec::new();
    decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode legacy empty final block");
    assert_eq!(encoded, payload);

    static NONTERMINAL_BLOCKS: &[BlockSchema] = &[
        BlockSchema { name: "Variable", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
        BlockSchema { name: "Single", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
    ];
    static NONTERMINAL_SCHEMA: MessageSchema = MessageSchema {
        name: "NonterminalVariable",
        id: MessageId::high(0x72),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: NONTERMINAL_BLOCKS,
    };
    assert_eq!(SchemaMessage::decode(&NONTERMINAL_SCHEMA, &[]).err(), Some(LLUdpError::TruncatedPacket));
}

#[test]
fn missing_fields_zero_fill() {
    let payload = [0x11];
    let decoded = SchemaMessage::decode(&COMPAT_SCHEMA, &payload).expect("decode older message");

    assert_eq!(decoded.blocks[0].instances[0].fields, [FieldValue::U8(0x11), FieldValue::U16(0)]);
    assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0)]);
    assert_eq!(decoded.blocks[1].instances[1].fields, [FieldValue::U32(0)]);

    let mut encoded = Vec::new();
    decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode older message");
    assert_eq!(encoded, payload);
}

#[test]
fn explicit_zero_differs_from_compatibility_fill() {
    let supplied = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11, 0, 0]).expect("decode explicitly supplied zero");
    let omitted = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11]).expect("decode omitted compatibility fields");

    assert_eq!(supplied.blocks[0].instances[0].fields[1], FieldValue::U16(0));
    assert_eq!(omitted.blocks[0].instances[0].fields[1], FieldValue::U16(0));
    assert_eq!(supplied.field_was_supplied(0, 0, 1), Some(true));
    assert_eq!(omitted.field_was_supplied(0, 0, 1), Some(false));
    assert_eq!(omitted.field_was_supplied(1, 0, 0), Some(false));
    assert_eq!(omitted.field_was_supplied(4, 0, 0), None);
    assert_ne!(supplied, omitted, "wire presence is part of the schema message's observable value");
}

#[test]
fn counts_repeated_padding() {
    static FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
    static BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: FIXED_FIELDS }];
    static SCHEMA: MessageSchema = MessageSchema {
        name: "RepeatedFixedPadding",
        id: MessageId::high(0x73),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: BLOCKS,
    };

    let decoded = SchemaMessage::decode(&SCHEMA, &[u8::MAX]).expect("decode omitted repeated fields");
    let expected = size_of::<BlockValues>()
        + usize::from(u8::MAX) * size_of::<BlockInstance>()
        + usize::from(u8::MAX) * size_of::<FieldValue>()
        + usize::from(u8::MAX) * 32;

    assert_eq!(decoded.blocks[0].instances.len(), usize::from(u8::MAX));
    assert!(decoded.retained_capacity_bytes() >= expected);
}

#[test]
fn preflights_padding_and_truncated_variables() {
    static FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
    static FIXED_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: FIXED_FIELDS }];
    static FIXED_SCHEMA: MessageSchema = MessageSchema {
        name: "RepeatedFixedPaddingBound",
        id: MessageId::high(0x74),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: FIXED_BLOCKS,
    };
    static VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Variable(1) }];
    static VARIABLE_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: VARIABLE_FIELDS }];
    static VARIABLE_SCHEMA: MessageSchema = MessageSchema {
        name: "TruncatedVariableBound",
        id: MessageId::high(0x75),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: VARIABLE_BLOCKS,
    };

    let payload = [u8::MAX];
    let bound = SchemaMessage::allocation_bound(&FIXED_SCHEMA, &payload).expect("bound repeated compatibility padding");
    let decoded = SchemaMessage::decode(&FIXED_SCHEMA, &payload).expect("decode repeated compatibility padding");
    assert!(bound >= decoded.retained_capacity_bytes());

    assert_eq!(SchemaMessage::allocation_bound(&VARIABLE_SCHEMA, &[2, 1]), Err(LLUdpError::TruncatedPacket));
}

#[test]
fn rejects_allocation_bound_overflow() {
    assert_eq!(checked_add(usize::MAX, 1), Err(LLUdpError::AllocationBoundOverflow));
    assert_eq!(checked_product(usize::MAX, 2), Err(LLUdpError::AllocationBoundOverflow));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn repeated_fixed_field_preflight_covers_schema_expansion(count in any::<u8>(), available in 0usize..=8192) {
        let mut payload = Vec::with_capacity(1 + available);
        payload.push(count);
        payload.resize(1 + available, 0x5a);

        let bound = SchemaMessage::allocation_bound(&PREFLIGHT_FIXED_SCHEMA, &payload)
            .expect("checked repeated fixed-field bound");
        let decoded = SchemaMessage::decode(&PREFLIGHT_FIXED_SCHEMA, &payload)
            .expect("fixed compatibility fields zero-fill safely");

        prop_assert!(bound >= decoded.retained_capacity_bytes());
    }

    #[test]
    fn variable_field_preflight_checks_declared_lengths(declared in 0u16..=u16::MAX, available in 0usize..=1024) {
        let mut payload = declared.to_le_bytes().to_vec();
        payload.resize(2 + available, 0xa5);

        if usize::from(declared) > available {
            prop_assert_eq!(
                SchemaMessage::allocation_bound(&PREFLIGHT_VARIABLE_SCHEMA, &payload),
                Err(LLUdpError::TruncatedPacket)
            );
        } else {
            let bound = SchemaMessage::allocation_bound(&PREFLIGHT_VARIABLE_SCHEMA, &payload)
                .expect("complete variable field has a checked bound");
            let decoded = SchemaMessage::decode(&PREFLIGHT_VARIABLE_SCHEMA, &payload)
                .expect("complete variable field decodes");
            prop_assert!(bound >= decoded.retained_capacity_bytes());
        }
    }
}

#[test]
fn generated_schema_bounds_fit_packet_limit() {
    for schema in schema::MESSAGES {
        let mut bytes = schema.blocks.len() * size_of::<BlockValues>() + 64 * 1024;
        for block in schema.blocks {
            let count = match block.mode {
                BlockMode::Single => 1,
                BlockMode::Multiple(count) => usize::try_from(count).expect("template count fits usize"),
                BlockMode::Variable => usize::from(u8::MAX),
            };
            let fixed_bytes = block.fields.iter().fold(0usize, |total, field| match field.kind {
                FieldKind::Fixed(size) => total.saturating_add(usize::try_from(size).expect("fixed field fits usize")),
                _ => total,
            });
            let instance_capacity = size_of::<BlockInstance>()
                .saturating_add(block.fields.len().saturating_mul(size_of::<FieldValue>()))
                .saturating_add(fixed_bytes);
            bytes = bytes.saturating_add(count.saturating_mul(instance_capacity));
        }
        assert!(bytes <= crate::lludp::MAX_PACKET_RETAINED_CAPACITY_BYTES, "{} requires {bytes} bytes", schema.name);
    }
}

#[test]
fn missing_middle_count_fails() {
    assert_eq!(SchemaMessage::decode(&VARIABLE_SUFFIX_SCHEMA, &[0x11]).err(), Some(LLUdpError::TruncatedPacket));
}

#[test]
fn middle_variable_block() {
    let payload = [0x11, 0x01, 0x22, 0x33, 0x44, 0x55, 0x66];
    let decoded = SchemaMessage::decode(&VARIABLE_SUFFIX_SCHEMA, &payload).expect("decode middle variable block");

    assert_eq!(decoded.blocks[1].instances.len(), 1);
    assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0x5544_3322)]);
    assert_eq!(decoded.blocks[2].instances[0].fields, [FieldValue::U8(0x66)]);

    let mut encoded = Vec::new();
    decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode middle variable block");
    assert_eq!(encoded, payload);
}

#[test]
fn partial_field_zero_fills() {
    let decoded = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11, 0x22]).expect("decode partial fixed field");

    assert_eq!(decoded.blocks[0].instances[0].fields, [FieldValue::U8(0x11), FieldValue::U16(0)]);
    assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0)]);
    assert_eq!(decoded.blocks[1].instances[1].fields, [FieldValue::U32(0)]);
    assert!(decoded.trailing_bytes().is_empty());

    let mut encoded = Vec::new();
    decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode partial field");
    assert_eq!(encoded, [0x11]);
}

#[test]
fn variable_truncation_fails() {
    let one_byte_prefix = FieldSchema { name: "Data", kind: FieldKind::Variable(1) };
    assert_eq!(decode_field(&mut Reader::new(&[0x02, 0xaa]), &one_byte_prefix).err(), Some(LLUdpError::TruncatedPacket));

    let two_byte_prefix = FieldSchema { name: "Data", kind: FieldKind::Variable(2) };
    assert_eq!(decode_field(&mut Reader::new(&[0x01]), &two_byte_prefix).err(), Some(LLUdpError::TruncatedPacket));
}

#[test]
fn bad_shapes_rejected() {
    assert_eq!(SchemaMessage::new(&BLOCK_MODES_SCHEMA, Vec::new()).err(), Some(LLUdpError::InvalidSchemaMessage));
    assert_eq!(
        SchemaMessage::new(
            &BLOCK_MODES_SCHEMA,
            vec![
                BlockValues::single(vec![FieldValue::U16(1)]),
                BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U16(1)]); 2]),
                BlockValues::new(Vec::new()),
            ],
        )
        .err(),
        Some(LLUdpError::InvalidSchemaMessage)
    );
}

#[test]
fn length_overflow_rejected() {
    let mut fields = all_field_values();
    fields[17] = FieldValue::Bytes(vec![0; 3]);
    assert_eq!(SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(fields)]).err(), Some(LLUdpError::InvalidSchemaMessage));

    let mut fields = all_field_values();
    fields[18] = FieldValue::Bytes(vec![0; 256]);
    assert_eq!(SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(fields)]).err(), Some(LLUdpError::InvalidSchemaMessage));

    let variable_instances = (0..=u8::MAX).map(|_| BlockInstance::new(vec![FieldValue::U32(1)])).collect();
    let blocks = vec![
        BlockValues::single(vec![FieldValue::U8(1)]),
        BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U16(1)]), BlockInstance::new(vec![FieldValue::U16(2)])]),
        BlockValues::new(variable_instances),
    ];
    assert_eq!(SchemaMessage::new(&BLOCK_MODES_SCHEMA, blocks).err(), Some(LLUdpError::InvalidSchemaMessage));
}

fn sample_value(kind: FieldKind) -> FieldValue {
    match kind {
        FieldKind::Bool => FieldValue::Bool(true),
        FieldKind::F32 => FieldValue::F32(1.5),
        FieldKind::F64 => FieldValue::F64(-2.25),
        FieldKind::IpAddr => FieldValue::IpAddr([192, 0, 2, 1]),
        FieldKind::IpPort => FieldValue::IpPort([0x12, 0x34]),
        FieldKind::Null => FieldValue::Null,
        FieldKind::Quaternion => FieldValue::Quaternion(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0)),
        FieldKind::Uuid => FieldValue::Uuid(Uuid::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
        FieldKind::Vector3 => FieldValue::Vector3(Vec3::new(1.0, -2.0, 0.5)),
        FieldKind::Vector3d => FieldValue::Vector3d(DVec3::new(1.0, -2.0, 0.5)),
        FieldKind::Vector4 => FieldValue::Vector4(Vec4::new(1.0, -2.0, 0.5, 4.0)),
        FieldKind::S8 => FieldValue::S8(-2),
        FieldKind::S16 => FieldValue::S16(-3),
        FieldKind::S32 => FieldValue::S32(-4),
        FieldKind::S64 => FieldValue::S64(-5),
        FieldKind::U8 => FieldValue::U8(5),
        FieldKind::U16 => FieldValue::U16(0x0607),
        FieldKind::U32 => FieldValue::U32(0x0809_0a0b),
        FieldKind::U64 => FieldValue::U64(0x0809_0a0b_0c0d_0e0f),
        FieldKind::Fixed(size) => FieldValue::Bytes(vec![0x5a; size as usize]),
        FieldKind::Variable(_) => FieldValue::Bytes(vec![0xaa, 0xbb]),
    }
}

#[test]
fn schemas_round_trip() {
    for schema in schema::MESSAGES {
        let blocks = schema
            .blocks
            .iter()
            .map(|block| {
                let count = match block.mode {
                    BlockMode::Single => 1,
                    BlockMode::Multiple(count) => count as usize,
                    BlockMode::Variable => 2,
                };
                let instances =
                    (0..count).map(|_| BlockInstance::new(block.fields.iter().map(|field| sample_value(field.kind)).collect())).collect();
                BlockValues::new(instances)
            })
            .collect();
        let message = SchemaMessage::new(schema, blocks).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
        let mut expected = Vec::new();
        message.encode_payload(&mut Writer::new(&mut expected)).unwrap_or_else(|error| panic!("{}: {error}", schema.name));

        let decoded = SchemaMessage::decode(schema, &expected).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
        let mut actual = Vec::new();
        decoded.encode_payload(&mut Writer::new(&mut actual)).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
        assert_eq!(actual, expected, "schema {}", schema.name);
    }
}
