use std::{
    sync::{Arc, Barrier, mpsc},
    thread,
    time::Duration,
};

use proptest::prelude::*;
use tempfile::tempdir;
use utils::AssetId;
use uuid::Uuid;

use super::*;

fn resource(id: u128, kind: Kind) -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(id)), kind }
}

fn validated_extent(total_size: u64, offset: u64, bytes: Vec<u8>) -> ValidatedExtent {
    ValidatedExtent::from_validated(total_size, offset, bytes)
}

#[test]
fn adjacent_and_matching_extents_merge() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(1, Kind::Texture);
    cache.put_extent(resource, validated_extent(9, 3, b"def".to_vec())).expect("store second range");
    cache.put_extent(resource, validated_extent(9, 0, b"abc".to_vec())).expect("store first range");
    cache.put_extent(resource, validated_extent(9, 2, b"cdefg".to_vec())).expect("store agreeing overlap");

    let AssetLookup::Partial(asset) = cache.lookup(resource) else {
        panic!("merged asset is available");
    };
    assert_eq!(asset.total_size, 9);
    assert_eq!(asset.extents, vec![Extent { offset: 0, bytes: b"abcdefg".to_vec() }]);
    assert!(!asset.is_complete());
    cache.put_extent(resource, validated_extent(9, 7, b"hi".to_vec())).expect("complete asset");
    let AssetLookup::Hit(asset) = cache.lookup(resource) else {
        panic!("complete asset is available");
    };
    assert!(asset.is_complete());
    cache.shutdown().expect("close cache");
}

#[test]
fn conflicting_overlap_preserves_existing_extent() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(2, Kind::Texture);
    cache.put_extent(resource, validated_extent(8, 0, b"abcd".to_vec())).expect("store initial range");

    std::assert_matches!(cache.put_extent(resource, validated_extent(8, 2, b"ZZ".to_vec())), Err(AssetCacheError::ConflictingOverlap));
    let AssetLookup::Partial(asset) = cache.lookup(resource) else {
        panic!("initial extent remains available");
    };
    assert_eq!(asset.extents, vec![Extent { offset: 0, bytes: b"abcd".to_vec() }]);
    cache.shutdown().expect("close cache");
}

#[test]
fn unrelated_assets_use_independent_merge_locks() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let first = resource(1, Kind::Texture);
    let first_stripe = stripe_index(first).expect("first stripe");
    let second = (2..10_000u128)
        .map(|id| resource(id, Kind::Texture))
        .find(|resource| stripe_index(*resource).is_ok_and(|stripe| stripe != first_stripe))
        .expect("resource on another stripe");
    let held = cache.write_guard(first).expect("lock first asset");
    let (sender, receiver) = mpsc::sync_channel(1);

    thread::scope(|scope| {
        let cache_ref = &cache;
        scope.spawn(move || {
            let result = cache_ref.put_extent(second, validated_extent(4, 0, b"data".to_vec()));
            sender.send(result).expect("send unrelated write result");
        });
        receiver.recv_timeout(Duration::from_secs(3)).expect("unrelated asset write completes").expect("store unrelated asset");
    });

    drop(held);
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handle released")).shutdown().expect("close cache");
}

#[test]
fn concurrent_extents_for_one_asset_both_survive() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(4, Kind::Texture);
    let barrier = Arc::new(Barrier::new(2));

    thread::scope(|scope| {
        for (offset, bytes) in [(0, b"abc".to_vec()), (3, b"def".to_vec())] {
            let cache = &cache;
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                cache.put_extent(resource, validated_extent(6, offset, bytes))
            });
        }
    });

    let AssetLookup::Hit(asset) = cache.lookup(resource) else {
        panic!("concurrent asset extents form a complete payload");
    };
    assert_eq!(asset.extents, vec![Extent { offset: 0, bytes: b"abcdef".to_vec() }]);
    cache.shutdown().expect("close cache");
}

