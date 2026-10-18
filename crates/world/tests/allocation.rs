use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    num::NonZeroU32,
};

use base::UUID;
use math::{Quat, Vec3};
use world::{
    ApplyOutcome::Introduced,
    Kind::Object,
    MAX_QUERY_STATE_MAINTENANCE_WORK, ObjectObservation,
    OrderEvidence::Ordered,
    ParentUpdate::{Local, Root},
    Pose, Region, UpdateSource, World, WorldLimits,
};

struct CountingAllocator;

#[derive(Clone, Copy, Default)]
struct Allocations {
    live_bytes: i128,
    peak_bytes: i128,
    allocation_calls: usize,
    reallocation_calls: usize,
    active: bool,
}

thread_local! {
    static ALLOCATIONS: Cell<Allocations> = const { Cell::new(Allocations {
        live_bytes: 0,
        peak_bytes: 0,
        allocation_calls: 0,
        reallocation_calls: 0,
        active: false,
    }) };
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// SAFETY: pointers, sizes, and layouts are forwarded unchanged to System. Thread-local accounting
// does not allocate or change the memory returned to the caller.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the allocator contract supplies a valid layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as i128, false);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the allocator contract supplies a valid layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            account(layout.size() as i128, false);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout are forwarded from the original allocation.
        unsafe { System.dealloc(pointer, layout) };
        account(-(layout.size() as i128), false);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the live allocation and requested new size are forwarded unchanged.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            account(new_size as i128 - layout.size() as i128, true);
        }
        resized
    }
}

fn account(bytes: i128, reallocation: bool) {
    let _ = ALLOCATIONS.try_with(|allocations| {
        let mut state = allocations.get();
        state.live_bytes += bytes;
        if state.active {
            if reallocation {
                state.reallocation_calls += 1;
            } else if bytes > 0 {
                state.allocation_calls += 1;
            }
            state.peak_bytes = state.peak_bytes.max(state.live_bytes);
        }
        allocations.set(state);
    });
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, Allocations) {
    let baseline = ALLOCATIONS.with(|allocations| {
        let mut state = allocations.get();
        state.active = true;
        state.peak_bytes = state.live_bytes;
        state.allocation_calls = 0;
        state.reallocation_calls = 0;
        allocations.set(state);
        state.live_bytes
    });

    let value = operation();
    let result = ALLOCATIONS.with(|allocations| {
        let mut state = allocations.get();
        state.active = false;
        allocations.set(state);
        Allocations {
            live_bytes: state.live_bytes - baseline,
            peak_bytes: state.peak_bytes - baseline,
            allocation_calls: state.allocation_calls,
            reallocation_calls: state.reallocation_calls,
            active: false,
        }
    });
    (value, result)
}

#[test]
fn world_memory_baseline() {
    for count in [40_000_usize, 65_536] {
        let regions = (0..count.div_ceil(10_000)).map(|index| Region::new(index as u64 + 1)).collect::<Vec<_>>();
        let baseline = ALLOCATIONS.with(|allocations| allocations.get().live_bytes);
        let (world, allocations) = measure(|| {
            let limits = WorldLimits {
                max_region_registrations: 8,
                max_entities: 65_536,
                max_entities_per_region: 10_000,
                max_tombstones_per_region: 4_096,
                max_tombstones: 16_384,
                max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4_096),
                max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16_384),
                max_journal_entries: 4_096,
                max_journal_changes: 65_536,
                max_region_name_bytes: 256,
                max_recovery_targets: 256,
            };
            let mut world = World::new(limits);
            for region in &regions {
                let _ = world.adopt_region(*region).expect("adopt benchmark region");
            }
            for index in 0..count {
                let region_index = index / 10_000;
                let local_id = NonZeroU32::new((index % 10_000 + 1) as u32).expect("nonzero local ID");
                let parent = if local_id.get() == 1 { Root } else { Local(NonZeroU32::new(1).expect("root local ID")) };
                let mut observation = ObjectObservation::new(
                    UpdateSource::new(regions[region_index], 100_001 + index as u32, Ordered(100_001 + index as u64), 0),
                    UUID::from_u128(index as u128 + 1),
                    local_id,
                    Object,
                    parent,
                );
                observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
                observation.scale = Some(Vec3::ONE);
                assert!(matches!(world.observe_object(observation).expect("insert benchmark object").outcome, Introduced(_)));
            }
            loop {
                let maintenance = world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("maintain query state");
                if maintenance.dirty_scopes == 0 {
                    break;
                }
            }
            world
        });

        assert_eq!(world.object_count(), count);
        assert!(allocations.live_bytes > 0);
        assert!(allocations.peak_bytes >= allocations.live_bytes);
        assert!(allocations.allocation_calls > 0);
        let mut world = world;
        let (removed, retirement) = measure(|| {
            let mut removed = 0;
            for region in &regions {
                let applied = world.retire_region(*region).expect("retire benchmark region");
                let world::RegionOutcome::Retired { entities_removed, .. } = applied.outcome else { panic!("expected region retirement") };
                removed += entities_removed;
            }
            removed
        });
        assert_eq!(removed, count);
        assert_eq!(world.object_count(), 0);
        drop(world);
        let remaining_bytes = ALLOCATIONS.with(|allocations| allocations.get().live_bytes);
        assert_eq!(
            remaining_bytes,
            baseline,
            "{count} objects left {} B allocated after World drop; retained={} B, peak={} B, allocations={}, reallocations={}, retirement allocations={}, retirement reallocations={}, retirement peak additional={} B",
            remaining_bytes - baseline,
            allocations.live_bytes,
            allocations.peak_bytes,
            allocations.allocation_calls,
            allocations.reallocation_calls,
            retirement.allocation_calls,
            retirement.reallocation_calls,
            retirement.peak_bytes,
        );
        eprintln!(
            "{count} linked objects: retained={} B, peak={} B, allocations={}, reallocations={}, retained/object={:.1} B",
            allocations.live_bytes,
            allocations.peak_bytes,
            allocations.allocation_calls,
            allocations.reallocation_calls,
            allocations.live_bytes as f64 / count as f64,
        );
        eprintln!(
            "{count} linked object retirements: allocations={}, reallocations={}, peak additional={} B",
            retirement.allocation_calls, retirement.reallocation_calls, retirement.peak_bytes,
        );
    }
}
