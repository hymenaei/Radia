Radia is a GPUI viewer for Second Life. Package ownership and Cargo dependencies define its architecture.

## Current workspace

```text
apps/viewer/           # package `viewer`, executable `radia`
crates/common/base/    # package `base`
crates/common/llsd/    # package `llsd`
crates/common/math/    # package `math`
crates/common/testing/ # package `testing`
crates/network/        # package `network`
crates/protocol/       # package `protocol`
crates/world/          # package `world`
```

## Package ownership

### `apps/viewer` (`viewer`, executable `radia`)

Owns the GPUI application and workflows such as login, chat, inventory favorites, and region lookup. It calls `network` for grid services and adapts simulator events into world events. It owns session lifetimes and background world intake; UI and rendering read world APIs rather than packets.

### `crates/common/base` (`base`)

Owns primitives shared across subsystems and re-exports `uuid::Uuid` as `UUID`. It has no LLSD or higher-level dependencies. `common` is a directory grouping, not a Cargo package.

### `crates/common/llsd` (`llsd`)

Owns the generic LLSD model and XML, binary, and notation codecs. It uses `base::UUID` directly.

### `crates/common/math` (`math`)

Owns shared `glam` vectors, matrices, quaternions, and affine transforms. It has no viewer, protocol, network, or world concepts.

### `crates/common/testing` (`testing`)

Owns fixtures and helpers used by `network`, `protocol`, and `viewer` tests. Production code does not depend on it.

### `crates/network` (`network`)

Owns login orchestration, capability HTTP, UDP transport, simulator connections, LLUDP circuit sequencing and reliability, EventQueue polling, and the session API. It supplies metadata and XML depth limits to `protocol::login`, while handling login HTTP, status, retries, response limits, capture, grid selection, credentials, MFA, endpoint validation, and session setup.

Each connection task owns its UDP socket, Circuit, handshake, and reliability state, then forwards application messages as generic `Event` values. The session event channel holds 32 events; each simulator can buffer 16 more, and a separate session buffer holds 64 capability events that arrive before `SimulatorConnected`. On saturation, the simulator records coverage loss and starts bounded logout while continuing control and ACK processing. It admits an event before committing receive history or acknowledging its reliable packet, leaving rejected packets eligible for retransmission until teardown. ACKs, retries, handshakes, keepalives, logout, and application sends use one session-wide byte budget. Each connection keeps its own bounded send queue; ACKs preempt deferred non-ACK sends, and logout has a dedicated slot outside that queue. Overload is reported through `Event::Failed` so the viewer clears the closed session.

Current resource limits:

| Path | Limits and ownership |
| --- | --- |
| Login | 16 MiB response; 64 nested XML elements. |
| EventQueue | 32 MiB XML; 1,024 events per batch; 64 nested containers; estimated 256 KiB per event and 8 MiB per batch. Preflight and reserve the computed LLSD plus conversion bound before decoding; release decoder memory after conversion, and hold a separate retained-event permit until `Session::recv_event` returns the batch. |
| Capability clients | Per session: four ordinary request slots, nine EventQueue polling slots, and a 72 MiB retained-event budget. Ordinary request bodies are limited to 1 MiB; ordinary and EventQueue response bodies to 32 MiB each. Concurrent raw bodies could reach 128 MiB and 288 MiB respectively, before decoded values, XML tokens, or capture copies. These limits bound input growth but do not establish an acceptable peak RSS; measure live and peak memory under representative traffic before tuning them. |
| Inbound LLUDP | Eight concurrent packet decodes; 32 MiB shared retained-event ceiling, with neighbors limited to 24 MiB together and 4 MiB each. The coordinator reclassifies retained packet bytes synchronously on promotion; a demoted connection over neighbor limits remains charged and blocks new neighbor admission until its events drain. The coordinator allows up to 32 neighbors. |
| Outbound commands | A 256-entry queue; caller-controlled chat storage is limited to 7,121 bytes and object-request vectors to 255 entries per message before admission. |
| Outbound LLUDP | A session-wide token bucket defaults to 64,000 bytes per second and an 8 KiB burst. `SessionOptions` sets the rate. Each simulator queues at most eight datagrams, charged at 8 KiB each, for up to five seconds. Control traffic precedes deadline and fair traffic; the active primary receives twice a neighbor's scheduling weight. The default remains a starting value pending multi-simulator benchmark results. |

`Session::start` runs a Tokio runtime on a named `network` thread. `Session` owns the only event receiver and exposes `recv_event` and `send`; cloneable `SessionHandle`s support concurrent sends, capture control, status, and shutdown. MFA uses an `MfaChallenge` continuation. Operations return typed `network::Error` variants for queue saturation, closed sessions, capture failures, and capability failures; asynchronous login and simulator failures arrive through `Event::Failed`. Capability URLs use opaque `capabilities::CapabilityUrl` handles. `capabilities::Client` provides typed LLSD `get` and `post`, raw `get_bytes` for binary resources, and redacted debug output. Outbound messages use the sealed typed-message boundary.

### `crates/protocol` (`protocol`)

Owns Second Life login XML-RPC and LLUDP wire formats. `login.rs` uses `dxr` to encode and parse `login_to_simulator`, exposes typed wire results, and preserves additional response fields. It enforces the caller's XML depth limit without choosing transport policy. Login metadata and values are `protocol::login::{ResponseData, Value}`; Network re-exports `StartLocation` for `LoginCredentials`. LLUDP framing, IDs, codecs, zero coding, and protocol types also live here. `build.rs` parses the message template offline and generates IDs and schema metadata. `Message` uses handwritten variants for supported messages and schema-backed values for the rest. Protocol has no network I/O or viewer behavior.

### `crates/world` (`world`)

Owns observed regions and entities, their identities, lifetimes, relationships, transforms, indexes, recent metadata, and bounded change/read APIs. It applies world events and depends only on `base` for UUIDs and `math` for linear algebra. It has no GPUI, viewer, network, protocol, or LLSD dependency.

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
network --dev--> testing
protocol --dev--> testing
viewer --dev--> testing
```

`protocol` has no Tokio or reqwest dependency, and `llsd` has no Second Life concepts. The viewer adapts network events to world events, keeping `world` usable by tools and scene consumers.

## Proposed architecture

The table shows how future presentation and supporting services fit around the current crate graph.

| Area                | Responsibility                                                                                                                                                     | Likely module names                                     |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| Application         | Starts systems, wires their lifetimes, and coordinates user workflows.                                                                                             | `apps/viewer`                                           |
| World and features  | The `world` crate owns observed world state, identity, relationships, transforms, and queries. The viewer owns agent policy, inventory, social features, and chat. | `crates/world`, `avatar`, `inventory`, `social`, `chat` |
| Presentation        | Owns UI interaction, scene rendering, audio, media, and voice.                                                                                                     | `gui`, `renderer`, `audio`, `media`, `voice`            |
| Supporting services | Fetches and decodes assets, stores local data, manages settings, and isolates operating-system integration.                                                        | `assets`, `storage`, `settings`, `platform`             |
| Grid communication  | Handles login, capability requests, simulator transport, and wire formats.                                                                                         | Existing `network` and `protocol` crates                |
