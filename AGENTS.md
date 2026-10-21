Radia is a Second Life viewer written in Rust. It is a redesign, not a port. It must interoperate fully with the Second Life ecosystem while being simpler, safer, and faster than the legacy viewer.

The legacy viewer is a reference for protocol behavior, content compatibility, and externally visible behavior only. Its abstractions, ownership and threading models, rendering design, and workarounds are not requirements. Keep the compatibility requirement a pattern serves and redesign the rest. Do not port C++ patterns.

## Non-negotiables

1. **Responsive**. Radia handles simulator traffic, streams assets, updates scenes, runs the gui, and renders without any workload starving the others. Budget latency, frame time, allocations, copies, and synchronization. Design for long sessions and large scenes.
2. **Hostile input**. Simulator data, assets, and user-created content are untrusted. Malformed, unusual, and very complex content must not crash, stall, or exhaust the memory of the viewer. Valid but strange legacy content must still work.
3. **Simple**. The smallest model that makes correct behavior unsurprising wins.

## Working style

- Do not preserve complexity because it exists. Do not add machinery because it looks impressive.
- Measure twice, cut once. YAGNI. Fight scope creep. Do what was asked, minimally and realistically, and add nothing creative.
- Put code in existing files. Create a file only for a new logical component.
- These are defaults. The developer's instructions override them. If a rule fights the task, say so and get sign-off before breaking it.

## Code

- Correctness and clarity first, then the budgets above.
- Add dependencies only for a concrete benefit; prefer existing ones and std.
- Minimize visibility. Use `unsafe` only when necessary.
- Propagate with `?`. `unwrap`, `expect`, indexing, slicing, and unchecked arithmetic are for invariants you can prove, never for wire, asset, or user data.
- Handle every fallible result. `let _ =` on a `Result` hides a failure; propagate it, handle it, or log it where it is handled.
- Newtypes for easily confused IDs and values with domain rules; counts and sizes stay primitive.
- Enums for exclusive states, with data in the variants. Type-state only for fixed lifecycles.
- `Option<T>` for absence, `Result<Option<T>, E>` for absence vs failure.
- Typed domain errors. Log where handled, not at every propagation layer. Stale updates and deferred recovery are outcomes, not errors. Failures that affect the user reach the gui as typed outcomes it can show.
- World mutations: validate and plan before committing. Never leave state or indexes inconsistent on failure.
- Packet blocks: validate framing, shared context, and ordering first. Isolate independent failures, keep earlier valid changes, report partial outcomes, and stop when a failure invalidates later blocks.
- Choose storage by access and update patterns. Keep layout and index invariants private.

## Concurrency

- One owner for authoritative mutable state, including `World`. Cross threads with immutable snapshots or change streams, not broad locks.
- No blocking or expensive work on gui or render paths. Redraw on change, with no continuously repainting animations.
- Bound queues and in-flight work. Apply backpressure, cancellation, coalescing, and prioritization while preserving protocol ordering and acks.
- Add subsystem schedulers or budgets only when a workload justifies them; no global scheduling infrastructure.
- Scope clones by shadowing inside the spawn block (`let ctx = ctx.clone();`) so handles drop with the task.

## Cover the whole path

Before calling work done, say which of these applied:

- **Update paths**. Full, compressed, cached, and terse object updates reach the same `World` invariants.
- **Reverse transitions**. Every way in has a way out and a way to observe it: kill, derez, unlink, detach, region leave, cache eviction.
- **Regions**. Root and neighbor regions behave differently, and so do crossing and teardown.
- **Scale**. Exercise the large scene and the long session, not one object.
- **Docs**. Update the doc that owns the fact in the same change.

## Naming

- Concise names; no redundant qualifiers in public APIs. Short locals are fine when context is clear (`pkt`, `req`, `obj`, `ctx`, `r`, `e`, `d`).
- Import items with `use`, even single-use ones. Qualify through the parent module only on ambiguity (`fmt::Result`); never rename with `as`. `use Trait as _` is allowed.
- Use `super` at most once in a path. Never write `super::super`; use `crate::` for anything above the parent module.
- Import enum variants by name only if distinctive and not a prelude name. No glob imports except `use super::*` in tests.
- `Udp` is the generic transport; `LLUdp` is Second Life's protocol. Keep wire-template and external spellings exact.

## Comments

No comments or doc comments in source, without exception. If code needs one, refactor names, types, or structure instead. Remove existing comments from code you touch.

## Testing

- Use `$rust-testing` for all Rust test work.
- Smallest proof first. Run tests and clippy for the crates you touched. Run the workspace-wide suite only when asked, because CI owns it.
- Test behavior through public APIs. Skip tests that assert callback wiring or mirror the implementation.
- Use property-based or stateful tests where operations interact (reordering, reparenting, stale updates, ID reuse). Generators must be domain-valid and failures reproducible.
- Wait on completion signals, never on sleeps or polling. A test that needs a timeout to pass is wrong.
- Ask before launching the gui viewer, driving a browser, or any computer use.

## Versioning

- SemVer with a single shared workspace version; keep `Cargo.lock` in sync.
- Before finalizing a PR, decide whether it is a release boundary. If so, bump the version and explain it in the PR description. No bumps for routine merges or internal refactors.
- No git tags, publishing, or GitHub releases without approval.

## Documentation

- `docs/vision.md`: product direction and architectural principles. Follow when making architectural and product decisions.
- `docs/architecture.md`: high-level architecture, subsystem responsibilities, state ownership, data flow, crate boundaries, and dependencies.
- `docs/reference.md`: guidance for consulting the official Second Life viewer, including which systems to study, which to reference only for behavior, and which implementations to ignore.
- `docs/tooling.md`: commands, testing, fuzzing, benchmarks, profiling, analysis.

Durable architecture and decisions go in these docs. Do not describe crate layout or data flow in this file.

## Keeping this file sharp

- A rule earns its place only if it is non-obvious, came up more than once, and is concrete enough to act on.
- Write traps to avoid, not maps to follow.
