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

Start with the protocol, format, observed behavior, or failing compatibility case. Use specifications and focused tests before tracing large sections of legacy code. Read the smallest relevant source area when the contract remains unclear.

For each compatibility decision, record the behavior Radia must preserve, the source URL and revision, the specification or template version, and the provenance of any fixture. Redact session data before adding captured fixtures. Implement the behavior behind Radia's own boundaries and verify it with tests. Do not translate source files or mirror abstractions. The ignore categories are priorities, not bans on inspection; consult any file that resolves a real compatibility question.

**Preserve what Second Life requires. Redesign how Radia achieves it.**

## Verified rendering baseline

For Classic output comparisons, use the installed Firestorm 7.2.4.80712 Mid graphics preset at `C:\Program Files\Firestorm-Releasex64\app_settings`. The matching source tree is pinned to [`10bd3c9f930c76e1427ddd4ecece6cdf36b4406d`](https://github.com/FirestormViewer/phoenix-firestorm/tree/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d); do not substitute current upstream `develop`. The observed default has HDR enabled when supported, exposure 1, ACES tone mapping mixed at 0.7, dynamic exposure coefficient 0.5, diffuse luminance scale 0.5, and sky exposure settings disabled. User overrides are unknown.

The pinned [`luminanceF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/luminanceF.glsl) samples the centered 60% crop shifted down by 10% in OpenGL texture coordinates, scales ordinary diffuse color but not sky or skip-atmosphere surfaces, then adds emissive radiance. The viewer reduces a 256 by 256 luminance image before exposure evaluation. Deferred PBR writes emissive factor times its linearized texture to a separate output in [`pbropaqueF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/pbropaqueF.glsl); [`glowcombineF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/interface/glowcombineF.glsl) adds that output to diffuse color. Legacy fullbright uses its own unlit output path in [`fullbrightF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/fullbrightF.glsl). [`postDeferredTonemap.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/postDeferredTonemap.glsl) applies exposure and the configurable tone-map mix at the final output boundary. [`moonF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/moonF.glsl) and [`starsF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/starsF.glsl) sample the current diffuse texture; [`cloudsF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/deferred/cloudsF.glsl) consumes cloud-noise channels as data; and [`waterF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/environment/waterF.glsl) composes water base color without the commented-out atmospheric additive mix. These are behavior references for the installed build, not a rendering architecture to port.

### Water waves, fog and reference differences

The installed 7.2.4.80712 class1 [`waterV.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class1/environment/waterV.glsl) applies `v.x += (cos(v.x * 0.08) + sin(v.y * 0.02)) * 6` at each water vertex before passing the wave coordinates to the fragment shader. [`llvowater.cpp`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/llvowater.cpp) builds one 256 m quad from two triangles sharing the world diagonal from `(0, 0)` to `(256, 256)`. At its center with zero animation, the interpolated swept X is `128.066951`; evaluating the sweep at the center instead gives `127.181892`. Radia computes the region wave value at patch vertices and evaluates the same region-aligned field for void-water ray hits. Firestorm caps its wave input radius at 2560 m; Radia keeps wave UVs unbounded because clamping produced visible radial stripes. Both paths preserve camera-relative world phase using f64 position reduction and compute UV gradients before repeat sampling.

The fog exponent path in [`llsettingsvo.cpp`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/llsettingsvo.cpp) reads component Z from `getClampedLightNorm()`. [`llenvironment.cpp`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/llenvironment.cpp) clamps the selected light's world Z to `-0.1` before `toLightNorm`, then maps world `(x, y, z)` to `(y, z, x)`. [`llsettingssky.cpp`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/llinventory/llsettingssky.cpp) selects the sun when its world Z is nonnegative, then the moon when its world Z is nonnegative, otherwise `(0, 0, -1)`. Therefore the water fog exponent uses world X, floored at `0.3`; the floor is separate from atmospheric attenuation.

The pinned class3 [`waterF.glsl`](https://github.com/FirestormViewer/phoenix-firestorm/blob/10bd3c9f930c76e1427ddd4ecece6cdf36b4406d/indra/newview/app_settings/shaders/class3/environment/waterF.glsl) passes `viewVec * 2048` to water fog, measured from the camera. Radia extends the sample 2048 m from the water intersection to avoid the high-altitude ring covered by `water_color_ignores_far_plane`. For a camera 20 m above water and a ray with Z slope `-0.01`, the intersection is 2000 m away: Firestorm's endpoint leaves 48 m of water path, while Radia's leaves 2048 m. With density 2, fog exponent `1 / 0.3`, slope `0.01`, and `F = 0.98`, the source fog equation yields `(L, D) = (0.9067385, 0.1437816)` and Radia's extended endpoint yields `(0.9903240, approximately 0)`. A temporary source-endpoint mutation fails the current no-normal high-altitude Vulkan ring regression; the extended endpoint passes it. The installed app settings do not include the stock normal-map bytes, so the sampled-wave GPU regression uses the existing independent `test-support` `RGBA_J2C` fixture through Render's raw texture view and water sampler. Its full-scene high-altitude row profile changes sharply with that synthetic fixture under either endpoint, so this does not establish that the extended endpoint matches Firestorm with the stock map. Keep the extended endpoint until that asset is available and that comparison can be repeated; do not add another haze term to mask the result.

Firestorm's class3 water fragment shader calls `sampleReflectionProbesWater` for reflected radiance. Radia currently uses the sky-only `water_radiance_from_sky_without_probes` fallback, so reflected scene and local-probe parity remains incomplete.
