/**
 * @file localization_test.cpp
 * @brief
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include <algorithm>
#include "../test/lltut.h"
#include "localization/localization.h"

namespace tut {
struct rduilocalization_data {};
typedef test_group<rduilocalization_data> rduilocalization_test;
typedef rduilocalization_test::object rduilocalization_object;
rduilocalization_test rduilocalization_testcase("rduilocalization");

template<> template<> void rduilocalization_object::test<1>() {
    rdui::LocalizationCatalog catalog;
    const auto result = catalog.loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      title: Title
      fallback: Fallback
  pt:
    name: Português
    strings:
      title: Título
  ar:
    name: العربية
    direction: rtl
    fallback: pt
    strings: {}
)YAML",
                                         "localization.yaml");

    ensure("valid localization loads", result.ok());
    ensure_equals("locale count retained", catalog.locales().size(), 3U);
    ensure_equals("locale enumeration ignores YAML authoring order", catalog.locales().front().id, "ar");
    ensure_equals("native UTF-8 name preserved", catalog.locale("PT")->name, "Português");
    ensure_equals("omitted direction is ltr", static_cast<int>(catalog.locale("pt")->direction),
                  static_cast<int>(rdui::LayoutDirection::LeftToRight));
    ensure_equals("rtl direction retained", static_cast<int>(catalog.locale("ar")->direction), static_cast<int>(rdui::LayoutDirection::RightToLeft));
    ensure_equals("declared default retained", catalog.defaultLocaleId(), "en");
    ensure_equals("explicit locale translation resolved", catalog.get("pt", "title"), "Título");
    ensure_equals("direct default fallback resolved", catalog.get("pt", "fallback"), "Fallback");
    ensure_equals("explicit fallback chain resolved", catalog.get("ar", "title"), "Título");
    ensure_equals("globally missing string shows key", catalog.get("pt", "missingKey"), "missingKey");
    ensure("locale identity is case-insensitive", catalog.containsLocale("AR"));
}

template<> template<> void rduilocalization_object::test<2>() {
    rdui::LocalizationCatalog catalog;
    ensure("initial catalog loads",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      value: live
)YAML")
               .ok());

    const auto duplicate = catalog.loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      value: first
      value: second
)YAML");
    ensure("duplicate entries reject catalog", !duplicate.ok());
    ensure_equals("failed candidate preserves live catalog", catalog.get("en", "value"), "live");
}

template<> template<> void rduilocalization_object::test<3>() {
    rdui::LocalizationCatalog catalog;
    const auto malformed = catalog.loadYaml("defaultLocale: [", "broken.yaml");
    ensure("malformed YAML rejected", !malformed.ok());
    ensure_equals("diagnostic has stable code", malformed.errors.front().code, "localization.yaml.invalid");
    ensure_equals("diagnostic identifies source", malformed.errors.front().source, "broken.yaml");
    ensure("wrong root rejected", !catalog.loadYaml("- en").ok());
    ensure("missing default rejected",
           !catalog
                .loadYaml(R"YAML(
locales:
  en:
    name: English
    strings: {}
)YAML")
                .ok());
    ensure("empty catalog rejected", !catalog.loadYaml("defaultLocale: en\nlocales: {}\n").ok());
    ensure("implicit numeric value rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      version: 1
)YAML")
                .ok());
    ensure("quoted numeric value accepted",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      version: "1"
)YAML")
               .ok());
}

template<> template<> void rduilocalization_object::test<4>() {
    rdui::LocalizationCatalog catalog;
    const auto missing_default_key = catalog.loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
  pt:
    name: Português
    strings:
      onlyPt: Só
)YAML",
                                                      "localization.yaml");
    ensure("translation absent from default locale rejects catalog", !missing_default_key.ok());
    ensure("missing default key diagnostic is stable",
           std::any_of(missing_default_key.errors.begin(), missing_default_key.errors.end(),
                       [](const rdui::Diagnostic& diagnostic) { return diagnostic.code == "localization.default.string_missing"; }));

    ensure("snake-case String Key rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      invalid_key: Value
)YAML")
                .ok());
    ensure("single lowerCamelCase segment accepted",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      commonReady: Ready
      runtimeUi.level: Level
)YAML")
               .ok());
}

template<> template<> void rduilocalization_object::test<5>() {
    rdui::LocalizationCatalog catalog;
    ensure("fallback cycles reject catalog",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
  pt:
    name: Português
    fallback: es
    strings: {}
  es:
    name: Español
    fallback: pt
    strings: {}
)YAML")
                .ok());
    ensure("unknown fallback rejects catalog",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
  pt-BR:
    name: Português
    fallback: pt
    strings: {}
)YAML")
                .ok());
    ensure("noncanonical Locale ID rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
  pt-br:
    name: Português
    strings: {}
)YAML")
                .ok());
}

template<> template<> void rduilocalization_object::test<6>() {
    rdui::LocalizationCatalog catalog;
    const auto result = catalog.loadYamlLayers({
        {"base/localization.yaml", R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      title: Base
)YAML"},
        {"derived/localization.yaml", R"YAML(
locales:
  en:
    strings:
      title: Derived
      addedByLayer: Added
  ar:
    name: العربية
    direction: rtl
    strings:
      title: مشتق
)YAML"},
    });

    ensure("localization layers merge", result.ok());
    ensure_equals("derived default String replaces base", catalog.get("en", "title"), "Derived");
    ensure_equals("derived default locale may add a key", catalog.get("en", "addedByLayer"), "Added");
    ensure_equals("new locale inherits new default key", catalog.get("ar", "addedByLayer"), "Added");
    ensure_equals("new locale value resolves", catalog.get("ar", "title"), "مشتق");

    ensure("override defaultLocale rejected",
           !catalog
                .loadYamlLayers({
                    {"base/localization.yaml", R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
)YAML"},
                    {"derived/localization.yaml", R"YAML(
defaultLocale: en
locales: {}
)YAML"},
                })
                .ok());
}

template<> template<> void rduilocalization_object::test<7>() {
    rdui::LocalizationCatalog catalog;
    ensure("plural catalog loads",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      itemCount:
        one: "You have {count} item"
        other: "You have {count} items"
      sheepCount: "You have {count} sheep"
  ar:
    name: العربية
    direction: rtl
    strings:
      itemCount:
        zero: "لديك {count} عناصر"
        one: "لديك عنصر واحد"
        two: "لديك عنصران"
        few: "لديك {count} عناصر"
        many: "لديك {count} عنصرًا"
        other: "لديك {count} عنصر"
      sheepCount:
        one: "{count} خروف"
        other: "{count} خراف"
)YAML")
               .ok());

    const rdui::LocalizationRequest items = rdui::LocalizationRequest::plural("itemCount", "count", 2);
    const std::string english = catalog.get("en", items);
    ensure("English other category selected", english.find("items") != std::string::npos);
    const std::string arabic = catalog.get("ar", items);
    ensure("Arabic two category selected", arabic.find("عنصران") != std::string::npos);

    const rdui::LocalizationRequest sheep = rdui::LocalizationRequest::plural("sheepCount", "count", 2);
    ensure("scalar acts as universal plural form", catalog.get("en", sheep).find("sheep") != std::string::npos);

    ensure_equals("missing plural selector displays raw key", catalog.get("en", "itemCount"), "itemCount");
    ensure("missing other category rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      itemCount:
        one: One
)YAML")
                .ok());
    ensure("exact plural selector rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      itemCount:
        "=0": None
        other: Other
)YAML")
                .ok());
}

template<> template<> void rduilocalization_object::test<8>() {
    rdui::LocalizationCatalog catalog;
    ensure("Rich Strings load",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      rich: 'Text <b>{userName}</b><br/><kbd binding="toggle-fly"/>'
      escaped: 'Write \{count} and \<b>'
)YAML")
               .ok());

    rdui::LocalizationArguments arguments;
    arguments.emplace("userName", "<b>Bruno</b>");
    const rdui::LocalizationRequest rich = rdui::LocalizationRequest::text("rich", std::move(arguments));
    const rdui::InlineContent resolved = catalog.resolve("en", rich);
    ensure_equals("Rich String has four root nodes", resolved.nodes().size(), 4U);
    ensure_equals("bold node retained", static_cast<int>(resolved.nodes()[1].kind()), static_cast<int>(rdui::InlineContentKind::B));
    ensure("inserted argument remains text", resolved.nodes()[1].children().front().value().find("<b>Bruno</b>") != std::string::npos);
    ensure_equals("escaped syntax renders literally", catalog.get("en", "escaped"), "Write {count} and <b>");

    ensure("unknown tag rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      value: "<script>bad</script>"
)YAML")
                .ok());
    ensure("mismatched kbd binding rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      shortcut: '<kbd binding="toggle-fly"/>'
  pt:
    name: Português
    strings:
      shortcut: '<kbd binding="open-map"/>'
)YAML")
                .ok());
}

template<> template<> void rduilocalization_object::test<9>() {
    rdui::LocalizationCatalog catalog;
    std::string decomposed = R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      value: "Cafe)YAML";
    decomposed += "\xCC\x81";
    decomposed += "\"\n";
    ensure("non-NFC text rejected", !catalog.loadYaml(decomposed).ok());

    ensure("tab in localized text rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales:\n  en:\n    name: English\n    strings:\n      value: \"a\\tb\"\n").ok());

    ensure("anchors and aliases rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      first: &shared Shared
      second: *shared
)YAML")
                .ok());
    ensure("custom tags rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      value: !custom Text
)YAML")
                .ok());
    ensure("merge keys rejected",
           !catalog
                .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      <<: { value: Text }
)YAML")
                .ok());
}

template<> template<> void rduilocalization_object::test<10>() {
    rdui::LocalizationCatalog catalog;
    ensure("translation may use plural selector omitted by default",
           catalog
               .loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      onlineFriends: "Friends online"
  ru:
    name: Русский
    strings:
      onlineFriends:
        one: "{count} друг в сети"
        few: "{count} друга в сети"
        many: "{count} друзей в сети"
        other: "{count} друга в сети"
)YAML")
               .ok());

    const rdui::LocalizationRequest request = rdui::LocalizationRequest::plural("onlineFriends", "count", 2);
    ensure("implicit selector argument formats", catalog.get("ru", request).find("друга") != std::string::npos);
    const rdui::LocalizationRequest wrong_request = rdui::LocalizationRequest::plural("onlineFriends", "otherCount", 2);
    ensure_equals("wrong selector name displays raw key", catalog.get("ru", wrong_request), "onlineFriends");
}
} // namespace tut
