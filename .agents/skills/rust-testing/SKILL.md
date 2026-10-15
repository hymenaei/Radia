---
name: rust-testing
description: Design, write, review, and run high-value Rust tests. Use for test planning, naming, unit/integration/doctests, proptest, async and concurrency regressions, cargo-nextest, cargo-mutants, coverage, flaky tests, and test-suite audits. Focus on behavioral confidence rather than test counts or coverage percentages.
---

# Rust Testing

Write the _smallest reliable tests that protect meaningful contracts_. Follow the project's established structure and tools; this skill is guidance, not permission to add dependencies, refactor production code, run an expensive suite, or chase metrics without a reason. For planning or review-only requests, inspect and report without changing files or running tests. For implementation requests, run only checks requested by the user or required by repository policy.

## Operating workflow

1. **Inspect first.** Read the relevant behavior, callers, existing tests, fixtures, manifests, workspace policy, and CI commands. Identify the actual contract and observable consequences of breaking it. Confirm tool availability rather than assuming tools are installed.
2. **Choose risk-driven cases.** List normal, boundary, invalid, failure, recovery, and lifecycle scenarios. Prioritize parsers, wire formats, state transitions, data loss, security boundaries, concurrency, cleanup, and prior regressions. For a simple change, a few strong tests can be enough.
3. **Choose the right layer.** Use focused unit tests for local invariants; integration tests for public contracts and subsystem interaction; doctests for instructive public examples; property tests when an invariant spans many inputs; targeted real-process tests for OS/process behavior. Prefer the lowest layer that _faithfully_ verifies the contract, not always the smallest unit.
4. **State the failure mode before coding.** For each planned test, answer: "Which plausible regression would fail this test?" If there is no compelling answer, revise or skip it.
5. **Implement clear Arrange–Act–Assert tests.** Keep setup minimal, assertions specific, and expected values independent of the implementation under test. Assert errors, state changes, ordering, cleanup, and side effects when these are the contract.
6. **Validate progressively.** Run the narrow test first, then affected crate/integration tests, then workspace and platform checks proportional to the change. Run doctests separately when using Nextest. Do not report tests as passing unless executed.
7. **Review the tests themselves.** Remove redundant or brittle assertions. Check that a deliberately plausible incorrect implementation would be caught. Use coverage and mutation testing to _find gaps_, not to hit a score.
8. **Report results.** Summarize protected behavior, commands executed, pass/fail/skipped outcomes, remaining gaps, and any tests not run with a reason.

## What to test

- **Observable behavior:** inputs, outputs, side effects, public error contracts, state transitions, resource release, and interoperability.
- **Boundaries:** empty/single/max-size input; just below/at/above limits; zero, overflow, truncation, unknown variants, malformed bytes, Unicode, and platform-specific behavior _as applicable_.
- **Failure and recovery:** cancellation, timeouts, partial writes, disconnects, retries, backpressure, stale generations, duplicate/out-of-order events, and idempotent cleanup.
- **Invariants:** properties preserved across many inputs or command sequences; error paths that must not panic; bounded memory or queue admission where specified.
- **Regression cases:** reproduce bugs deterministically, confirm failure before the fix when feasible, then retain a focused assertion.
- **Integration seams:** boundaries where independently correct modules can fail together (e.g., codec → protocol → network → state).

See [test selection and naming](references/test-design.md) for priority, examples, and explicit skip criteria.

## What _not_ to test by default

- Every function, branch, getter, enum variant, or derived trait solely because it exists.
- The compiler/standard-library behavior, or implementation details (private call counts, allocations, exact internal data layout) unless those are actual performance or compatibility contracts.
- Identical results produced by calling the same implementation twice as both system and expected-value oracle.
- Internal logging text, incidental ordering, exact timing, or intermediate states that the contract does not expose.
- Broad duplicates of an already proven behavior without new risk, or snapshots that must be rewritten after harmless refactors.
- Live services, arbitrary sleeps, globally shared ports/files, uncontrolled randomness, or wall-clock deadlines in normal deterministic tests.
- An artificial 100% coverage or 100% mutation-kill goal. A _skipped_ test is not evidence of coverage.

These are heuristics, not blanket bans: test private helpers, formatting, memory behavior, and exact wire bytes when they represent real contracts or high-risk algorithms.

## Naming and structure

