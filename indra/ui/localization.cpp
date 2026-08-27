/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "localization.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <optional>
#include <regex>
#include <set>
#include <sstream>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <vector>
#include <simdutf.h>
#include <unicode/fmtable.h>
#include <unicode/locid.h>
#include <unicode/msgfmt.h>
#include <unicode/normalizer2.h>
#include <unicode/uchar.h>
#include <unicode/unistr.h>
#include <yaml-cpp/eventhandler.h>
#include <yaml-cpp/parser.h>
#include <yaml-cpp/yaml.h>
#include "layout/document.h"
#include "text/inlineelements.h"

namespace radia::ui {
namespace {
class StringValue {
public:
    StringValue(std::string pattern, std::multiset<std::string> shortcutIds, std::string sourceName, std::size_t sourceLine)
        : mPattern(std::move(pattern)), mShortcutIds(std::move(shortcutIds)), source(std::move(sourceName)), line(sourceLine) {}

    const std::string& pattern() const { return mPattern; }
    const std::multiset<std::string>& shortcutIds() const { return mShortcutIds; }

    std::string source;
    std::size_t line = 0;

private:
    std::string mPattern;
    std::multiset<std::string> mShortcutIds;
};

using StringMap = std::unordered_map<std::string, StringValue>;

struct ParsedLocale {
    std::string localeId;
    std::string source;
    std::size_t line = 0;
    std::optional<std::string> fallback;
    bool stringsPresent = false;
    StringMap strings;
};

struct ParsedCatalog {
    std::optional<std::string> defaultLocale;
    std::vector<ParsedLocale> locales;
};

struct LocaleRecord {
    LocaleInfo info;
    StringMap strings;
    std::vector<std::size_t> fallbackChain;
    icu::Locale locale;
};

std::string localeIdentity(const std::string& localeId) {
    std::string result = localeId;
    std::transform(result.begin(), result.end(), result.begin(), [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
    return result;
}

std::size_t lineOf(const YAML::Node& node) {
    const YAML::Mark mark = node.Mark();
    return mark.is_null() ? 0 : static_cast<std::size_t>(mark.line + 1);
}

std::size_t columnOf(const YAML::Node& node) {
    const YAML::Mark mark = node.Mark();
    return mark.is_null() ? 0 : static_cast<std::size_t>(mark.column + 1);
}

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

std::size_t markupLine(std::size_t sourceLine, const SourceLocation& location) {
    return sourceLine + (location.line > 0 ? location.line - 1 : 0);
}

void appendLocalizedMarkupDiagnostics(const InlineValidationResult& validation, DiagnosticResult& result, const std::string& sourceName,
                                      std::size_t sourceLine) {
    std::set<std::size_t> attributeNodes;
    for (const InlineValidationFinding& finding : validation.findings) {
        const SourceLocation& elementLocation = finding.elementSource.begin;
        const SourceLocation& location = finding.source.begin;
        switch (finding.kind) {
            case InlineValidationKind::UnsupportedElement:
                result.error("localization.string.tag_invalid", "Element <" + finding.elementName + "> is not allowed in localized content.",
                             sourceName, markupLine(sourceLine, elementLocation));
                break;
            case InlineValidationKind::NotImplemented:
                result.error("localization.string.tag_invalid", "Element <" + finding.elementName + "> is not allowed in localized content.",
                             sourceName, markupLine(sourceLine, elementLocation));
                break;
            case InlineValidationKind::AttributeUnknown:
                if (!attributeNodes.insert(elementLocation.offset).second) break;
                if (finding.tag == Tag::Kbd)
                    result.error("localization.string.attribute_invalid", "Localized <kbd> accepts only the shortcut attribute.", sourceName,
                                 markupLine(sourceLine, elementLocation));
                else
                    result.error("localization.string.attribute_invalid", "Localized inline Elements do not accept attributes.", sourceName,
                                 markupLine(sourceLine, elementLocation));
                break;
            case InlineValidationKind::KbdShortcutRequired:
                result.error("localization.string.kbd_shortcut_required", "Localized <kbd> requires a shortcut attribute.", sourceName,
                             markupLine(sourceLine, elementLocation));
                break;
            case InlineValidationKind::KbdShortcutInvalid:
                result.error("localization.string.kbd_shortcut_invalid", "Localized <kbd> shortcut must be a valid identifier.", sourceName,
                             markupLine(sourceLine, location));
                break;
            case InlineValidationKind::ChildrenUnsupported:
                result.error("localization.string.children_invalid", "Localized <" + finding.elementName + "> cannot contain content.", sourceName,
                             markupLine(sourceLine, elementLocation));
                break;
        }
    }
}

void collectLocalizedShortcutIds(const std::vector<SourceContent>& content, std::multiset<std::string>& shortcutIds) {
    for (const SourceContent& item : content) {
        if (!item.node) continue;
        const SourceNode& node = *item.node;
        if (node.tag == Tag::Kbd) {
            const auto shortcut = node.attributes.find("shortcut");
            if (shortcut != node.attributes.end()) shortcutIds.insert(shortcut->second.value);
        }
        collectLocalizedShortcutIds(node.content, shortcutIds);
    }
}

std::optional<std::multiset<std::string>> parseLocalizedMarkup(const std::string& source, DiagnosticResult& result, const std::string& sourceName,
                                                               std::size_t sourceLine) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<div>" + source + "</div>", sourceName);
    for (const Diagnostic& diagnostic : parsed.errors)
        result.error("localization.string.markup_invalid", diagnostic.message, sourceName,
                     markupLine(sourceLine, {diagnostic.line, diagnostic.column, 0}), diagnostic.column);
    if (!parsed.ok() || !parsed.document || !parsed.document->root || result.hasErrors()) return std::nullopt;

    const InlineValidationResult validation = validateInlineContent(parsed.document->root->content, localizedInlineTags());
    appendLocalizedMarkupDiagnostics(validation, result, sourceName, sourceLine);
    if (result.hasErrors()) return std::nullopt;
    std::multiset<std::string> shortcutIds;
    collectLocalizedShortcutIds(parsed.document->root->content, shortcutIds);
    return shortcutIds;
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

struct MappingEntry {
    std::string key;
    YAML::Node keyNode;
    YAML::Node value;
};

class YamlCatalogParser {
public:
    YamlCatalogParser(const std::string& yaml, const std::string& sourceName, bool base) : mYaml(yaml), mSourceName(sourceName), mBase(base) {}

    DiagnosticResult parse(ParsedCatalog& catalog) {
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

        for (const MappingEntry& entry : mapping(node, "locale " + locale.localeId, {"fallback", "strings"}))
            if (entry.key == "fallback") parseLocaleFallback(entry.value, locale);
            else parseStrings(entry.value, locale);

        if (!locale.stringsPresent)
            mResult.error("localization.locale.strings_missing", "Every locale entry requires a strings mapping: " + locale.localeId + ".",
                          mSourceName, locale.line);
        return locale;
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
        if (!node.IsScalar()) {
            mResult.error("localization.string.value_invalid", "Localized value must be a YAML ICU MessageFormat string.", mSourceName, lineOf(node),
                          columnOf(node));
            return std::nullopt;
        }

        const std::optional<std::string> text = scalar(node, "Localized String " + key);
        if (!text) return std::nullopt;
        const std::optional<std::multiset<std::string>> shortcutIds = parseLocalizedMarkup(*text, mResult, mSourceName, lineOf(node));
        if (!shortcutIds) return std::nullopt;
        return StringValue(*text, *shortcutIds, mSourceName, lineOf(keyNode));
    }

    const std::string& mYaml;
    const std::string& mSourceName;
    bool mBase = false;
    DiagnosticResult mResult;
};

DiagnosticResult parseYamlCatalog(const std::string& yaml, const std::string& sourceName, bool base, ParsedCatalog& catalog) {
    return YamlCatalogParser(yaml, sourceName, base).parse(catalog);
}

constexpr const char* kFirstStrongIsolate = "\xE2\x81\xA8";
constexpr const char* kPopDirectionalIsolate = "\xE2\x81\xA9";

std::string unicodeToUtf8(const icu::UnicodeString& value) {
    std::string result;
    value.toUTF8String(result);
    return result;
}

std::string escapeMarkup(std::string_view value) {
    std::string result;
    for (const char character : value) {
        switch (character) {
            case '&': result += "&amp;"; break;
            case '<': result += "&lt;"; break;
            case '>': result += "&gt;"; break;
            case '"': result += character; break;
            case '\'': result += character; break;
            default: result += character; break;
        }
    }
    return result;
}

class TokenFormat final : public icu::Format {
public:
    explicit TokenFormat(std::string token) : mToken(icu::UnicodeString::fromUTF8(token)) {}

    bool operator==(const icu::Format& other) const override {
        const auto* token = dynamic_cast<const TokenFormat*>(&other);
        return token && token->mToken == mToken;
    }

    TokenFormat* clone() const override { return new TokenFormat(*this); }

    icu::UnicodeString& format(const icu::Formattable&, icu::UnicodeString& appendTo, icu::FieldPosition&, UErrorCode& status) const override {
        if (U_SUCCESS(status)) appendTo.append(mToken);
        return appendTo;
    }

    void parseObject(const icu::UnicodeString&, icu::Formattable&, icu::ParsePosition& position) const override {
        position.setErrorIndex(position.getIndex());
    }

private:
    icu::UnicodeString mToken;
};

using ReplacementMap = std::unordered_map<std::string, std::string>;

struct FormattedMessage {
    std::string text;
    ReplacementMap replacements;
};

void appendReplacements(std::string_view text, const ReplacementMap& replacements, std::string& output) {
    std::size_t cursor = 0;
    while (cursor < text.size()) {
        std::size_t tokenPosition = std::string_view::npos;
        std::string_view token;
        const std::string* replacement = nullptr;
        for (const auto& [candidate, value] : replacements) {
            const std::size_t position = text.find(candidate, cursor);
            if (position != std::string_view::npos && position < tokenPosition) {
                tokenPosition = position;
                token = candidate;
                replacement = &value;
            }
        }

        if (!replacement) {
            output += text.substr(cursor);
            return;
        }

        output += text.substr(cursor, tokenPosition - cursor);
        output += *replacement;
        cursor = tokenPosition + token.size();
    }
}

std::string argumentToken(std::size_t index) {
    return "\xEE\x80\x80radia" + std::to_string(index) + "\xEE\x80\x81";
}

std::optional<FormattedMessage> formatMessage(const LocaleRecord& locale, const StringValue& value, const LocalizedText& localizedText) {
    UParseError parseError{};
    UErrorCode status = U_ZERO_ERROR;
    const icu::UnicodeString pattern = icu::UnicodeString::fromUTF8(value.pattern());
    icu::MessageFormat formatter(pattern, locale.locale, parseError, status);
    if (U_FAILURE(status)) return std::nullopt;

    std::vector<icu::UnicodeString> names;
    std::vector<icu::Formattable> arguments;
    names.reserve(localizedText.arguments().size());
    arguments.reserve(localizedText.arguments().size());

    FormattedMessage formatted;
    std::size_t tokenIndex = 0;
    for (const auto& [name, argument] : localizedText.arguments()) {
        names.push_back(icu::UnicodeString::fromUTF8(name));
        if (const auto* text = std::get_if<std::string>(&argument.value())) {
            arguments.emplace_back(icu::UnicodeString::fromUTF8(*text));
            const std::string token = argumentToken(tokenIndex++);
            formatter.setFormat(names.back(), TokenFormat(token), status);
            if (U_FAILURE(status)) return std::nullopt;

            const std::string isolated = std::string(kFirstStrongIsolate) + *text + kPopDirectionalIsolate;
            formatted.replacements.emplace(token, escapeMarkup(isolated));
        } else if (const auto* integer = std::get_if<std::int64_t>(&argument.value())) {
            arguments.emplace_back(*integer);
        } else {
            const double number = std::get<double>(argument.value());
            if (!std::isfinite(number)) return std::nullopt;
            arguments.emplace_back(number);
        }
    }

    icu::UnicodeString output;
    formatter.format(names.empty() ? nullptr : names.data(), arguments.empty() ? nullptr : arguments.data(), static_cast<int32_t>(arguments.size()),
                     output, status);
    if (U_FAILURE(status)) return std::nullopt;
    formatted.text = unicodeToUtf8(output);
    return formatted;
}
void appendPlainText(const SourceNode& node, std::string& output) {
    for (const SourceContent& content : node.content) {
        if (content.isText()) {
            output += content.text;
            continue;
        }

        if (content.node->tag == Tag::Br) output += '\n';
        else appendPlainText(*content.node, output);
    }
}

std::optional<std::string> plainTextFromMarkup(const std::string& markup) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<div>" + markup + "</div>", "<localized text>");
    if (!parsed.ok()) return std::nullopt;

    std::string result;
    appendPlainText(*parsed.document->root, result);
    return result;
}
} // namespace

struct LocalizationCatalog::Impl {
    DiagnosticResult load(const std::vector<ResourceLayer>& layers);
    void compile(DiagnosticResult& result);

