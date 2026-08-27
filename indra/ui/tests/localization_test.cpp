/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <gtest/gtest.h>
#include "localization.h"

namespace {
using radia::ui::LayoutDirection;
using radia::ui::LocalizationArguments;
using radia::ui::LocalizationCatalog;
using radia::ui::LocalizedText;
using radia::ui::ResourceLayer;
using ::testing::Message;

constexpr char kCatalogWithFallbacks[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {title: Title, fallback: Fallback}}, "
                                         "pt: {strings: {title: Título}}, "
                                         "ar: {fallback: pt, strings: {}}}\n";

constexpr char kPluralCatalog[] = "defaultLocale: en\n"
                                  "locales: {en: {strings: {"
                                  "itemCount: 'You have {count, plural, one {You have one item} other {You have # items}}', "
                                  "sheepCount: 'You have {count, plural, one {one sheep} other {# sheep}}'}}, "
                                  "ar: {strings: {"
                                  "itemCount: '{count, plural, zero {لديك 0 عناصر} one {لديك عنصر واحد} two {لديك عنصران} "
                                  "few {لديك # عناصر} many {لديك # عنصرًا} other {لديك # عنصر}}', "
                                  "sheepCount: '{count, plural, one {# خروف} other {# خراف}}'}}}\n";

constexpr char kRichTextCatalog[] = "defaultLocale: en\n"
                                    "locales: {en: {strings: {"
                                    "rich: 'TextElement <b>{userName}</b><br/><kbd shortcut=\"toggle-fly\"/>', "
                                    "escaped: 'Write ''{count}'' and &lt;b&gt;' }}}\n";
} // namespace

TEST(LocalizationCatalogTest, LoadsLocaleMetadataAndSupportsCaseInsensitiveLookup) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kCatalogWithFallbacks, "localization.yaml").hasErrors());

    const auto locales = catalog.locales();
    ASSERT_EQ(locales.size(), std::size_t(3));
    EXPECT_EQ(locales.front().localeId, "ar");
    EXPECT_EQ(catalog.defaultLocaleId(), "en");
    EXPECT_TRUE(catalog.containsLocale("AR"));

    const auto* portuguese = catalog.locale("PT");
    ASSERT_NE(portuguese, nullptr);
    EXPECT_FALSE(portuguese->name.empty());
    EXPECT_EQ(portuguese->direction, LayoutDirection::LeftToRight);

    const auto* arabic = catalog.locale("ar");
    ASSERT_NE(arabic, nullptr);
    EXPECT_EQ(arabic->direction, LayoutDirection::RightToLeft);
}

TEST(LocalizationCatalogTest, ResolvesTranslationsThroughDefaultAndExplicitFallbacks) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kCatalogWithFallbacks).hasErrors());

    EXPECT_TRUE(catalog.containsDefaultString("title"));
    EXPECT_FALSE(catalog.containsDefaultString("missingKey"));
    EXPECT_EQ(catalog.resolveText("pt", "title"), "Título");
    EXPECT_EQ(catalog.resolveText("pt", "fallback"), "Fallback");
    EXPECT_EQ(catalog.resolveText("ar", "title"), "Título");
    EXPECT_EQ(catalog.resolveText("pt", "missingKey"), "missingKey");
}

TEST(LocalizationCatalogTest, PreservesCommittedCatalogWhenReplacementFails) {
    constexpr char kCommittedCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {value: live}}}\n";

    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kCommittedCatalog).hasErrors());

    constexpr char kDuplicateValueCatalog[] = "defaultLocale: en\n"
                                              "locales: {en: {strings: "
                                              "{value: first, value: second}}}\n";
    const auto duplicate = catalog.loadYaml(kDuplicateValueCatalog);

    EXPECT_TRUE(duplicate.hasErrors());
    EXPECT_EQ(catalog.resolveText("en", "value"), "live");
}

TEST(LocalizationCatalogTest, ReportsMalformedYamlWithSourceDiagnostic) {
    LocalizationCatalog catalog;
    constexpr char kMalformedCatalog[] = "defaultLocale: [";
    const auto result = catalog.loadYaml(kMalformedCatalog, "broken.yaml");

    ASSERT_TRUE(result.hasErrors());
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
        {"missing default locale", "locales: {en: {strings: {}}}\n"},
        {"empty locale map", "defaultLocale: en\nlocales: {}\n"},
        {"implicit numeric value", "defaultLocale: en\nlocales: {en: {strings: {version: 1}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid catalog shape: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_TRUE(catalog.loadYaml(test.yaml).hasErrors());
    }
}

