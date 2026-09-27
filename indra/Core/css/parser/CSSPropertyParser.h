/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstddef>
#include <limits>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>
#include "CSSProperties.h"
#include "CSSValue.h"
#include "types.h"

namespace radia::ui {
using LonghandParser = std::optional<CSSValue> (*)(CSSValueRange&);

struct ShorthandLonghand {
    CSSProperty property;
    LonghandParser parser;
    CSSValue initial;

    constexpr ShorthandLonghand(CSSProperty property, LonghandParser parser, CSSKeyword initial)
        : property(property), parser(parser), initial(CSS::Keyword{initial}) {}

    constexpr ShorthandLonghand(CSSProperty property, LonghandParser parser, CSSValue initial)
        : property(property), parser(parser), initial(std::move(initial)) {}
};

struct ParsedLonghand {
    std::optional<CSSProperty> property;
    CSSValue value;

    ParsedLonghand() = default;
    ParsedLonghand(CSSProperty property, CSSValue value) : property(property), value(std::move(value)) {}
};

using ParsedProperties = std::vector<ParsedLonghand>;

template<CSSProperty property> struct PropertyTraits;
template<CSSProperty property> bool expandProperty(const CSSValue&, ParsedProperties&);
template<typename Pattern, typename Longhands> struct ShorthandPatternParser;

inline const CSSValue& shorthandValue(const CSSValue& value) {
    return value;
}
inline const CSSValue& shorthandValue(const ParsedLonghand& longhand) {
    return longhand.value;
}

template<CSSProperty property> inline constexpr LonghandParser shorthandPropertyParser = [] {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::Longhands; }) return shorthandPropertyParser<Traits::Longhands::firstProperty>;
    else return Traits::parser;
}();

template<CSSProperty property> inline constexpr auto shorthandPropertyInitial = [] {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::Longhands; }) return shorthandPropertyInitial<Traits::Longhands::firstProperty>;
    else return Traits::initial;
}();

template<CSSProperty property> CSSValue shorthandPropertyInitialValue() {
    using Initial = std::remove_cv_t<decltype(shorthandPropertyInitial<property>)>;
    if constexpr (std::is_same_v<Initial, CSSKeyword>) return CSS::Keyword{shorthandPropertyInitial<property>};
    else return shorthandPropertyInitial<property>;
}

template<CSSProperty... propertyIDs> struct PropertyList;

template<CSSProperty first, CSSProperty... rest> struct PropertyList<first, rest...> {
    static constexpr std::array<CSSProperty, sizeof...(rest) + 1> properties{first, rest...};
    static constexpr CSSProperty firstProperty = first;

    static void appendInitialValues(ParsedProperties& result) {
        result.emplace_back(first, shorthandPropertyInitialValue<first>());
        (result.emplace_back(rest, shorthandPropertyInitialValue<rest>()), ...);
    }

    template<std::size_t index = 0, typename Values> static bool expand(const Values& values, ParsedProperties& result) {
        if constexpr (index == properties.size()) return true;
        else {
            constexpr CSSProperty property = properties[index];
            const std::size_t size = result.size();
            if (!expandProperty<property>(shorthandValue(values[index]), result) || !expand<index + 1>(values, result)) {
                result.resize(size);
                return false;
            }
            return true;
        }
    }
};

struct ListCardinality {
    std::size_t minimum;
    std::size_t maximum;

    friend constexpr bool operator==(const ListCardinality&, const ListCardinality&) = default;
};

inline constexpr std::size_t Infinite = std::numeric_limits<std::size_t>::max();
inline constexpr ListCardinality OneOrMore{1, Infinite};
inline constexpr ListCardinality ZeroOrMore{0, Infinite};
inline constexpr ListCardinality Optional{0, 1};

namespace CSSPropertyParserDetail {
template<auto parser> std::optional<CSSValue> parseValue(CSSValueRange& range) {
    auto value = parser(range);
    if (!value) return std::nullopt;
    return CSSValue{std::move(*value)};
}

template<std::size_t minimum, auto... parsers> std::optional<CSSValue> parseAnyOrder(CSSValueRange& range) {
    constexpr std::size_t count = sizeof...(parsers);
    static_assert(count > 0);
    static_assert(minimum <= count);

    using Parser = std::optional<CSSValue> (*)(CSSValueRange&);
    constexpr std::array<Parser, count> candidates{&parseValue<parsers>...};
    std::array<std::optional<CSSValue>, count> values;
    std::array<bool, count> used{};
    std::array<std::optional<CSSValue>, count> bestValues;
    std::optional<detail::CSSTokenRange> committedRange;
    std::size_t furthest = range.range.begin;

    const auto parse = [&](auto&& self, CSSValueRange current, std::size_t parsedCount) -> void {
        if (parsedCount >= minimum && current.range.begin >= furthest) {
            committedRange = current.range;
            bestValues = values;
            furthest = current.range.begin;
        }

        for (std::size_t index = 0; index < count; ++index) {
            if (used[index]) continue;

            CSSValueRange candidate = current;
            auto value = candidates[index](candidate);
            if (!value) continue;
            if (candidate.range.begin == current.range.begin) {
                const auto* emptyList = std::get_if<CSS::List>(&*value);
                if (!emptyList || !emptyList->values.empty()) continue;
            }

            used[index] = true;
            values[index] = std::move(*value);
            self(self, candidate, parsedCount + 1);
            values[index].reset();
            used[index] = false;
        }
    };

    parse(parse, range, 0);
    if (!committedRange) return std::nullopt;

    CSS::List result;
    for (auto& value : bestValues)
        if (value) CSSValueDetail::appendListValue(result, std::move(*value));
    range.range = *committedRange;
    return CSSValue{std::move(result)};
}
} // namespace CSSPropertyParserDetail

