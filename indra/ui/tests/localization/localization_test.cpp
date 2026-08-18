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
#include <gtest/gtest.h>
#include "localization/localization.h"

namespace {
using radia::ui::InlineContentKind;
using radia::ui::LayoutDirection;
using radia::ui::LocalizationArguments;
using radia::ui::LocalizationCatalog;
using radia::ui::LocalizationRequest;
using radia::ui::ResourceLayer;
using ::testing::Message;

constexpr char kCatalogWithFallbacks[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {title: Title, fallback: Fallback}}, "
                                         "pt: {name: Português, strings: {title: Título}}, "
                                         "ar: {name: العربية, direction: rtl, fallback: pt, strings: {}}}\n";

constexpr char kPluralCatalog[] = "defaultLocale: en\n"
                                  "locales: {en: {name: English, strings: {"
                                  "itemCount: {one: \"You have {count} item\", other: \"You have {count} items\"}, "
                                  "sheepCount: \"You have {count} sheep\"}}, "
                                  "ar: {name: العربية, direction: rtl, strings: {"
                                  "itemCount: {zero: \"لديك {count} عناصر\", one: \"لديك عنصر واحد\", "
                                  "two: \"لديك عنصران\", few: \"لديك {count} عناصر\", "
                                  "many: \"لديك {count} عنصرًا\", other: \"لديك {count} عنصر\"}, "
                                  "sheepCount: {one: \"{count} خروف\", other: \"{count} خراف\"}}}}\n";

constexpr char kRichTextCatalog[] = "defaultLocale: en\n"
                                    "locales: {en: {name: English, strings: {"
                                    "rich: 'Text <b>{userName}</b><br/><kbd shortcut=\"toggle-fly\"/>', "
                                    "escaped: 'Write \\{count} and \\<b>'}}}\n";
} // namespace

TEST(LocalizationCatalogTest, LoadsLocaleMetadataAndSupportsCaseInsensitiveLookup) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kCatalogWithFallbacks, "localization.yaml").ok());

    const auto locales = catalog.locales();
    ASSERT_EQ(locales.size(), std::size_t(3));
    EXPECT_EQ(locales.front().localeId, "ar");
    EXPECT_EQ(catalog.defaultLocaleId(), "en");
    EXPECT_TRUE(catalog.containsLocale("AR"));

    const auto* portuguese = catalog.locale("PT");
    ASSERT_NE(portuguese, nullptr);
    EXPECT_EQ(portuguese->name, "Português");
    EXPECT_EQ(portuguese->direction, LayoutDirection::LeftToRight);

    const auto* arabic = catalog.locale("ar");
    ASSERT_NE(arabic, nullptr);
    EXPECT_EQ(arabic->direction, LayoutDirection::RightToLeft);
}

TEST(LocalizationCatalogTest, ResolvesTranslationsThroughDefaultAndExplicitFallbacks) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kCatalogWithFallbacks).ok());

    EXPECT_TRUE(catalog.containsDefaultString("title"));
    EXPECT_FALSE(catalog.containsDefaultString("missingKey"));
    EXPECT_EQ(catalog.get("pt", "title"), "Título");
    EXPECT_EQ(catalog.get("pt", "fallback"), "Fallback");
    EXPECT_EQ(catalog.get("ar", "title"), "Título");
    EXPECT_EQ(catalog.get("pt", "missingKey"), "missingKey");
}

TEST(LocalizationCatalogTest, PreservesCommittedCatalogWhenReplacementFails) {
    constexpr char kCommittedCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {value: live}}}\n";

    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kCommittedCatalog).ok());

    constexpr char kDuplicateValueCatalog[] = "defaultLocale: en\n"
                                              "locales: {en: {name: English, strings: "
                                              "{value: first, value: second}}}\n";
    const auto duplicate = catalog.loadYaml(kDuplicateValueCatalog);

    EXPECT_FALSE(duplicate.ok());
    EXPECT_EQ(catalog.get("en", "value"), "live");
}

TEST(LocalizationCatalogTest, ReportsMalformedYamlWithSourceDiagnostic) {
    LocalizationCatalog catalog;
    constexpr char kMalformedCatalog[] = "defaultLocale: [";
    const auto result = catalog.loadYaml(kMalformedCatalog, "broken.yaml");

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "localization.yaml.invalid");
    EXPECT_EQ(result.errors.front().source, "broken.yaml");
}

TEST(LocalizationCatalogTest, RejectsInvalidCatalogShapes) {
    struct InvalidCatalogCase {
        const char* name;
        const char* yaml;
    };

    const InvalidCatalogCase cases[] = {
        {"sequence root", "- en"},
        {"missing default locale", "locales: {en: {name: English, strings: {}}}\n"},
        {"empty locale map", "defaultLocale: en\nlocales: {}\n"},
        {"implicit numeric value", "defaultLocale: en\nlocales: {en: {name: English, strings: {version: 1}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid catalog shape: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_FALSE(catalog.loadYaml(test.yaml).ok());
    }
}

TEST(LocalizationCatalogTest, AcceptsQuotedNumericTextValues) {
    constexpr char kQuotedNumericCatalog[] = "defaultLocale: en\n"
                                             "locales: {en: {name: English, strings: {version: \"1\"}}}\n";

    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kQuotedNumericCatalog).ok());

    EXPECT_EQ(catalog.get("en", "version"), "1");
}