    LocaleRecord* locale(const std::string& localeId);
    const StringValue* find(const LocaleRecord& locale, const std::string& key) const;

    std::vector<LocaleRecord> locales;
    std::unordered_map<std::string, std::size_t> localeIndices;
    std::string defaultLocale;
};

namespace {
std::string localeDisplayName(const icu::Locale& locale) {
    icu::UnicodeString displayName;
    locale.getDisplayName(locale, displayName);
    return unicodeToUtf8(displayName);
}

LayoutDirection localeDirection(const icu::Locale& locale) {
    return locale.isRightToLeft() ? LayoutDirection::RightToLeft : LayoutDirection::LeftToRight;
}

void deriveLocaleInfo(LocaleRecord& locale) {
    locale.info.name = localeDisplayName(locale.locale);
    locale.info.direction = localeDirection(locale.locale);
}

void validateMessageFormat(const StringValue& value, const LocaleRecord& locale, const std::string& key, DiagnosticResult& result) {
    UParseError parseError{};
    UErrorCode status = U_ZERO_ERROR;
    const icu::UnicodeString pattern = icu::UnicodeString::fromUTF8(value.pattern());
    const icu::MessageFormat format(pattern, locale.locale, parseError, status);
    (void)format;
    if (U_FAILURE(status)) {
        const std::size_t column = parseError.offset < 0 ? 0 : static_cast<std::size_t>(parseError.offset + 1);
        result.error("localization.string.message_invalid", "Invalid ICU MessageFormat for String '" + key + "'.", value.source, value.line, column);
    }
}
} // namespace

DiagnosticResult LocalizationCatalog::Impl::load(const std::vector<ResourceLayer>& layers) {
    DiagnosticResult result;
    if (layers.empty()) {
        result.error("localization.layers.empty", "No localization layers were provided.");
        return result;
    }

    ParsedCatalog base;
    result.append(parseYamlCatalog(layers.front().source, layers.front().sourceName, true, base));
    if (result.hasErrors()) return result;

    defaultLocale = *base.defaultLocale;
    const std::string defaultIdentity = localeIdentity(defaultLocale);
    for (ParsedLocale& parsed : base.locales) {
        LocaleRecord locale;
        locale.info.localeId = parsed.localeId;
        if (parsed.fallback) locale.info.fallback = *parsed.fallback;
        else if (localeIdentity(parsed.localeId) != defaultIdentity) locale.info.fallback = defaultLocale;

        if (localeIdentity(parsed.localeId) == defaultIdentity && parsed.fallback)
            result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);

        locale.strings = std::move(parsed.strings);
        localeIndices.emplace(localeIdentity(locale.info.localeId), locales.size());
        locales.push_back(std::move(locale));
    }
    if (result.hasErrors()) return result;

