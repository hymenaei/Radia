Radia is a Second Life viewer written in Rust. It is a redesign, not a port. It must interoperate fully with the Second Life ecosystem while being simpler, safer, and faster than the legacy viewer.

The legacy viewer is a reference for protocol behavior, content compatibility, and externally visible behavior only. Its abstractions, ownership and threading models, rendering design, and workarounds are not requirements. Keep the compatibility requirement a pattern serves and redesign the rest. Do not port C++ patterns.

## Non-negotiables

1. **Responsive**. Radia handles simulator traffic, streams assets, updates scenes, runs the gui, and renders without any workload starving the others. Budget latency, frame time, allocations, copies, and synchronization. Design for long sessions and large scenes.
2. **Hostile input**. Simulator data, assets, and user-created content are untrusted. Malformed, unusual, and very complex content must not crash, stall, or exhaust the memory of the viewer. Valid but strange legacy content must still work.
3. **Simple**. The smallest model that makes correct behavior unsurprising wins.

## Working style

- Do not preserve complexity because it exists. Do not add machinery because it looks impressive.
- Measure twice, cut once. YAGNI. Fight scope creep. Do what was asked, minimally and realistically, and add nothing creative.
- Reuse before writing. Search the workspace for an existing implementation and extend it instead of adding a parallel one.
- Replace, do not layer. Delete what your change makes dead in the same change, shims for vanished callers included.
- Use available tools for requested work. Do not install, update, add, remove, or configure tools without an explicit request.
- Do not change toolchain settings, tool-only dependencies, build, test, lint, formatting, fuzzing, benchmarking, or profiling scripts, hooks, or CI workflows without an explicit request. Commands that change these count as tooling changes.
- When a requested change alters a documented command or workflow, update [tooling.md](docs/tooling.md). If a tool is missing or broken, report it and continue with independent work.

## Structure

- Before writing code, determine which concept owns it. Restructure first when the existing organization would obstruct the feature.
- Put reusable geometry and numeric primitives in math, and shared utility functions, types, helpers, and traits in utils. Keep subsystem policy with its owning crate.
- Keep each file focused on one cohesive responsibility. Split distinct responsibilities when justified, but keep small, closely related functionality together. Avoid unnecessary fragmentation.
- Group related functionality by concept. Keep shared types and coordination with the concept they serve. Give a substantial, distinct responsibility its own focused module when that clarifies ownership.
- Keep sibling modules independent where practical. Place genuinely shared functionality in mod.rs or a dedicated module rather than coupling siblings.
- Adding a variant should require minimal changes outside its module. Use enums for closed sets and traits for open sets or meaningful abstraction boundaries.
- Order Rust files as follows: module docs, inner attributes, module declarations (`mod`/`pub mod`), imports, re-exports (`pub use`), constants and statics, type aliases, types with their impls immediately following, free functions, then tests. Group imports as std, external and workspace crates, then crate/self/super together. Keep tests last, including test module declarations.
- A module with submodules is a directory with `mod.rs`; never place `foo.rs` beside `foo/`.
- Keep unit tests inline in the module they test. Move them to a sibling `<name>_tests.rs` only when they obscure the implementation.
- Each crate has one bench target: `benches/main.rs` declares every bench as a module, with a single `[[bench]] name = "main"` entry. Select benches with criterion filters, not extra targets.

## Code

- Add dependencies only for a concrete benefit; prefer existing ones and std.
- Minimize visibility. Use `unsafe` only when necessary.
- Propagate with `?`. `unwrap`, `expect`, indexing, slicing, and unchecked arithmetic are for invariants you can prove, never for wire, asset, or user data.
- Handle every fallible result. `let _ =` on a `Result` hides a failure; propagate it, handle it, or log it where it is handled.
- Newtypes for easily confused IDs and values with domain rules; counts and sizes stay primitive.
- Enums for exclusive states, with data in the variants. Type-state only for fixed lifecycles.
- `Option<T>` for absence, `Result<Option<T>, E>` for absence vs failure.
- Typed domain errors. Log where handled, not at every propagation layer. Stale updates and deferred recovery are outcomes, not errors. Failures that affect the user reach the gui as typed outcomes it can show.
- Tracing: `info` for lifecycle transitions, `warn` for degradation the user may notice, `error` for broken invariants or lost subsystems. No spans or per-item events on per-packet, per-object, or per-frame paths; aggregate into counters or rate-limited summaries. Record identifying fields once on the operation's span. Never log capability URLs or session secrets.
- World mutations: validate and plan before committing. Never leave state or indexes inconsistent on failure.
- Packet blocks: validate framing, shared context, and ordering first. Isolate independent failures, keep earlier valid changes, report partial outcomes, and stop when a failure invalidates later blocks.
- Keep layout and index invariants private.

