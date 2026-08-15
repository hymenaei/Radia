/**
 * @file localization_test.cpp
 * @brief Tests locale catalogs, fallback, plural, and localized text behavior.
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
struct localizationData {};
using localizationTest = test_group<localizationData>;
using localizationObject = localizationTest::object;
localizationTest localizationTestCase("localization");

template<> template<> void localizationObject::test<1>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {title: Title, fallback: Fallback}}, pt: {name: Português, strings: {title: Título}}, ar: {name: العربية, direction: rtl, fallback: pt, strings: {}}}\n";
    const auto result = catalog.loadYaml(kLocalization, "localization.yaml");

    ensure("valid localization loads", result.ok());
    ensure_equals("locale count retained", catalog.locales().size(), 3U);
    ensure_equals("locale enumeration ignores YAML authoring order", catalog.locales().front().localeId, "ar");
    ensure_equals("native UTF-8 name preserved", catalog.locale("PT")->name, "Português");
    ensure_equals("omitted direction is ltr", static_cast<int>(catalog.locale("pt")->direction),
                  static_cast<int>(radia::ui::LayoutDirection::LeftToRight));
    ensure_equals("rtl direction retained", static_cast<int>(catalog.locale("ar")->direction), static_cast<int>(radia::ui::LayoutDirection::RightToLeft));
    ensure_equals("declared default retained", catalog.defaultLocaleId(), "en");
    ensure_equals("explicit locale translation resolved", catalog.get("pt", "title"), "Título");
    ensure_equals("direct default fallback resolved", catalog.get("pt", "fallback"), "Fallback");
    ensure_equals("explicit fallback chain resolved", catalog.get("ar", "title"), "Título");
    ensure_equals("globally missing string shows key", catalog.get("pt", "missingKey"), "missingKey");
    ensure("locale identity is case-insensitive", catalog.containsLocale("AR"));
}

template<> template<> void localizationObject::test<2>() {
    radia::ui::LocalizationCatalog catalog;
    ensure("initial catalog loads", catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {value: live}}}\n").ok());

    const auto duplicate = catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {value: first, value: second}}}\n");
    ensure("duplicate entries reject catalog", !duplicate.ok());
    ensure_equals("failed candidate preserves live catalog", catalog.get("en", "value"), "live");
}

template<> template<> void localizationObject::test<3>() {
    radia::ui::LocalizationCatalog catalog;
    const auto malformed = catalog.loadYaml("defaultLocale: [", "broken.yaml");
    ensure("malformed YAML rejected", !malformed.ok());
    ensure_equals("diagnostic has stable code", malformed.errors.front().code, "localization.yaml.invalid");
    ensure_equals("diagnostic identifies source", malformed.errors.front().source, "broken.yaml");
    ensure("wrong root rejected", !catalog.loadYaml("- en").ok());
    ensure("missing default rejected", !catalog.loadYaml("locales: {en: {name: English, strings: {}}}\n").ok());
    ensure("empty catalog rejected", !catalog.loadYaml("defaultLocale: en\nlocales: {}\n").ok());
    ensure("implicit numeric value rejected", !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {version: 1}}}\n").ok());
    ensure("quoted numeric value accepted", catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {version: \"1\"}}}\n").ok());
}

template<> template<> void localizationObject::test<4>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kMissingDefaultLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {}}, pt: {name: Português, strings: {onlyPt: Só}}}\n";
    const auto missingDefaultKey = catalog.loadYaml(kMissingDefaultLocalization, "localization.yaml");
    ensure("translation absent from default locale rejects catalog", !missingDefaultKey.ok());
    ensure("missing default key diagnostic is stable",
           std::any_of(missingDefaultKey.errors.begin(), missingDefaultKey.errors.end(),
                       [](const radia::ui::Diagnostic& diagnostic) { return diagnostic.code == "localization.default.string_missing"; }));

    ensure("snake-case String Key rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {invalid_key: Value}}}\n").ok());
    ensure("single lowerCamelCase segment accepted",
           catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {commonReady: Ready, runtimeUi.level: Level}}}\n").ok());
}

template<> template<> void localizationObject::test<5>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kCyclicFallbackLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {}}, pt: {name: Português, fallback: es, strings: {}}, es: {name: Español, fallback: pt, strings: {}}}\n";
    ensure("fallback cycles reject catalog", !catalog.loadYaml(kCyclicFallbackLocalization).ok());
    ensure("unknown fallback rejects catalog",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {}}, pt-BR: {name: Português, fallback: pt, strings: {}}}\n")
                .ok());
    ensure("noncanonical Locale ID rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {}}, pt-br: {name: Português, strings: {}}}\n").ok());
}

template<> template<> void localizationObject::test<6>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kDerivedLocalization =
        "locales: {en: {strings: {title: Derived, addedByLayer: Added}}, ar: {name: العربية, direction: rtl, strings: {title: مشتق}}}\n";
    const auto result = catalog.loadYamlLayers({
        {"base/localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {title: Base}}}\n"},
        {"derived/localization.yaml", kDerivedLocalization},
    });

    ensure("localization layers merge", result.ok());
    ensure_equals("derived default String replaces base", catalog.get("en", "title"), "Derived");
    ensure_equals("derived default locale may add a key", catalog.get("en", "addedByLayer"), "Added");
    ensure_equals("new locale inherits new default key", catalog.get("ar", "addedByLayer"), "Added");
    ensure_equals("new locale value resolves", catalog.get("ar", "title"), "مشتق");

    ensure("override defaultLocale rejected",
           !catalog
                .loadYamlLayers({
                    {"base/localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n"},
                    {"derived/localization.yaml", "defaultLocale: en\nlocales: {}\n"},
                })
                .ok());
}

template<> template<> void localizationObject::test<7>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kPluralLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {itemCount: {one: \"You have {count} item\", other: \"You have {count} items\"}, sheepCount: \"You have {count} sheep\"}}, ar: {name: العربية, direction: rtl, strings: {itemCount: {zero: \"لديك {count} عناصر\", one: \"لديك عنصر واحد\", two: \"لديك عنصران\", few: \"لديك {count} عناصر\", many: \"لديك {count} عنصرًا\", other: \"لديك {count} عنصر\"}, sheepCount: {one: \"{count} خروف\", other: \"{count} خراف\"}}}}\n";
    ensure("plural catalog loads", catalog.loadYaml(kPluralLocalization).ok());

    const radia::ui::LocalizationRequest items = radia::ui::LocalizationRequest::plural("itemCount", "count", 2);
    const std::string english = catalog.get("en", items);
    ensure("English other category selected", english.find("items") != std::string::npos);
    const std::string arabic = catalog.get("ar", items);
    ensure("Arabic two category selected", arabic.find("عنصران") != std::string::npos);

    const radia::ui::LocalizationRequest sheep = radia::ui::LocalizationRequest::plural("sheepCount", "count", 2);
    ensure("scalar acts as universal plural form", catalog.get("en", sheep).find("sheep") != std::string::npos);

    ensure_equals("missing plural selector displays raw key", catalog.get("en", "itemCount"), "itemCount");
    ensure("missing other category rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {itemCount: {one: One}}}}\n").ok());
    ensure("exact plural selector rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {itemCount: {\"=0\": None, other: Other}}}}\n").ok());
}

template<> template<> void localizationObject::test<8>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kRichLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {rich: 'Text <b>{userName}</b><br/><kbd shortcut=\"toggle-fly\"/>', escaped: 'Write \\{count} and \\<b>'}}}\n";
    ensure("Rich Strings load", catalog.loadYaml(kRichLocalization).ok());

    radia::ui::LocalizationArguments arguments;
    arguments.emplace("userName", "<b>Bruno</b>");
    const radia::ui::LocalizationRequest rich = radia::ui::LocalizationRequest::text("rich", std::move(arguments));
    const radia::ui::InlineContent resolved = catalog.resolve("en", rich);
    ensure_equals("Rich String has four root nodes", resolved.nodes().size(), 4U);
    ensure_equals("bold node retained", static_cast<int>(resolved.nodes()[1].kind()), static_cast<int>(radia::ui::InlineContentKind::B));
    ensure("inserted argument remains text", resolved.nodes()[1].children().front().value().find("<b>Bruno</b>") != std::string::npos);
    ensure_equals("escaped syntax renders literally", catalog.get("en", "escaped"), "Write {count} and <b>");

    ensure("unknown tag rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {value: \"<script>bad</script>\"}}}\n").ok());
    const char* kMismatchedKbdLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {shortcut: '<kbd shortcut=\"toggle-fly\"/>'}}, pt: {name: Português, strings: {shortcut: '<kbd shortcut=\"open-map\"/>'}}}\n";
    ensure("mismatched kbd binding rejected", !catalog.loadYaml(kMismatchedKbdLocalization).ok());
}

template<> template<> void localizationObject::test<9>() {
    radia::ui::LocalizationCatalog catalog;
    std::string decomposed = "defaultLocale: en\nlocales: {en: {name: English, strings: {value: \"Cafe";
    decomposed += "\xCC\x81";
    decomposed += "\"\n";
    ensure("non-NFC text rejected", !catalog.loadYaml(decomposed).ok());

    ensure("tab in localized text rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {value: \"a\\tb\"}}}\n").ok());

    ensure("anchors and aliases rejected",
           !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {first: &shared Shared, second: *shared}}}\n").ok());
    ensure("custom tags rejected", !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {value: !custom Text}}}\n").ok());
    ensure("merge keys rejected", !catalog.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {<<: {value: Text}}}}\n").ok());
}

template<> template<> void localizationObject::test<10>() {
    radia::ui::LocalizationCatalog catalog;
    const char* kPluralTranslation =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {onlineFriends: \"Friends online\"}}, ru: {name: Русский, strings: {onlineFriends: {one: \"{count} друг в сети\", few: \"{count} друга в сети\", many: \"{count} друзей в сети\", other: \"{count} друга в сети\"}}}}\n";
    ensure("translation may use plural selector omitted by default", catalog.loadYaml(kPluralTranslation).ok());

    const radia::ui::LocalizationRequest request = radia::ui::LocalizationRequest::plural("onlineFriends", "count", 2);
    ensure("implicit selector argument formats", catalog.get("ru", request).find("друга") != std::string::npos);
    const radia::ui::LocalizationRequest wrongRequest = radia::ui::LocalizationRequest::plural("onlineFriends", "otherCount", 2);
    ensure_equals("wrong selector name displays raw key", catalog.get("ru", wrongRequest), "onlineFriends");
}
} // namespace tut