template<auto... parsers> std::optional<CSSValue> parseOneOf(CSSValueRange& range) {
    static_assert(sizeof...(parsers) > 0);

    using Parser = std::optional<CSSValue> (*)(CSSValueRange&);
    constexpr std::array<Parser, sizeof...(parsers)> candidates{&CSSPropertyParserDetail::parseValue<parsers>...};
    std::optional<CSSValue> bestValue;
    std::optional<detail::CSSTokenRange> bestRange;
    for (Parser parser : candidates) {
        CSSValueRange copy = range;
        const std::size_t start = copy.range.begin;
        auto value = parser(copy);
        if (!value) continue;
        if (copy.range.begin == start) {
            const auto* list = std::get_if<CSS::List>(&*value);
            if (!list || !list->values.empty()) continue;
        }
        if (!bestRange || copy.range.begin > bestRange->begin) {
            bestValue = std::move(*value);
            bestRange = copy.range;
        }
    }
    if (!bestRange) return std::nullopt;
    range.range = *bestRange;
    return bestValue;
}

template<auto... parsers> std::optional<CSSValue> parseAllAnyOrder(CSSValueRange& range) {
    return CSSPropertyParserDetail::parseAnyOrder<sizeof...(parsers), parsers...>(range);
}

template<auto... parsers>
    requires((std::is_invocable_v<decltype(parsers), CSSValueRange&> && ...) && sizeof...(parsers) > 0)
std::optional<CSSValue> parseOneOrMoreAnyOrder(CSSValueRange& range) {
    return CSSPropertyParserDetail::parseAnyOrder<1, parsers...>(range);
}

template<CSSProperty property> bool parseLonghand(CSSValueRange& range, ParsedProperties& result) {
    auto copy = range;
    auto value = PropertyTraits<property>::parser(copy);
    if (!value || CSSValueDetail::nextToken(copy)) return false;

    result.push_back({property, std::move(*value)});
    range.range = copy.range;
    return true;
}

inline bool isGenericFontFamilyKeyword(CSSKeyword keyword) {
    switch (keyword) {
        case CSSKeyword::Cursive:
        case CSSKeyword::Fantasy:
        case CSSKeyword::Fangsong:
        case CSSKeyword::Math:
        case CSSKeyword::Monospace:
        case CSSKeyword::SansSerif:
        case CSSKeyword::Serif:
        case CSSKeyword::SystemUi:
        case CSSKeyword::UiMonospace:
        case CSSKeyword::UiRounded:
        case CSSKeyword::UiSansSerif:
        case CSSKeyword::UiSerif: return true;
        default: return false;
    }
}

inline std::optional<CSSValue> parseFontFamily(CSSValueRange& range) {
    const auto families = detail::splitCSSOnDelimiter(range.stream, range.range, ',');
    if (families.empty()) return std::nullopt;

    CSS::List result;
    for (const detail::CSSTokenRange family : families) {
        if (family.begin == family.end) return std::nullopt;

        const auto& tokens = range.stream.tokens();
        if (family.end == family.begin + 1 && tokens[family.begin].kind == detail::CSSTokenKind::String) {
            const auto name = detail::decodeCSSString(range.stream.text(family.begin));
            if (!name) return std::nullopt;
            result.values.emplace_back(CSS::String{*name});
            continue;
        }

        if (tokens[family.begin].kind == detail::CSSTokenKind::Function) {
            const auto function = detail::parseCSSFunction(range.stream, family);
            if (!function || function->name != "generic") return std::nullopt;
            const auto argument = detail::trimCSSRange(range.stream, function->body);
            if (argument.end != argument.begin + 1 || tokens[argument.begin].kind != detail::CSSTokenKind::Ident) return std::nullopt;
            const auto keyword = findCSSKeyword(detail::normalizeCSSKeyword(range.stream, argument));
            if (!keyword
                || (*keyword != CSSKeyword::Fangsong
                    && *keyword != CSSKeyword::Kai
                    && *keyword != CSSKeyword::KhmerMul
                    && *keyword != CSSKeyword::Nastaliq))
                return std::nullopt;

            CSS::List arguments;
            arguments.values.emplace_back(CSS::Keyword{*keyword});
            result.values.emplace_back(std::make_shared<const CSS::Function>(CSS::Function{"generic", std::move(arguments)}));
            continue;
        }

        std::string name;
        std::size_t identifierCount = 0;
        for (std::size_t token = family.begin; token < family.end; ++token) {
            if (detail::isCSSTrivia(tokens[token].kind)) continue;
            if (tokens[token].kind != detail::CSSTokenKind::Ident) return std::nullopt;

            const std::string identifier = detail::decodeCSSIdentifier(range.stream.text(token));
            const std::string normalized = detail::lower(identifier);
            if (normalized == "default"
                || normalized == "inherit"
                || normalized == "initial"
                || normalized == "revert"
                || normalized == "revert-layer"
                || normalized == "unset")
                return std::nullopt;
            if (identifierCount++) name.push_back(' ');
            name += identifier;
        }
        if (!identifierCount) return std::nullopt;

        if (identifierCount == 1) {
            const auto keyword = findCSSKeyword(detail::lower(name));
            if (keyword && isGenericFontFamilyKeyword(*keyword)) {
                result.values.emplace_back(CSS::Keyword{*keyword});
                continue;
            }
        }
        result.values.emplace_back(CSS::String{std::move(name)});
    }

    range.range.begin = range.range.end;
    return CSSValue{std::move(result)};
}

inline std::optional<CSSValue> parseBorderImageSource(CSSValueRange& range) {
    auto noneRange = range;
    if (const auto none = consumeKeyword<CSSKeyword::NoneValue>(noneRange)) {
        range.range = noneRange.range;
        return CSSValue{*none};
    }

    const auto token = CSSValueDetail::nextToken(range);
    if (!token) return std::nullopt;
    const detail::CSSTokenRange source{*token, detail::skipCSSComponent(range.stream, *token, range.range.end)};
    if (source.end == source.begin) return std::nullopt;
    if (detail::parseCSSUrl(range.stream, source)) {
        CSSValueDetail::consume(range, source);
        return CSSValue{CSS::String{detail::serializeCSSRange(range.stream, source)}};
    }

    const auto function = detail::parseCSSFunction(range.stream, source);
    if (!function
        || (function->name != "linear-gradient"
            && function->name != "repeating-linear-gradient"
            && function->name != "radial-gradient"
            && function->name != "repeating-radial-gradient"
            && function->name != "conic-gradient"
            && function->name != "repeating-conic-gradient"))
        return std::nullopt;
    const std::string serialized = detail::serializeCSSRange(range.stream, source);
    CSSValueDetail::consume(range, source);
    return CSSValue{CSS::String{serialized}};
}

