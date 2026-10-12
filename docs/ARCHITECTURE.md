Radia currently has one GPUI viewer application and seven supporting workspace crates. The application owns user-facing workflows and network-to-world adaptation. The `network` crate owns communication with Second Life. The `protocol` crate owns Second Life wire formats. The `llsd` crate owns the generic LLSD data format and codecs. The `math` crate owns shared linear algebra. The `world` crate owns the queryable state observed from simulators and exposes it to diagnostic tools and later scene presentation. The `base` crate owns small primitives shared across unrelated subsystems.

## Current workspace

```text
apps/viewer/          # package `viewer`, executable `radia`
crates/common/base/   # package `base`
crates/common/llsd/   # package `llsd`
crates/common/math/   # package `math`
crates/network/       # package `network`
crates/protocol/      # package `protocol`
crates/test-support/  # package `test-support`
crates/world/         # package `world`
```

## Package ownership

### `apps/viewer` (`viewer`, executable `radia`)

Owns the GPUI application and its current workflows: login, nearby chat, inventory favorites, region-name lookup, default agent updates, and remembered-login settings. It calls `network` for login, simulator, and capability services. It owns session lifetimes, maps network simulator identities to world region lifetimes, converts protocol messages into world events, and runs background world intake. UI and future rendering consume world read APIs; they do not handle packets.

### `crates/network` (`network`)

Owns login, capability HTTP, UDP transport, simulator connections, LLUDP circuit sequencing and reliability, event-queue polling, and the session API. Login responses are capped at 16 MiB and XML nesting at 64 elements before XML-RPC decoding. It handles the required circuit and simulator handshake, then forwards application messages through generic `Event` values. The event channel holds at most 32 events. The simulator keeps at most 16 more events while that channel is full. Simulator capability events received before their matching `SimulatorConnected` are held in a separate 64-event session buffer. At the ordinary queue limit, the simulator starts logout and drops application delivery, while continuing circuit sequence, duplicate, and reliable-ACK bookkeeping for received packets. It flushes queued ACKs when the event channel is full and otherwise uses the normal ACK timer. After logout, it reports the overload through `Event::Failed` so the viewer clears the closed session. The EventQueue parser caps XML responses at 32 MiB, batches at 1,024 events, nesting at 64 containers, estimated data per event at 256 KiB, and estimated decoded data per batch at 8 MiB. `Session::start` runs a Tokio runtime on a named `network` thread. `Session` owns the sole event receiver and provides `recv_event` and `send`; a cloneable `SessionHandle` supports concurrent sends, capture control, status queries, and shutdown. `Event::MfaRequired` carries an `MfaChallenge` continuation that submits the authenticator code. Public operations return `network::Error` with matchable variants for queue saturation, closed sessions, capture failures, and capability failures. Asynchronous login and simulator failures arrive through `Event::Failed`. Capability URLs use opaque `capabilities::CapabilityUrl` handles. `capabilities::Client` provides typed LLSD `get` and `post` operations and raw `get_bytes` for binary resources; debug output redacts URLs. Outbound messages use the sealed typed-message boundary.

All simulator capability clients in one session share four ordinary request slots, nine EventQueue polling slots, and a 72 MiB retained-capability-event budget. Each EventQueue batch reserves up to 8 MiB before LLSD decoding, then transfers byte permits with events through the network queues; `Session::recv_event` releases each permit when the caller receives the event. Ordinary capability request bodies are capped at 1 MiB, and each ordinary or EventQueue response is capped at 32 MiB. The per-response caps allow up to 128 MiB of ordinary response bodies and 288 MiB of EventQueue response bodies at once, before decoded LLSD values, XML tokens, or capture copies. The 256-entry outbound command queue also caps caller-controlled chat storage at 7,121 bytes and object-request vectors at 255 entries per message before admission. These limits bound input growth but do not establish an acceptable peak RSS. Measure peak and live memory with representative traffic before tuning them.

### `crates/protocol` (`protocol`)

Owns packet framing, message IDs, field codecs, zero coding, protocol enums, and LLUDP wire types. Its `build.rs` parses the checked-in message template offline and generates message IDs and schema metadata. The runtime message type keeps hand-written variants for messages with current clients and uses schema-backed messages for other definitions in the template. This crate has no network I/O or viewer behavior.

### `crates/common/base` (`base`)

