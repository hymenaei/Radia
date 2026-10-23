use std::{hint::black_box, mem::size_of};

use protocol::lludp::{FieldValue, Message, Packet};
use test_support::{
    allocation::{self, CountingAllocator, measure},
    lludp::vectors,
    parse_hex_fixture,
};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

#[test]
fn decode_allocations() {
    let mut wire = [0; 43];
    wire[4] = 1;
    wire[6..10].copy_from_slice(&[0xff, 0xff, 0, 98]);
    wire[42] = u8::MAX;

    let preflight = Packet::preflight(&wire).expect("preflight omitted ObjectData records");
    drop(Packet::decode(&wire).expect("warm decoder before measuring"));
    let baseline_bytes = allocation::live_bytes();
    let (packet, allocations) = measure(|| black_box(Packet::decode(black_box(&wire)).expect("decode omitted ObjectData records")));
    let live_bytes = allocations.live_bytes;
    let peak_bytes = allocations.peak_bytes;
    let allocation_calls = allocations.allocation_calls;
    let retained_bytes = packet.retained_capacity_bytes();
    let Message::Schema(message) = packet.message() else { panic!("ObjectShape must remain schema-backed") };
    let blocks = message.blocks();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[1].instances.len(), usize::from(u8::MAX));

    let mut missing_fields = 0;
    for (instance_index, instance) in blocks[1].instances.iter().enumerate() {
        for field_index in 0..instance.fields.len() {
            if message.field_was_supplied(1, instance_index, field_index) == Some(false) {
                missing_fields += 1;
            }
        }
    }

    assert_eq!(missing_fields, 4_845);
    assert_eq!(live_bytes, i128::try_from(retained_bytes).expect("retained capacity fits i128"));
    assert!(retained_bytes <= preflight.retained_capacity_upper_bound(), "retained storage exceeds preflight admission");
    assert!(peak_bytes <= preflight.peak_decode_allocation_bytes() as i128, "decode peak exceeds preflight admission");
    assert!(preflight.peak_decode_allocation_bytes() >= preflight.retained_capacity_upper_bound());

    drop(packet);
    assert_eq!(allocation::live_bytes(), baseline_bytes);

    eprintln!(
        "43-byte ObjectShape: {missing_fields} padded fields, {} B of padded FieldValue storage, {} B retained, {} B peak ({} allocations, {} reallocations)",
        missing_fields * size_of::<FieldValue>(),
        retained_bytes,
        peak_bytes,
        allocation_calls,
        allocations.reallocation_calls,
    );

    for (name, fixture) in [
        ("zero-coded AgentUpdate", vectors::AGENT_UPDATE_ZEROCODED),
        ("zero-coded ObjectUpdate", vectors::OBJECT_UPDATE),
        ("RegionHandshake strings", vectors::REGION_HANDSHAKE_STRINGS),
        ("packed ObjectUpdateCompressed", vectors::OBJECT_UPDATE_COMPRESSED_TYPED),
    ] {
        let wire = parse_hex_fixture(fixture).expect("parse allocation fixture");
        let preflight = Packet::preflight(&wire).expect("preflight allocation fixture");
        drop(Packet::decode(&wire).expect("warm typed decoder"));
        let baseline = allocation::live_bytes();
        let (packet, allocations) = measure(|| black_box(Packet::decode(black_box(&wire)).expect("decode allocation fixture")));
        let live = allocations.live_bytes;
        let peak = allocations.peak_bytes;

        assert_eq!(
            live,
            i128::try_from(packet.retained_capacity_bytes()).expect("retained capacity fits i128"),
            "retained capacity for {name}"
        );
        assert!(
            live <= i128::try_from(preflight.retained_capacity_upper_bound()).expect("retained bound fits i128"),
            "retained bound for {name}"
        );
        assert!(
            peak <= i128::try_from(preflight.peak_decode_allocation_bytes()).expect("peak bound fits i128"),
            "decode peak bound for {name}: measured={peak}, bound={}",
            preflight.peak_decode_allocation_bytes()
        );
        drop(packet);
        assert_eq!(allocation::live_bytes(), baseline, "allocation return for {name}");
        eprintln!("{name}: {} B wire, {live} B retained, {peak} B decode peak", wire.len());
    }
}
