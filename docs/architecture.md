Radia separates simulator observations, accepted world state, derived scene data, and GPU resources. This document describes the current ownership and data flow. Product direction lives in [vision.md](vision.md); working rules live in [AGENTS.md](../AGENTS.md).

## Data flow

Simulator traffic enters through LLUDP circuits and HTTP capabilities. `network` manages delivery and connection lifecycles; `protocol` supplies wire representations and codecs. The viewer's session intake translates messages into domain observations, preserving source identity, ordering evidence, partial updates, and coverage loss. `world` validates and applies those observations. Local requests become authoritative simulator state only when confirmed by observations.

Login returns the personal inventory and Library roots and folder skeletons. `protocol` parses those bounded wire values; the viewer session translates them into normalized categories and owns the single mutable inventory model. The model exposes immutable snapshots and revisions, while the session owns a watch channel of snapshots for GUI readers. Favorites lookup uses the personal folder type. Future inventory fetches and edits enter through the same model mutation API.

UDP inventory messages and EventQueue inventory events enter that same session intake in delivery order and translate into atomic inventory deltas. The session applies each delta before handling the next event. A known folder version rejects an older delta or fetch result. An unknown or mismatched delta version leaves the folder incomplete and reports that it needs a refetch. The session publishes the resulting snapshot before forwarding those requests to the fetch coordinator, which gives them foreground priority. The worker reads folder completeness from the latest session snapshot, so a parent fetch that makes a previously complete child incomplete queues that child for a fetch. UDP and EventQueue updates without version evidence use their delivered order.
The session fetch coordinator discovers AIS v3 first and requests recursive children with a bounded depth. If AIS is not granted, it posts `FetchInventoryDescendents2` or `FetchLibDescendents2` in batches of at most ten folders. `network` transports bounded capability responses, `protocol` decodes AIS and legacy LLSD into validated wire types, and the viewer maps those types into one folder application at a time. The session applies each result through `Inventory::apply_folder_fetch` and discards older versions when the local folder is complete. GUI folder requests preempt background work. The background walk starts with every known skeleton folder and follows newly fetched categories in both personal inventory and Library. It runs at most two requests at once, half of `network`'s four-request capability budget, leaving capacity for other capability traffic. Bounded queues and response limits contain memory use, and session cancellation stops the worker. The GUI receives the snapshot watch, a folder request handle, and a typed loading-status watch from the session.

The session opens one rebuildable SQLite cache per grid and agent after login. The cache reader and writer run on the storage worker. The session checks cached folder versions against the login skeleton, applies accepted folders through the inventory mutation API on blocking work, and publishes the candidate only if the model revision still matches. Live changes remain authoritative and enter the cache as ordered, bounded batches. A cache miss or damaged database leaves folder fetching responsible for the missing data.

The inventory window uses one immutable snapshot for its personal tree and read-only Library tree. It builds rows only for expanded folders and feeds them to GPUI's virtualized list. Folder expansion submits foreground work through the session fetch handle. Search scans a snapshot on the background executor, cancels superseded queries, caps results, and reruns when the coalescing snapshot watch publishes a new revision. Search and row building never block the GUI thread; the view redraws when its snapshot, loading status, or user input changes.

Render's scene reconciliation reads bounded World pages and change journals through the viewer's `WorldReader` bridge. It combines accepted object, terrain, and environment state with available assets, then publishes batches for the viewport's `SceneView`. GPU drawing borrows that view; it does not read network messages or mutate World.

Transport coverage loss and scene recovery are separate concerns. Session intake tracks where simulator observations may be incomplete and requests recovery where supported. Scene reconciliation can rebuild its derived state after history loss, rejected reads, or capacity pressure, but cannot recover observations that World never received. Neither successful rendering nor a populated scene proves complete simulator state.

## State and execution

The session intake task is the single owner of mutable World state. It services admitted read requests alongside incoming events; consumers receive bounded replies and revision notifications rather than locking World. Session and region identities, ordering clocks, and tombstones protect updates across deletion, local-ID reuse, and handoff.

`main` creates the Tokio service runtime and starts GPUI. Networking owns a separate runtime and network thread. The scene viewport owns its window/event-loop thread, `SceneView`, and renderer. Asset fetching and scene reconciliation run on service tasks; decoding, mesh generation, and texture preparation use blocking workers. Bounded queues, admission limits, cancellation, and generation checks keep slow or stale work from crossing lifecycle boundaries.

## Code map

Each entry identifies a crate's responsibility, its important boundaries, and constraints that callers must preserve. Module listings, numeric limits, and command references belong in source or [tooling.md](tooling.md).

### `apps/viewer`

Composes startup, GUI screens and primitives, persistence, and subsystem coordinators. Session intake is the sole owner of World and inventory, translating protocol messages into domain observations and deltas in source order and applying inventory fetches. The inventory worker composes capability discovery and bounded network requests with protocol's AIS and legacy LLSD decoders; it owns retries and foreground priority, then returns bounded folder results to the session owner instead of mutating the model. The inventory browser turns snapshots into ordered visible rows and bounded search results on GPUI's background executor. The asset coordinator runs network fetches on service workers and reserved cache reads, decodes, and writes on blocking workers. Successful network decodes enqueue bounded cache writes; session shutdown drains work already in flight. The viewer opens one cache at the local default location and shares raw cached payloads across sessions, while each session keeps its own Store and route state. The scene feed schedules reconciliation and delivers sequenced batches to the viewport.

### `crates/assets`

