---
name: rust-testing
description: Rust tests. Use for any test planning, writing, review, or running (selection, naming, fixtures, assertions, regressions, proptest, async, Nextest, coverage, cargo-mutants), including bare "add tests" or "review these tests".
---

# Rust testing

A test earns its place by catching a plausible failure of an observable contract, invariant, boundary, or regression, or by sharpening failure diagnosis. Coverage, test count, and mutation score are never reasons to add one. Use the project's layout, tools, and dependencies. Leave production code unchanged for test convenience.

Plan and review requests: inspect and report, no edits, no runs. Implementation requests: run what the user or repo policy requires.

## Workflow

1. **Inspect.** Read the behavior, callers, existing tests, and fixtures. List what must hold and what is already covered.
2. **Name the failure** each new case catches. Merge into an existing test when contract and setup match and a failure still diagnoses clearly.
3. **Pick the faithful boundary.** Unit for local invariants, integration for public contracts, doctest for public examples, proptest for broad invariants. UI flows drive real actions through the existing app test context and assert the resulting state.
4. **Write it.** Minimal setup. Expected values come from outside the code under test.
5. **Break it.** Mutate the code under test and watch the test fail with a message naming the broken contract, then revert. Ask of every kept test: can it pass with the product wrong, and can it fail after a behavior-preserving refactor? Skip the mutation only when it is expensive, and say so.
6. **Run** focused tests first (`cargo nextest run <filter>`), then broader by risk and repo policy. Report what ran and passed.

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

- Few local tests live inline in `#[cfg(test)] mod tests`. When `foo`'s tests pass about a screen or rival the production code, move them to `foo_tests.rs`, wired from `foo.rs` to keep private access. Follow an existing crate convention.

    ```rust
    #[cfg(test)]
    #[path = "foo_tests.rs"]
    mod tests;
    ```

- Public and subsystem contracts live in `tests/*.rs`, grouped by behavior. Each top-level file is its own crate, so split only for navigation or isolation.
- Merge cases that share contract, setup, and operation. Keep separate the cases with independent contracts or different setup. When one dimension varies, use a table or loop and put the case identity in the assertion message.

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

## Fixtures and helpers

- Inline small values. Large or reused inputs go in a named fixture directory.
- Expected data is independent: spec examples, hand-checked vectors, captured output from a compatible implementation, or a simpler model. Record provenance for external formats. Generate expected values with the code under test never.
- Expensive oracles: commit reviewed outputs and put regeneration behind a separate explicit command.
- Helpers sit at the narrowest scope: private, then `tests/support`, then a shared crate. The action and the assertion stay visible in the test body.

## Assertions

- `assert_eq!` over boolean checks. Match the error variant or fields when the error is contractual. Tests may return `Result` and use `?`.
- Assert everything the contract makes observable: result, state, side effects, ordering, cleanup, and the next operation after cleanup. `is_ok()` alone is incomplete.
- Private call counts, allocations, layout, logging, and timing only when contractual. Benchmarks cover performance.
- Snapshots only for stable, externally meaningful output.
- A round trip alone passes with a matching encoder/decoder bug, so pair it with an independent fixture or semantic invariant.
- `#[should_panic]` only for intentional panic contracts. Fallible APIs and untrusted input assert a structured error.

## Regressions

Reduce the bug to a deterministic example that names the violated contract. Keep it after the fix and confirm it fails before the fix when practical. Encode the contract, not the issue number or a private call sequence.

## Property tests

Use for invariants over many inputs. Sampling is not proof, so keep named tests for known bugs and exact boundaries. Confirm the project already depends on proptest.

| Family                | Property or oracle                                                |
| --------------------- | ----------------------------------------------------------------- |
| Round trip            | `decode(encode(x)) == x`, paired with independent fixtures        |
| Canonicalization      | `normalize(normalize(x)) == normalize(x)`                         |
| Inverse               | `remove(insert(s, x), x) == s` under preconditions                |
| Differential          | Optimized and independent reference agree (watch for shared code) |
| Model / state machine | Random command sequences match a reference model after every step |
| Metamorphic           | An allowed input transform implies a predictable output relation  |
| Robustness            | Arbitrary malformed bytes never panic and respect resource caps   |