#[test]
fn mismatched_size_and_invalid_ranges_are_rejected() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(3, Kind::PbrMaterial);
    cache.put_extent(resource, validated_extent(12, 0, b"abc".to_vec())).expect("store extent");

    std::assert_matches!(cache.put_extent(resource, validated_extent(13, 3, b"def".to_vec())), Err(AssetCacheError::SizeMismatch));
    std::assert_matches!(cache.put_extent(resource, validated_extent(12, 11, b"xy".to_vec())), Err(AssetCacheError::InvalidExtent));
    std::assert_matches!(cache.put_extent(resource, validated_extent(12, 0, Vec::new())), Err(AssetCacheError::InvalidExtent));
    cache.shutdown().expect("close cache");
}

#[test]
fn malformed_asset_payload_is_reported_and_removed() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(4, Kind::Environment);
    cache.storage.put(storage_key(resource), b"not an extent record".to_vec()).expect("write malformed record");

    assert_eq!(cache.lookup(resource), AssetLookup::Corrupt);
    cache.shutdown().expect("close cache after removal");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("reopen cache");
    assert_eq!(cache.lookup(resource), AssetLookup::Miss);
    cache.shutdown().expect("close reopened cache");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn extent_sequences_match_byte_model(
        operations in proptest::collection::vec((0u8..5, 0u8..32, proptest::collection::vec(any::<u8>(), 1..9)), 1..30)
    ) {
        let directory = tempdir().expect("cache directory");
        let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
        let resource = resource(5, Kind::Texture);
        let mut model = [None; 32];
        let mut present = false;

        for (action, offset, bytes) in operations {
            match action {
                0 | 1 => {
                    let start = usize::from(offset);
                    let allowed = 32usize.saturating_sub(start);
                    let count = bytes.len().min(allowed);
                    let input = bytes.get(..count).expect("bounded extent bytes").to_vec();
                    let conflict = input.iter().enumerate().any(|(index, byte)| {
                        model.get(start + index).is_some_and(|old| old.is_some_and(|old_byte| old_byte != *byte))
                    });
                    let result = cache.put_extent(resource, validated_extent(32, u64::from(offset), input.clone()));
                    if conflict {
                        match result {
                            Err(AssetCacheError::ConflictingOverlap) => {}
                            actual => prop_assert!(false, "expected conflicting overlap, got {actual:?}"),
                        }
                    } else {
                        prop_assert!(result.is_ok());
                        for (index, byte) in input.into_iter().enumerate() {
                            if let Some(slot) = model.get_mut(start + index) {
                                *slot = Some(byte);
                            }
                        }
                        present = true;
                    }
                }
                2 => {
                    cache.clear().expect("clear cache");
                    model.fill(None);
                    present = false;
                }
                3 => {
                    cache.set_size_limit(storage::MIN_SIZE_LIMIT).expect("evict all entries");
                    model.fill(None);
                    present = false;
                    cache.set_size_limit(storage::DEFAULT_SIZE_LIMIT).expect("restore size limit");
                }
                _ => {
                    cache.set_size_limit(storage::DEFAULT_SIZE_LIMIT).expect("raise size limit");
                }
            }

            prop_assert!(cache.accounted_size() <= cache.size_limit());
            let complete = model.iter().all(Option::is_some);
            match (present, complete, cache.lookup(resource)) {
                (false, _, AssetLookup::Miss) => {}
                (true, true, AssetLookup::Hit(asset)) | (true, false, AssetLookup::Partial(asset)) => {
                    let mut expected = Vec::new();
                    let mut cursor = 0usize;
                    while cursor < model.len() {
                        if model.get(cursor).copied().flatten().is_none() {
                            cursor += 1;
                            continue;
                        }
                        let start = cursor;
                        let mut bytes = Vec::new();
                        while let Some(Some(byte)) = model.get(cursor) {
                            bytes.push(*byte);
                            cursor += 1;
                        }
                        expected.push(Extent { offset: u64::try_from(start).expect("model offset"), bytes });
                    }
                    prop_assert_eq!(asset.total_size, 32);
                    prop_assert_eq!(asset.extents, expected);
                }
                (expected, _, actual) => prop_assert!(false, "expected present={expected}, got {actual:?}"),
            }
        }
        cache.shutdown().expect("close cache");
    }
}