Decodes textures, materials, and environments and owns their values plus the bounded CPU mesh representation. Its per-session Store coordinates demand, route eligibility, priority, retries, refinement, and work reservations. The persistent cache covers its texture, PBR material, legacy material, and environment kinds; locally generated prim meshes remain in Render's memory cache. `next_job` yields disk reads, network fetches, or decode inputs for retained texture data. Disk-read and network-response completions enter the same decode admission path; misses, corrupt data, and unavailable storage continue to fetch without consuming a failed attempt. Only successfully decoded normalized network payloads enter the bounded asynchronous writer, and those raw payloads persist across sessions. Texture refinement reuses its retained source. Cache reads use a kind-specific payload bound, which storage checks before opening or allocating for the bytes. The asset cache maps each resource to a namespace index and UUID; its storage root declares the directories and extensions, and it exposes usage counters and admits writes only while bounded in-flight and budget reservations are available. Persistent layout, hashing, quotas, recovery, process locking, and eviction belong to `storage`. The viewer owns network fetching and decode scheduling, so asset policy stays independent of transport.

### `crates/llsd`

Owns LLSD values, Serde integration, and XML, binary, and notation codecs. Decode limits bound document size, nesting, value counts, and scalar or container sizes; callers can tighten them for their own admission budgets. Simulator messages and domain semantics belong to higher layers.

### `crates/inventory`

Owns normalized personal inventory and Library categories, items, parent and child indexes, link resolution, folder completeness, validated mutation planning, and the SQLite cache schema and queries. The database stores `meta`, `categories`, and `items`. `meta` records schema version, canonical grid identity, agent, Library owner, cache generation, and shutdown state. Categories and items use `(owner_id, category_id)` and `(owner_id, item_id)` primary keys. `categories_by_parent(owner_id, parent_id)` serves `SELECT ... FROM categories WHERE owner_id=? AND parent_id=?`; `items_by_parent(owner_id, parent_id)` serves the equivalent child-item query. Link targets stay in the item's asset ID and asset type, so link resolution uses the model's in-memory index without another table or index.

Only complete folders from the current cache generation can be reused, and the login skeleton must report the same known server version. The cache reader validates owners, roots, parent links, cycles, child counts, row values, and model limits before it returns data. It caps the estimated decoded tree at 256 MiB, charging 1 KiB per item, 256 bytes per category, and the UTF-8 bytes in stored names and descriptions. A failed validation discards and recreates the database. `storage` owns SQLite connections and worker mechanics; `inventory` owns SQL, migrations, row mapping, and version checks. `protocol` owns wire parsing and `network` owns transport; `inventory` has no dependency on either boundary.

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

Owns versioned, size-bounded persistent bytes. Each consumer opens a root and declares its fixed namespace table as validated relative directories and lowercase alphanumeric extensions. A key is a namespace index and a 128-bit name; storage resolves it to `<namespace>/<first-two-name-hex>/<hyphenated-lowercase-name>.<extension>`. The snapshot stores the namespace table once, and snapshot records and journal entries store only the namespace index and name. Each consumer root has one snapshot, journal, process lock, size limit, and LRU across its namespaces. Namespace tables can change: recovery remaps entries whose directory and extension remain declared, drops entries for removed namespaces, and sweeps their files as orphans.

The generic SQLite worker keeps each connection on a dedicated thread. Each transaction holds at most 256 rows and 1 MiB of estimated payload, which limits lock duration and the work held by one commit. The queue admits at most 64 commands, 131,072 rows, or 32 MiB. These caps allow short bursts without retaining a whole inventory update. Reads run on the same worker. The worker enables WAL, applies a 25 ms busy timeout, and runs `quick_check` when it opens a database. A per-database maintenance lock protects repair: the worker removes the database and WAL files only after every process has released its shared lock. Corrupt files and incompatible schemas are recreated. Busy writes return a typed outcome so consumers can defer or drop cache work without waiting on SQLite.

The asset cache opens the `assets/` consumer root under the default cache directory, so its index files are in `cache/assets/`. Asset payloads are stored as raw normalized network bytes: textures use `assets/textures/<pp>/<uuid>.j2c`; PBR materials use `assets/materials/pbr/<pp>/<uuid>.llsd`; legacy materials use `assets/materials/legacy/<pp>/<uuid>.llsd`; environments use `assets/environments/<pp>/<uuid>.llsd`. PBR files are LLSD-wrapped glTF JSON; legacy material and environment files are LLSD. The separate material directories distinguish their encodings. Other asset namespaces can be added when those kinds are supported. A future object cache can open a separate consumer root with its own index and size limit.

One writer batches journal appends and compacts them into atomic snapshots; bounded readers verify SHA-256 hashes on access without waiting for disk writes and reject entries over the caller's size bound before opening or allocating for their payloads. Startup replays the snapshot and journal, while the writer sweeps orphan files in bounded directory steps, including old flat data and temporary files. Shard directories are created lazily, removed when empty, and included in size accounting. Replacements stage a temporary file beside the fixed destination and atomically rename it. An entry state protects the old index record during replacement; journal writes happen without the index lock, and map publication holds it only briefly. A hash mismatch retries when the selected index entry changed during the read, while a mismatch against the still-current entry during replacement returns unavailable. If a crash leaves file contents ahead of the journal, hash verification returns Corrupt or Miss rather than unchecked bytes. Data and journal appends are not synced per write; readers mark missing, short, oversized, or hash-mismatched entries invalid, then the writer removes them. Compacted snapshots and reset journals are synced before replacement. Consumers choose keys and validate their domain payloads; storage records usage and applies LRU eviction.

The inventory cache lives under `inventory/<grid-hash>/<agent-uuid>.sqlite` below the default cache directory. The lowercase grid hash comes from the canonical grid identity; the database stores that identity as metadata, never a login URL or capability. Inventory sends each change set through the bounded worker queue. The writer commits child rows before it marks a folder complete. If work cannot enter the queue, a reserved control command advances the cache generation, making existing complete markers ineligible for reuse.

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