Owns foundational primitives used across otherwise unrelated crates. It currently re-exports `uuid::Uuid` as `UUID`. It does not depend on LLSD or any higher-level subsystem. The `common` directory is only a grouping; it is not a Cargo package.

### `crates/common/math` (`math`)

Owns shared linear algebra types and operations through `glam`, including vectors, matrices, quaternions, and affine transforms. It has no dependency on viewer, protocol, network, or world concepts.

### `crates/world` (`world`)

Owns the persistent in-memory model of observed regions and entities, their identities and lifetimes, relationships, transforms, indexes, recent metadata, bounded change publication, and query/read APIs. It applies world-owned events and has no dependency on GPUI, viewer, network, protocol, or LLSD. Its production dependencies are `base` for UUID values and `math` for linear algebra. A developer floater and later scene presentation read the same world data.

### `crates/common/llsd` (`llsd`)

Owns the generic LLSD data model and XML, binary, and notation codecs. LLSD uses `base::UUID` directly; UUID remains a core type rather than an LLSD-specific wrapper.

### `crates/test-support` (`test-support`)

Owns fixtures and helpers shared by tests. `network`, `protocol`, and `viewer` use it as a dev-dependency. Production code does not depend on it.

## Dependency direction

The arrows show Cargo dependencies. The `dev` edges are Cargo dev-dependencies, so they do not appear in the production dependency graph.

```text
viewer -> network -> protocol -> base
viewer -> protocol
viewer -> llsd -> base
viewer -> base
viewer -> world -> base
viewer -> world -> math
protocol -> math
network -> llsd
network -> base
network --dev--> test-support
protocol --dev--> test-support
viewer --dev--> test-support
```

`base` stays near the bottom of the graph and contains no subsystem-specific behavior. `protocol` stays independent of login, transport, and UI concepts. `llsd` stays independent of Second Life protocol and viewer concepts.
`world` stays independent of message decoding, simulator transport, and presentation. The viewer is the adapter between network events and world-owned events; future scene consumers and tools can depend on `world` directly.

## Where new code belongs

- Put wire layouts and packet encoding or decoding in `protocol`.
- Put generic LLSD data types and codecs in `crates/common/llsd`; keep capability and viewer-specific structures in their current owners.
- Put small primitives shared across otherwise unrelated crates in `crates/common/base`.
- Put reusable linear algebra in `crates/common/math`. Keep simulator coordinates and transforms in `world`, wire layouts in `protocol`, and renderer-specific behavior in presentation code.
- Put login, HTTP and UDP transport, LLUDP circuit state, and simulator session state in `network`.
- Put observed world state, identity, relationships, transforms, and read/query contracts in `crates/world`. Keep region-name resolution, agent movement policy, session ownership, and network-to-world adaptation in the viewer; `network` transports protocol messages without choosing world behavior.
- Put screens, user workflows, and local viewer state in `apps/viewer`.
- Put shared fixtures and test helpers in `test-support`.

Extract a crate when a responsibility needs its own dependency boundary or multiple workspace packages need to share it. Otherwise, keep the code in its current owner.

## Proposed architecture

The current crate graph is the viewer's starting point. Keep application workflows and transport adapters in the viewer. The world model has its own crate so inspection tools and later scene presentation use the same state without inheriting viewer, transport, or UI dependencies.

| Area                | Responsibility                                                                                                                                                     | Likely module names                                     |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| Application         | Starts systems, wires their lifetimes, and coordinates user workflows.                                                                                             | `apps/viewer`                                           |
| World and features  | The `world` crate owns observed world state, identity, relationships, transforms, and queries. The viewer owns agent policy, inventory, social features, and chat. | `crates/world`, `avatar`, `inventory`, `social`, `chat` |
| Presentation        | Owns UI interaction, scene rendering, audio, media, and voice.                                                                                                     | `gui`, `renderer`, `audio`, `media`, `voice`            |
| Supporting services | Fetches and decodes assets, stores local data, manages settings, and isolates operating-system integration.                                                        | `assets`, `storage`, `settings`, `platform`             |
| Grid communication  | Handles login, capability requests, simulator transport, and wire formats.                                                                                         | Existing `network` and `protocol` crates                |

The viewer application coordinates feature state with services. Its background adapter turns network events into world-owned events and applies them to the `world` model. UI and rendering consume world read APIs rather than packets or transport types. Asset loading, decoding, and local storage run outside the UI and render paths, then provide ready data or handles to presentation code.
