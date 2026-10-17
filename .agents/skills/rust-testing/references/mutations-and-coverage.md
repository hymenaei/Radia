# Mutation testing and coverage

Coverage shows which code ran. Mutation testing asks whether tests detect selected changes to that code. Neither measures test purpose or correctness on its own.

Use coverage to find high-risk branches and contracts that lack an exercise. Use mutation testing when a plausible fault is hard to judge by inspection, starting with the affected function or package. Neither warrants a new test for every change or a target score.

When a mutation survives, inspect it. Add or strengthen a test only if it exposes an observable contract that the suite does not protect. Ignore behaviorally equivalent mutations with a narrow, documented reason. A compile failure is unviable, and a timeout needs investigation rather than automatic credit as a caught fault.

In Radia, consult [coverage commands](../../../../docs/tooling.md#test-coverage) and the repository's `.cargo/mutants.toml`. Keep runs focused for routine work. Nextest does not run doctests, so results can miss behavior covered only by doctests.
