Run commands from the repository root unless stated otherwise. Windows is the primary development platform; use WSL2 for Linux builds and fuzzing. CI checks Windows, macOS, and Linux. The Rust toolchain follows the stable channel rather than a fixed release. CI installs Nextest 0.9; other development tools may vary by checkout.

Network HTTPS uses Reqwest 0.13 with rustls and aws-lc. Building aws-lc requires a C/C++ compiler. Windows x86-64 assembly uses NASM when available or the crate's enabled prebuilt objects. The selected non-FIPS build does not require CMake, bindgen, or Go. See the [aws-lc-rs build guide](https://github.com/aws/aws-lc-rs#build) for supported toolchains. Certificate verification follows the operating system trust configuration.

## Development

```bash
# Build
cargo check --workspace --all-targets
cargo build --workspace --locked
cargo build --workspace --release --locked
cargo build -p <crate>
cargo build -p viewer --bin viewer

# Run viewer
cargo run
cargo run --release
cargo run -p viewer --bin viewer
cargo run -p <package-with-binary> --release
cargo run -p <package-with-binary> -- <args>

# Build diagnostics
cargo build --workspace --timings

# Format
cargo fmt --all
cargo fmt --all -- --check

# Lint
cargo lint
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p <crate> --all-targets -- -D warnings

# Test
cargo xtest --workspace
cargo xtest -p <crate>
cargo nextest run --workspace --all-features
cargo nextest run -p <crate>
cargo nextest list --workspace
cargo test --workspace
cargo test -p <crate>

# Run specific tests
cargo nextest run -p <crate> -E 'test(<name>)'
cargo nextest run -p <crate> --run-ignored only
cargo test -p <crate> <name>
cargo test -p <crate> -- --nocapture

```

- `cargo lint` checks all targets and features with warnings denied.
- `cargo xtest` runs Nextest with all features, but does not explicitly select the workspace. Add `--workspace` when needed.
- Replace `<crate>`, `<name>`, and `<args>` with the relevant values.
- Optional features may require platform-specific dependencies.

## Git hooks

Enable the repository hooks once in each checkout:

```bash
git config --local core.hooksPath .githooks
```

This command changes the local Git configuration for the current checkout.

The pre-commit hook checks formatting. The pre-push hook runs Clippy on all targets and features with warnings denied, then runs the all-feature workspace test suite through Nextest. CI uses the same workspace and feature scope. Its Clippy command does not pass `-D warnings`.

## Fuzzing

Use WSL2 and nightly Rust. `fuzz/` is a separate workspace.

```bash
# Discover targets
cargo +nightly fuzz list

# Time-limited runs
cargo +nightly fuzz run <target> -- -max_total_time=<seconds> -max_len=<bytes>
cargo +nightly fuzz run <target> -- -max_total_time=<seconds>

# Run with a dictionary
cargo +nightly fuzz run <target> -- -dict=fuzz/dictionaries/<target>.dict -max_total_time=<seconds>

# Fixed number of executions
cargo +nightly fuzz run <target> -- -runs=<count>

# Reproduce and minimize a failure
cargo +nightly fuzz run <target> <artifact-path>
cargo +nightly fuzz tmin <target> <artifact-path>

# Minimize corpus
cargo +nightly fuzz cmin <target>

# Add a target
cargo +nightly fuzz add <target>
```

- Corpus: `fuzz/corpus/<target>/`
- Dictionaries: `fuzz/dictionaries/`
- Failures: `fuzz/artifacts/<target>/`
- Coverage: `fuzz/coverage/<target>/`

Corpus inputs are reused across runs. Back up the corpus before minimization. Use sanitized LLUDP fixtures as seeds; remove private session data before committing fixtures.

Dictionaries contain protocol-specific byte sequences that guide mutations. Create and commit curated dictionaries using verified protocol constants. Pass `-dict=<file-path>` explicitly when running with a dictionary.

Fuzzing detects crashes and other failures, not semantic protocol correctness.

### Fuzz coverage

Requires `cargo-binutils` and nightly LLVM tools in WSL2.

```bash
# Select target
TARGET="<target>"

# Generate coverage data
cargo +nightly fuzz coverage "$TARGET"

# Locate instrumented executable
BIN=$(find fuzz/target target -type f -path "*/coverage/*/release/$TARGET" -print -quit 2>/dev/null)

# Generate HTML report
cargo +nightly cov -- show "$BIN" \
  -instr-profile="fuzz/coverage/$TARGET/coverage.profdata" \
  -format=html \
  -output-dir="fuzz/coverage/$TARGET/html"

# Open report
explorer.exe "$(wslpath -w "$(pwd)/fuzz/coverage/$TARGET/html/index.html")"
```

Verify `BIN` points to the instrumented executable. Coverage reflects saved corpus execution, not whole-workspace coverage.

