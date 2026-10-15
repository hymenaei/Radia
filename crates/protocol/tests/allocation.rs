use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    mem::size_of,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use protocol::lludp::{FieldValue, Message, Packet};
use testing::{lludp::vectors, parse_hex_fixture};

struct CountingAlloc;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

// SAFETY: the allocator forwards every allocation, deallocation, and resize to `System` with its original layout and pointer. Its accounting
// state is atomic and does not affect the returned memory.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc::alloc` supplies a valid layout; `System` handles the allocation contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc::alloc_zeroed` supplies a valid layout; `System` handles the allocation contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller must provide a pointer and layout from a successful allocation; both are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller must provide a live allocation and matching layout; these and the requested size are forwarded unchanged.
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
            record_allocation(new_size);
            if ACTIVE.load(Ordering::Relaxed) {
                REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        }
        resized
    }
}

fn record_allocation(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if ACTIVE.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }
}

#[test]
fn decode_allocations() {
    let mut wire = [0; 43];
    wire[4] = 1;
    wire[6..10].copy_from_slice(&[0xff, 0xff, 0, 98]);
    wire[42] = u8::MAX;

    let preflight = Packet::preflight(&wire).expect("preflight omitted ObjectData records");
    drop(Packet::decode(&wire).expect("warm decoder before measuring"));
    let baseline_bytes = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_bytes, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    REALLOCATIONS.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    let packet = black_box(Packet::decode(black_box(&wire)).expect("decode omitted ObjectData records"));
    let retained_bytes = packet.retained_capacity_bytes();
    let Message::Schema(message) = packet.message() else { panic!("ObjectShape must remain schema-backed") };
    let blocks = message.blocks();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[1].instances.len(), usize::from(u8::MAX));

    let mut missing_fields = 0;
    let mut field_capacity = 0;
    for (instance_index, instance) in blocks[1].instances.iter().enumerate() {
        field_capacity += instance.fields.capacity();
        for field_index in 0..instance.fields.len() {
            if message.field_was_supplied(1, instance_index, field_index) == Some(false) {
                missing_fields += 1;
            }
        }
    }

    let live_bytes = LIVE_BYTES.load(Ordering::Relaxed) - baseline_bytes;
    let peak_bytes = PEAK_BYTES.load(Ordering::Relaxed) - baseline_bytes;
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let reallocations = REALLOCATIONS.load(Ordering::Relaxed);
    ACTIVE.store(false, Ordering::Relaxed);

    assert_eq!(missing_fields, 4_845);
    assert_eq!(field_capacity, 4_845);
    assert_eq!(live_bytes, retained_bytes);
    assert_eq!(preflight.retained_capacity_upper_bound(), retained_bytes);
    assert_eq!(preflight.peak_decode_allocation_bytes(), peak_bytes);
    assert!(preflight.peak_decode_allocation_bytes() >= preflight.retained_capacity_upper_bound());
    assert_eq!(reallocations, 0, "the measured decode should not grow a buffer after allocation");

    drop(packet);
    assert_eq!(LIVE_BYTES.load(Ordering::Relaxed), baseline_bytes);

    eprintln!(
        "43-byte ObjectShape: {missing_fields} padded fields, {} B of padded FieldValue storage, {} B retained, {} B peak ({} allocations total)",
        missing_fields * size_of::<FieldValue>(),
        retained_bytes,
        peak_bytes,
        allocations,
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
        let baseline = LIVE_BYTES.load(Ordering::Relaxed);
        PEAK_BYTES.store(baseline, Ordering::Relaxed);
        ACTIVE.store(true, Ordering::Relaxed);
        let packet = black_box(Packet::decode(black_box(&wire)).expect("decode allocation fixture"));
        let live = LIVE_BYTES.load(Ordering::Relaxed) - baseline;
        let peak = PEAK_BYTES.load(Ordering::Relaxed) - baseline;
        ACTIVE.store(false, Ordering::Relaxed);

        assert_eq!(live, packet.retained_capacity_bytes(), "retained capacity for {name}");
        assert!(live <= preflight.retained_capacity_upper_bound(), "retained bound for {name}");
        assert!(
            peak <= preflight.peak_decode_allocation_bytes(),
            "decode peak bound for {name}: measured={peak}, bound={}",
            preflight.peak_decode_allocation_bytes()
        );
        drop(packet);
        assert_eq!(LIVE_BYTES.load(Ordering::Relaxed), baseline, "allocation return for {name}");
        eprintln!("{name}: {} B wire, {live} B retained, {peak} B decode peak", wire.len());
    }
}
