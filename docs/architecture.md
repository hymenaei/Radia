Radia separates simulator observations, accepted world state, derived scene data, and GPU resources. This document describes the current ownership and data flow. Product direction lives in [vision.md](vision.md); working rules live in [AGENTS.md](../AGENTS.md).

## Data flow

Simulator traffic enters through LLUDP circuits and HTTP capabilities. `network` manages delivery and connection lifecycles; `protocol` supplies wire representations and codecs. The viewer's session intake translates messages into domain observations, preserving source identity, ordering evidence, partial updates, and coverage loss. `world` validates and applies those observations. Local requests become authoritative simulator state only when confirmed by observations.

Render's scene reconciliation reads bounded World pages and change journals through the viewer's `WorldReader` bridge. It combines accepted object, terrain, and environment state with available assets, then publishes batches for the viewport's `SceneView`. GPU drawing borrows that view; it does not read network messages or mutate World.

Transport coverage loss and scene recovery are separate concerns. Session intake tracks where simulator observations may be incomplete and requests recovery where supported. Scene reconciliation can rebuild its derived state after history loss, rejected reads, or capacity pressure, but cannot recover observations that World never received. Neither successful rendering nor a populated scene proves complete simulator state.

## State and execution

The session intake task is the single owner of mutable World state. It services admitted read requests alongside incoming events; consumers receive bounded replies and revision notifications rather than locking World. Session and region identities, ordering clocks, and tombstones protect updates across deletion, local-ID reuse, and handoff.

`main` creates the Tokio service runtime and starts GPUI. Networking owns a separate runtime and network thread. The scene viewport owns its window/event-loop thread, `SceneView`, and renderer. Asset fetching and scene reconciliation run on service tasks; decoding, mesh generation, and texture preparation use blocking workers. Bounded queues, admission limits, cancellation, and generation checks keep slow or stale work from crossing lifecycle boundaries.

## Code map

Each entry identifies a crate's responsibility, its important boundaries, and constraints that callers must preserve. Module listings, numeric limits, and command references belong in source or [tooling.md](tooling.md).

### `apps/viewer`

Composes startup, GUI screens and primitives, persistence, and subsystem coordinators. Session intake is the sole owner of World and translates protocol messages into domain observations while preserving source order and coverage-loss information. The asset coordinator supplies network routes and executes bounded asset jobs; the scene feed schedules reconciliation and delivers sequenced batches to the viewport.

### `crates/assets`

Decodes textures, materials, and environments and owns their values plus the bounded CPU mesh representation. Its per-session Store coordinates demand, route eligibility, priority, retries, refinement, and work reservations. `next_job` yields fetch work or cached decode input. Fetched bytes become a `DecodeInput`; the Store admits that input as a `DecodeJob`, and decoding produces a charged `LoadCompletion`. Its `cache` layer maps asset kind and ID to generic storage keys and merges validated byte extents. Persistent layout, hashing, quotas, recovery, process locking, and eviction belong to `storage`. The viewer owns network fetching and decode scheduling, so asset policy stays independent of transport.

### `crates/llsd`

Owns LLSD values, Serde integration, and XML, binary, and notation codecs. Decode limits bound document size, nesting, value counts, and scalar or container sizes; callers can tighten them for their own admission budgets. Simulator messages and domain semantics belong to higher layers.

### `crates/math`

Provides shared geometry and numeric primitives, with selected `glam` types re-exported through this crate. UV transforms are shared by World appearances and decoded materials. Frustum operations use zero-to-one clip depth; ray intersections preserve the parameter in `origin + direction * t`, including for unnormalized directions.

### `crates/network`

Owns login, session lifecycle, simulator links, and capability traffic. A session coordinates the network thread, primary and neighbor links, event routing, handoff, logout, and capability-event decoding. Each circuit owns one simulator's UDP socket, LLUDP ordering and delivery state, send pacing, and bounded packet admission. Capability discovery and event queues share the bounded HTTP client; event envelopes carry retained-memory charges with their events. Session events include source-specific ordering evidence, while bounded command and event admission preserve protocol order and report coverage loss separately from ordinary failure. World and rendering state belong elsewhere.

### `crates/protocol`

Owns Second Life wire types, LLUDP framing and field codecs, and typed messages grouped by protocol area. Each typed message owns its codec and allocation preflight; a declarative list generates message variants and ID dispatch. Build-generated schema metadata comes from a vendored, SHA-256-verified template and remains the fallback for untyped messages. External message spellings and layouts stay exact. Framing and allocation validation belong here, while domain acceptance belongs to World.

### `crates/render`

Derives bounded scene state from World and Assets, then renders it through GPU-owned resources. Reconciliation reads bounded World pages and change journals; it owns visibility and mesh demand, selects prim detail, tessellates prims, builds terrain and water geometry, and resolves materials. Region and parcel environment assignments come from World and publish as decoded scene environments; experience overrides bypass World and are published through Render.

`scene::prim` and `scene::surface` build the reusable CPU `assets::mesh::Mesh`; `scene::mesh` retains meshes and their identities. The model layer holds scene objects, regions, patches, and recovery status. `SceneView` applies sequence-checked batches and acknowledges delivery; abandoned deliveries trigger recovery, and stale work cannot publish after recovery or teardown. GPU modules own surfaces, buffers, textures, draw preparation, and resource retirement.

### `crates/storage`

Owns versioned, size-bounded persistent bytes for opaque fixed-size keys. One writer batches index journal appends and compacts them into atomic snapshots; bounded readers verify SHA-256 hashes on access without waiting for disk writes. Startup replays the snapshot and journal, while the writer sweeps orphan files in bounded background steps. Data and journal appends are not synced per write; missing or short data is removed at lookup, and bytes are never returned before size and hash verification. Compacted snapshots and reset journals are synced before replacement. Consumers choose keys and validate their domain payloads; storage records usage and applies LRU eviction.

### `crates/test-support`

Provides shared LLUDP vectors and a hex parser, texture samples with independent pixel expectations, and synchronous allocation measurements. Production crates use it only as a dev-dependency. Network keeps its process-wide probes local because they measure a different scope.

### `crates/utils`

Provides shared domain ID newtypes and small general-purpose helpers such as atomic file replacement. ID wrappers keep UUID representation private, and domain-specific validity rules stay with each owning type. It has no subsystem dependencies.

### `crates/world`

Owns accepted simulator state for regions, objects, relationships, poses, appearances, terrain, media, and environment. Mutations validate and plan before committing state and indexes. Full observations and partial patches share one field pipeline for validation, authority, and freshness; separate update clocks preserve untouched fields. PBR assignments remain distinct from face overrides. Agent handoff is an observation from the destination region that may move the self agent.

Bounded reads and change history expose current state, unresolved relationships, and whether more data is available. Consumers resynchronize when journal history has been evicted.

## Keeping this file sharp

- Describe the current architecture. Put product direction in [vision.md](vision.md), tool instructions in [tooling.md](tooling.md), and legacy research guidance in [reference.md](reference.md).
- Start with ownership: identify authoritative state, producers and consumers, and cross-crate boundaries. Explain flows that affect ordering, backpressure, consistency, or recovery. A crate list alone is not enough.
- Keep the invariants a change author must preserve. Include type or module names when they clarify a boundary; leave local algorithms, exhaustive inventories, and numeric limits to code.
- Give each fact one home. Link to its owning document instead of copying its guidance.
- Update this file when ownership, dependencies, boundaries, or recovery semantics change. Check claims against implementation, especially guarantees about limits and completeness.
- Remove obsolete statements and repetition before adding detail. Keep enough context to explain why the boundaries matter.