template<char separator, ListCardinality cardinality, auto parser> std::optional<CSSValue> parseListSeparatedBy(CSSValueRange& range) {
    static_assert(cardinality.minimum <= cardinality.maximum);

    auto copy = range;
    CSS::List values;
    std::size_t count = 0;

    while (count < cardinality.maximum) {
        CSSValueRange item = copy;
        bool hadSeparator = false;
        if constexpr (separator != ' ') {
            if (count) {
                const auto delimiter = CSSValueDetail::nextToken(item);
                if (!delimiter) break;
                if (!CSSValueDetail::isDelimiter(item, *delimiter, separator)) break;
                CSSValueDetail::consume(item, *delimiter);
                hadSeparator = true;
            }
        }

        const std::size_t start = item.range.begin;
        auto value = CSSPropertyParserDetail::parseValue<parser>(item);
        if (!value) {
            if (hadSeparator) return std::nullopt;
            break;
        }
        if (item.range.begin == start) return std::nullopt;
        copy.range = item.range;
        if constexpr (separator == ',')
            if (auto* group = std::get_if<CSS::List>(&*value)) values.values.emplace_back(std::make_shared<const CSS::List>(std::move(*group)));
            else CSSValueDetail::appendListValue(values, std::move(*value));
        else CSSValueDetail::appendListValue(values, std::move(*value));
        ++count;
    }

    if (count < cardinality.minimum) return std::nullopt;

    range.range = copy.range;
    return CSSValue{std::move(values)};
}

template<auto... parserFunctions> std::optional<CSSValue> parseSequence(CSSValueRange& range) {
    static_assert(sizeof...(parserFunctions) > 0);

    using Parser = std::optional<CSSValue> (*)(CSSValueRange&);
    constexpr std::array<Parser, sizeof...(parserFunctions)> parsers{&CSSPropertyParserDetail::parseValue<parserFunctions>...};
    auto copy = range;
    CSS::List values;
    for (Parser parser : parsers) {
        const std::size_t start = copy.range.begin;
        auto value = parser(copy);
        if (!value) return std::nullopt;
        if (copy.range.begin == start) {
            const auto* list = std::get_if<CSS::List>(&*value);
            if (!list || !list->values.empty()) return std::nullopt;
        }
        CSSValueDetail::appendListValue(values, std::move(*value));
    }

    range.range = copy.range;
    return CSSValue{std::move(values)};
}

template<auto parser> std::optional<CSSValue> parseOptional(CSSValueRange& range) {
    return parseListSeparatedBy<' ', Optional, parser>(range);
}

template<auto parser> std::optional<CSSValue> parseStar(CSSValueRange& range) {
    return parseListSeparatedBy<' ', ZeroOrMore, parser>(range);
}

template<auto parser> std::optional<CSSValue> parsePlus(CSSValueRange& range) {
    return parseListSeparatedBy<' ', OneOrMore, parser>(range);
}

template<std::size_t count, auto parser> std::optional<CSSValue> parseExactly(CSSValueRange& range) {
    return parseListSeparatedBy<' ', {count, count}, parser>(range);
}

template<ListCardinality cardinality, auto parser> std::optional<CSSValue> parseRange(CSSValueRange& range) {
    return parseListSeparatedBy<' ', cardinality, parser>(range);
}

template<auto parser> std::optional<CSSValue> parseHash(CSSValueRange& range) {
    return parseListSeparatedBy<',', OneOrMore, parser>(range);
}

template<std::size_t count, auto parser> std::optional<CSSValue> parseHash(CSSValueRange& range) {
    return parseListSeparatedBy<',', {count, count}, parser>(range);
}

template<ListCardinality cardinality, auto parser> std::optional<CSSValue> parseHash(CSSValueRange& range) {
    return parseListSeparatedBy<',', cardinality, parser>(range);
}

template<auto parser> std::optional<CSSValue> parseRequired(CSSValueRange& range) {
    auto copy = range;
    auto value = CSSPropertyParserDetail::parseValue<parser>(copy);
    if (!value) return std::nullopt;
    if (const auto* list = std::get_if<CSS::List>(&*value); list && list->values.empty()) return std::nullopt;
    range.range = copy.range;
    return value;
}

template<CSSProperty property> bool expandProperty(const CSSValue&, ParsedProperties&);

template<std::size_t size> struct CSSFunctionName {
    char value[size]{};

    consteval CSSFunctionName(const char (&name)[size]) {
        for (std::size_t index = 0; index < size; ++index) value[index] = name[index];
    }

    constexpr std::string_view view() const { return {value, size - 1}; }
    friend constexpr bool operator==(const CSSFunctionName&, const CSSFunctionName&) = default;
};

template<CSSFunctionName name, auto parser> std::optional<CSSValue> parseFunction(CSSValueRange& range) {
    auto copy = range;
    const auto token = CSSValueDetail::nextToken(copy);
    if (!token || copy.stream.tokens()[*token].kind != detail::CSSTokenKind::Function) return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingCSSToken || close >= copy.range.end) return std::nullopt;
    const auto function = detail::parseCSSFunction(copy.stream, {*token, close + 1});
    if (!function || function->name != name.view()) return std::nullopt;

    CSSValueRange arguments{copy.stream, function->body};
    auto parsed = CSSPropertyParserDetail::parseValue<parser>(arguments);
    if (!parsed || CSSValueDetail::nextToken(arguments)) return std::nullopt;

    CSS::List values;
    CSSValueDetail::appendListValue(values, std::move(*parsed));
    CSSValueDetail::consume(copy, close);
    range.range = copy.range;
    return CSSValue{std::make_shared<const CSS::Function>(CSS::Function{function->name, std::move(values)})};
}