- Use `prop_assert!` and `prop_assert_eq!` for clean shrinking.
- Prefer built-in strategies: ranges, regex strings, `collection::vec`, `prop_oneof!`, `prop::option::of`, `prop::sample::select`.
- Weight boundaries explicitly (empty, max length, duplicate keys, out-of-order steps, invalid UTF-8, truncated headers).
- Build valid values with `prop_map`, not `prop_assume!` filtering. `prop_flat_map` only for dependent values.
- Bound sizes and case counts to the CI budget without shrinking the domain to hide a failure.
- Turn each meaningful minimized counterexample into a named deterministic test. Keep `proptest-regressions` files under version control per repo policy.
- Pitfalls: circular oracles, overbroad domains, generators that never produce structured malformed data (conflicting lengths, nested fields), final-state-only checks in state machines, real sleeps, singular cases forced into properties.

## Async and concurrency

Force the smallest interleaving that could violate a guarantee, using channels, oneshots, barriers, mock transports, paused clocks, or a deterministic runner. Time passes only through the clock.

Scenarios (pick those the contract implies): cancellation at states where cleanup differs, late events after replacement or disconnect, full queue or closed receiver, visible ordering, retry caps and idempotency, shutdown drain or cancel, partial I/O, EOF, malformed input, duplicate responses, lock ordering.

```rust
use std::{future::pending, time::Duration};
use tokio::{sync::oneshot, time};

#[tokio::test(start_paused = true)]
async fn request_times_out_at_deadline() {
    let (started_tx, started_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        time::timeout(Duration::from_secs(5), async {
            started_tx.send(()).expect("test receiver is alive");
            pending::<()>().await
        })
        .await
    });

    started_rx.await.expect("request started");
    time::sleep(Duration::from_secs(4)).await;
    assert!(!task.is_finished());
    time::sleep(Duration::from_secs(1)).await;
    assert!(task.await.expect("task must complete").is_err());
}
```

`start_paused` needs Tokio's `macros`, `rt`, `time`, and `test-util` features and does not advance `std::time::Instant`. Swap `pending` for the real operation and assert the timeout error and cleanup.

- Verify cancellation cleanup through later behavior (new work is admitted), not private counters.
- Races reproducible only probabilistically get a separate stress test plus a deterministic regression. Use `loom` for atomics and lock-free code.
- Real sockets bind loopback on OS-assigned ports. Each test owns and cleans up its tasks, processes, channels, and files.
- Asserting absence (nothing sent, state unchanged, no task in flight) needs a finite bound. Advance a fake clock, close the event source, or exhaust a bounded queue.
- A failing race test points to a scheduler, timing, global-state, or shutdown dependency. Fix that instead of raising timeouts or adding retries.

## Unsafe and FFI

Run unsafe Rust under Miri. Miri cannot execute foreign calls, so test FFI wrappers against a fake behind the boundary. Cover ownership and lifetime transfer, drop order, and null or invalid inputs through the safe wrapper.

## Coverage and mutation

Coverage shows what ran. Mutation shows whether tests notice changes. Neither measures correctness, and neither gets a target score.

- Coverage finds high-risk branches and contracts nothing exercises.
- Run mutation when a fault is hard to judge by inspection, starting with the affected function or package.
- A surviving mutant earns a test only when it exposes an unprotected observable contract. Equivalent mutants get a narrow, documented exclusion. Compile failures are unviable. Timeouts need investigation.
- Doctests are invisible to Nextest and coverage results.

## Guardrails

- No new mocking frameworks, harnesses, or generic helpers for one-off tests. Use lightweight fakes or in-memory transports at the real boundary.
- Control clocks, RNG seeds, queues, network responses, and scheduling. Use temp dirs and OS-assigned ports.
- Important tests run in the ordinary local and CI path. `#[ignore]` is for manual, expensive, external, or stress tests, each with a documented command.
- Fix flakiness with synchronization and isolation. A retry is a temporary diagnostic.
- Respect feature flags, MSRV, supported OSes, and the user's scope. A narrow change stays narrow.
- Failing checks stay visible: no weakened assertions, dropped in-scope failures, excluded relevant mutants, or unjustified ignores.