TEST(LocalizationCatalogTest, RejectsLocalesMissingDefaultTranslations) {
    constexpr char kMissingDefaultTranslationCatalog[] = "defaultLocale: en\n"
                                                         "locales: {en: {name: English, strings: {}}, "
                                                         "pt: {name: Português, strings: "
                                                         "{onlyPt: Só}}}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYaml(kMissingDefaultTranslationCatalog, "localization.yaml");

    ASSERT_FALSE(result.ok());
    EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(),
                            [](const radia::ui::Diagnostic& diagnostic) { return diagnostic.code == "localization.default.string_missing"; }));
}

TEST(LocalizationCatalogTest, RejectsSnakeCaseStringKeys) {
    constexpr char kSnakeCaseKeyCatalog[] = "defaultLocale: en\n"
                                            "locales: {en: {name: English, strings: {invalid_key: Value}}}\n";

    LocalizationCatalog catalog;

    EXPECT_FALSE(catalog.loadYaml(kSnakeCaseKeyCatalog).ok());
}

TEST(LocalizationCatalogTest, AcceptsLowerCamelCaseKeysWithDottedSegments) {
    constexpr char kDottedKeyCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: "
                                         "{commonReady: Ready, runtimeUi.level: Level}}}\n";

    LocalizationCatalog catalog;

    ASSERT_TRUE(catalog.loadYaml(kDottedKeyCatalog).ok());

    EXPECT_EQ(catalog.get("en", "commonReady"), "Ready");
    EXPECT_EQ(catalog.get("en", "runtimeUi.level"), "Level");
}

TEST(LocalizationCatalogTest, RejectsInvalidFallbackReferencesAndLocaleIds) {
    struct InvalidFallbackCase {
        const char* name;
        const char* yaml;
    };

    const InvalidFallbackCase cases[] = {
        {"fallback cycle",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {}}, "
         "pt: {name: Português, fallback: es, strings: {}}, "
         "es: {name: Español, fallback: pt, strings: {}}}\n"},
        {"unknown fallback",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {}}, "
         "pt-BR: {name: Português, fallback: pt, strings: {}}}\n"},
        {"noncanonical locale id",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {}}, "
         "pt-br: {name: Português, strings: {}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid fallback case: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_FALSE(catalog.loadYaml(test.yaml).ok());
    }
}

TEST(LocalizationCatalogTest, MergesLayersAndPreservesInheritedDefaultStrings) {
    constexpr char kBaseLayerCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {title: Base}}}\n";
    constexpr char kDerivedLayerCatalog[] = "locales: {en: {strings: {title: Derived, addedByLayer: Added}}, "
                                            "ar: {name: العربية, direction: rtl, strings: {title: مشتق}}}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYamlLayers({
        ResourceLayer{"base/localization.yaml", kBaseLayerCatalog},
        ResourceLayer{"derived/localization.yaml", kDerivedLayerCatalog},
    });

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(catalog.get("en", "title"), "Derived");
    EXPECT_EQ(catalog.get("en", "addedByLayer"), "Added");
    EXPECT_EQ(catalog.get("ar", "addedByLayer"), "Added");
    EXPECT_EQ(catalog.get("ar", "title"), "مشتق");
}

TEST(LocalizationCatalogTest, RejectsLayersThatRedefineTheDefaultLocale) {
    constexpr char kBaseLayerCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {}}}\n";
    constexpr char kDefaultLocaleOverrideLayer[] = "defaultLocale: en\n"
                                                   "locales: {}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYamlLayers({
        ResourceLayer{"base/localization.yaml", kBaseLayerCatalog},
        ResourceLayer{"derived/localization.yaml", kDefaultLocaleOverrideLayer},
    });

    EXPECT_FALSE(result.ok());
}

TEST(LocalizationCatalogTest, SelectsPluralFormsForEachLocale) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kPluralCatalog).ok());
    EXPECT_TRUE(catalog.pluralCapable("itemCount"));

    const auto request = LocalizationRequest::plural("itemCount", "count", 2);
    const std::string english = catalog.get("en", request);
    const std::string arabic = catalog.get("ar", request);

    EXPECT_NE(english.find("items"), std::string::npos);
    EXPECT_NE(arabic.find("عنصران"), std::string::npos);
}

TEST(LocalizationCatalogTest, UsesScalarTranslationsForPluralRequests) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kPluralCatalog).ok());

    const auto request = LocalizationRequest::plural("sheepCount", "count", 2);

    EXPECT_NE(catalog.get("en", request).find("sheep"), std::string::npos);
}