template<CSSFunctionName name> std::optional<CSSValue> consumeFunction(CSSValueRange& range) {
    auto copy = range;
    const auto token = CSSValueDetail::nextToken(copy);
    if (!token || copy.stream.tokens()[*token].kind != detail::CSSTokenKind::Function) return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingCSSToken || close >= copy.range.end) return std::nullopt;
    const auto function = detail::parseCSSFunction(copy.stream, {*token, close + 1});
    if (!function || function->name != name.view()) return std::nullopt;

    CSSValueDetail::consume(copy, close);
    range.range = copy.range;
    return CSSValue{std::make_shared<const CSS::Function>(CSS::Function{function->name, {}})};
}

template<CSS::BlockType type, auto parser> std::optional<CSSValue> parseBlock(CSSValueRange& range) {
    auto copy = range;
    const auto token = CSSValueDetail::nextToken(copy);
    if (!token) return std::nullopt;

    const auto kind = copy.stream.tokens()[*token].kind;
    constexpr detail::CSSTokenKind open = [] {
        if constexpr (type == CSS::BlockType::Parentheses) return detail::CSSTokenKind::OpenParen;
        if constexpr (type == CSS::BlockType::Brackets) return detail::CSSTokenKind::OpenBracket;
        return detail::CSSTokenKind::OpenBrace;
    }();
    if (kind != open) return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingCSSToken || close >= copy.range.end) return std::nullopt;
    CSSValueRange contents{copy.stream, {*token + 1, close}};
    auto parsed = CSSPropertyParserDetail::parseValue<parser>(contents);
    if (!parsed || CSSValueDetail::nextToken(contents)) return std::nullopt;

    CSS::List values;
    CSSValueDetail::appendListValue(values, std::move(*parsed));
    CSSValueDetail::consume(copy, close);
    range.range = copy.range;
    return CSSValue{std::make_shared<const CSS::Block>(CSS::Block{type, std::move(values)})};
}

template<char literal> std::optional<CSSValue> consumeLiteral(CSSValueRange& range) {
    const auto token = CSSValueDetail::nextToken(range);
    if (!token || !CSSValueDetail::isDelimiter(range, *token, literal)) return std::nullopt;
    CSSValueDetail::consume(range, *token);
    return CSSValue{CSS::List{}};
}

template<CSSProperty property> bool parseShorthand(CSSValueRange& range, ParsedProperties& result) {
    auto copy = range;
    using Traits = PropertyTraits<property>;
    using Longhands = typename Traits::Longhands;
    auto values = [&] {
        if constexpr (requires { typename Traits::SyntaxParser; }) return Traits::SyntaxParser::parse(copy, Longhands{});
        else return ShorthandPatternParser<typename Traits::ShorthandPattern, Longhands>::parse(copy);
    }();
    if (!values) return false;

    if (!Longhands::expand(*values, result)) return false;
    if constexpr (requires { typename Traits::ResetLonghands; }) Traits::ResetLonghands::appendInitialValues(result);
    range.range = copy.range;
    return true;
}

inline CSSValue copyCSSValueComponent(const CSSValueComponent& value) {
    return std::visit(
        [](const auto& item) -> CSSValue {
            using Item = std::remove_cvref_t<decltype(item)>;
            if constexpr (std::is_same_v<Item, std::shared_ptr<const CSS::List>>) return *item;
            else return item;
        },
        value);
}

inline std::optional<std::array<CSSValue, 4>> expandFourValues(std::vector<CSSValue> values) {
    if (values.empty() || values.size() > 4) return std::nullopt;
    const std::size_t right = values.size() > 1 ? 1 : 0;
    const std::size_t bottom = values.size() > 2 ? 2 : 0;
    const std::size_t left = values.size() > 3 ? 3 : right;
    return std::array<CSSValue, 4>{values[0], values[right], values[bottom], values[left]};
}

inline std::optional<std::array<CSSValue, 4>> expandFourValues(const CSSValue& value) {
    std::vector<CSSValue> values;
    if (const auto* list = std::get_if<CSS::List>(&value)) {
        values.reserve(list->values.size());
        for (const CSSValueComponent& item : list->values) values.push_back(copyCSSValueComponent(item));
    } else values.push_back(value);
    return expandFourValues(std::move(values));
}

template<std::size_t count> std::optional<std::array<CSSValue, count>> unpackCSSValueList(const CSSValue& value) {
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list || list->values.size() != count) return std::nullopt;

    std::array<CSSValue, count> result;
    for (std::size_t index = 0; index < count; ++index) result[index] = copyCSSValueComponent(list->values[index]);
    return result;
}

inline CSSValue makeCornerRadiusValue(const CSSValue& horizontal, const CSSValue& vertical) {
    CSS::List values;
    CSSValueDetail::appendListValue(values, horizontal);
    CSSValueDetail::appendListValue(values, vertical);
    return CSSValue{std::move(values)};
}

inline std::optional<RectEdges<CSSValue>> consumeRectEdges(CSSValueRange& range, LonghandParser parser) {
    auto copy = range;
    std::vector<CSSValue> values;
    while (values.size() < 4 && CSSValueDetail::nextToken(copy)) {
        CSSValueRange item = copy;
        const std::size_t start = item.range.begin;
        auto value = parser(item);
        if (!value || item.range.begin == start) break;
        values.push_back(std::move(*value));
        copy.range = item.range;
    }

    if (CSSValueDetail::nextToken(copy)) return std::nullopt;
    auto expanded = expandFourValues(std::move(values));
    if (!expanded) return std::nullopt;

    RectEdges<CSSValue> result{
        std::move((*expanded)[0]),
        std::move((*expanded)[1]),
        std::move((*expanded)[2]),
        std::move((*expanded)[3]),
    };
    range.range = copy.range;
    return result;
}

