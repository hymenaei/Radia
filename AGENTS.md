Radia is a Second Life viewer written in Rust. Its goal is to interoperate with the Second Life ecosystem.

Use the legacy viewer as a reference for protocol behavior, content compatibility, and externally visible behavior. Treat its internal abstractions, ownership and threading models, rendering design, workarounds, and hardware constraints as implementation choices. Preserve compatibility requirements, and redesign the implementation wherever compatibility allows. Prefer simple, safe, explicit Rust designs over ports of legacy C++ patterns.

Radia must stay responsive while processing simulator traffic, streaming assets, updating scenes, running the UI, and rendering. Account for latency, frame time, allocations, copies, synchronization, cache locality, and scheduling as workloads and sessions grow.

Treat simulator data, assets, and user-created content as untrusted. Handle malformed, unusual, legacy, and high-complexity content without sacrificing stability or ecosystem compatibility.

## Guidelines

- Write idiomatic, pragmatic Rust. Prioritize correctness, clarity, simplicity, and maintainability. Follow Rust and project conventions without forcing or avoiding language features.
- Prefer clear ownership, straightforward control flow, and cohesive responsibilities. Avoid speculative generality, unnecessary indirection, and abstractions without a concrete benefit.
- Express meaningful invariants through types where practical. Validate untrusted input, minimize visibility, and use `unsafe` only when necessary, with documented safety invariants.
- Add dependencies only when they provide a concrete benefit. Prefer existing dependencies and standard-library functionality when appropriate.
- Write labels and messages in plain words with standard punctuation. Do not use decorative symbols as text; use words or icon components instead.

## Naming

- Prefer concise, natural names that remain clear in context. Avoid redundant qualifiers and repeated context, particularly in public APIs.
- Use short locals and parameters when context makes them clear: `pkt`, `req`, `obj`, `ctx`, `r`, `e`, and `d` are fine when unambiguous. Match name specificity to scope and usage.
- Use familiar or contextually clear abbreviations, not obscure shorthand. Keep meanings consistent within a function or module; never expand names solely for verbosity.
- Import types, traits, functions, constants, and enum variants with `use` rather than writing fully qualified paths at use sites, including single-use items.
- Keep a module-qualified path only when the bare name would be ambiguous or collide with another import (e.g. `fmt::Result` vs `io::Result`). Do not rename imports with `as` to avoid a collision; qualify through the parent module instead.
- `use Trait as _` is allowed for trait-only imports.
- Import enum variants by name only when the name is distinctive and does not match a prelude item (`Some`, `None`, `Ok`, `Err`). Otherwise keep `Enum::Variant`. Do not glob-import variants, except `use super::*` in test modules.
- In Rust identifiers and diagnostics, use `Udp` for generic transport and `LLUdp` for Second Life's protocol. Preserve exact external and wire-template spellings.

## API and Data Modeling

- Keep APIs semantically consistent without forcing identical signatures. Prefer direct arguments; use operation structs for meaningful grouped data or invariants. Use methods for behavior belonging to a type and free functions for independent operations.
- Prefer concrete types and inherent methods. Add traits or generics for useful contracts, real boundaries, or justified reuse—not hypothetical flexibility.
- Use newtypes for easily confused identities or values with domain rules; keep ordinary counts and sizes primitive. Use enums for exclusive states, with state-specific data in their variants, and separate fields for facts that can coexist. Use type-state only when a fixed lifecycle benefits from compile-time checks.
- Match return types to caller needs: `Option<T>` for ordinary absence, `Result<Option<T>, E>` for distinct absence and failure, and domain enums for meaningful alternatives. Put outcome-specific data in its variant; use structured results when effects can coexist. Avoid redundant wrappers.
- Expose useful domain APIs while keeping storage and invariant-sensitive state private. Prefer public fields for plain data, meaningful accessors for encapsulated state, and lightweight borrowed views where useful. Avoid getters that only mirror fields.
- Choose storage by real access and update patterns, not entity counts alone. Encapsulate indexing and alignment invariants; keep consumers independent of layout.

## Control Flow and State Changes

- Prefer explicit loops and `match` for mutations and meaningful branches, `?` for propagation, and iterators or combinators for simple transformations. Avoid chains that hide decisions.
- Keep domain errors typed and add reporting context at application boundaries. Represent expected states, such as stale updates or deferred recovery, as outcomes. Log where errors are handled, not at every propagation layer; avoid noisy per-block warnings.
- Validate framing, shared context, and ordering before applying blocks. Isolate independent failures, preserve earlier valid changes, and report partial outcomes. Stop when a failure invalidates later processing.
- Preserve invariants per world mutation: validate and plan dependent changes before committing, without leaving state or indexes inconsistent on failure. Use rollback only when simpler steps cannot guarantee correctness.

