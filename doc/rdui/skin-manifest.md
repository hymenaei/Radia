# Radia Skin Manifest

Status: draft public authoring contract.

Radia extends the viewer's existing skin-level `manifest.json`; it does not introduce another manifest. Top-level fields identify the Skin and provide shared package metadata. The `radia` object defines Radia UI resource roots during temporary LLUI coexistence.

```json
{
  "id": "alchemy.default",
  "name": "Second Life",
  "author": "Linden Lab",
  "url": "https://secondlife.com/",
  "notes": "Classic viewer skin",
  "preview": "preview.png",
  "base": null,
  "radia": {
    "stylesheet": "rdui/skin.radia",
    "layouts": "rdui/xui",
    "localization": "rdui/localization.yaml",
    "assets": "rdui/resources"
  }
}
```

`id` is stable machine identity used by catalog lookup and `base` references; directory
names and display text are not identity. During temporary LLUI coexistence, the viewer
maps the legacy selected Skin directory to the `id` in that directory's manifest.
`base` names at most one direct Base Skin by ID, or is `null` for a root Skin.

Skin IDs use lowercase dotted namespaces, such as `alchemy.default` or `bruno.midnight-blue`. Each segment contains ASCII lowercase letters and digits with optional internal hyphens. Spaces, slashes, Unicode, empty segments, leading or trailing hyphens, uppercase input, and case folding are forbidden.

`id`, `name`, `author`, `base`, and `radia` are required top-level fields. `url`, `notes`, and `preview` are optional. Missing required fields never receive placeholder defaults.

A root Skin with `base: null` must declare `stylesheet`, `layouts`, `localization`, and `assets`. A derived Skin may declare any nonempty subset of those entries; omitted resource classes come from its Base Skin chain. Requiring at least one Radia resource entry prevents meaningless derived manifests while avoiding empty placeholder files and directories.

A derived Skin references the exact installed Skin ID:

```json
{
  "id": "bruno.midnight",
  "name": "Midnight",
  "author": "Bruno",
  "base": "alchemy.default",
  "radia": {
    "stylesheet": "rdui/skin.radia"
  }
}
```

Resolution walks that one explicit chain base-first. Resource classes layer as follows:

- Layout files use their extension-preserving path relative to `radia.layouts` as
  identity. A derived file replaces the complete Base Skin file at the same path;
  other Base Skin layouts remain available.
- Stylesheet entrypoints are compiled base-first into one cascade. Later derived rules
  therefore win only through normal specificity and source-order rules.
- Localization catalogs merge by Locale ID and String Key base-first. A derived string
  replaces the same Base Skin locale/string pair; locale metadata is likewise
  replaced by the later declaration. Every layer uses the same `defaultLocale`.
- Assets use their path relative to `radia.assets` as identity. A derived file replaces
  the complete Base Skin asset at the same path.

Replacement is atomic, not a parse-success fallback. If a derived layout or asset
exists but is malformed, the candidate Skin Generation is rejected; the resolver does
not reveal the Base Skin file that it replaced. Skins outside the explicit chain never
contribute resources.

`radia.stylesheet` identifies one RSL entrypoint; that file composes modules using
Skin-local `@import` directives. Import paths resolve relative to the importing module,
must remain inside that same Skin, and never fall through to a Base Skin module with
the same path. Modules are discovered from the immutable Skin snapshot and are not
listed in the manifest. `radia.layouts` is the root of XML Layout Resources.
`radia.localization` identifies the strict YAML localization entrypoint.
`radia.assets` is the root of
supported icons and other visual resources. Shaders, active Color Scheme,
authoring-mode state, and RSL module lists do not belong in the manifest.

System discovers `radia.assets` recursively. Individual assets are never enumerated in viewer C++. An SVG below `icons/` receives its extensionless relative path as its case-sensitive resource name: `icons/actions/search.svg` is referenced by XML as `actions/search`. Adding an icon requires only the asset file and its XML reference. XML references to missing assets reject that View; unsupported or invalid files reject the candidate Skin Generation.

The supported icon format is deliberately small and strict. The root must be `<svg>` with a positive four-number `viewBox`. Icons may contain `<path d="...">` and `<circle cx="..." cy="..." r="...">`; supported path commands are `M`, `L`, `H`, `V`, `Q`, `C`, and `Z`, including their relative forms. Root presentation metadata may specify numeric `width`, `height`, and `stroke-width`, `fill="none"`, `stroke="currentColor"`, `stroke-linecap` (`butt`, `round`, or `square`), and `stroke-linejoin="round"`. Unsupported elements, attributes, commands, malformed numbers, and partial paths are errors with source locations. A failed icon exposes no partial compiled resource.

Manifest paths are Skin-relative and must remain inside the Skin root after canonicalization. Absolute paths, URLs in resource fields, traversal outside the Skin, missing required resources, unknown keys, and wrong JSON types are errors. Manifest and all declared resources validate atomically as part of a candidate Skin Generation.

The manifest has no Radia compatibility version. Every installed Skin targets the viewer's single rolling resource contract.

Normal installation rejects a Skin ID that is already installed. Replacing an installed Skin requires an explicit update action: unpack into staging, validate the complete resolved Skin Generation, then atomically replace the installed directory. Failure preserves both the installed files and last valid active generation. The installer never silently overwrites a Skin or creates two installed Skins with the same ID.
