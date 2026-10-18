use std::{hint::black_box, panic::catch_unwind};

use testing::allocation::{CountingAllocator, live_bytes, measure};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn measures_growth_and_release() {
    let baseline = live_bytes();
    let ((bytes, original_capacity), counts) = measure(|| {
        let mut bytes = black_box(Vec::<u8>::with_capacity(32));
        let original_capacity = bytes.capacity();
        bytes.reserve_exact(64);
        (bytes, original_capacity)
    });
    assert_eq!(counts.live_bytes, bytes.capacity() as i128);
    assert_eq!(counts.peak_bytes, counts.live_bytes + original_capacity as i128);
    assert_eq!(counts.allocation_calls, 1);
    assert_eq!(counts.reallocation_calls, 1);

    let ((), freed) = measure(|| drop(bytes));
    assert_eq!(freed.live_bytes, -counts.live_bytes);
    assert_eq!(freed.peak_bytes, 0);
    assert_eq!(live_bytes(), baseline);
}

#[test]
fn unwinding_resets_measurement() {
    assert!(catch_unwind(|| measure(|| panic!("measured operation failed"))).is_err());
    let (bytes, counts) = measure(|| black_box(vec![0_u8; 32]));
    assert_eq!(counts.live_bytes, bytes.capacity() as i128);
    assert_eq!(counts.allocation_calls, 1);
}

#[test]
fn rejects_nested_measurements_without_poisoning_the_outer_one() {
    let ((bytes, panic_bytes), counts) = measure(|| {
        let before = live_bytes();
        assert!(catch_unwind(|| measure(|| ())).is_err());
        let panic_bytes = live_bytes() - before;
        (black_box(vec![0_u8; 32]), panic_bytes)
    });
    assert_eq!(counts.live_bytes - panic_bytes, bytes.capacity() as i128);
    let ((), next) = measure(|| ());
    assert_eq!(next.live_bytes, 0);
    assert_eq!(next.allocation_calls, 0);
}