    for (std::size_t layerIndex = 1; layerIndex < layers.size(); ++layerIndex) {
        const ResourceLayer& layer = layers[layerIndex];
        ParsedCatalog patch;
        result.append(parseYamlCatalog(layer.source, layer.sourceName, false, patch));
        if (result.hasErrors()) return result;

        for (ParsedLocale& parsed : patch.locales) {
            const std::string identity = localeIdentity(parsed.localeId);
            if (identity == defaultIdentity && parsed.fallback) {
                result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);
                continue;
            }

            const auto existing = localeIndices.find(identity);
            if (existing == localeIndices.end()) {
                LocaleRecord locale;
                locale.info.localeId = parsed.localeId;
                locale.info.fallback = parsed.fallback.value_or(defaultLocale);
                locale.strings = std::move(parsed.strings);
                localeIndices.emplace(identity, locales.size());
                locales.push_back(std::move(locale));
                continue;
            }

            LocaleRecord& locale = locales[existing->second];
            if (parsed.fallback) locale.info.fallback = *parsed.fallback;
            for (auto& [key, value] : parsed.strings) locale.strings.insert_or_assign(std::move(key), std::move(value));
        }
        if (result.hasErrors()) return result;
    }

    compile(result);
    return result;
}

void LocalizationCatalog::Impl::compile(DiagnosticResult& result) {
    localeIndices.clear();

    std::sort(locales.begin(), locales.end(), [](const LocaleRecord& left, const LocaleRecord& right) {
        return localeIdentity(left.info.localeId) < localeIdentity(right.info.localeId);
    });
    for (std::size_t index = 0; index < locales.size(); ++index) {
        locales[index].fallbackChain.clear();
        localeIndices.emplace(localeIdentity(locales[index].info.localeId), index);
    }

    const auto defaultFound = localeIndices.find(localeIdentity(defaultLocale));
    if (defaultFound == localeIndices.end()) {
        result.error("localization.default.undefined", "defaultLocale is not defined in locales: " + defaultLocale + ".");
        return;
    }
    const std::size_t defaultIndex = defaultFound->second;

    for (std::size_t start = 0; start < locales.size(); ++start) {
        LocaleRecord& locale = locales[start];
        std::unordered_set<std::size_t> seen;
        std::size_t current = start;
        while (true) {
            if (!seen.insert(current).second) {
                result.error("localization.locale.fallback_cycle", "Locale fallback cycle contains " + locales[current].info.localeId + ".");
                break;
            }
            locale.fallbackChain.push_back(current);
            if (current == defaultIndex) break;

            const auto fallback = localeIndices.find(localeIdentity(locales[current].info.fallback));
            if (fallback == localeIndices.end()) {
                result.error("localization.locale.fallback_unknown",
                             "Unknown fallback locale " + locales[current].info.fallback + " for " + locales[current].info.localeId + ".");
                break;
            }
            current = fallback->second;
        }

        UErrorCode status = U_ZERO_ERROR;
        locale.locale = icu::Locale::forLanguageTag(locale.info.localeId, status);
        if (U_FAILURE(status) || locale.locale.isBogus())
            result.error("localization.locale.runtime_invalid", "Could not initialize locale services for " + locale.info.localeId + ".");
        else deriveLocaleInfo(locale);

        for (const auto& [key, value] : locale.strings) validateMessageFormat(value, locale, key, result);
    }
    if (result.hasErrors()) return;

    const StringMap& defaults = locales[defaultIndex].strings;
    for (const LocaleRecord& locale : locales) {
        for (const auto& [key, value] : locale.strings)
            if (!defaults.contains(key))
                result.error("localization.default.string_missing", "String '" + key + "' exists outside the default locale " + defaultLocale + ".",
                             value.source, value.line);
    }

    for (const auto& [key, defaultValue] : defaults) {
        for (const LocaleRecord& locale : locales) {
            const auto found = locale.strings.find(key);
            if (found == locale.strings.end()) continue;
            const StringValue& localized = found->second;
            if (localized.shortcutIds() != defaultValue.shortcutIds())
                result.error("localization.string.bindings_mismatch", "Translation must preserve the default kbd bindings: " + key + ".",
                             localized.source, localized.line);
        }
    }
}

