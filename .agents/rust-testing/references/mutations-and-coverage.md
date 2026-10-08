# Mutation testing and coverage: interpret the evidence

Mutation testing helps answer: _would our tests catch a plausible fault?_ Coverage helps answer: _which code paths did our suite execute?_ Neither is a substitute for test design.

## cargo-mutants workflow

Run on a clean, deterministic baseline; flaky tests make results unreliable. Don't run the entire workspace by default after a small change.

```sh
cargo xtest -p protocol
cargo mutants -p protocol --list
cargo mutants -p protocol --file 'src/parser.rs'  # Uses configured test runner
```

Use `--re` to focus on a function or family of mutations, and `--exclude-re` for justified exclusions. `--list` shows candidates without execution. Mutation runs can be costly: start with high-risk files/functions, then optionally expand as a scheduled CI or audit task. Re-run the affected scope after adding tests.

### Outcomes

- **Caught:** at least one test failed for the mutation; useful evidence, not proof of completeness.
- **Missed:** tests passed; inspect whether changed behavior is distinguishable under the public contract. Add a meaningful assertion if the mutation exposes a genuine gap.
- **Unviable:** mutated source did not compile; not evidence about test quality.
- **Timeout:** altered program hung or exceeded the budget; investigate as a possible fault, not automatically a 'caught' mutant.

When a mutation survives, read the diff and ask:

1. Is the mutated behavior observably incorrect or incompatible with a real requirement?
2. Is it already tested but the assertion too weak, or is the scenario missing?
3. Which smallest behavior-oriented test would catch the fault _and_ survive a refactor?
4. Is the mutation behaviorally equivalent or unimportant (e.g. incidental debug output)? Document a targeted exclusion where appropriate.

Do not write bizarre tests that assert private statements specifically to kill one mutant. Do not use a mutation score as a blind mandatory gate. An uncaught `return Ok(())` in a critical write path matters more than several equivalent formatting mutations.

### cargo-mutants + Nextest caveat

First inspect and honor the project's existing `.cargo/mutants.toml` (or explicitly selected config). If it specifies `test_tool = "nextest"`, **do not redundantly pass `--test-tool`**. Nextest doesn't run doctests; mutation results for behaviors tested **only** through doctests can be misleading. Choose `cargo test` for mutation testing when doctests are important, or add equivalent executable tests outside doctests. Nextest can be faster or slower depending on the suite: benchmark the actual project rather than presuming it is always faster.

## cargo-llvm-cov workflow

```sh
cargo llvm-cov -p protocol
cargo llvm-cov nextest --workspace  # Optional: with Nextest installed
```

Look at uncovered error branches, critical state transitions, and integration seams. Coverage counts execution, not correctness. Don't write valueless tests for branch coverage alone. Remember doctest coverage has special restrictions and may not be included by default in `cargo-llvm-cov`; read current tool documentation before interpreting a number.

## Suggested cadence

- Every change: focused unit/integration regression tests.
- Before merging a risky change: affected-crate suites, supported feature configurations, and relevant platform checks.
- Occasionally / after large parser, protocol, or state-machine changes: targeted property and mutation runs, optional coverage review.
- Scheduled CI or manual audit: wider mutation, sanitizer, soak, or platform matrix checks when justified by risk and runtime.

For Radia protocol, network, LLSD, and capture changes, use targeted mutation testing when ordinary tests may miss edge cases. Prefer affected packages and reserve workspace-wide runs for deeper reviews.

No unsupported tool should be treated as a required dependency. State clearly when mutation/coverage wasn't run.
