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
crates/common/base/    # package `base`
crates/common/llsd/    # package `llsd`
crates/common/math/    # package `math`
crates/common/testing/ # package `testing`
crates/network/        # package `network`
crates/protocol/       # package `protocol`
crates/render/         # package `render`
crates/scene/          # package `scene`
crates/world/          # package `world`
```

Dependencies point down this list, from the viewer to the common crates.

### `apps/viewer`

### `crates/assets`

### `crates/common/base`

### `crates/common/llsd`

### `crates/common/math`

### `crates/common/testing`

### `crates/network`

### `crates/protocol`

### `crates/render`

### `crates/scene`

### `crates/world`

## Keeping this file sharp

Don't edit yet.
