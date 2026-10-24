Radia is not a port of the official viewer. Existing viewers are references for behavior and compatibility, not blueprints. We study legacy structures, formats, and limits first, then redesign whatever gets in the way.

Radia fixes how the viewer works before it changes how the world looks. The first goal is a viewer that is simpler, safer, faster, and easier to maintain, and that shows Second Life as it exists today. Content, materials, environments, lighting, water, avatars, and interactions should look and behave the way their creators intended. Once that base holds, Radia goes beyond what Second Life can express.

## Current milestone

The current milestone is the Classic compatibility baseline. It should preserve the workflows and content residents rely on, with rendering that matches closely without copying the official viewer's interface pixel for pixel. Radia does not identify or offer selectable editions while this work is in progress.

## Later direction

After the Classic baseline is complete, the project can decide whether to offer additional editions and how to name and select them. One possible direction is to apply newer material, lighting, water, atmosphere, and cloud techniques to existing Second Life content without requiring re-upload or re-authoring. Its scope and acceptance criteria remain open until the current milestone is complete.

Any later edition should preserve the content the Classic baseline can show. If editions are introduced, keep their distinct rendering and interface behavior isolated so changes in one do not alter another.

## Principles

- **Simplicity.** Prefer direct, cohesive designs. Don't add indirection, duplication, or abstraction that nothing needs.
- **Safety and correctness.** Ownership is clear, invariants are strong, behavior is predictable, and untrusted data and failures are handled defensively.
- **Maintainability.** Code is readable, APIs are intuitive, names mean something, and each subsystem has a defined job.
- **Performance.** Overhead is low, interaction is responsive, and frame times are stable. We get there through sound architecture and measured optimization.
- **Independence.** Existing designs get re-examined, Radia's own included. No decision is permanent because it is old.
- **Compatibility.** Preserve external behavior and content semantics without inheriting the legacy implementation.
- **Restraint.** Build for current requirements. Add an extension point when a concrete feature needs one, and skip speculative frameworks.

## Compatibility at the boundaries

Second Life's protocols, formats, and internal structures don't decide how Radia organizes its systems. Data that is split or duplicated for historical reasons becomes a coherent domain model with clear responsibilities. Diffuse textures are a good example. Second Life handles them apart from the other legacy material properties, but Radia can represent them as a single material, whatever the wire format looks like. The same goes for objects, assets, environments, rendering, and networking.

Radia reads and writes Second Life data faithfully at the edges. Internally it normalizes, validates, and reorganizes that data. This doesn't mean merging everything into bigger structures. Data is grouped or separated by meaning, ownership, lifecycle, and usage. The goal is coherence, not fewer types.

**External compatibility must not become internal architectural conformity.**
