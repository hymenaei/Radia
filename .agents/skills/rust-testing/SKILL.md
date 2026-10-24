---
name: rust-testing
description: Rust tests. Use for any test planning, writing, review, or running (selection, naming, oracles, fixtures, assertions, regressions, proptest, async, Nextest, Miri, coverage, cargo-mutants), including bare "add tests" or "review these tests".
---

# Rust testing

A test earns its place by catching a plausible failure of a contract (observable behavior, invariant, boundary, or regression), or by sharpening failure diagnosis. Coverage, test count, and mutation score are never reasons to add one. Follow the project's layout, tools, and dependencies. Leave production code unchanged for test convenience.

## Modes

- **Plan or review.** Steps 1 to 3, then judge existing tests with step 5's two questions by reading. Report findings, each naming the missing or weak contract. No edits, no runs.
- **Implement.** All steps. Run what the user or repo policy requires.

## Workflow

1. **Inspect.** Read the behavior, callers, existing tests, and fixtures. Done when you can list what must hold and which of it is already covered.
2. **Name the failure** each new case catches, in one line. A case with no such line is cut. Merge into an existing test when contract and setup match and a failure still diagnoses clearly.
3. **Pick the faithful boundary.** Unit for local invariants, integration for public contracts, doctest for public examples, proptest for broad invariants. UI flows drive real actions through the existing app test context and assert the resulting state.
4. **Write it.** Minimal setup. Expected values come from an independent oracle (see Oracles and fixtures).
5. **Challenge it.** Ask of every kept test whether it can pass with the product wrong or fail after a behavior-preserving refactor. Use mutation testing when inspection cannot answer that confidently, following Coverage and mutation. Revert any manual mutation.
6. **Run** focused tests first (`cargo nextest run <filter>`), then broader by risk and repo policy. Nextest skips doctests, so run `cargo test --doc` when doctests changed. Done when every added or changed test has run green, or the report says why not. Report what ran and passed.

## Selecting cases

Rank by impact and likelihood: data loss, compatibility, security, state transitions, concurrency, parsing, cleanup, reported regressions. A few strong cases beat many weak ones.

| Contract       | High-value case                                                | Low-value                             |
| -------------- | -------------------------------------------------------------- | ------------------------------------- |
| Frame parser   | Truncated length prefix returns `Incomplete`, nothing consumed | Private helper returns a nonempty vec |
| Bounded client | Cancellation releases admission exactly once                   | Internal refcount with no contract    |
| Event state    | Out-of-order events never resurrect a stale generation         | Exact helper call counts              |
| Serializer     | Independent known-good bytes, specified canonical order        | One happy-path round trip             |
| Config         | Precedence, invalid values, specified unknown-key handling     | Getter equals an internal constant    |
| Async retry    | Fail, retry, succeed, no duplicate application                 | Sleep, then check a log was written   |

Test private code only when a complex local invariant is out of reach of public behavior.

## Placement

- Few local tests live inline in `#[cfg(test)] mod tests`. When the tests outgrow the code under test, or fixtures and table data bury it, move them to `foo_tests.rs`, wired from `foo.rs` so the module stays a child with private access. An existing crate convention wins.

  ```rust
  #[cfg(test)]
  #[path = "foo_tests.rs"]
  mod tests;
  ```

- Keep tests last in the file, including declarations of external test modules, as required by AGENTS.md.

- Public and subsystem contracts live in `tests/*.rs`, grouped by behavior. Each top-level file is its own crate, so split only for navigation or isolation.
- Keep separate the cases with independent contracts or different setup. When one dimension varies, use a table or loop and put the case identity in the assertion message.

## Naming

Max 5 words, `<subject>_<outcome>`. Add a condition only to separate siblings. Drop the module, type, and function (context already names them), setup, inputs, and filler (`test_`, `should`, `when`, `returns`, `correctly`). Match sibling style. Details go in the assertion message. Needing more words means two contracts (split) or encoded setup (cut). Property tests are named for the invariant. No issue IDs.

```rust
// not: parse_frame_with_truncated_length_prefix_returns_incomplete
fn truncated_prefix_is_incomplete() {}

// not: when_a_request_is_canceled_the_permit_is_released
fn cancel_releases_permit() {}

// not: config_precedence_env_overrides_file_overrides_default
fn env_beats_file() {}
```

## Oracles and fixtures

- An **oracle** is where expected values come from, and it sits outside the code under test: spec examples, hand-checked vectors, captured output from a compatible implementation, or a simpler model. Record provenance for external formats.
- Expensive oracles: commit reviewed outputs and put regeneration behind a separate explicit command.
- Inline small values. Large or reused inputs go in a named fixture directory.
- Helpers sit at the narrowest scope: private, then `tests/support`, then a shared crate. The action and the assertion stay visible in the test body.

## Assertions

- `assert_eq!` over boolean checks. Match the error variant or fields when the error is contractual. Tests may return `Result` and use `?`.
- Assert everything the contract makes observable: result, state, side effects, ordering, cleanup, and the next operation after cleanup. `is_ok()` alone is incomplete.
- Private call counts, allocations, layout, logging, and timing only when contractual. Benchmarks cover performance.
- Snapshots only for stable, externally meaningful output.
- A round trip alone passes with a matching encoder/decoder bug, so pair it with an independent oracle or a semantic invariant.
- `#[should_panic]` only for intentional panic contracts. Fallible APIs and untrusted input assert a structured error.

## Regressions

Reduce the bug to a deterministic example that names the violated contract. Keep it after the fix and confirm it goes red before the fix when practical. Encode the contract, not the issue number or a private call sequence.

## Disclosed references

- **Property tests.** Before writing or reviewing any proptest or generated-input test, read `references/proptest.md`.
- **Async and concurrency.** Before writing or reviewing tests that involve tasks, timers, channels, cancellation, locks, or real I/O, read `references/async.md`.

## Unsafe and FFI

Run unsafe Rust under Miri when the code and target are supported. If Miri is unavailable or unsupported, report that and test the safe boundary. Miri cannot execute foreign calls, so test FFI wrappers against a fake behind the boundary. Cover ownership and lifetime transfer, drop order, and null or invalid inputs through the safe wrapper.

## Coverage and mutation

Coverage shows what ran. Mutation shows whether tests notice changes. Neither measures correctness, and neither gets a target score.

- Coverage finds high-risk branches and contracts nothing exercises.
- Run cargo-mutants when a fault is hard to judge by inspection, starting with the affected function or package. A small manual mutation is also useful for a specific contract; revert it afterward.
- A surviving mutant earns a test only when it exposes an unprotected observable contract. Equivalent mutants get a narrow, documented exclusion. Compile failures are unviable. Timeouts need investigation.

## Guardrails

- Reuse what the project has. A one-off test gets a lightweight fake or an in-memory transport at the real boundary, not a new mocking framework, harness, or generic helper.
- Control clocks, RNG seeds, queues, network responses, and scheduling. Use temp dirs and OS-assigned ports.
- Important tests run in the ordinary local and CI path. `#[ignore]` is for manual, expensive, external, or stress tests, each with a documented command.
- Fix flakiness at its source: synchronization, isolation, global state, or shutdown order. A longer timeout or a retry is a temporary diagnostic.
- Respect feature flags, MSRV, supported OSes, and the user's scope. A narrow change stays narrow.
- Keep failing checks visible: assertions keep their strength, in-scope failures get reported, relevant mutants stay in the run, and every ignore carries a reason.
