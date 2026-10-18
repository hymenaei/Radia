# Property-based testing with proptest

Use `proptest` when the important behavior is an invariant spanning many possible inputs. Generated testing samples inputs and does not prove an invariant for all possibilities. Keep fixed named regression tests for known bugs and exact boundaries; property tests _complement_ them. Confirm that the project permits the dev-dependency (`proptest = "1"` is an example version requirement, not an instruction to change project policy).

## Strong property families

| Family              | Property / independent oracle                                            | Typical usage                                              |
| ------------------- | ------------------------------------------------------------------------ | ---------------------------------------------------------- |
| Round-trip          | `decode(encode(x)) == x` for valid representable `x`                     | Codecs, structured formats; pair with independent fixtures |
| Canonicalization    | `normalize(normalize(x)) == normalize(x)`                                | Paths, identifiers, normalized data                        |
| Inverse / algebraic | `remove(insert(s, x), x) == s` subject to preconditions                  | Collections, indexes, mutable state                        |
| Differential        | Optimized parser and independent reference parser agree                  | High-risk parsers; beware shared code                      |
| Model/state machine | Random command sequences preserve reference-model results and invariants | Connection state, queues, protocols                        |
| Metamorphic         | An allowed input transformation implies a predictable output relation    | Parsers, sorting, search, deduplication                    |
| Robustness          | Arbitrary malformed bytes never panic and respect resource caps          | Untrusted decoding, network boundaries                     |

Do not write tautologies (`x == x`), test standard library functionality, or silently derive the expectation from the same function. A no-panic property alone does not establish correctness.

## Simple property (adapt the function name)

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn canonicalize_is_idempotent(input in "[a-zA-Z0-9 _-]{0,80}") {
        let once = canonicalize(&input);
        let twice = canonicalize(&once);
        prop_assert_eq!(twice, once);
    }
}
```

This snippet assumes the project already defines `canonicalize(&str) -> String` (or a compatible type). `prop_assert!`/`prop_assert_eq!` return test-case failures and produce less noisy shrinking output than ordinary panic assertions.

## Encoding property (adapt to project APIs)

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn frame_roundtrip_preserves_payload(
        payload in proptest::collection::vec(any::<u8>(), 0..2048)
    ) {
        let encoded = encode_frame(&payload);
        let decoded = decode_frame(&encoded).expect("encoded frame must decode");
        prop_assert_eq!(decoded, payload);
    }
}
```

This is a _round-trip example_, not a complete codec specification. Test known-good bytes independently, test frames at the protocol size limit explicitly, and assert correct rejection of truncated/overlong/unknown payloads. Use a bounded generator matching the intended domain; do not accidentally feed multi-megabyte values into every CI case.

## Design effective strategies

- Prefer existing strategies: `any::<u16>()`, `0u32..=MAX`, string regex strategies, `proptest::collection::vec(strategy, length_range)`, `prop_oneof!` for variants, `prop::option::of`, and `prop::sample::select` when useful.
- Give boundary cases explicit weight: empty vectors, maximum lengths, duplicate keys, out-of-order steps, invalid UTF-8, truncated headers. Random generation rarely hits exact boundary values reliably.
- Construct _valid_ values by construction, preferably with a strategy mapping a constrained domain into a valid type. Avoid excessive `prop_assume!` filtering, which can cause many rejections and poor shrinking.
- Use `prop_flat_map` for dependent values only when necessary; favor smaller, simpler shrinks. Avoid deeply nested strategies and global `boxed()` abstractions without concrete need.
- For state machines, generate short command sequences with validity preconditions captured in a simple model. Check the model and the system after each step, including terminal cleanup.
- Bound input sizes and case counts to the contract and runtime budget. Explain deliberate reductions, keep exact boundaries as named cases, and never shrink the domain just to hide a failure.

## Failures, shrinking, and reproducibility

When proptest finds a failure, inspect its minimized counterexample. Under normal std settings, proptest saves regression seeds in a `proptest-regressions` directory associated with the source file; keep useful persisted regressions under version control according to repository policy. Turn a meaningful minimal failure into a clear named deterministic test when it documents a distinct contract or boundary; keep the property as ongoing broader protection.

Seed persistence is configurable and may differ without `std`; do not assume a specific persistence path in all environments. Never "fix" a failed property by shrinking the input domain until it disappears without justifying the excluded domain.

## Frequent pitfalls

- **Circular oracle:** encode+decode may agree on the same wrong interpretation; use fixtures/spec or another implementation.
- **Overbroad input domain:** requirements may exclude some values; encode preconditions in strategies, not speculative `prop_assume!` calls.
- **Overfitting to generators:** real malformed data may contain structured length conflicts or nested fields; generate these intentionally.
- **Weak state-machine checks:** only asserting final output can hide transient violations; check after each event.
- **Time-dependent properties:** avoid real sleeps and unbounded waits in a property test.
- **Making everything a property:** precise boundary and example-based regression tests are usually clearer and cheaper for singular cases.
