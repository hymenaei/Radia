# AGENTS.md

Radia is a Second Life viewer written in Rust. Its goal is to interoperate with the Second Life ecosystem.

Use the legacy viewer as a reference for protocol behavior, content compatibility, and externally visible behavior. Treat its internal abstractions, ownership and threading models, rendering design, workarounds, and hardware constraints as implementation choices. Preserve compatibility requirements, and redesign the implementation wherever compatibility allows. Prefer simple, safe, explicit Rust designs over ports of legacy C++ patterns.

Radia must stay responsive while processing simulator traffic, streaming and decoding assets, updating large scenes, running the UI, and rendering. Keep blocking and latency-sensitive work off render and UI paths. Account for frame time, latency, allocations, copies, synchronization, cache locality, and scheduling on hot paths.

Treat simulator data, assets, and user-created content as untrusted. Handle malformed, unusual, legacy, and high-complexity content without sacrificing stability or compatibility with the Second Life ecosystem.

Choose designs that remain responsive as scene size, network activity, asset volume, and session length grow.

## Guidelines

- Write idiomatic, pragmatic Rust. Prioritize correctness, clarity, simplicity, and maintainability. Follow established Rust and project conventions, using the language's type system and abstractions naturally rather than avoiding or forcing them.
- Prefer straightforward control flow, clear ownership, and cohesive APIs. Choose loops, iterators, concrete types, traits, and generics based on what best expresses the problem. Reuse and improve existing abstractions; introduce new ones only when justified by concrete needs.
- Express invariants through types where practical, making invalid states difficult to represent. Handle errors deliberately, validate untrusted input, minimize visibility, and use `unsafe` only when necessary, with documented safety invariants.
- Avoid speculative generality, premature optimization, unnecessary indirection, excessive abstraction, and avoidable cloning, allocations, or shared ownership. Refactor when there is a concrete benefit, not merely for stylistic preference, and keep changes focused on the task.
- Keep the UI thread responsive and independent of blocking or computationally expensive work. Use clear, non-blocking boundaries between the UI and other subsystems, with bounded queues, backpressure, or update coalescing where appropriate. Avoid blocking waits, lock contention, and excessive per-frame work that could stall input, layout, or rendering.
- Treat simulator data, assets, and user content as untrusted and fallible. Handle malformed and high-complexity input without losing ecosystem compatibility.
- Add dependencies only when they provide a concrete benefit. Prefer existing dependencies and standard-library functionality when appropriate.
- Avoid implementation comments. Use Rust documentation comments (///, //!) for public contracts, invariants, and non-obvious behavior. Allow safety explanations where required.
- Use `$rust-testing` for Rust test design, names, implementation, review, and execution.
- Use the Rust Analyzer MCP for semantic navigation and diagnostics. Use the Cargo checks below to confirm build and lint results.
- Update existing documentation when behavior, APIs, or contracts change. Create new documentation files only when explicitly requested.

## Rustdoc

- Document public APIs with concise summaries and explain contracts that aren't obvious from their types or signatures, such as invariants, units, ordering, ownership, threading, and failure conditions. Avoid redundant documentation.
- Provide brief `//!` overviews for library crates and important modules. Use rustdoc links where helpful.
- Include `# Examples` and `# Errors`, `# Panics`, or `# Safety` sections when they add meaningful information. Prefer runnable doctests.
- Follow official Rust documentation conventions. Keep documentation accurate and focused on what callers need to know, without unnecessary boilerplate.

## Validation

Use Cargo's default `target/` directory. Set `CARGO_TARGET_DIR` or pass `--target-dir` only when the user explicitly requests a custom directory.

Confirm public signatures and documented behavior before coding.

After changing public rustdoc, run `cargo doc --workspace --no-deps` and fix rustdoc warnings and broken links.

Before finishing substantial changes, run the relevant checks:

```sh
cargo fmt --all
cargo check --workspace
cargo lint --workspace
cargo xtest --workspace
```

Do not claim a check passed unless it was actually run.

## Profiling

- Profile representative workloads before optimizing.
- Keep tracing low-noise by default: prefer structured spans and meaningful events over frequent informational logging.
- Instrument coarse subsystem boundaries and meaningful hot operations by default. Add fine-grained or high-frequency instrumentation only when investigating a specific problem.
- Keep Tracy opt-in and prefer feeding existing tracing spans into Tracy rather than maintaining separate instrumentation.
- Keep credentials, session-specific identifiers, capability URLs, and user content out of trace fields. Avoid per-packet, per-frame, and other high-volume events by default.

## Linear workflow

- For work with a Linear issue ID or link, fetch the issue before editing and use its description and comments as task context.
- For a planned feature, bug fix, or multi-step change without an issue, search Linear first. Reuse a matching issue or create one if none exists. Skip issue creation for questions, small edits, and direct follow-ups.
- Move the issue to the team's active status when work starts. Update it when scope, blockers, or important implementation decisions change.
- Use the Linear issue ID in the branch and pull request so the work remains linked to the issue.
- When implementation is ready, comment with a concise summary and the checks run. Move the issue to the appropriate review status when applicable, and only move it to the team's completed status once the work is actually complete or merged.
- Leave the issue open when work is incomplete, blocked, awaiting review, or awaiting user input.
- Use the team's configured status names. If Linear is unavailable, continue the work and report that limitation.

## Agent Instructions

Keep this file high-signal. Add rules only for recurring, non-obvious project conventions or common mistakes. Do not turn `AGENTS.md` into documentation for the entire codebase.

Read `docs/ARCHITECTURE.md` when changing workspace membership, internal crate dependencies, or package ownership.