LocaleRecord* LocalizationCatalog::Impl::locale(const std::string& localeId) {
    const auto found = localeIndices.find(localeIdentity(localeId));
    return found == localeIndices.end() ? nullptr : &locales[found->second];
}

const StringValue* LocalizationCatalog::Impl::find(const LocaleRecord& locale, const std::string& key) const {
    const auto found = locale.strings.find(key);
    return found == locale.strings.end() ? nullptr : &found->second;
}

LocalizationCatalog::LocalizationCatalog() : mImpl(std::make_unique<Impl>()) {}

LocalizationCatalog::~LocalizationCatalog() = default;
LocalizationCatalog::LocalizationCatalog(LocalizationCatalog&&) noexcept = default;
LocalizationCatalog& LocalizationCatalog::operator=(LocalizationCatalog&&) noexcept = default;

DiagnosticResult LocalizationCatalog::loadYaml(const std::string& yaml, const std::string& sourceName) {
    return loadYamlLayers({ResourceLayer{sourceName, yaml}});
}

DiagnosticResult LocalizationCatalog::loadYamlLayers(const std::vector<ResourceLayer>& layers) {
    auto candidate = std::make_unique<Impl>();
    DiagnosticResult result = candidate->load(layers);
    if (!result.hasErrors()) mImpl = std::move(candidate);
    return result;
}

