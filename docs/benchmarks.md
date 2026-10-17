The workspace has Criterion targets in `llsd`, `network`, `protocol`, and `world`. Run every target with:

```powershell
cargo bench --workspace -- --noplot
```

Run one target at a time:

```powershell
cargo bench -p llsd --bench codec -- --noplot
cargo bench -p network --bench capabilities -- --noplot
cargo bench -p protocol --bench packets -- --noplot
cargo bench -p world --bench main -- --noplot
```

The LLSD target measures XML, binary, and notation encoding and decoding, plus bounded XML admission. The network target measures capability discovery and POST requests against local loopback servers. The protocol target measures LLUDP packet encoding and decoding, including object updates and rejected packets. The World target measures lifecycle, updates, queries, relationships, pose resolution, and large-scene operations.

To run only the large World workloads:

```powershell
cargo bench -p world --bench main -- 'world/(load_query|linkset_churn|spatial_repair)' --noplot
```

This filter selects first-page queries, linkset reparent churn, and spatial-index repair for the large World fixtures.

For a local comparison, run the same target and settings once with `--save-baseline before`, then rerun it with `--baseline before`. For example:

```powershell
cargo bench -p world --bench main -- --save-baseline before
cargo bench -p world --bench main -- --baseline before
```

Criterion stores results under the ignored `target/criterion` directory. Use the same machine and build settings for both runs.

These separate test commands are not Criterion benchmarks:

```powershell
cargo test -p world --test allocation -- --nocapture
cargo test -p viewer --lib --release sustained_world_load -- --ignored --nocapture --test-threads=1
```

The first reports World allocator measurements. The second runs the serial SessionWorld stress test with large object fixtures.