template<CSSProperty top, CSSProperty right, CSSProperty bottom, CSSProperty left>
std::optional<std::array<ParsedLonghand, 4>> parseRectEdges(CSSValueRange& range) {
    auto values = consumeRectEdges(range, PropertyTraits<top>::parser);
    if (!values) return std::nullopt;

    return std::array{
        ParsedLonghand{top, std::move(values->top)},
        ParsedLonghand{right, std::move(values->right)},
        ParsedLonghand{bottom, std::move(values->bottom)},
        ParsedLonghand{left, std::move(values->left)},
    };
}

template<CSSProperty... properties> std::optional<std::array<ParsedLonghand, sizeof...(properties)>> parseOneOrMoreAnyOrder(CSSValueRange& range) {
    constexpr std::size_t count = sizeof...(properties);
    static_assert(count > 0);
    constexpr std::array longhands{ShorthandLonghand{properties, shorthandPropertyParser<properties>, shorthandPropertyInitial<properties>}...};

    std::array<std::optional<CSSValue>, count> values;
    std::array<bool, count> used{};
    std::optional<std::array<ParsedLonghand, count>> result;
    detail::CSSTokenRange committedRange = range.range;

    const auto parse = [&](auto&& self, CSSValueRange current, std::size_t parsedCount) -> bool {
        if (!CSSValueDetail::nextToken(current)) {
            if (!parsedCount) return false;
            std::array<ParsedLonghand, count> parsed{};
            for (std::size_t index = 0; index < count; ++index)
                parsed[index] = {longhands[index].property, values[index] ? *values[index] : longhands[index].initial};
            result = std::move(parsed);
            committedRange = current.range;
            return true;
        }

        for (std::size_t index = 0; index < count; ++index) {
            if (used[index]) continue;
            CSSValueRange candidate = current;
            auto value = longhands[index].parser(candidate);
            if (!value || candidate.range.begin == current.range.begin) continue;
            used[index] = true;
            values[index] = std::move(*value);
            if (self(self, candidate, parsedCount + 1)) return true;
            values[index].reset();
            used[index] = false;
        }
        return false;
    };

    if (!parse(parse, range, 0)) return std::nullopt;
    range.range = committedRange;
    return result;
}

struct AnyOrder {};
struct CoalescingPair {};
struct CoalescingQuad {};
struct Layered {};
struct SlashSeparated {};
struct SpaceSeparated {};

template<CSSProperty property> bool expandProperty(const CSSValue& value, ParsedProperties& result) {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::ShorthandPattern; })
        return ShorthandPatternParser<typename Traits::ShorthandPattern, typename Traits::Longhands>::expand(value, result);
    else {
        result.push_back({property, value});
        return true;
    }
}