## Concurrency

- One owner for authoritative mutable state, including `World`. Cross threads with immutable snapshots or change streams, not broad locks.
- No blocking or expensive work on gui or render paths. Redraw on change, with no continuously repainting animations.
- Bound queues and in-flight work. Apply backpressure, cancellation, coalescing, and prioritization while preserving protocol ordering and acks.
- Add subsystem schedulers or budgets only when a workload justifies them; no global scheduling infrastructure.
- Scope clones by shadowing inside the spawn block (`let ctx = ctx.clone();`) so handles drop with the task.

## Cover the whole path

- **Update paths**. Full, compressed, cached, and terse object updates reach the same `World` invariants.
- **Reverse transitions**. Every way in has a way out and a way to observe it: kill, derez, unlink, detach, region leave, cache eviction.
- **Regions**. Root and neighbor regions behave differently, and so do crossing and teardown.
- **Scale**. Exercise the large scene and the long session, not one object.
- **Docs**. Update the doc that owns the fact in the same change.

## Naming

- Concise names; no redundant qualifiers in public APIs. Short locals are fine when context is clear (`pkt`, `req`, `obj`, `ctx`, `r`, `e`, `d`).
- Import types and traits directly when their names are clear (`HashMap`, `Uuid`). Prefer module-qualified free functions (`fs::read`, `mem::take`), and retain qualifiers that distinguish generic names (`io::Result`, `fmt::Error`).
- Choose the shortest path that preserves meaning. An inline qualified path is fine for an occasional use; import repeated or long paths. Use `as` only to resolve a collision or distinguish roles, not merely to shorten a name. `use Trait as _` is allowed.
- Use `super` at most once in a path. Never write `super::super`; use `crate::` for anything above the parent module.
- Import enum variants by name only if distinctive and not a prelude name. Keep imports explicit, except established `prelude::*` imports and `use super::*` in tests. Let rustfmt format and order imports.
- `Udp` is the generic transport; `LLUdp` is Second Life's protocol. Keep wire-template and external spellings exact.

## Comments

Do not add comments or doc comments to source, except a concise safety comment on an `unsafe {}` block. If code seems to need a comment to explain its behavior, improve its names, types, or structure until that explanation is clear from the code. Remove existing comments from code you touch.

## Testing

- Use [rust-testing](.agents/skills/rust-testing/SKILL.md) for all Rust test work.
- Smallest proof first. Run tests and clippy for the crates you touched. Run the workspace-wide suite only when asked, because CI owns it.
- Test behavior through public APIs. Skip tests that assert callback wiring or mirror the implementation.
- Use property-based or stateful tests where operations interact (reordering, reparenting, stale updates, ID reuse). Generators must be domain-valid and failures reproducible.
- Wait on completion signals, never on sleeps or polling.
- Integration tests are plain `tests/<name>.rs` files, auto-discovered by Cargo; no `[[test]]` entries. Only when one grows into several files does it become `tests/<name>/main.rs` with its submodules.
- Use `std::assert_matches!` instead of `assert!(matches!(..))`, so failures print the actual value.
- Ask before launching the gui viewer, driving a browser, or any computer use.

## Versioning

- SemVer with a single shared workspace version; keep `Cargo.lock` in sync.
- Before finalizing a PR, decide whether it is a release boundary. If so, bump the version and explain it in the PR description. No bumps for routine merges or internal refactors.
- No git tags, publishing, or GitHub releases without approval.

## Documentation

- [architecture.md](docs/architecture.md): high-level architecture, subsystem responsibilities, state ownership, data flow, crate boundaries, and dependencies.
- [reference.md](docs/reference.md): guidance for consulting the official Second Life viewer, including which systems to study, which to reference only for behavior, and which implementations to ignore.
- [tooling.md](docs/tooling.md): commands, testing, fuzzing, benchmarks, profiling, analysis.
- [vision.md](docs/vision.md): product direction and architectural principles. Follow when making architectural and product decisions.

Durable architecture and decisions go in these docs. Do not describe crate layout or data flow in this file.

## Keeping this file sharp

- A rule earns its place only if it is non-obvious, came up more than once, and is concrete enough to act on.
- Write traps to avoid, not maps to follow.