TEST(LocalizationCatalogTest, AcceptsQuotedNumericTextValues) {
    constexpr char kQuotedNumericCatalog[] = "defaultLocale: en\n"
                                             "locales: {en: {strings: {version: \"1\"}}}\n";

    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kQuotedNumericCatalog).hasErrors());

    EXPECT_EQ(catalog.resolveText("en", "version"), "1");
}

TEST(LocalizationCatalogTest, RejectsLocalesMissingDefaultTranslations) {
    constexpr char kMissingDefaultTranslationCatalog[] = "defaultLocale: en\n"
                                                         "locales: {en: {strings: {}}, "
                                                         "pt: {strings: "
                                                         "{onlyPt: Só}}}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYaml(kMissingDefaultTranslationCatalog, "localization.yaml");

    ASSERT_TRUE(result.hasErrors());
    EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(),
                            [](const radia::ui::Diagnostic& diagnostic) { return diagnostic.code == "localization.default.string_missing"; }));
}

TEST(LocalizationCatalogTest, RejectsSnakeCaseStringKeys) {
    constexpr char kSnakeCaseKeyCatalog[] = "defaultLocale: en\n"
                                            "locales: {en: {strings: {invalid_key: Value}}}\n";

    LocalizationCatalog catalog;

    EXPECT_TRUE(catalog.loadYaml(kSnakeCaseKeyCatalog).hasErrors());
}

TEST(LocalizationCatalogTest, AcceptsLowerCamelCaseKeysWithDottedSegments) {
    constexpr char kDottedKeyCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: "
                                         "{commonReady: Ready, runtimeUi.level: Level}}}\n";

    LocalizationCatalog catalog;

    ASSERT_FALSE(catalog.loadYaml(kDottedKeyCatalog).hasErrors());

    EXPECT_EQ(catalog.resolveText("en", "commonReady"), "Ready");
    EXPECT_EQ(catalog.resolveText("en", "runtimeUi.level"), "Level");
}

TEST(LocalizationCatalogTest, RejectsInvalidFallbackReferencesAndLocaleIds) {
    struct InvalidFallbackCase {
        const char* name;
        const char* yaml;
    };

    const InvalidFallbackCase cases[] = {
        {"fallback cycle",
         "defaultLocale: en\n"
         "locales: {en: {strings: {}}, "
         "pt: {fallback: es, strings: {}}, "
         "es: {fallback: pt, strings: {}}}\n"},
        {"unknown fallback",
         "defaultLocale: en\n"
         "locales: {en: {strings: {}}, "
         "pt-BR: {fallback: pt, strings: {}}}\n"},
        {"noncanonical locale id",
         "defaultLocale: en\n"
         "locales: {en: {strings: {}}, "
         "pt-br: {strings: {}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid fallback case: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_TRUE(catalog.loadYaml(test.yaml).hasErrors());
    }
}

TEST(LocalizationCatalogTest, MergesLayersAndPreservesInheritedDefaultStrings) {
    constexpr char kBaseLayerCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {title: Base}}}\n";
    constexpr char kDerivedLayerCatalog[] = "locales: {en: {strings: {title: Derived, addedByLayer: Added}}, "
                                            "ar: {strings: {title: مشتق}}}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYamlLayers({
        ResourceLayer{"base/localization.yaml", kBaseLayerCatalog},
        ResourceLayer{"derived/localization.yaml", kDerivedLayerCatalog},
    });

    ASSERT_FALSE(result.hasErrors());
    EXPECT_EQ(catalog.resolveText("en", "title"), "Derived");
    EXPECT_EQ(catalog.resolveText("en", "addedByLayer"), "Added");
    EXPECT_EQ(catalog.resolveText("ar", "addedByLayer"), "Added");
    EXPECT_EQ(catalog.resolveText("ar", "title"), "مشتق");
}

