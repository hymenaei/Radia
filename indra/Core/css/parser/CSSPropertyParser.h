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
#include "LayoutGeometry.h"

namespace Core::CSS {
using LonghandParser = std::optional<Value> (*)(ValueRange&);

struct ShorthandLonghand {
    Property property;
    LonghandParser parser;
    Value initial;

    constexpr ShorthandLonghand(Property property, LonghandParser parser, KeywordName initial)
        : property(property)
        , parser(parser)
        , initial(Keyword {initial}) {}

    constexpr ShorthandLonghand(Property property, LonghandParser parser, Value initial)
        : property(property)
        , parser(parser)
        , initial(std::move(initial)) {}
};

struct ParsedLonghand {
    std::optional<Property> property;
    Value value;

    ParsedLonghand() = default;
    ParsedLonghand(Property property, Value value)
        : property(property)
        , value(std::move(value)) {}
};

using ParsedProperties = std::vector<ParsedLonghand>;

template<Property property> struct PropertyTraits;
template<Property property> bool expandProperty(const Value&, ParsedProperties&);
template<typename Pattern, typename Longhands> struct ShorthandPatternParser;

inline const Value& shorthandValue(const Value& value) { return value; }
inline const Value& shorthandValue(const ParsedLonghand& longhand) { return longhand.value; }

template<Property property>
inline constexpr LonghandParser kShorthandPropertyParser = [] {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::Longhands; })
        return kShorthandPropertyParser<Traits::Longhands::kFirstProperty>;
    else
        return Traits::parser;
}();

template<Property property>
inline constexpr auto kShorthandPropertyInitial = [] {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::Longhands; })
        return kShorthandPropertyInitial<Traits::Longhands::kFirstProperty>;
    else
        return Traits::initial;
}();

template<Property property> Value shorthandPropertyInitialValue() {
    using Initial = std::remove_cv_t<decltype(kShorthandPropertyInitial<property>)>;
    if constexpr (std::is_same_v<Initial, KeywordName>)
        return Keyword {kShorthandPropertyInitial<property>};
    else
        return kShorthandPropertyInitial<property>;
}

template<Property... propertyIDs> struct PropertyList;

