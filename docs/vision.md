Radia is not a port of the official viewer. Existing viewers are references for behavior and compatibility, not blueprints. We study legacy structures, formats, and limits first, then redesign whatever gets in the way.

Radia fixes how the viewer works before it changes how the world looks. The first goal is a viewer that is simpler, safer, faster, and easier to maintain, and that shows Second Life as it exists today. Content, materials, environments, lighting, water, avatars, and interactions should look and behave the way their creators intended. Once that base holds, Radia goes beyond what Second Life can express.

## Classic and Modern

Radia is one viewer with two editions. Both run on the same world model, asset handling, networking core, and gui framework. An edition decides which renderer features, environment system, and default interface are active. It is a configuration, not a fork.

Classic is the compatibility edition. Its contract is that someone coming from the official viewer keeps their workflow and their content. Rendering should match closely, but pixel-perfect output and a copy of the official gui are not required. Classic is the default edition and the first milestone.

Modern is the edition that moves past that contract. The protocol and content formats are still Second Life's, so modern's job is to render and operate that same content to a roughly 2018 standard. That means a renderer redesigned around current lighting and material techniques, better water, atmosphere, and volumetric clouds, and an interface rebuilt for current expectations. Nothing is re-uploaded or re-authored. Existing content gets the new pipeline applied to it.

Modern is a superset of classic, so anything classic can show, modern can. Edition-specific code stays isolated, so work on Modern cannot change how classic behaves.

## Principles

- **Simplicity.** Prefer direct, cohesive designs. Don't add indirection, duplication, or abstraction that nothing needs.
- **Safety and correctness.** Ownership is clear, invariants are strong, behavior is predictable, and untrusted data and failures are handled defensively.
- **Maintainability.** Code is readable, APIs are intuitive, names mean something, and each subsystem has a defined job.
- **Performance.** Overhead is low, interaction is responsive, and frame times are stable. We get there through sound architecture and measured optimization.
- **Independence.** Existing designs get re-examined, Radia's own included. No decision is permanent because it is old.
- **Compatibility.** Preserve external behavior and content semantics without inheriting the legacy implementation.
- **Restraint.** Build for current requirements. Add an extension point when a second edition or feature needs one, and skip speculative frameworks.

## Compatibility at the boundaries

Second Life's protocols, formats, and internal structures don't decide how Radia organizes its systems. Data that is split or duplicated for historical reasons becomes a coherent domain model with clear responsibilities. Diffuse textures are a good example. Second Life handles them apart from the other legacy material properties, but Radia can represent them as a single material, whatever the wire format looks like. The same goes for objects, assets, environments, rendering, and networking.

Radia reads and writes Second Life data faithfully at the edges. Internally it normalizes, validates, and reorganizes that data. This doesn't mean merging everything into bigger structures. Data is grouped or separated by meaning, ownership, lifecycle, and usage. The goal is coherence, not fewer types.

**External compatibility must not become internal architectural conformity.**