template<char separator, CSSProperty... properties> struct SeparatedShorthandPatternParser {
    static_assert(sizeof...(properties) > 1);

    static auto parse(CSSValueRange& range) {
        constexpr std::size_t count = sizeof...(properties);
        constexpr std::array propertyIDs{properties...};
        constexpr std::array<LonghandParser, count> parsers{shorthandPropertyParser<properties>...};
        auto copy = range;
        std::array<ParsedLonghand, count> result{};

        for (std::size_t index = 0; index < count; ++index) {
            if constexpr (separator == '/') {
                if (index && !consumeLiteral<'/'>(copy)) return std::optional<std::array<ParsedLonghand, count>>{};
            }

            const std::size_t start = copy.range.begin;
            auto value = parsers[index](copy);
            if (!value || copy.range.begin == start) return std::optional<std::array<ParsedLonghand, count>>{};
            result[index] = {propertyIDs[index], std::move(*value)};
        }

        if (CSSValueDetail::nextToken(copy)) return std::optional<std::array<ParsedLonghand, count>>{};
        range.range = copy.range;
        return std::optional{std::move(result)};
    }

    static bool expand(const CSSValue& value, ParsedProperties& result) {
        constexpr std::size_t count = sizeof...(properties);
        auto values = unpackCSSValueList<count>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

template<CSSProperty... properties> struct ShorthandPatternParser<SlashSeparated, PropertyList<properties...>>
    : SeparatedShorthandPatternParser<'/', properties...> {};

template<CSSProperty... properties> struct ShorthandPatternParser<SpaceSeparated, PropertyList<properties...>>
    : SeparatedShorthandPatternParser<' ', properties...> {};

template<CSSProperty first, CSSProperty second> struct ShorthandPatternParser<CoalescingPair, PropertyList<first, second>> {
    static auto parse(CSSValueRange& range) {
        auto copy = range;
        auto firstValue = shorthandPropertyParser<first>(copy);
        if (!firstValue) return std::optional<std::array<ParsedLonghand, 2>>{};

        std::optional<CSSValue> secondValue;
        if (CSSValueDetail::nextToken(copy)) {
            secondValue = shorthandPropertyParser<second>(copy);
            if (!secondValue) return std::optional<std::array<ParsedLonghand, 2>>{};
        } else {
            auto secondCopy = range;
            secondValue = shorthandPropertyParser<second>(secondCopy);
            if (!secondValue || CSSValueDetail::nextToken(secondCopy)) return std::optional<std::array<ParsedLonghand, 2>>{};
            copy.range = secondCopy.range;
        }
        if (CSSValueDetail::nextToken(copy)) return std::optional<std::array<ParsedLonghand, 2>>{};

        range.range = copy.range;
        return std::optional{std::array{ParsedLonghand{first, std::move(*firstValue)}, ParsedLonghand{second, std::move(*secondValue)}}};
    }

    static bool expand(const CSSValue& value, ParsedProperties& result) {
        std::array<CSSValue, 2> values{value, value};
        if (const auto* list = std::get_if<CSS::List>(&value)) {
            if (list->values.empty() || list->values.size() > values.size()) return false;
            values[0] = copyCSSValueComponent(list->values[0]);
            values[1] = list->values.size() == 1 ? values[0] : copyCSSValueComponent(list->values[1]);
        }
        return PropertyList<first, second>::expand(values, result);
    }
};

template<CSSProperty top, CSSProperty right, CSSProperty bottom, CSSProperty left>
struct ShorthandPatternParser<CoalescingQuad, PropertyList<top, right, bottom, left>> {
    static auto parse(CSSValueRange& range) { return parseRectEdges<top, right, bottom, left>(range); }

    static bool expand(const CSSValue& value, ParsedProperties& result) {
        auto values = expandFourValues(value);
        return values && PropertyList<top, right, bottom, left>::expand(*values, result);
    }
};

template<CSSProperty... properties> struct ShorthandPatternParser<AnyOrder, PropertyList<properties...>> {
    static auto parse(CSSValueRange& range) { return parseOneOrMoreAnyOrder<properties...>(range); }

    static bool expand(const CSSValue& value, ParsedProperties& result) {
        auto values = unpackCSSValueList<sizeof...(properties)>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

inline void appendLayeredValue(CSS::List& list, CSSValue value) {
    if (auto* group = std::get_if<CSS::List>(&value)) list.values.emplace_back(std::make_shared<const CSS::List>(std::move(*group)));
    else CSSValueDetail::appendListValue(list, std::move(value));
}

template<CSSProperty... properties> struct ShorthandPatternParser<Layered, PropertyList<properties...>> {
    static_assert(sizeof...(properties) > 1);

    static auto parse(CSSValueRange& range) {
        constexpr std::size_t count = sizeof...(properties);
        auto copy = range;
        const auto layers = detail::splitCSSOnDelimiter(copy.stream, copy.range, ',');
        if (layers.empty()) return std::optional<std::array<ParsedLonghand, count>>{};

        std::array<CSS::List, count> values;
        for (const detail::CSSTokenRange layerRange : layers) {
            CSSValueRange layer{copy.stream, layerRange};
            auto parsed = ShorthandPatternParser<AnyOrder, PropertyList<properties...>>::parse(layer);
            if (!parsed || CSSValueDetail::nextToken(layer)) return std::optional<std::array<ParsedLonghand, count>>{};
            for (std::size_t index = 0; index < count; ++index) appendLayeredValue(values[index], std::move((*parsed)[index].value));
        }

        constexpr std::array propertyIDs{properties...};
        std::array<ParsedLonghand, count> result{};
        for (std::size_t index = 0; index < count; ++index) result[index] = {propertyIDs[index], std::move(values[index])};
        copy.range.begin = copy.range.end;
        range.range = copy.range;
        return std::optional{std::move(result)};
    }

    static bool expand(const CSSValue& value, ParsedProperties& result) {
        auto values = unpackCSSValueList<sizeof...(properties)>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

struct ParsePlaceContent {
    template<CSSProperty alignContent, CSSProperty justifyContent>
    static auto parse(CSSValueRange& range, PropertyList<alignContent, justifyContent>) {
        auto copy = range;
        auto first = shorthandPropertyParser<alignContent>(copy);
        if (!first) return std::optional<std::array<ParsedLonghand, 2>>{};
        if (!CSSValueDetail::nextToken(copy) && containsBaseline(*first)) {
            range.range = copy.range;
            return std::optional{
                std::array{ParsedLonghand{alignContent, std::move(*first)}, ParsedLonghand{justifyContent, CSS::Keyword{CSSKeyword::Start}}}};
        }
        return ShorthandPatternParser<CoalescingPair, PropertyList<alignContent, justifyContent>>::parse(range);
    }

    static bool containsBaseline(const CSS::List& list) {
        for (const CSSValueComponent& component : list.values) {
            if (const auto* keyword = std::get_if<CSS::Keyword>(&component); keyword && *keyword == CSSKeyword::Baseline) return true;
            if (const auto* nested = std::get_if<std::shared_ptr<const CSS::List>>(&component); nested && *nested && containsBaseline(**nested))
                return true;
        }
        return false;
    }

    static bool containsBaseline(const CSSValue& value) {
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) return *keyword == CSSKeyword::Baseline;
        if (const auto* list = std::get_if<CSS::List>(&value)) return containsBaseline(*list);
        return false;
    }
};

struct ParseFlex {
    template<CSSProperty grow, CSSProperty shrink, CSSProperty basis>
    static std::optional<std::array<ParsedLonghand, 3>> parse(CSSValueRange& range, PropertyList<grow, shrink, basis>) {
        auto noneRange = range;
        if (consumeKeyword<CSSKeyword::NoneValue>(noneRange)) {
            if (CSSValueDetail::nextToken(noneRange)) return std::nullopt;
            range.range = noneRange.range;
            return std::array{
                ParsedLonghand{grow, CSS::Number{0.f}},
                ParsedLonghand{shrink, CSS::Number{0.f}},
                ParsedLonghand{basis, CSS::Keyword{CSSKeyword::Auto}},
            };
        }

        auto copy = range;
        constexpr std::array properties{grow, shrink, basis};
        constexpr std::array<LonghandParser, 3> parsers{
            shorthandPropertyParser<grow>,
            shorthandPropertyParser<shrink>,
            shorthandPropertyParser<basis>,
        };
        std::array<std::optional<CSSValue>, 3> values;
        std::array<std::size_t, 3> order{};
        std::size_t count = 0;
        while (CSSValueDetail::nextToken(copy)) {
            bool matched = false;
            for (std::size_t index = 0; index < properties.size(); ++index) {
                if (values[index]) continue;
                CSSValueRange candidate = copy;
                auto value = parsers[index](candidate);
                if (!value || candidate.range.begin == copy.range.begin) continue;
                values[index] = std::move(*value);
                order[count++] = index;
                copy.range = candidate.range;
                matched = true;
                break;
            }
            if (!matched) return std::nullopt;
        }

        if (!count || (!values[0] && values[1])) return std::nullopt;
        if (values[0] && values[1] && values[2]) {
            const bool growShrinkBasis = order == std::array<std::size_t, 3>{0, 1, 2};
            const bool basisGrowShrink = order == std::array<std::size_t, 3>{2, 0, 1};
            if (!growShrinkBasis && !basisGrowShrink) return std::nullopt;
        }

        range.range = copy.range;
        return std::array{
            ParsedLonghand{grow, values[0] ? std::move(*values[0]) : CSSValue{CSS::Number{1.f}}},
            ParsedLonghand{shrink, values[1] ? std::move(*values[1]) : CSSValue{CSS::Number{1.f}}},
            ParsedLonghand{basis, values[2] ? std::move(*values[2]) : CSSValue{CSS::LengthPercentage{CSS::Percentage{0.f}}}},
        };
    }
};

struct ParseFont {
    template<CSSProperty fontStyle, CSSProperty fontWeight, CSSProperty fontWidth, CSSProperty fontSize, CSSProperty lineHeight,
             CSSProperty fontFamily>
    static std::optional<std::array<ParsedLonghand, 6>> parse(CSSValueRange& range,
                                                              PropertyList<fontStyle, fontWeight, fontWidth, fontSize, lineHeight, fontFamily>) {
        constexpr std::array longhands{
            ShorthandLonghand{fontStyle, shorthandPropertyParser<fontStyle>, shorthandPropertyInitial<fontStyle>},
            ShorthandLonghand{fontWeight, shorthandPropertyParser<fontWeight>, shorthandPropertyInitial<fontWeight>},
            ShorthandLonghand{fontWidth, shorthandPropertyParser<fontWidth>, shorthandPropertyInitial<fontWidth>},
            ShorthandLonghand{fontSize, shorthandPropertyParser<fontSize>, shorthandPropertyInitial<fontSize>},
            ShorthandLonghand{lineHeight, shorthandPropertyParser<lineHeight>, shorthandPropertyInitial<lineHeight>},
            ShorthandLonghand{fontFamily, shorthandPropertyParser<fontFamily>, shorthandPropertyInitial<fontFamily>},
        };
        const auto components = detail::splitCSSComponents(range.stream, range.range, true);
        if (components.size() < 2) return std::nullopt;

        const auto parseComponent = [&](LonghandParser parser, detail::CSSTokenRange component) -> std::optional<CSSValue> {
            CSSValueRange value{range.stream, component};
            auto parsed = parser(value);
            return parsed && !CSSValueDetail::nextToken(value) ? parsed : std::nullopt;
        };

        std::array<CSSValue, 6> values;
        for (std::size_t index = 0; index < longhands.size(); ++index) values[index] = longhands[index].initial;

        std::size_t fontSizeIndex = components.size();
        for (std::size_t index = 0; index < components.size(); ++index) {
            auto parsed = parseComponent(longhands[3].parser, components[index]);
            if (!parsed) continue;
            fontSizeIndex = index;
            values[3] = std::move(*parsed);
            break;
        }
        if (fontSizeIndex == components.size() || fontSizeIndex + 1 >= components.size()) return std::nullopt;

        std::size_t familyStart = fontSizeIndex + 1;
        if (CSSValueDetail::isDelimiter(range, components[familyStart].begin, '/')) {
            if (familyStart + 2 >= components.size()) return std::nullopt;
            const auto parsed = parseComponent(longhands[4].parser, components[familyStart + 1]);
            if (!parsed) return std::nullopt;
            values[4] = *parsed;
            familyStart += 2;
        }

        CSSValueRange familyRange{range.stream, {components[familyStart].begin, range.range.end}};
        auto parsedFamily = longhands[5].parser(familyRange);
        if (!parsedFamily || CSSValueDetail::nextToken(familyRange)) return std::nullopt;
        values[5] = std::move(*parsedFamily);

        constexpr std::array<std::size_t, 3> optionalIndices{0, 1, 2};
        std::array<bool, optionalIndices.size()> used{};
        for (std::size_t component = 0; component < fontSizeIndex; ++component) {
            bool matched = false;
            for (std::size_t index = 0; index < optionalIndices.size(); ++index) {
                if (used[index]) continue;
                const std::size_t longhandIndex = optionalIndices[index];
                const auto parsed = parseComponent(longhands[longhandIndex].parser, components[component]);
                if (!parsed) continue;
                if (longhandIndex == 2 && std::holds_alternative<CSS::Percentage>(*parsed)) continue;
                values[longhandIndex] = *parsed;
                used[index] = true;
                matched = true;
                break;
            }
            if (!matched) return std::nullopt;
        }

        range.range.begin = range.range.end;
        return std::array{
            ParsedLonghand{fontStyle, std::move(values[0])},  ParsedLonghand{fontWeight, std::move(values[1])},
            ParsedLonghand{fontWidth, std::move(values[2])},  ParsedLonghand{fontSize, std::move(values[3])},
            ParsedLonghand{lineHeight, std::move(values[4])}, ParsedLonghand{fontFamily, std::move(values[5])},
        };
    }
};

struct ParseBorderRadius {
    template<CSSProperty topLeft, CSSProperty topRight, CSSProperty bottomRight, CSSProperty bottomLeft>
    static std::optional<std::array<ParsedLonghand, 4>> parse(CSSValueRange& range, PropertyList<topLeft, topRight, bottomRight, bottomLeft>) {
        auto copy = range;
        auto horizontal = parseRange<{1, 4}, consumeLengthPercentage<Nonnegative>>(copy);
        if (!horizontal) return std::nullopt;
        auto horizontalValues = expandFourValues(*horizontal);
        if (!horizontalValues) return std::nullopt;

        auto verticalValues = *horizontalValues;
        CSSValueRange verticalRange = copy;
        if (consumeLiteral<'/'>(verticalRange)) {
            auto vertical = parseRange<{1, 4}, consumeLengthPercentage<Nonnegative>>(verticalRange);
            if (!vertical) return std::nullopt;
            auto expandedVertical = expandFourValues(*vertical);
            if (!expandedVertical) return std::nullopt;
            verticalValues = std::move(*expandedVertical);
            copy.range = verticalRange.range;
        }
        if (CSSValueDetail::nextToken(copy)) return std::nullopt;

        range.range = copy.range;
        return std::array{
            ParsedLonghand{topLeft, makeCornerRadiusValue((*horizontalValues)[0], verticalValues[0])},
            ParsedLonghand{topRight, makeCornerRadiusValue((*horizontalValues)[1], verticalValues[1])},
            ParsedLonghand{bottomRight, makeCornerRadiusValue((*horizontalValues)[2], verticalValues[2])},
            ParsedLonghand{bottomLeft, makeCornerRadiusValue((*horizontalValues)[3], verticalValues[3])},
        };
    }
};

struct ParseBorderImage {
    template<CSSProperty source, CSSProperty slice, CSSProperty width, CSSProperty outset, CSSProperty repeat>
    static std::optional<std::array<ParsedLonghand, 5>> parse(CSSValueRange& range, PropertyList<source, slice, width, outset, repeat>) {
        const auto components = detail::splitCSSComponents(range.stream, range.range, true);
        if (components.empty()) return std::nullopt;

        std::array<CSSValue, 5> values{
            shorthandPropertyInitialValue<source>(), shorthandPropertyInitialValue<slice>(),  shorthandPropertyInitialValue<width>(),
            shorthandPropertyInitialValue<outset>(), shorthandPropertyInitialValue<repeat>(),
        };
        using State = std::array<bool, 3>;
        std::optional<std::array<CSSValue, 5>> parsedValues;
        const auto parseComplete = [&range](LonghandParser parser, detail::CSSTokenRange tokens) -> std::optional<CSSValue> {
            CSSValueRange candidate{range.stream, tokens};
            auto value = parser(candidate);
            return value && !CSSValueDetail::nextToken(candidate) ? value : std::nullopt;
        };
        const auto parseSliceGroup = [&range, &parseComplete](std::size_t start, std::size_t end) -> std::optional<std::array<CSSValue, 3>> {
            const detail::CSSTokenRange tokens{start, end};
            const auto parts = detail::splitCSSComponents(range.stream, tokens, true);
            std::array<std::size_t, 2> slashes{};
            std::size_t slashCount = 0;
            for (std::size_t index = 0; index < parts.size(); ++index) {
                if (!CSSValueDetail::isDelimiter(CSSValueRange{range.stream, tokens}, parts[index].begin, '/')) continue;
                if (slashCount == slashes.size()) return std::nullopt;
                slashes[slashCount++] = index;
            }
            if (parts.empty() || (slashCount == 1 && slashes[0] == parts.size() - 1) || (slashCount == 2 && slashes[1] == parts.size() - 1))
                return std::nullopt;

            const std::size_t sliceEnd = slashCount ? parts[slashes[0]].begin : tokens.end;
            const auto sliceValue = parseComplete(shorthandPropertyParser<slice>, {tokens.begin, sliceEnd});
            if (!sliceValue) return std::nullopt;
            std::array<CSSValue, 3> result{*sliceValue, shorthandPropertyInitialValue<width>(), shorthandPropertyInitialValue<outset>()};
            if (!slashCount) return result;

            const std::size_t widthStart = parts[slashes[0]].end;
            const std::size_t widthEnd = slashCount == 2 ? parts[slashes[1]].begin : tokens.end;
            CSSValueRange widthRange{range.stream, {widthStart, widthEnd}};
            if (CSSValueDetail::nextToken(widthRange)) {
                const auto widthValue = parseComplete(shorthandPropertyParser<width>, {widthStart, widthEnd});
                if (!widthValue) return std::nullopt;
                result[1] = *widthValue;
            } else if (slashCount == 1) return std::nullopt;

            if (slashCount == 2) {
                const std::size_t outsetStart = parts[slashes[1]].end;
                CSSValueRange outsetRange{range.stream, {outsetStart, tokens.end}};
                if (!CSSValueDetail::nextToken(outsetRange)) return std::nullopt;
                const auto outsetValue = parseComplete(shorthandPropertyParser<outset>, {outsetStart, tokens.end});
                if (!outsetValue) return std::nullopt;
                result[2] = *outsetValue;
            }
            return result;
        };

        const auto parse = [&](auto&& self, std::size_t position, State used) -> bool {
            if (position == components.size()) {
                if (!used[1]) return false;
                parsedValues = values;
                return true;
            }

            if (!used[0]) {
                if (auto value = parseComplete(shorthandPropertyParser<source>, components[position])) {
                    values[0] = std::move(*value);
                    used[0] = true;
                    if (self(self, position + 1, used)) return true;
                    used[0] = false;
                    values[0] = shorthandPropertyInitialValue<source>();
                }
            }
            if (!used[2]) {
                for (std::size_t count = 1; count <= 2 && position + count <= components.size(); ++count) {
                    const auto value =
                        parseComplete(shorthandPropertyParser<repeat>, {components[position].begin, components[position + count - 1].end});
                    if (!value) continue;
                    values[4] = *value;
                    used[2] = true;
                    if (self(self, position + count, used)) return true;
                    used[2] = false;
                    values[4] = shorthandPropertyInitialValue<repeat>();
                }
            }
            if (!used[1]) {
                for (std::size_t end = position + 1; end <= components.size(); ++end) {
                    const auto parsed = parseSliceGroup(components[position].begin, components[end - 1].end);
                    if (!parsed) continue;
                    values[1] = (*parsed)[0];
                    values[2] = (*parsed)[1];
                    values[3] = (*parsed)[2];
                    used[1] = true;
                    if (self(self, end, used)) return true;
                    used[1] = false;
                    values[1] = shorthandPropertyInitialValue<slice>();
                    values[2] = shorthandPropertyInitialValue<width>();
                    values[3] = shorthandPropertyInitialValue<outset>();
                }
            }
            return false;
        };

        if (!parse(parse, 0, {})) return std::nullopt;
        range.range.begin = range.range.end;
        return std::array{
            ParsedLonghand{source, std::move((*parsedValues)[0])}, ParsedLonghand{slice, std::move((*parsedValues)[1])},
            ParsedLonghand{width, std::move((*parsedValues)[2])},  ParsedLonghand{outset, std::move((*parsedValues)[3])},
            ParsedLonghand{repeat, std::move((*parsedValues)[4])},
        };
    }
};
} // namespace radia::ui
