This file records Second Life domain terms and protocol behavior. `docs/ARCHITECTURE.md` records current crate ownership.

## Language

- **Grid**: A network of regions and services. Agni and Aditi are separate Second Life grids.
- **World**: Regions and entities currently known to Radia, not the entire grid or the renderer.
- **Region**: A spatial area containing terrain, parcels, objects, and avatars. Standard regions are 256 × 256 meters; larger regions also exist.
- **Simulator**: The server process simulating a region. "Sim" is sometimes used for a region, but the concepts differ.
- **Region id**: A region's assigned identity, distinct from its location and current simulator connection.
- **Region handle**: A protocol `u64` encoding the global X/Y coordinates of a region's origin. It identifies a location, not a permanent region identity.
- **Region instance**: One observed region lifetime in Radia, distinguished from a previous incarnation after replacement or reset.
- **Region crossing**: Movement of an avatar or object between neighboring regions, requiring a transfer of simulation responsibility.
- **Parcel**: A land subdivision within a region. An estate administers one or more regions.
- **Agent**: A user's participation in the grid, associated with their account UUID. The root agent occupies the controlling region; child agents maintain neighboring presence.
- **Avatar**: The agent's in-world representation, distinct from their account or session.
- **Appearance bake**: A composite texture generated from avatar wearables and appearance layers, generally through the server-side baking service.
- **Bakes on mesh**: A mechanism allowing compatible mesh attachments to display avatar baked textures instead of requiring separate texture copies.
- **Session**: One authenticated login and its activity. Radia's runtime `Session` manages communication; the protocol `SessionId` identifies the login.
- **Agent session**: Radia's login-derived data, not another runtime session.
- **Simulator connection**: One connection incarnation. Radia's `SimulatorId` currently identifies this incarnation, not a persistent simulator process.
- **Circuit**: A logical LLUDP channel with sequencing, ACKs, retransmission, and liveness. Its circuit code is not a connection ID.
- **Capability**: A service accessed through an opaque HTTP(S) URL, typically obtained from a seed capability. URLs are sensitive.
- **Event queue**: A capability-based long-poll event stream, distinct from UDP traffic and Radia's internal events.
- **Object**: An ambiguous Second Life term that may refer to a prim, an entire linkset, or a simulator object record. Prefer the specific term when necessary.
- **PCode**: Protocol code identifying an object's broad type, such as a volume, avatar, tree, or grass. Not every simulator object is a prim.
- **Attachment**: An object or linkset attached to an avatar rather than rezzed in the world. It follows an attachment point and retains its own prims, identity, and properties.
- **Attachment point**: A designated location on an avatar's skeleton or HUD. Body attachment points follow skeletal animation; HUD attachment points use screen-relative positioning.
- **HUD**: An attachment rendered in the wearer's screen space, invisible to other residents.
- **Wearable**: An avatar appearance item, such as clothing or body parts, distinct from an attached object.
- **Prim**: An individual in-world object component, including standard, sculpted, and mesh primitives. Each prim has its own identity, transform, and properties.
- **Root prim**: The primary prim of a linkset, defining its root transform and object identity.
- **Child prim**: A prim linked to the root, with its transform expressed relative to the root when using linkset-local coordinates.
- **Linkset**: One root prim and zero or more linked child prims, treated as a single object. Second Life does not support nested prim linking.
- **Entity**: An internally identified world object or avatar, independent of protocol packets and rendering.
- **Object id**: The UUID of a rezzed prim, distinct from an asset or inventory item UUID.
- **Object local id**: A simulator-assigned `u32` identifying an object in a region's active simulation. It can be reused.
- **Entity**: A World-owned generational handle to one observed object or avatar. Its lifetime is limited to the World that created it.
- **Asset**: Stored content such as a mesh, texture, sound, or animation, identified by an asset UUID.
- **Inventory item**: An entry with its own UUID, metadata, permissions, and potentially an asset reference.
- **Global position**: Grid-wide coordinates, generally represented at higher precision.
- **Region-local position**: Coordinates relative to a region's origin.
- **Agent-relative position**: Coordinates relative to the viewer agent's reference origin, currently the controlling region origin in the legacy viewer.
- **Parent-local transform**: A transform relative to a parent, such as a child prim relative to its linkset root. Attachments use an attachment-relative frame.

## Region crossing

An agent region crossing transfers control from a losing simulator to a gaining simulator. The viewer may receive events from both connections during the handoff.

The [Second Life wiki's crossing sequence](https://wiki.secondlife.com/wiki/Region_crossing) says `CrossedRegion` arrives through the EventQueue and `AgentMovementComplete` arrives from the gaining simulator over LLUDP. It says they may arrive out of order. The page labels this sequence as community-discovered and warns that it may contain errors.

In the [official viewer's `CrossedRegion` handler](https://github.com/secondlife/viewer/blob/e34b5c188e758b6d24b768cc44fbee459be2855c/indra/newview/llviewermessage.cpp#L2924-L2958), the viewer sends `CompleteAgentMovement` to the gaining simulator, adds or reuses its region, and sets its seed capability. A separate [movement-completion handler](https://github.com/secondlife/viewer/blob/e34b5c188e758b6d24b768cc44fbee459be2855c/indra/newview/llviewermessage.cpp#L2751-L2795) processes `AgentMovementComplete`.

## Relationships

- A **grid** contains many **regions**; Radia's **world** tracks only what is currently known.
- **Regions**, **simulators**, and **simulator connections** have distinct identities and lifetimes.
- One **session** can maintain multiple **simulator connections**, each with a circuit and capabilities.
- An **agent** has a root presence in its controlling region and may have child presence in neighboring regions.
- A **linkset** has one root prim and optional children; each prim has its own object identity.
- **Object ids**, **object local ids**, and **entity ids** are not interchangeable.
- A connected **circuit** does not imply completed region initialization. Packet sequence numbers are local to one simulator connection, so do not infer a total order across connections. Late events must not revive superseded state.
- An **avatar** can have multiple **attachments**, including multiple objects on the same attachment point.
- An **attachment** retains its **linkset** structure, but its root transform is relative to an **attachment point**.
- An **attachment** is a live object instance, distinct from its associated **inventory item**.
- An **appearance bake** is not equivalent to the individual textures or wearables that produced it.