## Ownership and Concurrency

- Give authoritative mutable state, including `World`, one clear owner. Borrow locally; use immutable snapshots or change streams across thread boundaries. Avoid broad shared locks; keep necessary synchronization narrow and bounded.
- Keep blocking or expensive work off UI and render paths. Bound queues and in-flight work; apply backpressure, safe cancellation, coalescing, and prioritization while preserving protocol ordering and acknowledgments.
- Add subsystem-specific schedulers or resource budgets when workloads justify them; avoid speculative global scheduling infrastructure.

## Organization and Refactoring

- Keep cohesive logic together. Extract helpers for real operations, important rules, or useful reuse, even with one caller. Do not split functions merely to reduce line count.
- Split modules by cohesive responsibility when useful, not by line count. Avoid excessive nesting or a file per type or operation.
- Refactor nearby code when it helps the task. Reconsider architecture when a concrete problem exposes a simpler, safer design. Keep unrelated cleanup separate; explain architectural, public API, or schema changes before implementing.
- Keep types in their natural owning crates. Extract narrowly shared concepts only across real dependency boundaries. Avoid dependency cycles and catch-all crates; map distinct domain identities in application adapters.

## Testing

- Use `$rust-testing` for Rust test design, names, implementation, review, execution, coverage analysis, and test-suite audits. Favor behavior-focused tests through meaningful APIs, asserting observable results and invariants; use focused unit tests where needed.
- Use property-based or stateful sequences when interacting operations threaten invariants, such as reordering, reparenting, stale updates, and ID reuse. Keep generators domain-valid and failures reproducible; retain important regressions.

## Rustdoc

- Prefer clear code, rustdoc, and maintained documentation over implementation comments. Add comments only for critical local constraints that cannot be made clear otherwise, and document `unsafe` safety invariants.
- Document non-obvious public contracts, including units, ordering, ownership, threading, invariants, and failures. Skip boilerplate for obvious fields, getters, and variants.
- Provide brief `//!` overviews for library crates and important modules. Use rustdoc links, runnable `# Examples`, and `# Errors`, `# Panics`, or `# Safety` sections when useful.
- Follow official Rustdoc conventions. Update existing docs when behavior or contracts change; create new documentation files only when explicitly requested.

## Validation

Must use Cargo's default `target/` directory. Set `CARGO_TARGET_DIR` or pass `--target-dir` only when the user explicitly requests a custom directory.

Use existing project paths and tool defaults for validation. Create task-specific scratch directories or copied inputs only when explicitly requested.

Use the Rust Analyzer MCP for semantic navigation and diagnostics. Confirm public signatures and documented behavior before coding.

After changing public rustdoc, run `cargo doc --workspace --no-deps` and fix rustdoc warnings and broken links.

Before finishing substantial changes, run the relevant checks:

```sh
cargo fmt --all
cargo check --workspace
cargo lint
cargo xtest --workspace
```

Do not claim a check passed unless it was actually run.

## Profiling

- Treat hot-path budgets as design constraints. Avoid obvious overhead when equally clear alternatives exist; profile representative workloads and justify complex optimizations with measurements or explicit requirements.
- Keep tracing low-noise by default: prefer structured spans and meaningful events over frequent informational logging.
- Instrument coarse subsystem boundaries and meaningful hot operations by default. Add fine-grained or high-frequency instrumentation only when investigating a specific problem.
- Keep Tracy opt-in and prefer feeding existing tracing spans into Tracy rather than maintaining separate instrumentation.
- Keep credentials, session-specific identifiers, capability URLs, and user content out of trace fields. Avoid per-packet, per-frame, and other high-volume events by default.

## Versioning

- Follow Semantic Versioning (SemVer).
- Before finalizing each PR, evaluate whether it completes a release-worthy milestone or otherwise establishes a release boundary.
- At release boundaries, autonomously determine the appropriate version bump from changes since the previous release.
- Do not bump versions for routine merges, individual issues, or internal refactors unless they constitute a release.
- Maintain the shared Cargo workspace version and update `Cargo.lock` consistently.
- Include version changes in the relevant PR, verify the workspace, and explain the bump in the PR description.
- Do not create Git tags, publish packages, or create GitHub releases without explicit approval.

## Agent Instructions

Keep this file high-signal. Add rules only for recurring, non-obvious project conventions or common mistakes. Do not turn `AGENTS.md` into documentation for the entire codebase.

- Read `docs/CONTEXT.md` when work touches Second Life terminology, simulator lifecycle, entity identity, or protocol and event ordering.
- Read `docs/ARCHITECTURE.md` when changing workspace membership, internal crate dependencies, or package ownership.
- Consult `docs/TOOLING.md` for development commands, testing, fuzzing, benchmarks, profiling, and code analysis.
