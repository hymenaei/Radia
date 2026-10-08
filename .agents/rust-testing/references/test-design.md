# Test selection, naming, and review

## Priority: protect behavior rather than implementation

Rank candidate tests using: (1) impact of breakage (data loss, security, compatibility, uptime), (2) likelihood of defect (complexity, churn, concurrency, parsing), (3) distinct failure modes not already covered, (4) confidence that the test will be deterministic and cheap, and (5) user-visible/API-contract consequences. One high-signal case often beats ten trivial tests.

| Contract            | High-value case                                                       | Usually low-value alternative                                   |
| ------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------- |
| Frame parser        | Truncated length prefix returns `Incomplete` without consuming state  | Every private helper returns a nonempty vector                  |
| Bounded HTTP client | Canceling an in-flight request releases admission exactly once        | Assert an internal `Arc` strong count with no contract          |
| World/event state   | Out-of-order connect/disconnect never resurrects stale generation     | Exact number of helper invocations                              |
| Serializer          | Golden known-good bytes, unknown fields, canonical order if specified | Serialize and immediately deserialize only one happy-path value |
| Configuration       | Precedence, missing/invalid fields, unknown keys if specified         | Assert default getter equals internal constant                  |
| Async retry         | Failure → retry → success without duplicate application               | `sleep(100ms)` then assert some log was written                 |
| Cache               | Correctness on hit, miss, invalidation; allocations only if a target  | Internal key layout after every change                          |

Testing private code is legitimate when it is a complex invariant that is difficult to expose faithfully through a public API. However, an external behavior test usually survives more refactoring.

## Case selection worksheet (do not turn into a large checklist for small changes)

- Given normal inputs, what externally observable state/output must follow?
- What are the exact boundary values, and one step beyond each boundary?
- How does invalid or malicious input fail (type, message if contractual, resource budget, no panic)?
- What changes when a task is canceled, races another task, or completes late?
- What remains invariant after repeated operations and recovery?
- Which existing test already provides equivalent coverage? Can it be extended rather than duplicated?
- Is the expected value independent of the implementation (spec, handwritten fixture, known reference, simpler model, algebraic invariant)?

## Test naming

Use the shortest name that uniquely communicates the behavior being tested. Prefer `condition_and_outcome` or `operation_when_condition_expected_outcome` only when that context helps. Module names supply context. Good names reveal _which behavior failed_:

```rust
#[test]
fn empty_payload_roundtrips() { /* ... */ }

#[test]
fn truncated_length_prefix_returns_incomplete() { /* ... */ }

#[test]
fn unknown_message_type_preserves_unread_bytes() { /* ... */ }

#[test]
fn release_called_twice_does_not_double_free() { /* ... */ }
```

Less useful:

```rust
#[test]
fn test_parser() { /* ambiguous */ }

#[test]
fn test_bug_158() { /* no behavior */ }

#[test]
fn test_process_packet_should_work_properly_in_the_case_of_some_error() { /* vague and verbose */ }
```

Property tests name the _property_, not the generator: `encode_decode_preserves_payload`, `canonicalize_is_idempotent`, `discarding_an_item_never_increases_queue_length`.

## Concrete assertion patterns

**Do:** assert the meaningful error variant, preserving test failure diagnostics.

```rust
let err = decode(&[0x80]).expect_err("one-byte header is truncated");
assert!(matches!(&err, DecodeError::Incomplete { .. }), "{err:?}");
```

**Do:** verify cleanup and a later operation, not only the first return value: `cancel -> permits restored -> subsequent request admitted`.

**Do:** verify byte-level golden fixtures for wire formats only when bytes and ordering matter; check fixture provenance and update deliberately when formats change.

**Don't:** reconstruct expected results using the same parser/serializer being tested, assert a call count without a behavioral reason, or duplicate the code algorithm in a test so both can share the same bug.

A round-trip property is valuable, but two mutually broken encoder/decoder implementations can still satisfy it. Pair round-trips with independent fixtures, an external compatible implementation, or semantic checks.

## Avoiding weak tests

- `assert!(result.is_ok())` is weak when correctness of value/state is important; assert that too.
- Snapshot tests are useful for stable output contracts, not arbitrary internal serialization.
- Only test panic when panic is intentional. Untrusted input usually deserves a structured error, not `#[should_panic]`.
- Avoid exact elapsed-time checks in unit tests. For timing requirements, use simulated time; for performance, use controlled benchmarks and distributions.
- Don't build an abstraction just to make an already accessible unit test possible. If testability reveals a genuine design boundary, refactoring can be worthwhile, but keep it focused.
- Don't force a test for a getter, delegation, enum discriminant, compiler property, or generated glue absent a real failure mode.
- Don't interpret "few tests" as permission to leave high-impact edge cases or complete lifecycles untested.

## Final self-review

Could this test pass when the product is wrong? Could it fail during a behavior-preserving refactor? Does it depend on wall time, execution order, or external state? If a bug recurs, will the failure identify the broken contract? Delete or strengthen tests that answer these badly.
