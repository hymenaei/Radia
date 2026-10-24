This document describes the high-level architecture of Radia. If you want to get familiar with the code base, start here.

See also `AGENTS.md` for working rules, including which changes need approval before implementation.

## Bird's eye view

On the highest level, Radia takes two streams of input, simulator data and user input, and produces frames.

Simulator data arrives over UDP circuits and HTTP capabilities. The `protocol` crate decodes it into typed messages. The session layer translates those messages into updates and applies them to the `World`, an in-memory model of the entities the simulator has told us about. This is the ground state. Nothing below the viewer reads files or sockets to answer a question about it.

From the `World`, `render::scene` derives a render-facing view: what is visible, where it is in global space, and what should be drawn. The viewer runs its reconciler task, answers bounded world reads and asset requests, and applies published batches to `SceneView`. The `render` crate draws that view using decoded data from `assets`. Scene state, GPU resources and decoded assets are derived state. They can be thrown away and rebuilt from the world and the asset cache.

## Code map

This section talks briefly about the important crates and data structures. Pay attention to the **Architecture Invariant** paragraphs. They often describe things that are deliberately absent from the source.

Note also which crates are **API Boundaries**. Rules at the boundary are different. Substantial changes to a boundary's public API need approval first, and everything behind it can change freely.

```text
apps/viewer/           # package `viewer`, executable `viewer`
crates/assets/         # package `assets`
crates/llsd/           # package `llsd`
crates/math/           # package `math`
crates/utils/          # package `utils`
crates/test-support/   # package `test-support`
crates/network/        # package `network`
crates/protocol/       # package `protocol`
crates/render/         # package `render`
crates/world/          # package `world`
```

The viewer coordinates the library crates. The common directory is being removed one crate at a time. Shared utilities and domain ID types live in utils, which has no subsystem dependencies. AssetId wraps uuid::Uuid so asset IDs cannot be confused with other identifiers. The base crate has been removed. Rust identifiers use Uuid while external wire names and serialization markers retain their protocol spelling.

### `apps/viewer`

### `crates/assets`

Assets owns request admission, bounded decoding, and retained content. Scratch reservations and resident charges are distinct private types. Retaining content consumes its scratch reservation; the resulting charge has no resize or split operations. Resident handles keep data charged until the final owner releases it, independently of scratch-work admission. Closing a job cancellation channel ends the cancellation wait.

Material decoding is grouped by legacy and PBR formats, with shared texture and sampler values in their parent module. Texture decoding, headers, and handle lifetimes share a texture module. Legacy envelope inflation grows with actual content while retaining its fixed decompression limit.

### `crates/llsd`

### `crates/math`

The math crate owns reusable geometry and numeric operations and re-exports selected glam types and scalar extension traits. Frustum extraction uses zero-to-one clip depth, including reversed depth, and tests bounding spheres against six planes. Plane coefficients and sphere calculations use double precision, and plane normal lengths are computed once per frustum. Axis-aligned Bounds, normal transforms, scale-reflection detection, and sRGB channel conversions also live here. Camera policy, mesh storage, and GPU packing stay with scene and render. Keep consumers importing from math so project-specific primitives have one home.

### `crates/utils`

Shared ID newtypes distinguish objects, assets, avatars, groups, regions, parcels, inventory items, folders, and experiences. ExperienceId rejects nil UUIDs; an absent experience is represented by Option. Their UUID representation is private. Wire codecs convert explicitly; subsystem APIs adopt these types during their individual reviews. Numeric region handles and parcel-local identifiers remain separate.

### `crates/test-support`

### `crates/network`

Session owns the network worker and event stream; SessionHandle provides bounded send and lifecycle controls. The simulator coordinator owns connection routing, root and neighbor roles, handoff, and teardown. Its connection, capability-event, event-routing, handoff, and runtime modules are children of that owner. Coverage loss remains observable when bounded queues cannot preserve application events.

Capabilities separate discovery and request admission, HTTP transport, and event-queue lifecycle. The budget module owns concurrency and response-memory limits. Checked reservations validate dedicated and shared capacity before changing accounting; permits retain admission until blocking work completes, even after its waiter is cancelled.

HTTP uses Reqwest 0.13 with default features disabled and the rustls feature enabled. The default crypto provider is aws-lc; platform certificate verification uses the operating system's trust policy. The viewer does not install a global crypto provider or disable certificate checks. Redirects remain disabled, compressed responses remain raw, and request deadlines cover response-body reads. The aws-lc native dependency adds build-tool requirements described in tooling.md.

### `crates/protocol`

Protocol owns wire framing, field codecs, typed messages, and generated metadata from the pinned Second Life message template. Messages are grouped by agent, chat, circuit, region, map, and object responsibilities. Shared object payloads and reconstruction rules belong to the object group; raw field reading and writing belong to the LLUDP codec. Each leaf module retains its protocol message name in snake_case. Root message re-exports keep common type imports concise. Wire names and field layouts remain exact.

Packet framing borrows the datagram and validates allocation bounds before decoded fields are allocated. A preflighted frame binds those bounds to the same input. Packet ownership, encoding, and retained-capacity accounting remain separate from the borrowed frame. Full, compressed, cached, and terse object layouts retain their individual compatibility rules and admission bounds, including permissive object values that the world layer validates independently.

LayerData keeps one shared module for its input limit, bit reader, and error type. Its terrain module owns Decoder, Patch, terrain limits, and coefficient reconstruction. Consumers use layer_data::terrain::{Decoder, Patch} and layer_data::Error. Terrain remains the only supported layer variant. Invalid terrain values preserve later independently framed patches; truncation and framing failures end iteration.

The vendored template is pinned to upstream commit d5445ef1ccb26864236653bbe91d6878bb2277fc. The build checks its normalized SHA-256 digest; the local copy removes trailing whitespace and one final blank line. Login uses the protocol-mandated MD5 representation. Both hash checks use the shared lowercase hexadecimal encoder in utils. Required login fields distinguish absence from invalid types, and UUID parsing borrows their text.

### `crates/render`

Render owns scene derivation under `render::scene`, plus the presentation surface, GPU resources, bounded uploads, and resource retirement. CPU texture preparation runs through worker jobs. Typed errors preserve underlying GPU errors through the standard error source chain. `render::material` groups legacy and PBR GPU materials; `render::scene` groups scene objects and views under model, and reconciliation reads, scheduling, and publication under reconcile. Reconciler remains the sole owner of its mutable state; extracting methods into child modules does not add shared ownership or synchronization.

Meshes assembled from buffers validate size limits, finite vertex data, triangle indices, and face ranges before publication. Render upload can rely on these private construction invariants when indexing mesh data.

Reconciler owns the derived scene and advances bounded reads, asset demand, mesh preparation, and publication. SceneView applies publications and supplies immutable render data to Renderer. Jobs and batches retain their lifetime and memory charges, so abandoned or stale work cannot become current after recovery or teardown. The read-completion method is complete_read, alongside complete_asset and complete_mesh; camera updates use set_camera. These replace the former SceneReconciler, complete, and camera names without compatibility aliases. Shared ExperienceId comes from utils.

### `crates/world`

## Keeping this file sharp

Don't edit yet.
