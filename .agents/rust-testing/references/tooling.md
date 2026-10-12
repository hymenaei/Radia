# Rust test organization and commands

Use commands supported by the project's actual toolchain, features, test runner, and CI policy. Bracketed placeholders below must be replaced with real values.

In Radia, use `cargo xtest` for ordinary test runs. It is the configured Nextest alias. Use `cargo test --doc --workspace` for doctests.

## Layout

```text
crate/
├── src/lib.rs                 # #[cfg(test)] mod tests { ... }
├── src/parser.rs              # parser-local tests where useful
├── tests/
│   ├── codec_contract.rs      # API integration tests (separate crate)
│   ├── network_lifecycle.rs
│   └── common/mod.rs          # shared support, not its own test binary
└── benches/                   # benchmarks, if appropriate
```

Unit tests within `src/` may access private implementation details, but should still test intentional invariants. Integration tests exercise public APIs as users do; each file in `tests/` is a separate test crate. Public doctests should demonstrate real usage and be runnable. For compile-time API guarantees, use rustdoc `compile_fail` tests or `trybuild` when diagnostics and multiple compiler outcomes need careful validation. Keep fixtures in a clear dedicated directory.

## Everyday commands

```sh
cargo test -p <crate> <filter>               # Named test/filter in package
cargo test -p <crate> --lib                  # Library unit tests only
cargo test -p <crate> --test <test-target>   # One integration test target
cargo test -p <crate> --doc                  # Doctests
cargo test --workspace                       # All selected workspace members
cargo test -p <crate> -- --nocapture         # Debug output
cargo test -p <crate> -- --include-ignored   # Include #[ignore] cases
cargo nextest run -p <crate>                 # Fast process-isolated suite
cargo nextest run --workspace                # Nextest across workspace
cargo nextest run --run-ignored=only         # Only ignored tests
```

`cargo test` compiles and runs unit, integration, and documentation tests (depending on target options). Nextest currently does **not** run doctests: explicitly run `cargo test --doc`. Nextest provides timeout controls and optional retries. Avoid enabling retries to mask flaky failures.

## Tools and when they're justified

- **cargo-nextest:** faster/isolation-focused default when already adopted; use filters and slow-test timeout controls, not arbitrary full-suite serialization.
- **proptest:** add dev-dependency only when the code has a meaningful property; keep regressions and independent fixtures.
- **cargo-mutants:** expensive targeted confidence audit; check `--list` first and honor existing `.cargo/mutants.toml` settings (including the runner) rather than overriding them unnecessarily.
- **cargo-llvm-cov:** optional coverage investigation. `cargo llvm-cov nextest` supports Nextest; doctest coverage requires special setup.
- **trybuild:** compile-pass/compile-fail of macros, traits, or type-level contracts; avoid pinning fragile compiler wording unnecessarily.
- **Miri:** useful for unsafe code and possible undefined behavior; not an ordinary proof of soundness.
- **loom:** systematic concurrency interleaving model for code that uses supported synchronization primitives; not a drop-in replacement for async integration tests.
- **Criterion:** statistically controlled benchmarks for performance; wall-time assertions under ordinary `cargo test` are usually flaky.

Only suggest/install tool dependencies when they earn their maintenance and CI costs. Benchmark test-runner performance before making Nextest, coverage, or mutation a mandatory expensive gate.

## Quality gates

1. Run focused tests and confirm they fail meaningfully against the bug when feasible.
2. Run affected package's ordinary suite and doctests if relevant.
3. Run cross-component/integration tests and supported features/targets proportional to risk.
4. Check flakiness using repeated runs _without hiding failures behind retries_ when a race is suspected.
5. Report what was and was not executed, including relevant environment constraints.
