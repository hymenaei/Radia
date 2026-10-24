This document describes the high-level architecture of Radia. If you want to get familiar with the code base, start here.

See also `AGENTS.md` for working rules, including which changes need approval before implementation.

## Bird's eye view

On the highest level, Radia takes two streams of input, simulator data and user input, and produces frames.

Simulator data arrives over UDP circuits and HTTP capabilities. The `protocol` crate decodes it into typed messages. The session layer translates those messages into updates and applies them to the `World`, an in-memory model of the entities the simulator has told us about. This is the ground state. Nothing below the viewer reads files or sockets to answer a question about it.

From the `World`, the `scene` crate derives a render-facing view: what is visible, where it is in global space, what should be drawn. The `render` crate draws that `Scene` using decoded data from `assets`. Scene, GPU resources and decoded assets are derived state. They can be thrown away and rebuilt from the world and the asset cache.

## Code map

This section talks briefly about the important crates and data structures. Pay attention to the **Architecture Invariant** paragraphs. They often describe things that are deliberately absent from the source.

Note also which crates are **API Boundaries**. Rules at the boundary are different. Substantial changes to a boundary's public API need approval first, and everything behind it can change freely.

```text
apps/viewer/           # package `viewer`, executable `radia`
crates/assets/         # package `assets`
crates/llsd/    # package `llsd`
crates/math/           # package `math`
crates/utils/          # package `utils`
crates/test-support/ # package `test-support`
crates/network/        # package `network`
crates/protocol/       # package `protocol`
crates/render/         # package `render`
crates/scene/          # package `scene`
crates/world/          # package `world`
```

The viewer coordinates the library crates. The common directory is being removed one crate at a time. Shared utilities and domain ID types live in utils, which has no subsystem dependencies. AssetId wraps uuid::Uuid so asset IDs cannot be confused with other identifiers. The base crate has been removed. Rust identifiers use Uuid while external wire names and serialization markers retain their protocol spelling.

### `apps/viewer`

### `crates/assets`

### `crates/llsd`

### `crates/math`

The math crate owns reusable geometry and numeric operations and re-exports selected glam types and scalar extension traits. Frustum extraction uses zero-to-one clip depth, including reversed depth, and tests bounding spheres against six planes. Plane coefficients and sphere calculations use double precision, and plane normal lengths are computed once per frustum. Axis-aligned Bounds, normal transforms, scale-reflection detection, and sRGB channel conversions also live here. Camera policy, mesh storage, and GPU packing stay with scene and render. Keep consumers importing from math so project-specific primitives have one home.

### `crates/utils`

Shared ID newtypes distinguish objects, assets, avatars, groups, regions, parcels, inventory items, and folders. Their UUID representation is private. Wire codecs convert explicitly; subsystem APIs adopt these types during their individual reviews. Numeric region handles and parcel-local identifiers remain separate.

### `crates/test-support`

### `crates/network`

### `crates/protocol`

Protocol owns wire framing, field codecs, typed messages, and generated metadata from the pinned Second Life message template. Messages are grouped by agent, chat, circuit, region, map, and object responsibilities. Shared object payloads and reconstruction rules belong to the object group; raw field reading and writing belong to the LLUDP codec. Each leaf module retains its protocol message name in snake_case. Root message re-exports keep common type imports concise. Wire names and field layouts remain exact.

Packet framing borrows the datagram and validates allocation bounds before decoded fields are allocated. A preflighted frame binds those bounds to the same input. Packet ownership, encoding, and retained-capacity accounting remain separate from the borrowed frame. Full, compressed, cached, and terse object layouts retain their individual compatibility rules and admission bounds, including permissive object values that the world layer validates independently.

LayerData keeps one shared module for its input limit, bit reader, and error type. Its terrain module owns Decoder, Patch, terrain limits, and coefficient reconstruction. Consumers use layer_data::terrain::{Decoder, Patch} and layer_data::Error. Terrain remains the only supported layer variant. Invalid terrain values preserve later independently framed patches; truncation and framing failures end iteration.

The vendored template is pinned to upstream commit d5445ef1ccb26864236653bbe91d6878bb2277fc. The build checks its normalized SHA-256 digest; the local copy removes trailing whitespace and one final blank line. Login uses the protocol-mandated MD5 representation. Both hash checks use the shared lowercase hexadecimal encoder in utils. Required login fields distinguish absence from invalid types, and UUID parsing borrows their text.

### `crates/render`

### `crates/scene`

### `crates/world`

## Keeping this file sharp

Don't edit yet.
