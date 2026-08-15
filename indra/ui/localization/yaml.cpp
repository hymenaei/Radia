/**
 * @file yaml.cpp
 * @brief Parses and validates strict YAML localization catalogs.
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
#include <regex>
#include <sstream>
#include <unordered_set>
#include <simdutf.h>
#include <unicode/normalizer2.h>
#include <unicode/uchar.h>
#include <unicode/unistr.h>
#include <yaml-cpp/eventhandler.h>
#include <yaml-cpp/parser.h>
#include <yaml-cpp/yaml.h>
#include "localization/internal.h"

namespace radia::ui::localization_detail {
namespace {
class StrictYamlEventHandler final : public YAML::EventHandler {
public:
    void OnDocumentStart(const YAML::Mark&) override {}
    void OnDocumentEnd() override {}
    void OnNull(const YAML::Mark& mark, YAML::anchor_t anchor) override { checkAnchor(mark, anchor); }
    void OnAlias(const YAML::Mark& mark, YAML::anchor_t) override { reject(mark, "YAML aliases are forbidden."); }
    void OnScalar(const YAML::Mark& mark, const std::string& tag, YAML::anchor_t anchor, const std::string&) override {
        checkTag(mark, tag);
        checkAnchor(mark, anchor);
    }
    void OnSequenceStart(const YAML::Mark& mark, const std::string& tag, YAML::anchor_t anchor, YAML::EmitterStyle::value) override {
        checkTag(mark, tag);
        checkAnchor(mark, anchor);
    }
    void OnSequenceEnd() override {}
    void OnMapStart(const YAML::Mark& mark, const std::string& tag, YAML::anchor_t anchor, YAML::EmitterStyle::value) override {
        checkTag(mark, tag);
        checkAnchor(mark, anchor);
    }
    void OnMapEnd() override {}
    void OnAnchor(const YAML::Mark& mark, const std::string&) override { reject(mark, "YAML anchors are forbidden."); }

    bool valid() const { return mReason.empty(); }
    const std::string& reason() const { return mReason; }
    const YAML::Mark& mark() const { return mMark; }

private:
    void checkAnchor(const YAML::Mark& mark, YAML::anchor_t anchor) {
        if (anchor != YAML::NullAnchor) reject(mark, "YAML anchors are forbidden.");
    }

    void checkTag(const YAML::Mark& mark, const std::string& tag) {
        if (!tag.empty() && tag != "?" && tag != "!") reject(mark, "Explicit and custom YAML tags are forbidden.");
    }

    void reject(const YAML::Mark& mark, const std::string& reason) {
        if (!mReason.empty()) return;
        mMark = mark;
        mReason = reason;
    }

    YAML::Mark mMark = YAML::Mark::null_mark();
    std::string mReason;
};

std::size_t lineOf(const YAML::Node& node) {
    const YAML::Mark mark = node.Mark();
    return mark.is_null() ? 0 : static_cast<std::size_t>(mark.line + 1);
}

std::size_t columnOf(const YAML::Node& node) {
    const YAML::Mark mark = node.Mark();
    return mark.is_null() ? 0 : static_cast<std::size_t>(mark.column + 1);
}

bool implicitNonString(const YAML::Node& node, const std::string& value) {
    if (node.Tag() != "?") return false;
    static const std::regex sYamlCoreScalarPattern(
        R"(^(?:~|null|Null|NULL|true|True|TRUE|false|False|FALSE|[-+]?(?:(?:0|[1-9][0-9_]*)(?:\.[0-9_]*)?(?:[eE][-+]?[0-9]+)?|0o[0-7_]+|0x[0-9a-fA-F_]+|\.(?:inf|Inf|INF|nan|NaN|NAN)))$)");
    return std::regex_match(value, sYamlCoreScalarPattern);
}

bool validCatalogText(const std::string& value) {
    if (!simdutf::validate_utf8(value)) return false;

    const icu::UnicodeString unicode = icu::UnicodeString::fromUTF8(value);
    for (int32_t offset = 0; offset < unicode.length();) {
        const UChar32 character = unicode.char32At(offset);
        offset += U16_LENGTH(character);
        if (character == '\n') continue;
        if (u_charType(character) == U_CONTROL_CHAR) return false;
    }

    UErrorCode status = U_ZERO_ERROR;
    const icu::Normalizer2* normalizer = icu::Normalizer2::getNFCInstance(status);
    return U_SUCCESS(status) && normalizer && normalizer->isNormalized(unicode, status) && U_SUCCESS(status);
}

std::optional<std::string> canonicalLanguageTag(const std::string& value) {
    if (value.empty() || !simdutf::validate_ascii(value)) return std::nullopt;
    UErrorCode status = U_ZERO_ERROR;
    const icu::Locale locale = icu::Locale::forLanguageTag(value, status);
    if (U_FAILURE(status) || locale.isBogus()) return std::nullopt;
    std::string canonical = locale.toLanguageTag<std::string>(status);
    if (U_FAILURE(status) || canonical.empty()) return std::nullopt;
    return canonical;
}

bool validStringKey(const std::string& stringKey) {
    static const std::regex sStringKeyPattern(R"(^[a-z][A-Za-z0-9]*(?:\.[a-z][A-Za-z0-9]*)*$)");
    return std::regex_match(stringKey, sStringKeyPattern);
}

bool validPluralCategory(const std::string& value) {
    static const std::unordered_set<std::string> sPluralCategories = {
        "zero", "one", "two", "few", "many", "other",
    };
    return sPluralCategories.contains(value);
}

struct MappingEntry {
    std::string key;
    YAML::Node keyNode;
    YAML::Node value;
};

class YamlCatalogParser {
public:
    YamlCatalogParser(const std::string& yaml, const std::string& sourceName, bool base) : mYaml(yaml), mSourceName(sourceName), mBase(base) {}

    LocalizationLoadResult parse(ParsedCatalog& catalog) {
        catalog = {};
        const std::optional<YAML::Node> root = loadRoot();
        if (root) parseRoot(*root, catalog);
        return std::move(mResult);
    }

private:
    std::optional<YAML::Node> loadRoot() {
        if (!simdutf::validate_utf8(mYaml)) {
            mResult.error("localization.yaml.utf8_invalid", "Localization YAML must be valid UTF-8.", mSourceName);
            return std::nullopt;
        }

        std::vector<YAML::Node> documents;
        try {
            std::istringstream input(mYaml);
            YAML::Parser parser(input);
            StrictYamlEventHandler eventHandler;
            while (parser.HandleNextDocument(eventHandler)) {}
            if (!eventHandler.valid()) {
                const YAML::Mark& mark = eventHandler.mark();
                mResult.error("localization.yaml.feature_forbidden", eventHandler.reason(), mSourceName,
                              mark.is_null() ? 0 : static_cast<std::size_t>(mark.line + 1),
                              mark.is_null() ? 0 : static_cast<std::size_t>(mark.column + 1));
                return std::nullopt;
            }
            documents = YAML::LoadAll(mYaml);
        } catch (const YAML::Exception& exception) {
            mResult.error("localization.yaml.invalid", "Could not parse Radia UI localization YAML: " + std::string(exception.what()), mSourceName,
                          exception.mark.is_null() ? 0 : static_cast<std::size_t>(exception.mark.line + 1),
                          exception.mark.is_null() ? 0 : static_cast<std::size_t>(exception.mark.column + 1));
            return std::nullopt;
        }

        if (documents.size() != 1 || !documents.front().IsMap()) {
            mResult.error("localization.root.invalid", "Localization YAML must contain exactly one mapping document.", mSourceName,
                          documents.empty() ? 0 : lineOf(documents.front()));
            return std::nullopt;
        }
        return documents.front();
    }

    std::optional<std::string> scalar(const YAML::Node& node, const std::string& field) {
        if (!node.IsScalar()) {
            mResult.error("localization.scalar.required", field + " must be a YAML string.", mSourceName, lineOf(node), columnOf(node));
            return std::nullopt;
        }

        const std::string value = node.Scalar();
        if (value.empty() || implicitNonString(node, value)) {
            mResult.error("localization.scalar.invalid",
                          field + " must be a non-empty YAML string; quote values that resemble numbers, booleans, or null.", mSourceName,
                          lineOf(node), columnOf(node));
            return std::nullopt;
        }
        if (!validCatalogText(value)) {
            mResult.error("localization.string.unicode_invalid", field + " must be NFC-normalized Unicode without tabs or control characters.",
                          mSourceName, lineOf(node), columnOf(node));
            return std::nullopt;
        }
        return value;
    }

    std::vector<MappingEntry> mapping(const YAML::Node& map, const std::string& field, const std::unordered_set<std::string>& allowed = {}) {
        std::vector<MappingEntry> result;
        if (!map.IsMap()) {
            mResult.error("localization.map.required", field + " must be a YAML mapping.", mSourceName, lineOf(map), columnOf(map));
            return result;
        }

        std::unordered_set<std::string> seen;
        for (const auto& entry : map) {
            const std::optional<std::string> key = scalar(entry.first, field + " field name");
            if (!key) continue;
            if (!seen.insert(*key).second) {
                mResult.error("localization.yaml.key_duplicate", "Duplicate YAML key in " + field + ": " + *key + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }
            if (!allowed.empty() && !allowed.contains(*key)) {
                mResult.error("localization.field.unknown", "Unknown field in " + field + ": " + *key + ".", mSourceName, lineOf(entry.first),
                              columnOf(entry.first));
                continue;
            }
            result.push_back({*key, entry.first, entry.second});
        }
        return result;
    }

    void parseRoot(const YAML::Node& root, ParsedCatalog& catalog) {
        YAML::Node locales;
        bool defaultSeen = false;
        bool localesSeen = false;
        for (const MappingEntry& entry : mapping(root, "localization root", {"defaultLocale", "locales"})) {
            if (entry.key == "defaultLocale") {
                defaultSeen = true;
                parseDefaultLocale(entry.value, catalog);
            } else {
                localesSeen = true;
                locales = entry.value;
            }
        }

        if (mBase && !defaultSeen) mResult.error("localization.default.missing", "Base localization YAML requires defaultLocale.", mSourceName);
        if (!localesSeen) {
            mResult.error("localization.locales.missing", "Localization YAML requires a locales mapping.", mSourceName);
            return;
        }
        parseLocales(locales, catalog);
    }

    void parseDefaultLocale(const YAML::Node& node, ParsedCatalog& catalog) {
        if (!mBase) {
            mResult.error("localization.layer.default_forbidden", "Localization override layers must omit defaultLocale.", mSourceName, lineOf(node),
                          columnOf(node));
            return;
        }

        const std::optional<std::string> defaultLocaleId = scalar(node, "defaultLocale");
        if (!defaultLocaleId) return;
        const std::optional<std::string> canonical = canonicalLanguageTag(*defaultLocaleId);
        if (!canonical || *canonical != *defaultLocaleId)
            mResult.error("localization.default.invalid", "defaultLocale must be a canonical BCP 47 Locale ID: " + *defaultLocaleId + ".",
                          mSourceName, lineOf(node), columnOf(node));
        else catalog.defaultLocale = *canonical;
    }

    void parseLocales(const YAML::Node& node, ParsedCatalog& catalog) {
        if (!node.IsMap()) {
            mResult.error("localization.locales.invalid", "locales must be a YAML mapping.", mSourceName, lineOf(node), columnOf(node));
            return;
        }

        std::unordered_set<std::string> identities;
        for (const auto& entry : node) {
            const std::optional<std::string> authoredId = scalar(entry.first, "Locale ID");
            if (!authoredId) continue;
            const std::optional<std::string> canonical = canonicalLanguageTag(*authoredId);
            if (!canonical || *canonical != *authoredId) {
                mResult.error("localization.locale.id_invalid", "Locale ID must be a canonical BCP 47 tag: " + *authoredId + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }

            std::string identity = *canonical;
            std::transform(identity.begin(), identity.end(), identity.begin(),
                           [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
            if (!identities.insert(identity).second) {
                mResult.error("localization.locale.duplicate", "Duplicate localization locale: " + *canonical + ".", mSourceName, lineOf(entry.first),
                              columnOf(entry.first));
                continue;
            }
            catalog.locales.push_back(parseLocale(*canonical, entry.first, entry.second));
        }

        if (mBase && catalog.locales.empty()) mResult.error("localization.locales.empty", "Base localization YAML defines no locales.", mSourceName);
    }

    ParsedLocale parseLocale(const std::string& localeId, const YAML::Node& localeIdNode, const YAML::Node& node) {
        ParsedLocale locale;
        locale.localeId = localeId;
        locale.source = mSourceName;
        locale.line = lineOf(localeIdNode);

        for (const MappingEntry& entry : mapping(node, "locale " + locale.localeId, {"name", "direction", "fallback", "strings"}))
            if (entry.key == "name") parseLocaleName(entry.value, locale);
            else if (entry.key == "direction") parseLocaleDirection(entry.value, locale);
            else if (entry.key == "fallback") parseLocaleFallback(entry.value, locale);
            else parseStrings(entry.value, locale);

        if (!locale.stringsPresent)
            mResult.error("localization.locale.strings_missing", "Every locale entry requires a strings mapping: " + locale.localeId + ".",
                          mSourceName, locale.line);
        if (mBase && !locale.name)
            mResult.error("localization.locale.name_missing", "Base locale has no display name: " + locale.localeId + ".", mSourceName, locale.line);
        return locale;
    }

    void parseLocaleName(const YAML::Node& node, ParsedLocale& locale) {
        const std::optional<std::string> name = scalar(node, "Locale name");
        if (!name) return;

        StringTemplate parsed;
        LocalizationLoadResult parsedResult;
        parseRichString(*name, parsed, parsedResult, mSourceName, lineOf(node));
        const bool rich = name->find('\n') != std::string::npos
            || !parsed.arguments.empty()
            || std::any_of(parsed.nodes.begin(), parsed.nodes.end(), [](const TemplateNode& value) { return value.kind != TemplateKind::Text; });
        if (rich || parsedResult.hasErrors())
            mResult.error("localization.locale.name_invalid", "Locale name must be a single-line plain string without markup or placeholders.",
                          mSourceName, lineOf(node), columnOf(node));
        else locale.name = *name;
    }

    void parseLocaleDirection(const YAML::Node& node, ParsedLocale& locale) {
        const std::optional<std::string> direction = scalar(node, "Locale direction");
        if (!direction) return;
        if (*direction == "ltr") locale.direction = LayoutDirection::LeftToRight;
        else if (*direction == "rtl") locale.direction = LayoutDirection::RightToLeft;
        else
            mResult.error("localization.locale.direction_invalid", "Locale direction must be exactly 'ltr' or 'rtl'.", mSourceName, lineOf(node),
                          columnOf(node));
    }

    void parseLocaleFallback(const YAML::Node& node, ParsedLocale& locale) {
        const std::optional<std::string> fallback = scalar(node, "Locale fallback");
        if (!fallback) return;
        const std::optional<std::string> canonical = canonicalLanguageTag(*fallback);
        if (!canonical || *canonical != *fallback)
            mResult.error("localization.locale.fallback_invalid", "Locale fallback must be a canonical BCP 47 Locale ID: " + *fallback + ".",
                          mSourceName, lineOf(node), columnOf(node));
        else locale.fallback = *canonical;
    }

    void parseStrings(const YAML::Node& node, ParsedLocale& locale) {
        locale.stringsPresent = true;
        if (!node.IsMap()) {
            mResult.error("localization.strings.invalid", "Locale strings must be a flat YAML mapping.", mSourceName, lineOf(node), columnOf(node));
            return;
        }

        std::unordered_set<std::string> keys;
        for (const auto& entry : node) {
            const std::optional<std::string> key = scalar(entry.first, "String Key");
            if (!key) continue;
            if (!validStringKey(*key)) {
                mResult.error("localization.string.key_invalid", "String Key must use lowerCamelCase segments: " + *key + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }
            if (!keys.insert(*key).second) {
                mResult.error("localization.string.duplicate", "Duplicate String Key in locale " + locale.localeId + ": " + *key + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }

            std::optional<StringValue> value = parseStringValue(*key, entry.first, entry.second);
            if (value) locale.strings.emplace(*key, std::move(*value));
        }
    }

    std::optional<StringValue> parseStringValue(const std::string& key, const YAML::Node& keyNode, const YAML::Node& node) {
        if (node.IsScalar()) {
            const std::optional<std::string> text = scalar(node, "Localized String " + key);
            if (!text) return std::nullopt;
            StringTemplate parsed;
            if (!parseRichString(*text, parsed, mResult, mSourceName, lineOf(node))) return std::nullopt;
            return StringValue(std::move(parsed), mSourceName, lineOf(keyNode));
        }
        if (node.IsMap()) {
            PluralTemplates plurals = parsePluralVariants(key, node);
            if (plurals.empty()) return std::nullopt;
            return StringValue(std::move(plurals), mSourceName, lineOf(keyNode));
        }

        mResult.error("localization.string.value_invalid", "Localized value must be a non-empty YAML string or Plural Category map.", mSourceName,
                      lineOf(node), columnOf(node));
        return std::nullopt;
    }

    PluralTemplates parsePluralVariants(const std::string& key, const YAML::Node& node) {
        PluralTemplates plurals;
        std::unordered_set<std::string> categories;
        for (const auto& entry : node) {
            const std::optional<std::string> category = scalar(entry.first, "Plural Category");
            if (!category) continue;
            if (!categories.insert(*category).second) {
                mResult.error("localization.plural.category_duplicate", "Duplicate Plural Category " + *category + " for " + key + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }
            if (!validPluralCategory(*category)) {
                mResult.error("localization.plural.category_invalid", "Unknown CLDR Plural Category: " + *category + ".", mSourceName,
                              lineOf(entry.first), columnOf(entry.first));
                continue;
            }

            const std::optional<std::string> text = scalar(entry.second, "Plural variant " + key + "." + *category);
            if (!text) continue;
            StringTemplate parsed;
            if (parseRichString(*text, parsed, mResult, mSourceName, lineOf(entry.second))) plurals.emplace(*category, std::move(parsed));
        }

        if (!plurals.contains("other"))
            mResult.error("localization.plural.other_missing", "Plural String requires an other variant: " + key + ".", mSourceName, lineOf(node),
                          columnOf(node));
        return plurals;
    }

    const std::string& mYaml;
    const std::string& mSourceName;
    bool mBase = false;
    LocalizationLoadResult mResult;
};
} // namespace

std::string localeIdentity(const std::string& localeId) {
    std::string result = localeId;
    std::transform(result.begin(), result.end(), result.begin(), [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
    return result;
}

LocalizationLoadResult parseYamlCatalog(const std::string& yaml, const std::string& sourceName, bool base, ParsedCatalog& catalog) {
    return YamlCatalogParser(yaml, sourceName, base).parse(catalog);
}
} // namespace radia::ui::localization_detail