## Test coverage

```bash
# Summary across workspace crates
cargo llvm-cov --workspace

# Show uncovered lines across the workspace
cargo llvm-cov --workspace --text --show-missing-lines

# Inspect uncovered lines in one package
cargo llvm-cov -p <crate> --text --show-missing-lines

# Generate an HTML report
cargo llvm-cov --workspace --html

# Generate an HTML report with Nextest and all features
cargo llvm-cov nextest --workspace --all-features --html

# Machine-readable per-file summary for scripts
cargo llvm-cov --workspace --json --summary-only
```

Report: `target/llvm-cov/html/index.html`

Start with the affected package when investigating a test gap. Workspace missing-line reports can be large. Coverage reflects the features and test targets selected by the command; `--summary-only` requires an export format such as `--json`, `--lcov`, or `--cobertura`.

## Benchmarks

```bash
# Build the single benchmark target for these crates
cargo bench -p network -p protocol --no-run
cargo bench -p inventory@0.0.1 --no-run
cargo bench -p viewer --no-run

# Smoke run each crate's benchmarks
cargo bench -p network -- --test
cargo bench -p protocol -- --test
cargo bench -p inventory@0.0.1 -- --test
cargo bench -p viewer -- --test

# Quick
cargo bench -p <crate> -- <filter> --sample-size 30 --warm-up-time 1 --measurement-time 3 --noplot

# Thorough
cargo bench -p <crate> -- <filter> --sample-size 200 --warm-up-time 5 --measurement-time 15
```

Each crate has one `main` benchmark target. Replace the package and Criterion filter as needed. The inventory package name is version-qualified because a dependency also uses `inventory`. For example, `cargo bench -p render -- reconcile` selects reconciliation benchmarks. The protocol receive benchmark is `cargo bench -p protocol -- receive_pipeline`. It reports packets per second for mixed packet framing, preflight, and decode. It does not include circuit deduplication or ack bookkeeping. The inventory benchmarks build 100,000 direct items, measure snapshot reads and one-item delta application, and print retained model memory plus delta allocation counts and peak bytes. `inventory/cache_warm_read_apply_100k` reports first and warm cache read/apply times and the database file size; compare it with `inventory/build_100k_items_and_snapshot` for the from-scratch path.

The viewer benchmark `cargo bench -p viewer -- inventory_browser` measures search over 100,000 items and visible-row construction when expanding a folder with 10,000 children. It prints median and p99 latency for both operations in addition to Criterion's samples.

- `cargo perf` runs workspace benchmarks with 200 samples, a 5-second warm-up, and a 15-second measurement. `cargo perf-quick` uses 30 samples, a 1-second warm-up, a 3-second measurement, and no plot. Use `cargo bench -p <crate> -- <filter> ...` for a focused run.
- `cargo profile`: viewer profiling build with Tracy.
- `cargo flamegraph`: requires a configured profiler backend; WSL2 may restrict Linux `perf`.

## Dependencies

```bash
# Dependency trees
cargo tree --workspace --duplicates
cargo tree -p <crate>
cargo tree -i <crate>

# Security and policies
cargo audit
cargo deny check

# Maintenance
cargo outdated --workspace
cargo +nightly udeps --workspace
```

- `cargo deny` requires `deny.toml`.
- `cargo outdated` may fail on Git/path dependencies, including `dxr`.
- `cargo udeps` may report false positives for optional features.
- Duplicate dependencies are not necessarily problems; inspect before consolidating.

## Code analysis

```bash
# Binary size
cargo bloat --release -p <crate> --bin <bin> --crates
cargo bloat --release -p <crate> --bin <bin> -n <count>

# Macro expansion
cargo expand -p <crate> --lib

# Undefined behavior checks
cargo +nightly miri test -p <crate>

# Mutation testing
cargo mutants -p <crate>
cargo mutants -f <file-path>
```

- `cargo bloat` measures linked code size, not memory usage.
- Miri may fail on unsupported OS operations; prefer focused tests.
- Mutation testing is expensive; start with individual files.
- `rust-analyzer-mcp` is available on Windows for editor and agent integration.

## Platforms

Windows is the primary local development platform. The CI workflow builds, lints, and tests on Windows, macOS, and Linux. It runs documentation tests on Linux.

Use WSL2 for Linux builds, fuzzing, corpus management, and fuzz coverage. Confirmed Linux utilities include `cargo-fuzz` and `cargo-binutils`. Prefer `~/Radia` over `/mnt/c/Radia` for Linux build performance. Keep build artifacts separate between platforms. Windows `.exe` tools exposed through WSL's `PATH` do not replace Linux-native installations.

On Linux, remembered login storage uses Secret Service and requires a running provider such as GNOME Keyring or KWallet.