- Use the shortest **snake_case** name that identifies the behavior under test. Let the module path supply context instead of repeating it in the test name.
- When a test covers related outcomes, name their shared contract instead of listing each outcome. Split only when the assertions protect independent contracts. Add a condition only when it distinguishes this case.
- Use `behavior_when_condition_expected_result` only when each part adds information. Avoid names that narrate setup.
- Avoid vague names such as `it_works`, `test1`, or `basic`, issue IDs alone, and redundant `test_` prefixes.
- Group by behavior/module and use a short, consistent local convention. Parameter cases must remain distinguishable in failures.
- Put close-to-implementation unit tests in `#[cfg(test)] mod tests`; external API tests in `tests/`; reusable fixtures in `tests/common/mod.rs` or a dedicated test-support module; doctests on useful public APIs. Never expose production APIs solely for tests without a concrete design justification.

## Assertion quality

- Prefer `assert_eq!`/`assert_ne!` over a bare boolean when the values aid diagnosis; for result variants, match the relevant error kind instead of only `is_err()` if that matters.
- For expected success, `.expect("why this must succeed")` or `?` in `Result`-returning tests is fine. For failure, inspect the failure itself.
- Avoid `#[should_panic]` for normal invalid input in fallible APIs; use it only for intentional panic contracts, with `expected` if practical.
- Use golden fixtures from _independent, reviewed_ reference bytes/specifications, not outputs blindly generated by the code being tested.
- Never assert only "no panic" when correctness, unchanged state, or an error code is part of the contract.

## Use specialized methods deliberately

| Technique                   | Reach for it when                                                   | Avoid when                                                |
| --------------------------- | ------------------------------------------------------------------- | --------------------------------------------------------- |
| Example-based unit tests    | Named edge cases and precise algorithms                             | Repeating the same trivial check                          |
| Integration tests           | Public API and cross-component flows                                | Can be verified more reliably at a simple seam            |
| Doctests / compile-fail     | Public usage and compile-time API contracts                         | Merely duplicating an internal unit test                  |
| **proptest**                | Parsers, codecs, normalization, invariants, state-machine sequences | No meaningful property or manageable strategy exists      |
| **cargo-mutants**           | Determine whether important behavior is actually asserted           | Treating every surviving mutant as a mandatory new test   |
| **cargo-llvm-cov**          | Find important paths not exercised                                  | Setting arbitrary high percentages as proof of quality    |
| **cargo-nextest**           | Fast repeatable suites, filters, test isolation                     | Assuming it includes doctests                             |
| Sanitizers / Miri / loom    | Unsafe code or nondeterministic concurrency                         | Replacing ordinary regression tests or running by default |
| Benchmarks (e.g. Criterion) | Latency/throughput/allocations are requirements                     | Timing assertions masquerading as unit tests              |

Read these only when relevant: [proptest](references/proptest.md), [async and concurrency](references/async-concurrency.md), [mutations and coverage](references/mutations-and-coverage.md), [test commands and layout](references/tooling.md).

## Project-specific guardrails

- Preserve established project test conventions and approved dependencies. Avoid new mocking frameworks, elaborate test harnesses, and generic helpers for one-off tests. Prefer lightweight fakes or in-memory transports that exercise the real boundary.
- Prefer deterministic control of clocks, RNG seeds, queues, network responses, and scheduling; use temporary directories and OS-assigned ports; clean up tasks and processes.
- Keep important tests runnable in the ordinary local/CI path. Reserve `#[ignore]` for consciously manual, expensive, external-resource, or extended stress tests with a documented run command.
- If a test is flaky, fix synchronization and isolation; automatic retry is diagnostics or temporary mitigation, not a correctness solution.
- Respect the repository's feature flags, supported toolchain/MSRV, operating systems, and user request. Avoid silently escalating a narrow change into a full mutation run or architecture rewrite.
- Never hide a failing check by weakening the assertion, reducing inputs, excluding mutants, or ignoring tests without explaining and justifying it.

## Quick validation guide

```sh
cargo test -p <crate> <test_name>               # Focused test filter
cargo xtest -p <crate>                           # Package suite with the configured runner
cargo test --doc --workspace                       # Nextest does not run doctests
cargo llvm-cov -p <crate>                        # Targeted coverage (optional)
cargo mutants -p <crate> --list                  # Inspect scope before mutation testing
cargo mutants -p <crate> --file 'src/parser.rs'  # Targeted; respects mutants.toml config
```

Commands are examples: replace placeholders with actual package/test names, honor the repository's `.cargo/mutants.toml` configuration (including the test runner), and check installed tool versions. Consult [tooling](references/tooling.md) for exact caveats and scoped workflows.
