use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};

thread_local! {
    static ALLOCATION_COUNT: Cell<AllocationCount> = const { Cell::new(AllocationCount { live: 0, peak: 0, active: false }) };
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct CountingAllocator;

#[derive(Clone, Copy, Default)]
struct AllocationCount {
    live: i128,
    peak: i128,
    active: bool,
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        account(-(layout.size() as i128));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            account(new_size as i128);
            account(-(layout.size() as i128));
        }
        resized
    }
}

fn account(bytes: i128) {
    ALLOCATION_COUNT
        .try_with(|count| {
            let mut state = count.get();
            state.live += bytes;
            if state.active {
                state.peak = state.peak.max(state.live);
            }
            count.set(state);
        })
        .unwrap_or_default();
}

pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, i128, i128, u128) {
    let baseline = ALLOCATION_COUNT.with(|count| {
        let mut state = count.get();
        state.active = true;
        state.peak = state.live;
        count.set(state);
        state.live
    });
    let started = Instant::now();
    let value = operation();
    let elapsed = started.elapsed().as_nanos();
    let (live, peak) = ALLOCATION_COUNT.with(|count| {
        let mut state = count.get();
        state.active = false;
        count.set(state);
        (state.live - baseline, state.peak - baseline)
    });
    (value, live, peak, elapsed)
}
