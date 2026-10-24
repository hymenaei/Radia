# Property tests

Use for invariants over many inputs. Sampling is not proof, so keep named tests for known bugs and exact boundaries. Confirm the project already depends on proptest.

| Family                | Property or oracle                                                |
| --------------------- | ----------------------------------------------------------------- |
| Round trip            | `decode(encode(x)) == x`, paired with an independent oracle       |
| Canonicalization      | `normalize(normalize(x)) == normalize(x)`                         |
| Inverse               | `remove(insert(s, x), x) == s` under preconditions                |
| Differential          | Optimized and independent reference agree (watch for shared code) |
| Model / state machine | Random command sequences match a reference model after every step |
| Metamorphic           | An allowed input transform implies a predictable output relation  |
| Robustness            | Arbitrary malformed bytes never panic and respect resource caps   |

## Writing

- Use `prop_assert!` and `prop_assert_eq!` for clean shrinking.
- Prefer built-in strategies: ranges, regex strings, `collection::vec`, `prop_oneof!`, `prop::option::of`, `prop::sample::select`.
- Weight boundaries explicitly: empty, max length, duplicate keys, out-of-order steps, invalid UTF-8, truncated headers.
- Build valid values with `prop_map`. `prop_flat_map` only for dependent values. Filtering with `prop_assume!` wastes cases and hides narrow domains.
- Bound sizes and case counts to the CI budget while keeping the full domain.

## Counterexamples

Turn each meaningful minimized counterexample into a named deterministic test. Persist `proptest-regressions` files only when repository policy tracks them.

## Pitfalls

- Circular oracles: the property reuses the logic under test.
- Overbroad domains that bury the interesting inputs.
- Generators that never produce structured malformed data (conflicting lengths, nested fields).
- State machines that check only the final state.
- Real sleeps.
- Singular cases forced into properties.
