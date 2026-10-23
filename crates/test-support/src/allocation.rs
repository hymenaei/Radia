use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

pub struct CountingAllocator;

#[derive(Clone, Copy, Debug, Default)]
pub struct Allocations {
    pub live_bytes: i128,
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

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128, false);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128, false);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        account(-(layout.size() as i128), false);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            account(new_size as i128, true);
            account(-(layout.size() as i128), false);
        }
        resized
    }
}

fn account(bytes: i128, reallocation: bool) {
    COUNTER
        .try_with(|counter| {
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
        })
        .unwrap_or(());
}

pub fn live_bytes() -> i128 {
    COUNTER.with(|counter| counter.get().live)
}

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
