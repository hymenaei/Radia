Run commands from the repository root unless stated otherwise. Windows is the primary development platform; use WSL2 for Linux builds and fuzzing. Tool versions are not pinned.

Network HTTPS uses Reqwest 0.13 with rustls and aws-lc. Building aws-lc requires a C/C++ compiler. Windows x86-64 assembly uses NASM when available or the crate's enabled prebuilt objects. The selected non-FIPS build does not require CMake, bindgen, or Go. See the [aws-lc-rs build guide](https://github.com/aws/aws-lc-rs#build) for supported toolchains. Certificate verification follows the operating system trust configuration.

## Development

```bash
# Build
cargo check --workspace --all-targets
cargo build --workspace --locked
cargo build --workspace --release --locked
cargo build -p <crate>
cargo build -p viewer --bin viewer

# Run Radia
cargo run -p viewer --bin viewer

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

# Run app
cargo run
cargo run --release
cargo run -p <crate> --release
cargo run -p <crate> -- <args>
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

The pre-commit hook checks formatting. The pre-push hook runs Clippy and the full workspace test suite with all features, matching CI.

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

Corpus inputs are reused across runs. Back up the corpus before minimization. Use sanitized LLUDP fixtures as seeds; never commit raw captures.

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
# Quick
cargo bench -p <crate> --bench <bench> -- --sample-size 30 --warm-up-time 1 --measurement-time 3 --noplot

# Thorough
cargo bench -p <crate> --bench <bench> -- --sample-size 200 --warm-up-time 5 --measurement-time 15
```

Replace the package and benchmark as needed.

- `cargo perf` / `cargo perf-quick`: configured but previously passed Criterion flags to libtest. Use explicit targets until verified.
- `cargo profile`: viewer profiling build with Tracy.
- `cargo flamegraph`: requires a configured profiler backend; WSL2 may restrict Linux `perf`.

Pass Criterion arguments only to Criterion benchmarks.

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

**Windows:** GUI development, builds, tests, benchmarks, profiling, coverage, Miri, and mutation testing.

**WSL2:** Linux builds, fuzzing, corpus management, and fuzz coverage. Confirmed Linux utilities: `cargo-fuzz` and `cargo-binutils`.

Prefer `~/Radia` over `/mnt/c/Radia` for Linux build performance. Keep build artifacts separate between platforms. Windows `.exe` tools exposed through WSL's `PATH` do not replace Linux-native installations.