template<Property first, Property... rest> struct PropertyList<first, rest...> {
    static constexpr std::array<Property, sizeof...(rest) + 1> kProperties {first, rest...};
    static constexpr Property kFirstProperty = first;

    static void appendInitialValues(ParsedProperties& result) {
        result.emplace_back(first, shorthandPropertyInitialValue<first>());
        (result.emplace_back(rest, shorthandPropertyInitialValue<rest>()), ...);
    }

    template<std::size_t index = 0, typename Values> static bool expand(const Values& values, ParsedProperties& result) {
        if constexpr (index == kProperties.size())
            return true;
        else {
            constexpr Property property = kProperties[index];
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

inline constexpr std::size_t kInfinite = std::numeric_limits<std::size_t>::max();
inline constexpr ListCardinality kOneOrMore {1, kInfinite};
inline constexpr ListCardinality kZeroOrMore {0, kInfinite};
inline constexpr ListCardinality kOptional {0, 1};

namespace detail {
template<auto parser> std::optional<Value> parseValue(ValueRange& range) {
    auto value = parser(range);
    if (!value)
        return std::nullopt;
    return Value {std::move(*value)};
}

template<std::size_t minimum, auto... parsers> std::optional<Value> parseAnyOrder(ValueRange& range) {
    constexpr std::size_t count = sizeof...(parsers);
    static_assert(count > 0);
    static_assert(minimum <= count);

    using Parser = std::optional<Value> (*)(ValueRange&);
    constexpr std::array<Parser, count> candidates {&parseValue<parsers>...};
    std::array<std::optional<Value>, count> values;
    std::array<bool, count> used {};
    std::array<std::optional<Value>, count> bestValues;
    std::optional<TokenRange> committedRange;
    std::size_t furthest = range.range.begin;

    const auto parse = [&](auto&& self, ValueRange current, std::size_t parsedCount) -> void {
        if (parsedCount >= minimum && current.range.begin >= furthest) {
            committedRange = current.range;
            bestValues = values;
            furthest = current.range.begin;
        }

        for (std::size_t index = 0; index < count; ++index) {
            if (used[index])
                continue;

            ValueRange candidate = current;
            auto value = candidates[index](candidate);
            if (!value)
                continue;
            if (candidate.range.begin == current.range.begin) {
                const auto* emptyList = std::get_if<List>(&*value);
                if (!emptyList || !emptyList->values.empty())
                    continue;
            }

            used[index] = true;
            values[index] = std::move(*value);
            self(self, candidate, parsedCount + 1);
            values[index].reset();
            used[index] = false;
        }
    };

    parse(parse, range, 0);
    if (!committedRange)
        return std::nullopt;

    List result;
    for (auto& value : bestValues)
        if (value)
            appendListValue(result, std::move(*value));
    range.range = *committedRange;
    return Value {std::move(result)};
}
} // namespace detail

template<auto... parsers> std::optional<Value> parseOneOf(ValueRange& range) {
    static_assert(sizeof...(parsers) > 0);

    using Parser = std::optional<Value> (*)(ValueRange&);
    constexpr std::array<Parser, sizeof...(parsers)> candidates {&detail::parseValue<parsers>...};
    std::optional<Value> bestValue;
    std::optional<detail::TokenRange> bestRange;
    for (Parser parser : candidates) {
        ValueRange copy = range;
        const std::size_t start = copy.range.begin;
        auto value = parser(copy);
        if (!value)
            continue;
        if (copy.range.begin == start) {
            const auto* list = std::get_if<List>(&*value);
            if (!list || !list->values.empty())
                continue;
        }
        if (!bestRange || copy.range.begin > bestRange->begin) {
            bestValue = std::move(*value);
            bestRange = copy.range;
        }
    }
    if (!bestRange)
        return std::nullopt;
    range.range = *bestRange;
    return bestValue;
}

template<auto... parsers> std::optional<Value> parseAllAnyOrder(ValueRange& range) {
    return detail::parseAnyOrder<sizeof...(parsers), parsers...>(range);
}

template<auto... parsers>
    requires((std::is_invocable_v<decltype(parsers), ValueRange&> && ...) && sizeof...(parsers) > 0)
std::optional<Value> parseOneOrMoreAnyOrder(ValueRange& range) {
    return detail::parseAnyOrder<1, parsers...>(range);
}

template<Property property> bool parseLonghand(ValueRange& range, ParsedProperties& result) {
    auto copy = range;
    auto value = PropertyTraits<property>::parser(copy);
    if (!value || detail::nextToken(copy))
        return false;

    result.push_back({property, std::move(*value)});
    range.range = copy.range;
    return true;
}

inline bool isGenericFontFamilyKeyword(KeywordName keyword) {
    switch (keyword) {
    case KeywordCursive:
    case KeywordFantasy:
    case KeywordFangsong:
    case KeywordMath:
    case KeywordMonospace:
    case KeywordSansSerif:
    case KeywordSerif:
    case KeywordSystemUi:
    case KeywordUiMonospace:
    case KeywordUiRounded:
    case KeywordUiSansSerif:
    case KeywordUiSerif:
        return true;
    default:
        return false;
    }
}

inline std::optional<Value> parseFontFamily(ValueRange& range) {
    const auto families = detail::splitOnDelimiter(range.stream, range.range, ',');
    if (families.empty())
        return std::nullopt;

    List result;
    for (const detail::TokenRange family : families) {
        if (family.begin == family.end)
            return std::nullopt;

        const auto& tokens = range.stream.tokens();
        if (family.end == family.begin + 1 && tokens[family.begin].kind == detail::TokenKind::String) {
            const auto name = detail::decodeString(range.stream.text(family.begin));
            if (!name)
                return std::nullopt;
            result.values.emplace_back(String {*name});
            continue;
        }

        if (tokens[family.begin].kind == detail::TokenKind::Function) {
            const auto function = detail::parseFunction(range.stream, family);
            if (!function || function->name != "generic")
                return std::nullopt;
            const auto argument = detail::trimRange(range.stream, function->body);
            if (argument.end != argument.begin + 1 || tokens[argument.begin].kind != detail::TokenKind::Ident)
                return std::nullopt;
            const auto keyword = findKeyword(detail::normalizeKeyword(range.stream, argument));
            if (!keyword
                || (*keyword != KeywordFangsong && *keyword != KeywordKai && *keyword != KeywordKhmerMul && *keyword != KeywordNastaliq))
                return std::nullopt;

            List arguments;
            arguments.values.emplace_back(Keyword {*keyword});
            result.values.emplace_back(std::make_shared<const Function>(Function {"generic", std::move(arguments)}));
            continue;
        }

        std::string name;
        std::size_t identifierCount = 0;
        for (std::size_t token = family.begin; token < family.end; ++token) {
            if (detail::isTrivia(tokens[token].kind))
                continue;
            if (tokens[token].kind != detail::TokenKind::Ident)
                return std::nullopt;

            const std::string identifier = detail::decodeIdentifier(range.stream.text(token));
            const std::string normalized = detail::lower(identifier);
            if (normalized == "default" || normalized == "inherit" || normalized == "initial" || normalized == "revert"
                || normalized == "revert-layer" || normalized == "unset")
                return std::nullopt;
            if (identifierCount++)
                name.push_back(' ');
            name += identifier;
        }
        if (!identifierCount)
            return std::nullopt;

        if (identifierCount == 1) {
            const auto keyword = findKeyword(detail::lower(name));
            if (keyword && isGenericFontFamilyKeyword(*keyword)) {
                result.values.emplace_back(Keyword {*keyword});
                continue;
            }
        }
        result.values.emplace_back(String {std::move(name)});
    }

    range.range.begin = range.range.end;
    return Value {std::move(result)};
}

inline std::optional<Value> parseBorderImageSource(ValueRange& range) {
    auto noneRange = range;
    if (const auto none = consumeKeyword<Keyword::NoneValue>(noneRange)) {
        range.range = noneRange.range;
        return Value {*none};
    }

    const auto token = detail::nextToken(range);
    if (!token)
        return std::nullopt;
    const detail::TokenRange source {*token, detail::skipComponent(range.stream, *token, range.range.end)};
    if (source.end == source.begin)
        return std::nullopt;
    if (detail::parseUrl(range.stream, source)) {
        detail::consume(range, source);
        return Value {String {detail::serializeRange(range.stream, source)}};
    }

    const auto function = detail::parseFunction(range.stream, source);
    if (!function
        || (function->name != "linear-gradient" && function->name != "repeating-linear-gradient" && function->name != "radial-gradient"
            && function->name != "repeating-radial-gradient" && function->name != "conic-gradient"
            && function->name != "repeating-conic-gradient"))
        return std::nullopt;
    const std::string serialized = detail::serializeRange(range.stream, source);
    detail::consume(range, source);
    return Value {String {serialized}};
}

template<char separator, ListCardinality cardinality, auto parser> std::optional<Value> parseListSeparatedBy(ValueRange& range) {
    static_assert(cardinality.minimum <= cardinality.maximum);

    auto copy = range;
    List values;
    std::size_t count = 0;

    while (count < cardinality.maximum) {
        ValueRange item = copy;
        bool hadSeparator = false;
        if constexpr (separator != ' ') {
            if (count) {
                const auto delimiter = detail::nextToken(item);
                if (!delimiter)
                    break;
                if (!detail::isDelimiter(item, *delimiter, separator))
                    break;
                detail::consume(item, *delimiter);
                hadSeparator = true;
            }
        }

        const std::size_t start = item.range.begin;
        auto value = detail::parseValue<parser>(item);
        if (!value) {
            if (hadSeparator)
                return std::nullopt;
            break;
        }
        if (item.range.begin == start)
            return std::nullopt;
        copy.range = item.range;
        if constexpr (separator == ',')
            if (auto* group = std::get_if<List>(&*value))
                values.values.emplace_back(std::make_shared<const List>(std::move(*group)));
            else
                detail::appendListValue(values, std::move(*value));
        else
            detail::appendListValue(values, std::move(*value));
        ++count;
    }

    if (count < cardinality.minimum)
        return std::nullopt;

    range.range = copy.range;
    return Value {std::move(values)};
}

template<auto... parserFunctions> std::optional<Value> parseSequence(ValueRange& range) {
    static_assert(sizeof...(parserFunctions) > 0);

    using Parser = std::optional<Value> (*)(ValueRange&);
    constexpr std::array<Parser, sizeof...(parserFunctions)> parsers {&detail::parseValue<parserFunctions>...};
    auto copy = range;
    List values;
    for (Parser parser : parsers) {
        const std::size_t start = copy.range.begin;
        auto value = parser(copy);
        if (!value)
            return std::nullopt;
        if (copy.range.begin == start) {
            const auto* list = std::get_if<List>(&*value);
            if (!list || !list->values.empty())
                return std::nullopt;
        }
        detail::appendListValue(values, std::move(*value));
    }

    range.range = copy.range;
    return Value {std::move(values)};
}

template<auto parser> std::optional<Value> parseOptional(ValueRange& range) { return parseListSeparatedBy<' ', kOptional, parser>(range); }

template<auto parser> std::optional<Value> parseStar(ValueRange& range) { return parseListSeparatedBy<' ', kZeroOrMore, parser>(range); }

template<auto parser> std::optional<Value> parsePlus(ValueRange& range) { return parseListSeparatedBy<' ', kOneOrMore, parser>(range); }

template<std::size_t count, auto parser> std::optional<Value> parseExactly(ValueRange& range) {
    return parseListSeparatedBy<' ', {count, count}, parser>(range);
}

template<ListCardinality cardinality, auto parser> std::optional<Value> parseRange(ValueRange& range) {
    return parseListSeparatedBy<' ', cardinality, parser>(range);
}

template<auto parser> std::optional<Value> parseHash(ValueRange& range) { return parseListSeparatedBy<',', kOneOrMore, parser>(range); }

template<std::size_t count, auto parser> std::optional<Value> parseHash(ValueRange& range) {
    return parseListSeparatedBy<',', {count, count}, parser>(range);
}

template<ListCardinality cardinality, auto parser> std::optional<Value> parseHash(ValueRange& range) {
    return parseListSeparatedBy<',', cardinality, parser>(range);
}

template<auto parser> std::optional<Value> parseRequired(ValueRange& range) {
    auto copy = range;
    auto value = detail::parseValue<parser>(copy);
    if (!value)
        return std::nullopt;
    if (const auto* list = std::get_if<List>(&*value); list && list->values.empty())
        return std::nullopt;
    range.range = copy.range;
    return value;
}

template<Property property> bool expandProperty(const Value&, ParsedProperties&);

template<std::size_t size> struct FunctionName {
    char value[size] {};

    consteval FunctionName(const char (&name)[size]) {
        for (std::size_t index = 0; index < size; ++index)
            value[index] = name[index];
    }

    constexpr std::string_view view() const { return {value, size - 1}; }
    friend constexpr bool operator==(const FunctionName&, const FunctionName&) = default;
};

template<FunctionName name, auto parser> std::optional<Value> parseFunction(ValueRange& range) {
    auto copy = range;
    const auto token = detail::nextToken(copy);
    if (!token || copy.stream.tokens()[*token].kind != detail::TokenKind::Function)
        return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingToken || close >= copy.range.end)
        return std::nullopt;
    const auto function = detail::parseFunction(copy.stream, {*token, close + 1});
    if (!function || function->name != name.view())
        return std::nullopt;

    ValueRange arguments {copy.stream, function->body};
    auto parsed = detail::parseValue<parser>(arguments);
    if (!parsed || detail::nextToken(arguments))
        return std::nullopt;

    List values;
    detail::appendListValue(values, std::move(*parsed));
    detail::consume(copy, close);
    range.range = copy.range;
    return Value {std::make_shared<const Function>(Function {function->name, std::move(values)})};
}

template<FunctionName name> std::optional<Value> consumeFunction(ValueRange& range) {
    auto copy = range;
    const auto token = detail::nextToken(copy);
    if (!token || copy.stream.tokens()[*token].kind != detail::TokenKind::Function)
        return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingToken || close >= copy.range.end)
        return std::nullopt;
    const auto function = detail::parseFunction(copy.stream, {*token, close + 1});
    if (!function || function->name != name.view())
        return std::nullopt;

    detail::consume(copy, close);
    range.range = copy.range;
    return Value {std::make_shared<const Function>(Function {function->name, {}})};
}

template<BlockType type, auto parser> std::optional<Value> parseBlock(ValueRange& range) {
    auto copy = range;
    const auto token = detail::nextToken(copy);
    if (!token)
        return std::nullopt;

    const auto kind = copy.stream.tokens()[*token].kind;
    constexpr detail::TokenKind open = [] {
        if constexpr (type == BlockType::Parentheses)
            return detail::TokenKind::OpenParen;
        if constexpr (type == BlockType::Brackets)
            return detail::TokenKind::OpenBracket;
        return detail::TokenKind::OpenBrace;
    }();
    if (kind != open)
        return std::nullopt;

    const std::size_t close = copy.stream.tokens()[*token].matching;
    if (close == detail::kNoMatchingToken || close >= copy.range.end)
        return std::nullopt;
    ValueRange contents {copy.stream, {*token + 1, close}};
    auto parsed = detail::parseValue<parser>(contents);
    if (!parsed || detail::nextToken(contents))
        return std::nullopt;

    List values;
    detail::appendListValue(values, std::move(*parsed));
    detail::consume(copy, close);
    range.range = copy.range;
    return Value {std::make_shared<const Block>(Block {type, std::move(values)})};
}

template<char literal> std::optional<Value> consumeLiteral(ValueRange& range) {
    const auto token = detail::nextToken(range);
    if (!token || !detail::isDelimiter(range, *token, literal))
        return std::nullopt;
    detail::consume(range, *token);
    return Value {List {}};
}

template<Property property> bool parseShorthand(ValueRange& range, ParsedProperties& result) {
    auto copy = range;
    using Traits = PropertyTraits<property>;
    using Longhands = typename Traits::Longhands;
    auto values = [&] {
        if constexpr (requires { typename Traits::SyntaxParser; })
            return Traits::SyntaxParser::parse(copy, Longhands {});
        else
            return ShorthandPatternParser<typename Traits::ShorthandPattern, Longhands>::parse(copy);
    }();
    if (!values)
        return false;

    if (!Longhands::expand(*values, result))
        return false;
    if constexpr (requires { typename Traits::ResetLonghands; })
        Traits::ResetLonghands::appendInitialValues(result);
    range.range = copy.range;
    return true;
}

inline Value copyValueComponent(const ValueComponent& value) {
    return std::visit(
        [](const auto& item) -> Value {
            using Item = std::remove_cvref_t<decltype(item)>;
            if constexpr (std::is_same_v<Item, std::shared_ptr<const List>>)
                return *item;
            else
                return item;
        },
        value);
}

inline std::optional<std::array<Value, 4>> expandFourValues(std::vector<Value> values) {
    if (values.empty() || values.size() > 4)
        return std::nullopt;
    const std::size_t right = values.size() > 1 ? 1 : 0;
    const std::size_t bottom = values.size() > 2 ? 2 : 0;
    const std::size_t left = values.size() > 3 ? 3 : right;
    return std::array<Value, 4> {values[0], values[right], values[bottom], values[left]};
}

inline std::optional<std::array<Value, 4>> expandFourValues(const Value& value) {
    std::vector<Value> values;
    if (const auto* list = std::get_if<List>(&value)) {
        values.reserve(list->values.size());
        for (const ValueComponent& item : list->values)
            values.push_back(copyValueComponent(item));
    } else
        values.push_back(value);
    return expandFourValues(std::move(values));
}

template<std::size_t count> std::optional<std::array<Value, count>> unpackValueList(const Value& value) {
    const auto* list = std::get_if<List>(&value);
    if (!list || list->values.size() != count)
        return std::nullopt;

    std::array<Value, count> result;
    for (std::size_t index = 0; index < count; ++index)
        result[index] = copyValueComponent(list->values[index]);
    return result;
}

inline Value makeCornerRadiusValue(const Value& horizontal, const Value& vertical) {
    List values;
    detail::appendListValue(values, horizontal);
    detail::appendListValue(values, vertical);
    return Value {std::move(values)};
}

inline std::optional<Layout::RectEdges<Value>> consumeRectEdges(ValueRange& range, LonghandParser parser) {
    auto copy = range;
    std::vector<Value> values;
    while (values.size() < 4 && detail::nextToken(copy)) {
        ValueRange item = copy;
        const std::size_t start = item.range.begin;
        auto value = parser(item);
        if (!value || item.range.begin == start)
            break;
        values.push_back(std::move(*value));
        copy.range = item.range;
    }

    if (detail::nextToken(copy))
        return std::nullopt;
    auto expanded = expandFourValues(std::move(values));
    if (!expanded)
        return std::nullopt;

    Layout::RectEdges<Value> result {
        std::move((*expanded)[0]),
        std::move((*expanded)[1]),
        std::move((*expanded)[2]),
        std::move((*expanded)[3]),
    };
    range.range = copy.range;
    return result;
}

template<Property top, Property right, Property bottom, Property left>
std::optional<std::array<ParsedLonghand, 4>> parseRectEdges(ValueRange& range) {
    auto values = consumeRectEdges(range, PropertyTraits<top>::parser);
    if (!values)
        return std::nullopt;

    return std::array {
        ParsedLonghand {top, std::move(values->top)},
        ParsedLonghand {right, std::move(values->right)},
        ParsedLonghand {bottom, std::move(values->bottom)},
        ParsedLonghand {left, std::move(values->left)},
    };
}

template<Property... properties>
std::optional<std::array<ParsedLonghand, sizeof...(properties)>> parseOneOrMoreAnyOrder(ValueRange& range) {
    constexpr std::size_t count = sizeof...(properties);
    static_assert(count > 0);
    constexpr std::array longhands {ShorthandLonghand {properties, kShorthandPropertyParser<properties>,
        kShorthandPropertyInitial<properties>}...};

    std::array<std::optional<Value>, count> values;
    std::array<bool, count> used {};
    std::optional<std::array<ParsedLonghand, count>> result;
    detail::TokenRange committedRange = range.range;

    const auto parse = [&](auto&& self, ValueRange current, std::size_t parsedCount) -> bool {
        if (!detail::nextToken(current)) {
            if (!parsedCount)
                return false;
            std::array<ParsedLonghand, count> parsed {};
            for (std::size_t index = 0; index < count; ++index)
                parsed[index] = {longhands[index].property, values[index] ? *values[index] : longhands[index].initial};
            result = std::move(parsed);
            committedRange = current.range;
            return true;
        }

        for (std::size_t index = 0; index < count; ++index) {
            if (used[index])
                continue;
            ValueRange candidate = current;
            auto value = longhands[index].parser(candidate);
            if (!value || candidate.range.begin == current.range.begin)
                continue;
            used[index] = true;
            values[index] = std::move(*value);
            if (self(self, candidate, parsedCount + 1))
                return true;
            values[index].reset();
            used[index] = false;
        }
        return false;
    };

    if (!parse(parse, range, 0))
        return std::nullopt;
    range.range = committedRange;
    return result;
}

struct AnyOrder {};
struct CoalescingPair {};
struct CoalescingQuad {};
struct Layered {};
struct SlashSeparated {};
struct SpaceSeparated {};

template<Property property> bool expandProperty(const Value& value, ParsedProperties& result) {
    using Traits = PropertyTraits<property>;
    if constexpr (requires { typename Traits::ShorthandPattern; })
        return ShorthandPatternParser<typename Traits::ShorthandPattern, typename Traits::Longhands>::expand(value, result);
    else {
        result.push_back({property, value});
        return true;
    }
}

template<char separator, Property... properties> struct SeparatedShorthandPatternParser {
    static_assert(sizeof...(properties) > 1);

    static auto parse(ValueRange& range) {
        constexpr std::size_t count = sizeof...(properties);
        constexpr std::array propertyIDs {properties...};
        constexpr std::array<LonghandParser, count> parsers {kShorthandPropertyParser<properties>...};
        auto copy = range;
        std::array<ParsedLonghand, count> result {};

        for (std::size_t index = 0; index < count; ++index) {
            if constexpr (separator == '/') {
                if (index && !consumeLiteral<'/'>(copy))
                    return std::optional<std::array<ParsedLonghand, count>> {};
            }

            const std::size_t start = copy.range.begin;
            auto value = parsers[index](copy);
            if (!value || copy.range.begin == start)
                return std::optional<std::array<ParsedLonghand, count>> {};
            result[index] = {propertyIDs[index], std::move(*value)};
        }

        if (detail::nextToken(copy))
            return std::optional<std::array<ParsedLonghand, count>> {};
        range.range = copy.range;
        return std::optional {std::move(result)};
    }

    static bool expand(const Value& value, ParsedProperties& result) {
        constexpr std::size_t count = sizeof...(properties);
        auto values = unpackValueList<count>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

template<Property... properties>
struct ShorthandPatternParser<SlashSeparated, PropertyList<properties...>> : SeparatedShorthandPatternParser<'/', properties...> {};

template<Property... properties>
struct ShorthandPatternParser<SpaceSeparated, PropertyList<properties...>> : SeparatedShorthandPatternParser<' ', properties...> {};

template<Property first, Property second> struct ShorthandPatternParser<CoalescingPair, PropertyList<first, second>> {
    static auto parse(ValueRange& range) {
        auto copy = range;
        auto firstValue = kShorthandPropertyParser<first>(copy);
        if (!firstValue)
            return std::optional<std::array<ParsedLonghand, 2>> {};

        std::optional<Value> secondValue;
        if (detail::nextToken(copy)) {
            secondValue = kShorthandPropertyParser<second>(copy);
            if (!secondValue)
                return std::optional<std::array<ParsedLonghand, 2>> {};
        } else {
            auto secondCopy = range;
            secondValue = kShorthandPropertyParser<second>(secondCopy);
            if (!secondValue || detail::nextToken(secondCopy))
                return std::optional<std::array<ParsedLonghand, 2>> {};
            copy.range = secondCopy.range;
        }
        if (detail::nextToken(copy))
            return std::optional<std::array<ParsedLonghand, 2>> {};

        range.range = copy.range;
        return std::optional {std::array {ParsedLonghand {first, std::move(*firstValue)},
            ParsedLonghand {second, std::move(*secondValue)}}};
    }

    static bool expand(const Value& value, ParsedProperties& result) {
        std::array<Value, 2> values {value, value};
        if (const auto* list = std::get_if<List>(&value)) {
            if (list->values.empty() || list->values.size() > values.size())
                return false;
            values[0] = copyValueComponent(list->values[0]);
            values[1] = list->values.size() == 1 ? values[0] : copyValueComponent(list->values[1]);
        }
        return PropertyList<first, second>::expand(values, result);
    }
};

template<Property top, Property right, Property bottom, Property left>
struct ShorthandPatternParser<CoalescingQuad, PropertyList<top, right, bottom, left>> {
    static auto parse(ValueRange& range) { return parseRectEdges<top, right, bottom, left>(range); }

    static bool expand(const Value& value, ParsedProperties& result) {
        auto values = expandFourValues(value);
        return values && PropertyList<top, right, bottom, left>::expand(*values, result);
    }
};

template<Property... properties> struct ShorthandPatternParser<AnyOrder, PropertyList<properties...>> {
    static auto parse(ValueRange& range) { return parseOneOrMoreAnyOrder<properties...>(range); }

    static bool expand(const Value& value, ParsedProperties& result) {
        auto values = unpackValueList<sizeof...(properties)>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

inline void appendLayeredValue(List& list, Value value) {
    if (auto* group = std::get_if<List>(&value))
        list.values.emplace_back(std::make_shared<const List>(std::move(*group)));
    else
        detail::appendListValue(list, std::move(value));
}

template<Property... properties> struct ShorthandPatternParser<Layered, PropertyList<properties...>> {
    static_assert(sizeof...(properties) > 1);

    static auto parse(ValueRange& range) {
        constexpr std::size_t count = sizeof...(properties);
        auto copy = range;
        const auto layers = detail::splitOnDelimiter(copy.stream, copy.range, ',');
        if (layers.empty())
            return std::optional<std::array<ParsedLonghand, count>> {};

        std::array<List, count> values;
        for (const detail::TokenRange layerRange : layers) {
            ValueRange layer {copy.stream, layerRange};
            auto parsed = ShorthandPatternParser<AnyOrder, PropertyList<properties...>>::parse(layer);
            if (!parsed || detail::nextToken(layer))
                return std::optional<std::array<ParsedLonghand, count>> {};
            for (std::size_t index = 0; index < count; ++index)
                appendLayeredValue(values[index], std::move((*parsed)[index].value));
        }

        constexpr std::array propertyIDs {properties...};
        std::array<ParsedLonghand, count> result {};
        for (std::size_t index = 0; index < count; ++index)
            result[index] = {propertyIDs[index], std::move(values[index])};
        copy.range.begin = copy.range.end;
        range.range = copy.range;
        return std::optional {std::move(result)};
    }

    static bool expand(const Value& value, ParsedProperties& result) {
        auto values = unpackValueList<sizeof...(properties)>(value);
        return values && PropertyList<properties...>::expand(*values, result);
    }
};

struct ParsePlaceContent {
    template<Property alignContent, Property justifyContent>
    static auto parse(ValueRange& range, PropertyList<alignContent, justifyContent>) {
        auto copy = range;
        auto first = kShorthandPropertyParser<alignContent>(copy);
        if (!first)
            return std::optional<std::array<ParsedLonghand, 2>> {};
        if (!detail::nextToken(copy) && containsBaseline(*first)) {
            range.range = copy.range;
            return std::optional {std::array {ParsedLonghand {alignContent, std::move(*first)},
                ParsedLonghand {justifyContent, Keyword {KeywordStart}}}};
        }
        return ShorthandPatternParser<CoalescingPair, PropertyList<alignContent, justifyContent>>::parse(range);
    }

    static bool containsBaseline(const List& list) {
        for (const ValueComponent& component : list.values) {
            if (const auto* keyword = std::get_if<Keyword>(&component); keyword && *keyword == KeywordBaseline)
                return true;
            if (const auto* nested = std::get_if<std::shared_ptr<const List>>(&component); nested && *nested && containsBaseline(**nested))
                return true;
        }
        return false;
    }

    static bool containsBaseline(const Value& value) {
        if (const auto* keyword = std::get_if<Keyword>(&value))
            return *keyword == KeywordBaseline;
        if (const auto* list = std::get_if<List>(&value))
            return containsBaseline(*list);
        return false;
    }
};

struct ParseFlex {
    template<Property grow, Property shrink, Property basis>
    static std::optional<std::array<ParsedLonghand, 3>> parse(ValueRange& range, PropertyList<grow, shrink, basis>) {
        auto noneRange = range;
        if (consumeKeyword<Keyword::NoneValue>(noneRange)) {
            if (detail::nextToken(noneRange))
                return std::nullopt;
            range.range = noneRange.range;
            return std::array {
                ParsedLonghand {grow, Number {0.f}},
                ParsedLonghand {shrink, Number {0.f}},
                ParsedLonghand {basis, Keyword {KeywordAuto}},
            };
        }

        auto copy = range;
        constexpr std::array properties {grow, shrink, basis};
        constexpr std::array<LonghandParser, 3> parsers {
            kShorthandPropertyParser<grow>,
            kShorthandPropertyParser<shrink>,
            kShorthandPropertyParser<basis>,
        };
        std::array<std::optional<Value>, 3> values;
        std::array<std::size_t, 3> order {};
        std::size_t count = 0;
        while (detail::nextToken(copy)) {
            bool matched = false;
            for (std::size_t index = 0; index < properties.size(); ++index) {
                if (values[index])
                    continue;
                ValueRange candidate = copy;
                auto value = parsers[index](candidate);
                if (!value || candidate.range.begin == copy.range.begin)
                    continue;
                values[index] = std::move(*value);
                order[count++] = index;
                copy.range = candidate.range;
                matched = true;
                break;
            }
            if (!matched)
                return std::nullopt;
        }

        if (!count || (!values[0] && values[1]))
            return std::nullopt;
        if (values[0] && values[1] && values[2]) {
            const bool growShrinkBasis = order == std::array<std::size_t, 3> {0, 1, 2};
            const bool basisGrowShrink = order == std::array<std::size_t, 3> {2, 0, 1};
            if (!growShrinkBasis && !basisGrowShrink)
                return std::nullopt;
        }

        range.range = copy.range;
        return std::array {
            ParsedLonghand {grow, values[0] ? std::move(*values[0]) : Value {Number {1.f}}},
            ParsedLonghand {shrink, values[1] ? std::move(*values[1]) : Value {Number {1.f}}},
            ParsedLonghand {basis, values[2] ? std::move(*values[2]) : Value {LengthPercentage {Percentage {0.f}}}},
        };
    }
};

struct ParseFont {
    template<Property fontStyle, Property fontWeight, Property fontWidth, Property fontSize, Property lineHeight, Property fontFamily>
    static std::optional<std::array<ParsedLonghand, 6>> parse(ValueRange& range,
        PropertyList<fontStyle, fontWeight, fontWidth, fontSize, lineHeight, fontFamily>) {
        constexpr std::array longhands {
            ShorthandLonghand {fontStyle, kShorthandPropertyParser<fontStyle>, kShorthandPropertyInitial<fontStyle>},
            ShorthandLonghand {fontWeight, kShorthandPropertyParser<fontWeight>, kShorthandPropertyInitial<fontWeight>},
            ShorthandLonghand {fontWidth, kShorthandPropertyParser<fontWidth>, kShorthandPropertyInitial<fontWidth>},
            ShorthandLonghand {fontSize, kShorthandPropertyParser<fontSize>, kShorthandPropertyInitial<fontSize>},
            ShorthandLonghand {lineHeight, kShorthandPropertyParser<lineHeight>, kShorthandPropertyInitial<lineHeight>},
            ShorthandLonghand {fontFamily, kShorthandPropertyParser<fontFamily>, kShorthandPropertyInitial<fontFamily>},
        };
        const auto components = detail::splitComponents(range.stream, range.range, true);
        if (components.size() < 2)
            return std::nullopt;

        const auto parseComponent = [&](LonghandParser parser, detail::TokenRange component) -> std::optional<Value> {
            ValueRange value {range.stream, component};
            auto parsed = parser(value);
            return parsed && !detail::nextToken(value) ? parsed : std::nullopt;
        };

        std::array<Value, 6> values;
        for (std::size_t index = 0; index < longhands.size(); ++index)
            values[index] = longhands[index].initial;

        std::size_t fontSizeIndex = components.size();
        for (std::size_t index = 0; index < components.size(); ++index) {
            auto parsed = parseComponent(longhands[3].parser, components[index]);
            if (!parsed)
                continue;
            fontSizeIndex = index;
            values[3] = std::move(*parsed);
            break;
        }
        if (fontSizeIndex == components.size() || fontSizeIndex + 1 >= components.size())
            return std::nullopt;

        std::size_t familyStart = fontSizeIndex + 1;
        if (detail::isDelimiter(range, components[familyStart].begin, '/')) {
            if (familyStart + 2 >= components.size())
                return std::nullopt;
            const auto parsed = parseComponent(longhands[4].parser, components[familyStart + 1]);
            if (!parsed)
                return std::nullopt;
            values[4] = *parsed;
            familyStart += 2;
        }

        ValueRange familyRange {range.stream, {components[familyStart].begin, range.range.end}};
        auto parsedFamily = longhands[5].parser(familyRange);
        if (!parsedFamily || detail::nextToken(familyRange))
            return std::nullopt;
        values[5] = std::move(*parsedFamily);

        constexpr std::array<std::size_t, 3> optionalIndices {0, 1, 2};
        std::array<bool, optionalIndices.size()> used {};
        for (std::size_t component = 0; component < fontSizeIndex; ++component) {
            bool matched = false;
            for (std::size_t index = 0; index < optionalIndices.size(); ++index) {
                if (used[index])
                    continue;
                const std::size_t longhandIndex = optionalIndices[index];
                const auto parsed = parseComponent(longhands[longhandIndex].parser, components[component]);
                if (!parsed)
                    continue;
                if (longhandIndex == 2 && std::holds_alternative<Percentage>(*parsed))
                    continue;
                values[longhandIndex] = *parsed;
                used[index] = true;
                matched = true;
                break;
            }
            if (!matched)
                return std::nullopt;
        }

        range.range.begin = range.range.end;
        return std::array {
            ParsedLonghand {fontStyle, std::move(values[0])},
            ParsedLonghand {fontWeight, std::move(values[1])},
            ParsedLonghand {fontWidth, std::move(values[2])},
            ParsedLonghand {fontSize, std::move(values[3])},
            ParsedLonghand {lineHeight, std::move(values[4])},
            ParsedLonghand {fontFamily, std::move(values[5])},
        };
    }
};

struct ParseBorderRadius {
    template<Property topLeft, Property topRight, Property bottomRight, Property bottomLeft>
    static std::optional<std::array<ParsedLonghand, 4>> parse(ValueRange& range, PropertyList<topLeft, topRight, bottomRight, bottomLeft>) {
        auto copy = range;
        auto horizontal = parseRange<{1, 4}, consumeLengthPercentage<kNonnegative>>(copy);
        if (!horizontal)
            return std::nullopt;
        auto horizontalValues = expandFourValues(*horizontal);
        if (!horizontalValues)
            return std::nullopt;

        auto verticalValues = *horizontalValues;
        ValueRange verticalRange = copy;
        if (consumeLiteral<'/'>(verticalRange)) {
            auto vertical = parseRange<{1, 4}, consumeLengthPercentage<kNonnegative>>(verticalRange);
            if (!vertical)
                return std::nullopt;
            auto expandedVertical = expandFourValues(*vertical);
            if (!expandedVertical)
                return std::nullopt;
            verticalValues = std::move(*expandedVertical);
            copy.range = verticalRange.range;
        }
        if (detail::nextToken(copy))
            return std::nullopt;

        range.range = copy.range;
        return std::array {
            ParsedLonghand {topLeft, makeCornerRadiusValue((*horizontalValues)[0], verticalValues[0])},
            ParsedLonghand {topRight, makeCornerRadiusValue((*horizontalValues)[1], verticalValues[1])},
            ParsedLonghand {bottomRight, makeCornerRadiusValue((*horizontalValues)[2], verticalValues[2])},
            ParsedLonghand {bottomLeft, makeCornerRadiusValue((*horizontalValues)[3], verticalValues[3])},
        };
    }
};

struct ParseBorderImage {
    template<Property source, Property slice, Property width, Property outset, Property repeat>
    static std::optional<std::array<ParsedLonghand, 5>> parse(ValueRange& range, PropertyList<source, slice, width, outset, repeat>) {
        const auto components = detail::splitComponents(range.stream, range.range, true);
        if (components.empty())
            return std::nullopt;

        std::array<Value, 5> values {
            shorthandPropertyInitialValue<source>(),
            shorthandPropertyInitialValue<slice>(),
            shorthandPropertyInitialValue<width>(),
            shorthandPropertyInitialValue<outset>(),
            shorthandPropertyInitialValue<repeat>(),
        };
        using State = std::array<bool, 3>;
        std::optional<std::array<Value, 5>> parsedValues;
        const auto parseComplete = [&range](LonghandParser parser, detail::TokenRange tokens) -> std::optional<Value> {
            ValueRange candidate {range.stream, tokens};
            auto value = parser(candidate);
            return value && !detail::nextToken(candidate) ? value : std::nullopt;
        };
        const auto parseSliceGroup = [&range, &parseComplete](std::size_t start, std::size_t end) -> std::optional<std::array<Value, 3>> {
            const detail::TokenRange tokens {start, end};
            const auto parts = detail::splitComponents(range.stream, tokens, true);
            std::array<std::size_t, 2> slashes {};
            std::size_t slashCount = 0;
            for (std::size_t index = 0; index < parts.size(); ++index) {
                if (!detail::isDelimiter(ValueRange {range.stream, tokens}, parts[index].begin, '/'))
                    continue;
                if (slashCount == slashes.size())
                    return std::nullopt;
                slashes[slashCount++] = index;
            }
            if (parts.empty() || (slashCount == 1 && slashes[0] == parts.size() - 1) || (slashCount == 2 && slashes[1] == parts.size() - 1))
                return std::nullopt;

            const std::size_t sliceEnd = slashCount ? parts[slashes[0]].begin : tokens.end;
            const auto sliceValue = parseComplete(kShorthandPropertyParser<slice>, {tokens.begin, sliceEnd});
            if (!sliceValue)
                return std::nullopt;
            std::array<Value, 3> result {*sliceValue, shorthandPropertyInitialValue<width>(), shorthandPropertyInitialValue<outset>()};
            if (!slashCount)
                return result;

            const std::size_t widthStart = parts[slashes[0]].end;
            const std::size_t widthEnd = slashCount == 2 ? parts[slashes[1]].begin : tokens.end;
            ValueRange widthRange {range.stream, {widthStart, widthEnd}};
            if (detail::nextToken(widthRange)) {
                const auto widthValue = parseComplete(kShorthandPropertyParser<width>, {widthStart, widthEnd});
                if (!widthValue)
                    return std::nullopt;
                result[1] = *widthValue;
            } else if (slashCount == 1)
                return std::nullopt;

            if (slashCount == 2) {
                const std::size_t outsetStart = parts[slashes[1]].end;
                ValueRange outsetRange {range.stream, {outsetStart, tokens.end}};
                if (!detail::nextToken(outsetRange))
                    return std::nullopt;
                const auto outsetValue = parseComplete(kShorthandPropertyParser<outset>, {outsetStart, tokens.end});
                if (!outsetValue)
                    return std::nullopt;
                result[2] = *outsetValue;
            }
            return result;
        };

        const auto parse = [&](auto&& self, std::size_t position, State used) -> bool {
            if (position == components.size()) {
                if (!used[1])
                    return false;
                parsedValues = values;
                return true;
            }

            if (!used[0]) {
                if (auto value = parseComplete(kShorthandPropertyParser<source>, components[position])) {
                    values[0] = std::move(*value);
                    used[0] = true;
                    if (self(self, position + 1, used))
                        return true;
                    used[0] = false;
                    values[0] = shorthandPropertyInitialValue<source>();
                }
            }
            if (!used[2]) {
                for (std::size_t count = 1; count <= 2 && position + count <= components.size(); ++count) {
                    const auto value =
                        parseComplete(kShorthandPropertyParser<repeat>, {components[position].begin, components[position + count - 1].end});
                    if (!value)
                        continue;
                    values[4] = *value;
                    used[2] = true;
                    if (self(self, position + count, used))
                        return true;
                    used[2] = false;
                    values[4] = shorthandPropertyInitialValue<repeat>();
                }
            }
            if (!used[1]) {
                for (std::size_t end = position + 1; end <= components.size(); ++end) {
                    const auto parsed = parseSliceGroup(components[position].begin, components[end - 1].end);
                    if (!parsed)
                        continue;
                    values[1] = (*parsed)[0];
                    values[2] = (*parsed)[1];
                    values[3] = (*parsed)[2];
                    used[1] = true;
                    if (self(self, end, used))
                        return true;
                    used[1] = false;
                    values[1] = shorthandPropertyInitialValue<slice>();
                    values[2] = shorthandPropertyInitialValue<width>();
                    values[3] = shorthandPropertyInitialValue<outset>();
                }
            }
            return false;
        };

        if (!parse(parse, 0, {}))
            return std::nullopt;
        range.range.begin = range.range.end;
        return std::array {
            ParsedLonghand {source, std::move((*parsedValues)[0])},
            ParsedLonghand {slice, std::move((*parsedValues)[1])},
            ParsedLonghand {width, std::move((*parsedValues)[2])},
            ParsedLonghand {outset, std::move((*parsedValues)[3])},
            ParsedLonghand {repeat, std::move((*parsedValues)[4])},
        };
    }
};
} // namespace Core::CSS
