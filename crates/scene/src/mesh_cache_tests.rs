use super::*;

fn key(detail: Detail) -> MeshKey {
    MeshKey { shape: PrimShape::default(), detail }
}

#[test]
fn visible_work_overtakes_background() {
    let mut meshes = MeshCache::new(Budget::default());
    for detail in [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High] {
        assert!(meshes.request(key(detail)));
    }
    meshes.prioritize(key(Detail::Low), 20);
    meshes.prioritize(key(Detail::High), 200);
    meshes.prioritize(key(Detail::High), 5);
    let first = meshes.start().unwrap();
    assert_eq!(first.key, key(Detail::High), "shared geometry keeps its greatest on-screen demand");
    meshes.complete(first.run());
    meshes.reset_priorities();
    meshes.prioritize(key(Detail::Medium), 100);
    let second = meshes.start().unwrap();
    assert_eq!(second.key, key(Detail::Medium), "camera changes replace previous priorities");
    meshes.complete(second.run());
    let third = meshes.start().unwrap();
    assert_eq!(third.key, key(Detail::Lowest), "equal priorities retain request order");
    meshes.complete(third.run());
    let last = meshes.start().unwrap();
    assert_eq!(last.key, key(Detail::Low));
    meshes.complete(last.run());
    assert!(!meshes.waiting(), "background geometry still completes");
}

#[test]
fn shared_admission_and_retry() {
    let budget = Budget::default();
    let mut a = MeshCache::new(budget.clone());
    let mut b = MeshCache::new(budget.clone());
    for detail in [Detail::Low, Detail::High] {
        assert!(a.request(key(detail)));
    }
    assert!(b.request(key(Detail::Medium)));
    let first = a.start().unwrap();
    let second = a.start().unwrap();
    assert!(b.start().is_none());
    assert_eq!(budget.charged_bytes(), 8 * 1024 * 1024);
    drop(first);
    let other = b.start().unwrap();
    assert!(a.start().is_none());
    drop(second);
    let retried = a.start().unwrap();
    a.complete(retried.run());
    assert!(matches!(a.get(key(Detail::Low)), PreparedMesh::Ready(_)));
    drop(other);
    drop(a);
    drop(b);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn stale_jobs_and_retained_charge() {
    let budget = Budget::default();
    let mut meshes = MeshCache::new(budget.clone());
    let key = key(Detail::High);
    assert!(meshes.request(key));
    let old = meshes.start().unwrap().run();
    meshes.release(key);
    assert!(meshes.request(key));
    let new = meshes.start().unwrap();
    meshes.complete(old);
    assert!(matches!(meshes.get(key), PreparedMesh::Pending));
    meshes.complete(new.run());
    let PreparedMesh::Ready(retained) = meshes.get(key) else {
        panic!("expected ready mesh");
    };
    let charge = budget.charged_bytes();
    assert!(charge > retained.mesh().owned_bytes() && charge < 4 * 1024 * 1024);
    meshes.release(key);
    drop(meshes);
    assert_eq!(budget.charged_bytes(), charge);
    let mut replacement = MeshCache::new(budget.clone());
    assert!(replacement.request(key));
    let job = replacement.start().unwrap();
    replacement.complete(job.run());
    let PreparedMesh::Ready(rebuilt) = replacement.get(key) else {
        panic!("expected rebuilt mesh");
    };
    assert_ne!(retained.id(), rebuilt.id(), "rebuilding a source key must not alias a retained GPU resource");
    drop(replacement);
    drop(rebuilt);
    assert_eq!(budget.charged_bytes(), charge);
    drop(retained);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn entry_limit_and_negative_cache() {
    let mut meshes = MeshCache::new(Budget::default());
    for begin in 0..MAX_ENTRIES {
        assert!(meshes.request(MeshKey { shape: PrimShape { profile_begin: begin as u16, ..PrimShape::default() }, detail: Detail::Low }));
    }
    let extra = MeshKey { shape: PrimShape { profile_curve: 15, ..PrimShape::default() }, detail: Detail::High };
    assert!(!meshes.request(extra));
    assert!(matches!(meshes.get(extra), PreparedMesh::Capacity));
    meshes.release(key(Detail::Low));
    assert!(meshes.request(extra));
    while let Some(job) = meshes.start() {
        meshes.complete(job.run());
    }
    assert!(matches!(meshes.get(extra), PreparedMesh::Failed(MeshError::UnsupportedCurve)));
    assert!(meshes.request(extra));
    assert!(meshes.start().is_none());
}

#[test]
fn cache_pressure_recovery() {
    let budget = Budget::default();
    let mut meshes = MeshCache::new(budget.clone());
    let keys: Vec<_> = (0..128)
        .map(|profile_begin| MeshKey {
            shape: PrimShape {
                path_curve: 0x20,
                profile_curve: 0,
                path_scale_x: 150,
                path_scale_y: 150,
                path_twist_begin: -100,
                path_twist: 100,
                path_revolutions: 200,
                path_skew: 95,
                profile_begin,
                ..PrimShape::default()
            },
            detail: Detail::High,
        })
        .collect();
    for key in &keys {
        assert!(meshes.request(*key));
    }
    for _ in 0..keys.len() {
        let Some(job) = meshes.start() else {
            break;
        };
        meshes.complete(job.run());
    }
    assert!(keys.iter().any(|key| matches!(meshes.get(*key), PreparedMesh::Capacity)));
    assert!(meshes.start().is_none(), "capacity cannot cause repeated decoding");
    assert!(meshes.bytes <= CACHE_BYTES);
    let key = *keys.iter().find(|key| matches!(meshes.get(**key), PreparedMesh::Ready(_))).unwrap();
    meshes.release(key);
    let job = meshes.start().expect("eviction makes blocked work eligible");
    meshes.complete(job.run());
    assert!(meshes.bytes <= CACHE_BYTES);
    drop(meshes);
    assert_eq!(budget.charged_bytes(), 0);
}