std::vector<LocaleInfo> LocalizationCatalog::locales() const {
    std::vector<LocaleInfo> result;
    result.reserve(mImpl->locales.size());
    for (const LocaleRecord& locale : mImpl->locales) result.push_back(locale.info);
    return result;
}

const std::string& LocalizationCatalog::defaultLocaleId() const {
    return mImpl->defaultLocale;
}

const LocaleInfo* LocalizationCatalog::locale(const std::string& localeId) const {
    const LocaleRecord* locale = mImpl->locale(localeId);
    return locale ? &locale->info : nullptr;
}

bool LocalizationCatalog::containsLocale(const std::string& localeId) const {
    return mImpl->locale(localeId) != nullptr;
}

bool LocalizationCatalog::containsDefaultString(const std::string& stringKey) const {
    const LocaleRecord* locale = mImpl->locale(mImpl->defaultLocale);
    return locale && mImpl->find(*locale, stringKey);
}

std::string LocalizationCatalog::resolveMarkup(const std::string& localeId, const LocalizedText& text) const {
    const LocaleRecord* active = mImpl->locale(localeId);
    if (!active) active = mImpl->locale(mImpl->defaultLocale);
    if (!active) return escapeMarkup(text.key());

    const StringValue* value = nullptr;
    for (const std::size_t localeIndex : active->fallbackChain) {
        value = mImpl->find(mImpl->locales[localeIndex], text.key());
        if (value) break;
    }
    if (!value) {
        LL_WARNS("UI") << "Unknown localization String Key: " << text.key() << LL_ENDL;
        return escapeMarkup(text.key());
    }

    const std::optional<FormattedMessage> formatted = formatMessage(*active, *value, text);
    if (!formatted) {
        LL_WARNS("UI") << "Could not format localization String Key: " << text.key() << LL_ENDL;
        return escapeMarkup(text.key());
    }

    std::string fallback;
    appendReplacements(formatted->text, formatted->replacements, fallback);
    return fallback;
}

std::string LocalizationCatalog::resolveText(const std::string& localeId, const LocalizedText& text) const {
    const std::optional<std::string> plainText = plainTextFromMarkup(resolveMarkup(localeId, text));
    return plainText.value_or(text.key());
}

std::string LocalizationCatalog::resolveText(const std::string& localeId, const std::string& stringKey) const {
    return resolveText(localeId, LocalizedText(stringKey));
}
} // namespace radia::ui
