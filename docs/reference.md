Radia uses the official Second Life viewer as a reference for compatibility, not as a codebase to port. We study the parts that explain how Second Life communicates, represents content, and behaves. We do not inherit a system just because the official viewer has one.

The goal is to recover the requirements, not the implementation. Protocols, formats, content semantics, and observable behavior matter. Class hierarchies, frameworks, ownership models, and historical workarounds do not unless they reveal a requirement we would otherwise miss.

## Ignore the implementation

These systems have responsibilities Radia may still need, but their existing implementations should not shape ours.

- **Gui and windows (`llui`, `llwindow`, XUI).** Build the interface with Radia's own framework. Consult old layouts for workflows and expected interactions, not widget or floater architecture.
- **Graphics infrastructure (`llrender`).** Do not reproduce OpenGL wrappers, state management, buffer abstractions, or resource lifetimes. Rendering behavior is a separate concern.
- **HTTP and utilities (`llcorehttp`, most of `llcommon`).** Use Rust networking, concurrency, collections, and error handling. Do not recreate generic event systems, thread wrappers, or singletons.
- **Generic math and parsing (`llmath`, `llxml`, `llfilesystem`).** Prefer existing Rust facilities. Domain-specific geometry, coordinate conventions, and serialized formats are exceptions.
- **Image infrastructure (`llimage`, `llimagej2coj`, `llkdu`).** Use suitable image and JPEG2000 libraries rather than porting decoders, image containers, or codec integration. Preserve the supported formats and their relevant semantics.
- **Build and diagnostics.** Ignore CMake, Autobuild, platform packaging internals, and the old crash-logging infrastructure. Radia owns its build, diagnostics, and distribution.

Ignoring an implementation does not mean ignoring its function. Radia still needs windows, HTTP, image decoding, and GPU resource management; it does not need Linden Lab's architecture for them.

## Study behavior, not architecture

These systems are useful references for what the viewer does, but Radia should model them independently.

- **World and scene.** Study object lifecycles, parenting, region changes, spatial behavior, visibility, and update handling in `newview/llviewerobject*`, `llviewerobjectlist*`, and related code. Do not copy the object hierarchy or global managers.
- **Rendering.** Study visible outcomes, material evaluation, transparency, lighting, terrain, water, and LOD selection. Do not carry over the pipeline's passes, OpenGL constraints, or scene organization by default.
- **Asset loading.** Study texture and mesh priorities, progressive loading, and fallback behavior. Design fetching, decoding, caching, and GPU ownership for Radia's own pipeline.
- **Environment.** Study how the viewer applies EEP settings and renders the resulting sky and water. Preserve the required Classic behavior without treating the legacy renderer as the only valid implementation.
- **Editing and interaction.** Study selection, transforms, linking, permissions, and editing workflows. Rebuild the tools around Radia's world and update model.

## Study carefully

These areas carry Second Life-specific contracts that can be difficult to reconstruct from a general-purpose library or a clean design alone.

- **Networking (`llmessage`, `message_template.msg`, relevant `newview` code).** Packet layouts, LLUDP reliability, capabilities, login, event delivery, ordering, and simulator transitions. Confirm behavior with specifications where possible.
- **Data formats (`llsd`, `llprimitive`, `llinventory`).** LLSD, asset formats, object properties, texture entries, legacy and PBR materials, inventory, and permissions. Decode faithfully before normalizing into Radia's domain model.
- **Geometry (`llmath/llvolume*`, relevant `llprimitive` code).** Prim parameters, tessellation, sculpt and mesh interpretation, face ordering, UVs, and LOD semantics. Generic vector and matrix implementations are not part of this requirement.
- **Avatars (`llappearance`, `llcharacter`, relevant `newview` code).** Skeletons, wearables, visual parameters, skinning, animations, attachments, and appearance composition.
- **Mesh and assets (`newview/llmeshrepository*` and related loaders).** Asset structure, mesh LODs, skin data, and compatibility-critical decoding behavior, not the existing fetching or threading design.
- **Environment data (relevant `newview/llenvironment*` code).** EEP formats, settings, precedence, region overrides, and day cycles. Keep these separate from how Radia renders the environment.

Source directories are pointers to useful behavior, not proposed crate boundaries. A file can contain both essential compatibility logic and infrastructure worth discarding.

## Defer until needed

Voice, embedded web media, marketplace integrations, advanced snapshots, and specialized tools need not shape the initial world, asset, and rendering foundation. Add them when the relevant milestone requires them, using the same compatibility rules.

Deferral is not removal from Classic's contract. A feature that residents rely on should eventually work even when its original implementation is discarded. Simulator physics, authoritative world simulation, and LSL execution belong to the simulator rather than the viewer; Radia only needs their viewer-facing interactions.

## How to use a reference

Start with the protocol, format, observed behavior, or failing compatibility case. Use specifications, and focused tests before tracing large sections of legacy code. Read the smallest relevant source area when the contract remains unclear.

Record the behavior Radia must preserve, implement it behind Radia's own boundaries, and verify it with tests. Do not translate source files or reproduce abstractions merely to make the implementations look alike. The ignore categories are priorities, not bans on inspection: any file may be consulted when it resolves a real compatibility question.

**Preserve what Second Life requires. Redesign how Radia achieves it.**
