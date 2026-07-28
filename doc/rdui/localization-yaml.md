# Radia localization YAML

Status: implemented version-one authoring model, defined by the localization grill.

## Base catalog

`localization.yaml` is the sole canonical UTF-8 localization source.

```yaml
defaultLocale: en

locales:
  en:
    name: English
    strings:
      commonReady: "Ready"
      runtimeUi.level: "Hole <b>{holeNumber}</b>"
      inventory.itemCount:
        one: "You have {count} item"
        other: "You have {count} items"

  pt-PT:
    name: Português (Portugal)
    strings:
      commonReady: "Pronto"

  pt-BR:
    name: Português (Brasil)
    fallback: pt-PT
    strings: {}

  ar:
    name: العربية
    direction: rtl
    strings:
      commonReady: "جاهز"
```

The root has no schema-version field for now. Unknown fields and duplicate YAML keys are errors. Locale mapping order has no meaning; the language picker owns its ordering.

## YAML profile

The file uses a strict YAML 1.2 subset:

- the source must be valid UTF-8;
- authored Unicode text must be NFC-normalized; validation rejects non-NFC text rather than silently rewriting it;
- comments and block scalars are supported;
- duplicate keys are errors;
- anchors, aliases, merge keys, and custom tags are forbidden;
- localized values must be YAML strings or Plural String maps, never implicit numbers, booleans, or nulls;
- empty and null localized values are errors;
- intentional decoded line breaks are allowed, while tabs, NUL, and other low-level control characters in catalog text are errors.

YAML comments are the version-one mechanism for translator notes. Structured translation metadata is deferred.

## Locales

`defaultLocale` is required in a base catalog and must identify an entry in `locales`. It defines the authoritative String Key set, has no `fallback`, and is the ultimate fallback.

Locale map keys are valid canonical BCP 47 tags, such as `en`, `pt-PT`, `pt-BR`, or `zh-Hant`. Locale identity is case-insensitive, but authored tags use canonical casing. Private-use BCP 47 tags are permitted.

Each locale has:

- required `name`: a plain-text autonym used by the language picker; no markup or placeholders;
- optional `direction`: `ltr` or `rtl`; omission means `ltr`, with no inference from the Locale ID;
- optional `fallback`: one explicit parent Locale ID; omission means direct fallback to `defaultLocale`;
- required `strings`: the locale's flat String map, written as `{}` when intentionally empty.

Fallback chains must reference known locales, contain no cycles, and end at `defaultLocale`. Fallback resolves a whole String Key. It never fills one Plural Category from another locale.

Future locale-specific `resources` are reserved for later design. A flag must not be assumed to represent every language, script, or regional locale correctly.

## String Keys

String Keys are case-sensitive lower-camel-case identifiers with one or more optionally dot-separated segments:

```text
commonReady
runtimeUi.level
inventory.itemCount
```

Each segment matches `[a-z][A-Za-z0-9]*`. Initialisms are treated as words (`runtimeUi`, `imageUrl`, `avatarId`), not uppercase runs. Periods organize names but do not create YAML hierarchy.

The effective default locale defines the allowed String Keys. Other locales cannot introduce a key absent from that effective default locale.

## Plain and Rich Strings

A scalar value is a plain or Rich String:

```yaml
commonReady: "Ready"
runtimeUi.level: "Hole <b>{holeNumber}</b>"
```

Rich Strings use bounded Radia Inline Content markup, not HTML or Markdown. Version one supports exactly:

- `<b>...</b>`;
- `<i>...</i>`;
- `<s>...</s>`;
- `<br/>`;
- `<kbd binding="lowercase-kebab-case-binding-id"/>`.

Tags are lowercase and case-sensitive, nesting must be well formed, and undocumented attributes are errors. General HTML, styling, scripts, images, events, and links are unsupported.

Parsing is token-aware. Only a valid String Placeholder or recognized Radia tag is special; ordinary `<`, `>`, `&`, and non-placeholder braces remain literal:

```yaml
comparison: "2 < 3 && values are {1, 2, 3}"
```

A backslash suppresses recognition when syntax must be displayed:

```yaml
placeholderHelp: 'Write \{count} to show the placeholder syntax'
markupHelp: 'Use \<b> and \</b> for bold markup'
```

`\\` represents a literal backslash. Single-quoted or block YAML scalars avoid YAML double-quote backslash processing.

