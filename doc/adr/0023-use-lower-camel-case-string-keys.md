# Use lower-camel-case String Keys

Radia localization will use case-sensitive lower-camel-case String Keys composed of one or more optionally dot-separated segments, such as `commonReady`, `runtimeUi.level`, and `imageUrl.invalid`; initialisms are treated as ordinary words. This deliberately replaces the prototype catalog's snake-case keys while migration is still inexpensive and aligns localization identifiers with Radia's authored schema naming without requiring every key to have a namespace.