TEST(LocalizationCatalogTest, RejectsLayersThatRedefineTheDefaultLocale) {
    constexpr char kBaseLayerCatalog[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {}}}\n";
    constexpr char kDefaultLocaleOverrideLayer[] = "defaultLocale: en\n"
                                                   "locales: {}\n";

    LocalizationCatalog catalog;
    const auto result = catalog.loadYamlLayers({
        ResourceLayer{"base/localization.yaml", kBaseLayerCatalog},
        ResourceLayer{"derived/localization.yaml", kDefaultLocaleOverrideLayer},
    });

    EXPECT_TRUE(result.hasErrors());
}

TEST(LocalizationCatalogTest, SelectsIcuPluralFormsForEachLocale) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kPluralCatalog).hasErrors());

    const LocalizedText request("itemCount", {{"count", 2}});
    const std::string english = catalog.resolveText("en", request);
    const std::string arabic = catalog.resolveText("ar", request);

    EXPECT_NE(english.find("items"), std::string::npos);
    EXPECT_NE(arabic.find("عنصران"), std::string::npos);
}

TEST(LocalizationCatalogTest, UsesIcuPluralFormattingForEveryMessage) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kPluralCatalog).hasErrors());

    const LocalizedText request("sheepCount", {{"count", 2}});

    EXPECT_NE(catalog.resolveText("en", request).find("sheep"), std::string::npos);
}

TEST(LocalizationCatalogTest, LeavesIcuPlaceholderWhenArgumentIsMissing) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kPluralCatalog).hasErrors());

    EXPECT_EQ(catalog.resolveText("en", "itemCount"), "You have {count}");
}

TEST(LocalizationCatalogTest, RejectsInvalidIcuMessageValues) {
    struct InvalidMessageCase {
        const char* name;
        const char* yaml;
    };

    const InvalidMessageCase cases[] = {
        {"malformed ICU message",
         "defaultLocale: en\n"
         "locales: {en: {strings: {itemCount: '{count, plural, one {One}'}}}\n"},
        {"YAML mapping instead of ICU message",
         "defaultLocale: en\n"
         "locales: {en: {strings: {itemCount: {one: One, other: Other}}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid ICU message: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_TRUE(catalog.loadYaml(test.yaml).hasErrors());
    }
}

TEST(LocalizationCatalogTest, ResolvesLocalizedTextAndEscapedMarkup) {
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kRichTextCatalog).hasErrors());

    LocalizationArguments arguments;
    arguments.emplace("userName", "<b>Bruno</b>");
    EXPECT_EQ(catalog.resolveText("en", LocalizedText("rich", std::move(arguments))),
              "TextElement \xE2\x81\xA8"
              "<b>Bruno</b>"
              "\xE2\x81\xA9\n");
    EXPECT_EQ(catalog.resolveText("en", "escaped"), "Write {count} and <b>");
}

TEST(LocalizationCatalogTest, ResolvesSemanticInlineElements) {
    constexpr char kSemanticCatalog[] = "defaultLocale: en\n"
                                        "locales: {en: {strings: {value: "
                                        "'<abbr>abbr</abbr> <b>b</b> <cite>cite</cite> <code>code</code> <dfn>dfn</dfn> "
                                        "<del>del</del> <em>em</em> <i>i</i> <ins>ins</ins> <mark>mark</mark> "
                                        "<q>q</q> <s>s</s> <small>small</small> <strong>strong</strong> <u>u</u>'}}}\n";
    LocalizationCatalog catalog;
    ASSERT_FALSE(catalog.loadYaml(kSemanticCatalog).hasErrors());

    const std::string expected = "<abbr>abbr</abbr> <b>b</b> <cite>cite</cite> <code>code</code> <dfn>dfn</dfn> "
                                 "<del>del</del> <em>em</em> <i>i</i> <ins>ins</ins> <mark>mark</mark> <q>q</q> "
                                 "<s>s</s> <small>small</small> <strong>strong</strong> <u>u</u>";
    EXPECT_EQ(catalog.resolveMarkup("en", LocalizedText("value")), expected);
    EXPECT_EQ(catalog.resolveText("en", "value"), "abbr b cite code dfn del em i ins mark q s small strong u");
}