Translations may change or reposition `b`, `i`, `s`, and `br`. They must preserve the default String's exact multiset of `kbd` Binding IDs.

## Whitespace

Radia preserves the YAML-decoded scalar exactly and performs no later trimming or whitespace collapsing. Each preserved newline becomes an Inline Content line break.

Use folded block scalars for source-wrapped prose:

```yaml
longHint: >-
  Click <b>Save</b> to apply your changes,
  then close the window.
```

Use literal block scalars for intentional rendered lines:

```yaml
twoLines: |-
  First line
  Second line
```

The `-` chomping indicator is recommended to avoid an accidental trailing newline.

## String Arguments

String Placeholders use untyped lower-camel-case names without dots:

```yaml
runtimeUi.level: "Hole {holeNumber}"
downloadStatus: "Downloaded {count} files for {userName}"
```

Runtime values carry their semantic types. Numeric arguments use Active Locale number formatting; callers supply text for identifiers or exact presentation. Inserted values are text, cannot inject markup, and receive automatic Unicode bidirectional isolation.

The union of placeholders in the default String's variants defines that String Key's allowed arguments. A translation or variant may omit an allowed argument, but cannot introduce another name. For a plural-capable key, the explicitly supplied plural-driving argument is the sole exception: every locale may use it even when the default String does not display it.

If a runtime call omits a used argument, Radia logs an error and leaves its `{argumentName}` visible; tests and debug validation fail. Unused extra runtime arguments are ignored.

## Plural Strings

A map containing CLDR Plural Categories is a cardinal Plural String:

```yaml
inventory.itemCount:
  one: "You have {count} item"
  other: "You have {count} items"
```

Allowed keys are `zero`, `one`, `two`, `few`, `many`, and `other`. `other` is required. Other categories are optional:

- the Active Locale's CLDR cardinal rules select a category;
- an omitted selected category uses `other`;
- a valid category unused by that locale is allowed;
- an unknown category name is an error;
- exact numeric selectors such as `=0` are unsupported.

The runtime call explicitly identifies which numeric String Argument drives plural selection. Version one permits exactly one plural-driving argument per String Key; Radia does not infer it merely because one or more numeric arguments are present. If any locale in the fully merged catalog defines a key as a Plural String map, every runtime request for that key must supply its plural-selection argument, even when the Active Locale resolves to a scalar. A scalar then acts as the universal form for every count. Omitting the required selector is a validation error where detectable; otherwise Radia logs the runtime error and displays the raw String Key.

A Plural String is atomic: if a locale defines the key, the map must itself be valid and no category comes from Locale Fallback.

The same String Key may be a scalar in one locale and a Plural String map in another:

```yaml
en:
  strings:
    sheepCount: "You have {count} sheep"

ru:
  strings:
    sheepCount:
      one: ...
      few: ...
      many: ...
      other: ...
```

The normal argument rule still applies: apart from the plural-driving argument, the nondefault Plural String cannot introduce a placeholder absent from the default String.

Multiple or nested plural selections within one String Key are deferred. Authors must not concatenate translated fragments to imitate them; use separate complete UI lines where the interface permits. Ordinal selection (`1st`, `2nd`, `3rd`, and similar) is also a known future requirement but has no version-one syntax.

## Layout and runtime failure

Text-bearing Layout Resource positions use one bare String Key:

```xml
<text>runtimeUi.level</text>
<button onClick="save">common.save</button>
```

No reference sigil is added. Complete sentence structure and inline markup live in `localization.yaml`; `{argument}` remains String Placeholder syntax inside localized values.

Skin validation rejects missing static layout references. An unknown dynamic runtime key logs an error and displays the raw String Key instead of crashing.

## Skin layers

The Base Skin catalog is complete and declares `defaultLocale`. A derived Skin localization layer is partial:

```yaml
locales:
  en:
    strings:
      commonSave: "Store"
```

Layer rules:

- override layers omit `defaultLocale` and inherit it;
- an existing locale override contains changed `strings` and inherits locale metadata;
- a layer may add a locale, which requires `name` and `strings` and may declare `direction` and `fallback`;
- a derived default-locale section may add String Keys needed by derived layouts;
- omitted locales and String Keys inherit;
- an authored String Key replaces the whole inherited value; Plural Categories never merge across layers;
- null cannot delete a locale, String Key, or Plural Category;
- the fully merged catalog must satisfy all base-catalog validation rules before publication.
