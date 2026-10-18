# Test design

## Select cases by risk

Prioritize impact, likelihood, uncovered failure modes, and whether a case can be tested deterministically at reasonable cost. Focus on data loss, compatibility, security, state transitions, concurrency, parsing, cleanup, and reported regressions. A few strong cases are often enough.

| Contract | High-value case | Usually low-value alternative |
| --- | --- | --- |
| Frame parser | Truncated length prefix returns `Incomplete` without consuming state | Every private helper returns a nonempty vector |
| Bounded client | Cancellation releases admission exactly once | Internal reference counts without a contract |
| World/event state | Out-of-order events never resurrect a stale generation | Exact helper invocation counts |
| Serializer | Independent known-good bytes and specified canonical ordering | One happy-path encode/decode round trip |
| Configuration | Precedence, invalid values, and specified unknown-key behavior | Getter equals an internal constant |
| Async retry | Failure, retry, then success without duplicate application | Sleep, then check that a log was written |

Test private code when a complex local invariant cannot be verified faithfully through a public behavior. Prefer the external contract when it gives the same confidence.

## Place and group tests

- Put module-local invariants in `#[cfg(test)] mod tests`. Use a sibling `*_tests.rs` only when it makes a cohesive private suite easier to navigate while retaining the module's private access.
- Put public API and subsystem contracts in `tests/*.rs`, grouped by behavior. Each top-level integration-test file is a separate test crate, so make shared support explicit and split only when the navigation or isolation benefit justifies another target.
- Merge cases when they exercise the same contract through the same setup and operation, and one failure diagnosis remains clear. Boundary variants of one invariant or linked outcomes such as error, unchanged state, and recovery often belong together.
- Keep cases separate when they exercise independent contracts, require materially different setup, or need distinct failure names. Use a table or loop when cases vary one dimension; include the case identity in assertion output. Do not combine unrelated checks to reduce test count or LOC.
- Split files when navigation, ownership, or setup becomes clearer, not at a line-count threshold. A large cohesive file can be easier to maintain than many tiny targets.

## Name the behavior

Use a short `snake_case` name that makes the failure understandable. Let the module path provide context, and follow the surrounding crate's prefix convention. Omit a redundant `test_` prefix when that is the local style; keep it when established by the suite. Add the condition and outcome only when they distinguish the case.

```rust
fn truncated_length_prefix_returns_incomplete() { /* ... */ }
fn unknown_message_type_preserves_unread_bytes() { /* ... */ }
fn canceling_a_request_releases_admission() { /* ... */ }
```

Avoid names such as `it_works`, `test_parser`, issue IDs alone, or long setup narratives. For property tests, name the invariant (`canonicalize_is_idempotent`), not the generator. In parameterized tests, include the case name or input in failure output.

## Keep fixtures and helpers trustworthy

- Use inline values for small cases. Store large, reused, or compatibility inputs in a clear fixture directory with descriptive names.
- Record provenance when a fixture represents an external format or oracle. Keep expected data independent: specification examples, hand-checked vectors, captured compatible output, or a simpler model. Do not generate the expected value with the implementation under test.
- For an expensive external oracle, check reviewed reference outputs in the ordinary suite and put regeneration behind a separate, explicit command. Keep fixture updates reviewable.
- Keep support code at the narrowest useful scope. Start with a private helper or crate-local `tests/support`; use a test-support crate only for domain-specific code shared across packages. A reusable test context is worthwhile when it composes realistic setup used repeatedly, not as a generic framework.
- Keep Arrange and Act visible at the call site. Helpers should remove incidental setup, not hide what behavior is exercised or what result is asserted.

## Assert observable outcomes

- Compare actual values with `assert_eq!` when it gives better diagnostics than a boolean. Inspect the relevant error variant or fields when the error contract matters.
- Assert the value or state required by the contract; `is_ok()` and no-panic checks alone are incomplete when more is observable.
- Assert state, side effects, ordering, and cleanup only when they are part of the contract. For lifecycle behavior, verify the next operation after cleanup as well as the immediate result.
- Give looped or parameterized assertions enough case context to identify the failing input. For long text or structured output, use existing diff-friendly helpers when available; avoid adding a dependency solely to change assertion formatting.
- Assert private call counts, allocations, layouts, logging, ordering, and timing only when the contract requires them. Use benchmarks to measure performance.
- Use snapshots only for stable, externally meaningful output. Avoid pinning incidental ordering or internal serialization.
- A round trip alone can miss matching encoder and decoder bugs. Pair it with an independent fixture, compatible implementation, or semantic invariant.
- Use `#[should_panic]` only for an intentional panic contract. Fallible APIs and untrusted input should normally assert a structured error.

## Retain useful regressions

Reduce a reported bug to a deterministic example, name the violated contract, and keep the case after the fix. Confirm it fails before the fix when practical. Don't encode only the issue number, a private call sequence, or an incidental implementation detail.

For suite maintenance, remove duplicate cases without distinct risk, arbitrary sleeps, dependence on test order or shared external state, and checks for compiler behavior or trivial delegation. Keep meaningful boundaries and full lifecycles even when the suite is small. Use deterministic clocks, seeded randomness, bounded synchronization, temporary resources, and explicit cleanup where needed.

Before keeping a test, ask: Could it pass with the product wrong? Could it fail after a behavior-preserving refactor? If it catches a regression, will the failure say which contract broke?