TEST(LocalizationCatalogTest, RejectsUnsupportedRichTextMarkup) {
    struct InvalidMarkupCase {
        const char* name;
        const char* yaml;
    };

    const InvalidMarkupCase cases[] = {
        {"unknown tag",
         "defaultLocale: en\n"
         "locales: {en: {strings: {value: \"<script>bad</script>\"}}}\n"},
        {"mismatched key binding",
         "defaultLocale: en\n"
         "locales: {en: {strings: {shortcut: '<kbd shortcut=\"toggle-fly\"/>'}}, "
         "pt: {strings: {shortcut: '<kbd shortcut=\"open-map\"/>'}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid rich text case: " << test.name);
        LocalizationCatalog catalog;
        EXPECT_TRUE(catalog.loadYaml(test.yaml).hasErrors());
    }
}

TEST(LocalizationCatalogTest, ReportsLocalizedInlineValidationDiagnostics) {
    struct InvalidMarkupCase {
        const char* yaml;
        const char* diagnostic;
    };

    const InvalidMarkupCase cases[] = {
        {"defaultLocale: en\nlocales:\n  en:\n    strings:\n      value: '<b emphasis=\"true\">bad</b>'\n", "localization.string.attribute_invalid"},
        {"defaultLocale: en\nlocales:\n  en:\n    strings:\n      value: '<kbd shortcut=\"toggle-fly\">bad</kbd>'\n", "localization.string.children_invalid"},
    };

    for (const auto& test : cases) {
        LocalizationCatalog catalog;
        const auto result = catalog.loadYaml(test.yaml);
        ASSERT_TRUE(result.hasErrors());
        EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(), [&](const radia::ui::Diagnostic& diagnostic) {
            return diagnostic.code == test.diagnostic;
        })) << (result.errors.empty() ? "no diagnostic" : result.errors.front().formatted());
    }
}

TEST(LocalizationCatalogTest, RejectsNonNfcTextAndUnsafeYamlFeatures) {
    LocalizationCatalog catalog;
    std::string nonNfc = "defaultLocale: en\nlocales: {en: {strings: {value: \"Cafe";
    nonNfc += "\xCC\x81";
    nonNfc += "\"\n";
    EXPECT_TRUE(catalog.loadYaml(nonNfc).hasErrors());

    struct UnsafeYamlCase {
        const char* name;
        const char* yaml;
    };

    const UnsafeYamlCase cases[] = {
        {"tab in localized text",
         "defaultLocale: en\n"
         "locales: {en: {strings: {value: \"a\\tb\"}}}\n"},
        {"anchors and aliases",
         "defaultLocale: en\n"
         "locales: {en: {strings: {first: &shared Shared, second: *shared}}}\n"},
        {"custom tag",
         "defaultLocale: en\n"
         "locales: {en: {strings: {value: !custom TextElement}}}\n"},
        {"merge key",
         "defaultLocale: en\n"
         "locales: {en: {strings: {<<: {value: TextElement}}}}\n"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "unsafe YAML feature: " << test.name);
        LocalizationCatalog unsafeCatalog;
        EXPECT_TRUE(unsafeCatalog.loadYaml(test.yaml).hasErrors());
    }
}

TEST(LocalizationCatalogTest, UsesLocaleSpecificIcuPluralForms) {
    constexpr char kLocaleSpecificPluralCatalog[] = "defaultLocale: en\n"
                                                    "locales: {en: {strings: "
                                                    "{onlineFriends: \"Friends online\"}}, "
                                                    "ru: {strings: {onlineFriends: "
                                                    "'{count, plural, one {# друг в сети} few {# друга в сети} "
                                                    "many {# друзей в сети} other {# друга в сети}}'}}}\n";

    LocalizationCatalog catalog;
    const auto loaded = catalog.loadYaml(kLocaleSpecificPluralCatalog);
    ASSERT_FALSE(loaded.hasErrors()) << (loaded.errors.empty() ? "unknown localization error" : loaded.errors.front().formatted());

    const LocalizedText request("onlineFriends", {{"count", 2}});
    const LocalizedText wrongRequest("onlineFriends", {{"otherCount", 2}});

    EXPECT_NE(catalog.resolveText("ru", request).find("друга"), std::string::npos);
    EXPECT_EQ(catalog.resolveText("ru", wrongRequest), "{count}");
}
