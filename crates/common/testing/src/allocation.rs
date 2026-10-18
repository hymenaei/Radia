//! Allocation measurements for synchronous operations confined to the calling thread.
//!
//! Install [`CountingAllocator`] as the test binary's global allocator before using these
//! functions. Counts include allocations and frees on this thread, regardless of where the
//! storage was created. They do not measure process memory or work on other threads.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

/// A system allocator with allocation accounting for each thread.
pub struct CountingAllocator;

/// Changes in heap usage during one measurement.
#[derive(Clone, Copy, Debug, Default)]
pub struct Allocations {
    /// Net change in live bytes, which can be negative when existing storage is freed.
    pub live_bytes: i128,
    /// Highest live-byte increase relative to the start, including the returned value.
    /// Reallocation conservatively counts old and replacement storage together, even if
    /// the system allocator can resize in place. This is not resident process memory.
    pub peak_bytes: i128,
    pub allocation_calls: usize,
    pub reallocation_calls: usize,
}

#[derive(Clone, Copy, Default)]
struct Counter {
    live: i128,
    measurement: Option<Allocations>,
}

thread_local! {
    static COUNTER: Cell<Counter> = const { Cell::new(Counter { live: 0, measurement: None }) };
}

// SAFETY: layouts and pointers are forwarded unchanged to System. Accounting uses const
// thread-local storage, does not allocate, and never changes the returned memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the allocator's layout contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128, false);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the allocator's layout contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128, false);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the original pointer and layout are passed to System.
        unsafe { System.dealloc(ptr, layout) };
        account(-(layout.size() as i128), false);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the live allocation and requested resize are forwarded unchanged.
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            account(new_size as i128, true);
            account(-(layout.size() as i128), false);
        }
        resized
    }
}

fn account(bytes: i128, reallocation: bool) {
    let _ = COUNTER.try_with(|counter| {
        let mut state = counter.get();
        state.live += bytes;
        if let Some(measurement) = &mut state.measurement {
            measurement.live_bytes += bytes;
            measurement.peak_bytes = measurement.peak_bytes.max(measurement.live_bytes);
            if reallocation {
                measurement.reallocation_calls += 1;
            } else if bytes > 0 {
                measurement.allocation_calls += 1;
            }
        }
        counter.set(state);
    });
}

/// Returns the calling thread's current net allocation count in bytes.
pub fn live_bytes() -> i128 {
    COUNTER.with(|counter| counter.get().live)
}

/// Measures one synchronous operation, excluding the measurement machinery itself.
///
/// The returned value remains live in the reported counts. Panics reset measurement state.
///
/// # Panics
/// Panics if a measurement is already active on this thread.
pub fn measure<T>(operation: impl FnOnce() -> T) -> (T, Allocations) {
    COUNTER.with(|counter| {
        let mut state = counter.get();
        assert!(state.measurement.is_none(), "allocation measurement is already active");
        state.measurement = Some(Allocations::default());
        counter.set(state);
    });
    let guard = Measurement;
    let value = operation();
    let counts = COUNTER.with(|counter| counter.get().measurement.expect("measurement remains active"));
    drop(guard);
    (value, counts)
}

struct Measurement;

impl Drop for Measurement {
    fn drop(&mut self) {
        COUNTER.with(|counter| {
            let mut state = counter.get();
            state.measurement = None;
            counter.set(state);
        });
    }
}
