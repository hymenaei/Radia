---
name: rust-testing
description: Plan, write, review, and run reliable Rust tests. Use for test selection, organization, naming, fixtures, assertions, regression cases, property and async tests, and Cargo test tools.
---

# Rust Testing

Add or change a test only when it adds distinct evidence for an observable contract, invariant, boundary, or plausible regression, or materially improves failure diagnosis. Do not add tests to increase LOC or test count, or to chase coverage or mutation scores. Follow the project's layout and tools; do not add dependencies or change production code solely for test convenience. For planning or review-only requests, inspect and report without editing or running tests. For implementation requests, run checks requested by the user or required by repository policy.

## Operating workflow

1. **Inspect the contract.** Read the behavior, callers, existing tests, fixtures, manifests, and project test commands. Identify what must remain true and what is already covered.
2. **Choose distinct evidence.** Name the plausible failure a new case would catch. Combine it with an existing test when contract, setup, and diagnosis stay clear; otherwise keep it separate only if it adds meaningful protection or diagnosis.
3. **Choose a faithful boundary.** Use unit tests for local invariants, integration tests for public contracts and interactions, and doctests for instructive public examples. Use property or stateful tests for broad invariants. For UI flows, drive real actions through the existing app test context and inspect resulting state.
4. **Keep failures legible.** Use minimal setup, meaningful inputs, and expected outcomes independent of the code under test. Share helpers only when they remove repeated domain setup without hiding the action or assertion.
5. **Validate in proportion.** Run focused tests first, then broader checks as required by risk and repository policy. With Nextest, run doctests separately. Report what ran and whether each result passed.

For case selection, merging and splitting, placement, naming, fixtures, assertions, and regression tests, read [test design](references/test-design.md).

## Specialized guidance

- For generated cases and shrinking, read [property tests](references/proptest.md).
- For async ordering, cancellation, and cleanup, read [async and concurrency](references/async-concurrency.md).
- For interpreting coverage or mutation results, read [mutation testing and coverage](references/mutations-and-coverage.md).
- For Radia commands and platform-specific tooling, follow the canonical [tooling guide](../../../docs/tooling.md).

## Project guardrails

- Preserve established project test conventions and approved dependencies. Avoid new mocking frameworks, elaborate test harnesses, and generic helpers for one-off tests. Prefer lightweight fakes or in-memory transports that exercise the real boundary.
- Prefer deterministic control of clocks, RNG seeds, queues, network responses, and scheduling; use temporary directories and OS-assigned ports; clean up tasks and processes.
- Keep important tests runnable in the ordinary local/CI path. Reserve `#[ignore]` for consciously manual, expensive, external-resource, or extended stress tests with a documented run command.
- If a test is flaky, fix synchronization and isolation; automatic retry is diagnostics or temporary mitigation, not a correctness solution.
- Respect the repository's feature flags, supported toolchain/MSRV, operating systems, and user request. Avoid silently escalating a narrow change into a full mutation run or architecture rewrite.
- In Radia, use Cargo's default `target/` directory, `crates/common/testing` for independent LLUDP vectors shared across packages, and package-local `tests/support` for integration helpers. Follow the testing crate README for fixture provenance and capture handling.
- Never hide a failing check by weakening assertions, dropping failing cases that are in scope, excluding relevant mutants, or ignoring the test without justification.