TEST(LocalizationCatalogTest, ReturnsRawKeyWhenPluralSelectionIsMissing) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kPluralCatalog).ok());

    EXPECT_EQ(catalog.get("en", "itemCount"), "itemCount");
}

TEST(LocalizationCatalogTest, RejectsIncompletePluralDefinitions) {
    struct InvalidPluralCase {
        const char* name;
        const char* yaml;
    };

    const InvalidPluralCase cases[] = {
        {"missing other category",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {itemCount: {one: One}}}}\n"},
        {"exact selector",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {itemCount: {\"=0\": None, other: Other}}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid plural definition: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_FALSE(catalog.loadYaml(test.yaml).ok());
    }
}

TEST(LocalizationCatalogTest, ResolvesRichTextAndEscapedSyntax) {
    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kRichTextCatalog).ok());

    LocalizationArguments arguments;
    arguments.emplace("userName", "<b>Bruno</b>");
    const auto resolved = catalog.resolve("en", LocalizationRequest::text("rich", std::move(arguments)));
    const auto& nodes = resolved.nodes();

    ASSERT_EQ(nodes.size(), std::size_t(4));
    EXPECT_EQ(nodes[0].kind(), InlineContentKind::Text);
    EXPECT_EQ(nodes[0].value(), "Text ");
    ASSERT_EQ(nodes[1].kind(), InlineContentKind::B);
    ASSERT_EQ(nodes[1].children().size(), std::size_t(1));
    EXPECT_EQ(nodes[1].children()[0].kind(), InlineContentKind::Text);
    EXPECT_NE(nodes[1].children()[0].value().find("<b>Bruno</b>"), std::string::npos);
    EXPECT_EQ(nodes[2].kind(), InlineContentKind::Br);
    EXPECT_EQ(nodes[3].kind(), InlineContentKind::Kbd);
    EXPECT_EQ(nodes[3].shortcutId(), "toggle-fly");
    EXPECT_EQ(catalog.get("en", "escaped"), "Write {count} and <b>");
}

TEST(LocalizationCatalogTest, RejectsUnsupportedRichTextMarkup) {
    struct InvalidMarkupCase {
        const char* name;
        const char* yaml;
    };

    const InvalidMarkupCase cases[] = {
        {"unknown tag",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {value: \"<script>bad</script>\"}}}\n"},
        {"mismatched key binding",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {shortcut: '<kbd shortcut=\"toggle-fly\"/>'}}, "
         "pt: {name: Português, strings: {shortcut: '<kbd shortcut=\"open-map\"/>'}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid rich text case: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_FALSE(catalog.loadYaml(test.yaml).ok());
    }
}

TEST(LocalizationCatalogTest, RejectsNonNfcTextAndUnsafeYamlFeatures) {
    LocalizationCatalog catalog;
    std::string nonNfc = "defaultLocale: en\nlocales: {en: {name: English, strings: {value: \"Cafe";
    nonNfc += "\xCC\x81";
    nonNfc += "\"\n";
    EXPECT_FALSE(catalog.loadYaml(nonNfc).ok());

    struct UnsafeYamlCase {
        const char* name;
        const char* yaml;
    };

    const UnsafeYamlCase cases[] = {
        {"tab in localized text",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {value: \"a\\tb\"}}}\n"},
        {"anchors and aliases",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {first: &shared Shared, second: *shared}}}\n"},
        {"custom tag",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {value: !custom Text}}}\n"},
        {"merge key",
         "defaultLocale: en\n"
         "locales: {en: {name: English, strings: {<<: {value: Text}}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "unsafe YAML feature: " << test.name);
        LocalizationCatalog unsafeCatalog;
        EXPECT_FALSE(unsafeCatalog.loadYaml(test.yaml).ok());
    }
}

TEST(LocalizationCatalogTest, UsesTranslationPluralArgumentsForLocaleSpecificForms) {
    constexpr char kLocaleSpecificPluralCatalog[] = "defaultLocale: en\n"
                                                    "locales: {en: {name: English, strings: "
                                                    "{onlineFriends: \"Friends online\"}}, "
                                                    "ru: {name: Русский, strings: {onlineFriends: "
                                                    "{one: \"{count} друг в сети\", "
                                                    "few: \"{count} друга в сети\", many: \"{count} друзей в сети\", "
                                                    "other: \"{count} друга в сети\"}}}}\n";

    LocalizationCatalog catalog;
    ASSERT_TRUE(catalog.loadYaml(kLocaleSpecificPluralCatalog).ok());

    const auto request = LocalizationRequest::plural("onlineFriends", "count", 2);
    const auto wrongRequest = LocalizationRequest::plural("onlineFriends", "otherCount", 2);

    EXPECT_NE(catalog.get("ru", request).find("друга"), std::string::npos);
    EXPECT_EQ(catalog.get("ru", wrongRequest), "onlineFriends");
}
