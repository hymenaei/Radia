use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    fmt::Write,
    hint::black_box,
    time::Instant,
};

use llsd::{Limits, Value, xml};

struct CountingAlloc;

#[derive(Clone, Copy, Default)]
struct Counter {
    live: i128,
    peak: i128,
    active: bool,
}

thread_local! {
    static COUNTER: Cell<Counter> = const { Cell::new(Counter { live: 0, peak: 0, active: false }) };
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

// SAFETY: layouts/pointers are forwarded unchanged to System. Const thread-local
// accounting does not allocate or alter memory returned to the caller.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            account(layout.size() as i128);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the original pointer and layout are passed to System.
        unsafe { System.dealloc(ptr, layout) };
        account(-(layout.size() as i128));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the live allocation and requested resize are passed unchanged.
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            account(new_size as i128);
            account(-(layout.size() as i128));
        }
        resized
    }
}

fn account(bytes: i128) {
    let _ = COUNTER.try_with(|counter| {
        let mut state = counter.get();
        state.live += bytes;
        if state.active {
            state.peak = state.peak.max(state.live);
        }
        counter.set(state);
    });
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, i128, i128, u128) {
    let baseline = COUNTER.with(|counter| {
        let mut state = counter.get();
        state.active = true;
        state.peak = state.live;
        counter.set(state);
        state.live
    });
    let started = Instant::now();
    let value = operation();
    let elapsed = started.elapsed().as_nanos();
    let (live, peak) = COUNTER.with(|counter| {
        let mut state = counter.get();
        state.active = false;
        counter.set(state);
        (state.live - baseline, state.peak - baseline)
    });
    (value, live, peak, elapsed)
}

#[test]
fn allocation_bounds() {
    let pairs = 131_068;
    let mut large_map = String::from("<llsd><map><key>id</key><integer>1</integer><key>events</key><array/><key>extra</key><map>");
    for index in 0..pairs {
        write!(large_map, "<key>k{index}</key><integer>0</integer>").expect("append XML pair");
    }
    large_map.push_str("</map></map></llsd>");
    let mut flat_array = String::from("<llsd><array>");
    for _ in 0..131_073 {
        flat_array.push_str("<undef/>");
    }
    flat_array.push_str("</array></llsd>");
    let binary = format!("<llsd><binary>{}</binary></llsd>", "AAAA".repeat(4 * 1024 * 1024));
    let split_text = format!("<llsd><string>{}<![CDATA[{}]]></string></llsd>", "x".repeat(100_001), "x".repeat(99_999));
    let mut node_boundary = String::from("<llsd><map>");
    for index in 0..144 {
        write!(node_boundary, "<key>k{index}</key><map><key>v</key><string>text</string></map>").expect("append nested map");
    }
    node_boundary.push_str("</map></llsd>");
    let limits = Limits { max_input_bytes: 32 * 1024 * 1024, max_values: 262_144, max_container_items: 262_144, ..Limits::default() };
    for (name, input) in [
        ("empty", "<llsd><undef/></llsd>".to_owned()),
        ("depth limit", format!("<llsd>{}<integer>1</integer>{}</llsd>", "<array>".repeat(64), "</array>".repeat(64))),
        ("entities", format!("<llsd><string>{}</string></llsd>", "&#x1f600;&amp;".repeat(4096))),
        ("binary attributes", format!("<llsd><binary {} encoding=\"base64\">AAAA</binary></llsd>", (0..1024).map(|i| format!("x{i}=\"ok\" ")).collect::<String>())),
        ("EventQueue shape", "<llsd><map><key>id</key><integer>1</integer><key>events</key><array><map><key>message</key><string>ParcelProperties</string><key>body</key><map><key>name</key><string>parcel</string></map></map></array></map></llsd>".to_owned()),
        ("large extra map", large_map),
        ("flat array growth", flat_array),
        ("large binary", binary),
        ("split scalar growth", split_text),
        ("nested map nodes", node_boundary),
    ] {
        let (bound, scan_live, scan_peak, scan_ns) = measure(|| xml::value_allocation_bound(black_box(input.as_bytes()), limits).expect("scan valid XML"));
        assert_eq!(scan_live, 0, "scanner allocations must return for {name}");
        let baseline = COUNTER.with(|counter| counter.get().live);
        let (value, retained, peak, decode_ns) = measure(|| xml::from_slice_with_limits::<Value>(black_box(input.as_bytes()), limits).expect("decode measured XML"));
        assert!(peak >= retained);
        assert!(peak <= bound as i128, "allocation underestimate for {name}: peak={peak}, bound={bound}");
        drop(value);
        assert_eq!(COUNTER.with(|counter| counter.get().live), baseline, "decoded storage must return for {name}");
        eprintln!("{name}: input={} B, retained={retained} B, decode_peak={peak} B, bound={bound} B, scan_peak={scan_peak} B, bound/peak={:.2}, scan/decode={:.2} ({scan_ns}/{decode_ns} ns)", input.len(), bound as f64 / peak.max(1) as f64, scan_ns as f64 / decode_ns.max(1) as f64);
    }
}

#[test]
fn malformed_names_use_bounded_memory() {
    for prefix in ["", "\u{feff}", "<llsd>", "<llsd> ", "<llsd><!--comment-->", "<llsd><string>x</string>"] {
        let small = format!("{prefix}<unknown>");
        let large = format!("{prefix}<{}>", "x".repeat(1024 * 1024));
        let (small_result, small_live, small_peak, _) = measure(|| xml::value_allocation_bound(small.as_bytes(), Limits::default()));
        let (large_result, large_live, large_peak, _) = measure(|| xml::value_allocation_bound(large.as_bytes(), Limits::default()));
        assert_eq!(small_result, Err(xml::BoundError::Malformed));
        assert_eq!(large_result, Err(xml::BoundError::Malformed));
        assert_eq!((small_live, large_live), (0, 0));
        assert_eq!(large_peak, small_peak, "malformed tag name must not be copied for {prefix:?}");
    }
}
