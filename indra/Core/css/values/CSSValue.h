/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <iterator>
#include <limits>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>
#include "CSSColor.h"
#include "CSSKeywords.h"
#include "CSSTokenStream.h"
#include "Color.h"

namespace Core::CSS {
struct String {
    std::string value;

    friend bool operator==(const String&, const String&) = default;
};

struct LightDarkColor;

class Color {
public:
    Color() = default;
    Color(Core::Color value)
        : mValue(value) {}
    Color(Keyword value)
        : mValue(value) {}
    Color(LightDarkColor value);

    const Core::Color* solidColor() const { return std::get_if<Core::Color>(&mValue); }
    const Keyword* keyword() const { return std::get_if<Keyword>(&mValue); }
    const LightDarkColor* lightDark() const;

    friend bool operator==(const Color&, const Color&);

private:
    std::variant<Core::Color, Keyword, std::shared_ptr<const LightDarkColor>> mValue;
};

struct LightDarkColor {
    Color light;
    Color dark;

    friend bool operator==(const LightDarkColor&, const LightDarkColor&) = default;
};

inline Color::Color(LightDarkColor value)
    : mValue(std::make_shared<const LightDarkColor>(std::move(value))) {}

inline const LightDarkColor* Color::lightDark() const {
    const auto* value = std::get_if<std::shared_ptr<const LightDarkColor>>(&mValue);
    return value ? value->get() : nullptr;
}

inline bool operator==(const Color& left, const Color& right) {
    if (left.mValue.index() != right.mValue.index())
        return false;
    if (const auto* solid = left.solidColor())
        return *solid == *right.solidColor();
    if (const auto* keyword = left.keyword())
        return *keyword == *right.keyword();
    const LightDarkColor* leftLightDark = left.lightDark();
    const LightDarkColor* rightLightDark = right.lightDark();
    return leftLightDark && rightLightDark && *leftLightDark == *rightLightDark;
}

enum class LengthUnit {
    Px,
    Em,
    Rem,
    Ch,
    Ex,
    Cap,
    Ic,
    Lh,
    Rlh,
    Vw,
    Vh,
    Vmin,
    Vmax,
    Vi,
    Vb,
    Svw,
    Svh,
    Svmin,
    Svmax,
    Svi,
    Svb,
    Lvw,
    Lvh,
    Lvmin,
    Lvmax,
    Lvi,
    Lvb,
    Dvw,
    Dvh,
    Dvmin,
    Dvmax,
    Dvi,
    Dvb,
    Cm,
    Mm,
    Q,
    In,
    Pt,
    Pc
};

struct Length {
    float value = 0.f;
    LengthUnit unit = LengthUnit::Px;

    constexpr Length() = default;
    constexpr Length(float value, LengthUnit unit)
        : value(value)
        , unit(unit) {}

    constexpr bool operator==(const Length&) const = default;
};

constexpr Length Px(float value = 0.f) { return {value, LengthUnit::Px}; }
constexpr Length Em(float value = 0.f) { return {value, LengthUnit::Em}; }
constexpr Length Rem(float value = 0.f) { return {value, LengthUnit::Rem}; }
constexpr Length Ch(float value = 0.f) { return {value, LengthUnit::Ch}; }
constexpr Length Ex(float value = 0.f) { return {value, LengthUnit::Ex}; }
constexpr Length Cap(float value = 0.f) { return {value, LengthUnit::Cap}; }
constexpr Length Ic(float value = 0.f) { return {value, LengthUnit::Ic}; }
constexpr Length Lh(float value = 0.f) { return {value, LengthUnit::Lh}; }
constexpr Length Rlh(float value = 0.f) { return {value, LengthUnit::Rlh}; }
constexpr Length Vw(float value = 0.f) { return {value, LengthUnit::Vw}; }
constexpr Length Vh(float value = 0.f) { return {value, LengthUnit::Vh}; }
constexpr Length Vmin(float value = 0.f) { return {value, LengthUnit::Vmin}; }
constexpr Length Vmax(float value = 0.f) { return {value, LengthUnit::Vmax}; }
constexpr Length Vi(float value = 0.f) { return {value, LengthUnit::Vi}; }
constexpr Length Vb(float value = 0.f) { return {value, LengthUnit::Vb}; }
constexpr Length Svw(float value = 0.f) { return {value, LengthUnit::Svw}; }
constexpr Length Svh(float value = 0.f) { return {value, LengthUnit::Svh}; }
constexpr Length Svmin(float value = 0.f) { return {value, LengthUnit::Svmin}; }
constexpr Length Svmax(float value = 0.f) { return {value, LengthUnit::Svmax}; }
constexpr Length Svi(float value = 0.f) { return {value, LengthUnit::Svi}; }
constexpr Length Svb(float value = 0.f) { return {value, LengthUnit::Svb}; }
constexpr Length Lvw(float value = 0.f) { return {value, LengthUnit::Lvw}; }
constexpr Length Lvh(float value = 0.f) { return {value, LengthUnit::Lvh}; }
constexpr Length Lvmin(float value = 0.f) { return {value, LengthUnit::Lvmin}; }
constexpr Length Lvmax(float value = 0.f) { return {value, LengthUnit::Lvmax}; }
constexpr Length Lvi(float value = 0.f) { return {value, LengthUnit::Lvi}; }
constexpr Length Lvb(float value = 0.f) { return {value, LengthUnit::Lvb}; }
constexpr Length Dvw(float value = 0.f) { return {value, LengthUnit::Dvw}; }
constexpr Length Dvh(float value = 0.f) { return {value, LengthUnit::Dvh}; }
constexpr Length Dvmin(float value = 0.f) { return {value, LengthUnit::Dvmin}; }
constexpr Length Dvmax(float value = 0.f) { return {value, LengthUnit::Dvmax}; }
constexpr Length Dvi(float value = 0.f) { return {value, LengthUnit::Dvi}; }
constexpr Length Dvb(float value = 0.f) { return {value, LengthUnit::Dvb}; }
constexpr Length Cm(float value = 0.f) { return {value, LengthUnit::Cm}; }
constexpr Length Mm(float value = 0.f) { return {value, LengthUnit::Mm}; }
constexpr Length Q(float value = 0.f) { return {value, LengthUnit::Q}; }
constexpr Length Inches(float value = 0.f) { return {value, LengthUnit::In}; }
constexpr Length Pt(float value = 0.f) { return {value, LengthUnit::Pt}; }
constexpr Length Pc(float value = 0.f) { return {value, LengthUnit::Pc}; }

struct Percentage {
    float value = 0.f;

    constexpr Percentage() = default;
    constexpr explicit Percentage(float value)
        : value(value) {}

    constexpr bool operator==(const Percentage&) const = default;
};

struct Number {
    float value = 0.f;

    constexpr Number() = default;
    constexpr explicit Number(float value)
        : value(value) {}

    constexpr bool operator==(const Number&) const = default;
};

using LengthPercentage = std::variant<Length, Percentage>;

struct Function;
struct Block;
struct List;

using ValueComponent = std::variant<Keyword, String, Number, Length, Percentage, Color, LengthPercentage, std::shared_ptr<const Function>,
    std::shared_ptr<const Block>, std::shared_ptr<const List>>;

struct List {
    std::vector<ValueComponent> values;
};

enum class BlockType {
    Parentheses,
    Brackets,
    Braces
};

struct Function {
    std::string name;
    List arguments;
};

struct Block {
    BlockType type;
    List values;
};

using Value = std::variant<Keyword, String, Number, Length, Percentage, Color, LengthPercentage, List, std::shared_ptr<const Function>,
    std::shared_ptr<const Block>>;
using ValueRange = detail::ValueRange;

template<typename T, typename Variant> struct VariantContains : std::false_type {};

template<typename T, typename... Values>
struct VariantContains<T, std::variant<Values...>> : std::bool_constant<(std::is_same_v<T, Values> || ...)> {};

struct Range {
    static constexpr float kInfinity = std::numeric_limits<float>::infinity();

    float minimum;
    float maximum;
};

inline constexpr Range kAnyRange {-Range::kInfinity, Range::kInfinity};
inline constexpr Range kNonnegative {0.f, Range::kInfinity};

namespace detail {
inline std::optional<std::size_t> nextToken(const ValueRange& value) {
    std::size_t token = value.range.begin;
    while (token < value.range.end && isTrivia(value.stream.tokens()[token].kind))
        ++token;
    if (token == value.range.end)
        return std::nullopt;
    return token;
}

inline bool inRange(float value, Range bounds) { return value >= bounds.minimum && value <= bounds.maximum; }

inline void consume(ValueRange& range, TokenRange component) {
    range.range.begin = component.end;
    while (range.range.begin < range.range.end && isTrivia(range.stream.tokens()[range.range.begin].kind))
        ++range.range.begin;
}

inline void consume(ValueRange& range, std::size_t token) { consume(range, {token, token + 1}); }

inline std::optional<Color> parseColor(ValueRange& range);

inline std::optional<Color> consumeColorArm(const TokenStream& stream, TokenRange component) {
    ValueRange range {stream, component};
    const auto color = parseColor(range);
    if (!color || nextToken(range))
        return std::nullopt;
    return color;
}

inline bool isDelimiter(const ValueRange& range, std::size_t token, char delimiter) {
    const auto kind = range.stream.tokens()[token].kind;
    if (delimiter == ',' && kind == TokenKind::Comma)
        return true;
    if (delimiter == ':' && kind == TokenKind::Colon)
        return true;
    if (delimiter == ';' && kind == TokenKind::Semicolon)
        return true;
    return kind == TokenKind::Delim && range.stream.text(token).size() == 1 && range.stream.text(token)[0] == delimiter;
}

template<typename Item> void appendListValue(List& list, Item&& value) {
    using Type = std::remove_cvref_t<Item>;
    if constexpr (std::is_same_v<Type, Value> || std::is_same_v<Type, ValueComponent>)
        std::visit(
            [&list](auto&& item) {
                appendListValue(list, std::forward<decltype(item)>(item));
            },
            std::forward<Item>(value));
    else if constexpr (std::is_same_v<Type, List>)
        list.values.insert(list.values.end(), std::make_move_iterator(value.values.begin()), std::make_move_iterator(value.values.end()));
    else
        list.values.emplace_back(std::forward<Item>(value));
}

inline std::optional<Color> parseColor(ValueRange& range) {
    const std::optional<std::size_t> tokenIndex = nextToken(range);
    if (!tokenIndex)
        return std::nullopt;

    const TokenRange tokenRange {*tokenIndex, *tokenIndex + 1};
    if (range.stream.tokens()[*tokenIndex].kind == TokenKind::Ident) {
        const auto keyword = findKeyword(normalizeKeyword(range.stream, tokenRange));
        if (keyword && (*keyword == KeywordCurrentColor || isSystemColorKeyword(*keyword))) {
            consume(range, *tokenIndex);
            return Color {Keyword {*keyword}};
        }
    }

    const std::size_t close = range.stream.tokens()[*tokenIndex].matching;
    const TokenRange functionRange {*tokenIndex, close == kNoMatchingToken ? range.range.end : close + 1};
    const auto function = parseFunction(range.stream, functionRange);
    if (function && function->name == "light-dark") {
        const std::vector<TokenRange> choices = splitOnDelimiter(range.stream, function->body, ',');
        if (choices.size() != 2)
            return std::nullopt;
        const auto light = consumeColorArm(range.stream, choices[0]);
        const auto dark = consumeColorArm(range.stream, choices[1]);
        if (!light || !dark)
            return std::nullopt;
        consume(range, functionRange);
        return Color {LightDarkColor {*light, *dark}};
    }

    const auto color = CSS::consumeColor(range.stream, function ? functionRange : tokenRange);
    if (!color)
        return std::nullopt;
    consume(range, function ? functionRange : tokenRange);
    return Color {*color};
}

inline std::optional<Length> makeLength(std::string_view unit, float value) {
    if (unit == "px")
        return Px(value);
    if (unit == "em")
        return Em(value);
    if (unit == "rem")
        return Rem(value);
    if (unit == "ch")
        return Ch(value);
    if (unit == "ex")
        return Ex(value);
    if (unit == "cap")
        return Cap(value);
    if (unit == "ic")
        return Ic(value);
    if (unit == "lh")
        return Lh(value);
    if (unit == "rlh")
        return Rlh(value);
    if (unit == "vw")
        return Vw(value);
    if (unit == "vh")
        return Vh(value);
    if (unit == "vmin")
        return Vmin(value);
    if (unit == "vmax")
        return Vmax(value);
    if (unit == "vi")
        return Vi(value);
    if (unit == "vb")
        return Vb(value);
    if (unit == "svw")
        return Svw(value);
    if (unit == "svh")
        return Svh(value);
    if (unit == "svmin")
        return Svmin(value);
    if (unit == "svmax")
        return Svmax(value);
    if (unit == "svi")
        return Svi(value);
    if (unit == "svb")
        return Svb(value);
    if (unit == "lvw")
        return Lvw(value);
    if (unit == "lvh")
        return Lvh(value);
    if (unit == "lvmin")
        return Lvmin(value);
    if (unit == "lvmax")
        return Lvmax(value);
    if (unit == "lvi")
        return Lvi(value);
    if (unit == "lvb")
        return Lvb(value);
    if (unit == "dvw")
        return Dvw(value);
    if (unit == "dvh")
        return Dvh(value);
    if (unit == "dvmin")
        return Dvmin(value);
    if (unit == "dvmax")
        return Dvmax(value);
    if (unit == "dvi")
        return Dvi(value);
    if (unit == "dvb")
        return Dvb(value);
    if (unit == "cm")
        return Cm(value);
    if (unit == "mm")
        return Mm(value);
    if (unit == "q")
        return Q(value);
    if (unit == "in")
        return Inches(value);
    if (unit == "pt")
        return Pt(value);
    if (unit == "pc")
        return Pc(value);
    return std::nullopt;
}
} // namespace detail

inline std::optional<Value> consumeColor(ValueRange& range) {
    auto color = detail::parseColor(range);
    if (!color)
        return std::nullopt;
    return Value {std::move(*color)};
}

template<typename... allowed> std::optional<Keyword> consumeKeyword(ValueRange& range) {
    const std::optional<std::size_t> tokenIndex = detail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::TokenKind::Ident)
        return std::nullopt;

    const auto keyword = findKeyword(detail::normalizeKeyword(range.stream, {*tokenIndex, *tokenIndex + 1}));
    if (!keyword || !((*keyword == allowed::value) || ...))
        return std::nullopt;
    detail::consume(range, *tokenIndex);
    return Keyword {*keyword};
}

template<Range bounds> std::optional<Number> consumeNumber(ValueRange& range) {
    const std::optional<std::size_t> tokenIndex = detail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::TokenKind::Number)
        return std::nullopt;
    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !detail::inRange(*value, bounds))
        return std::nullopt;
    detail::consume(range, *tokenIndex);
    return Number {*value};
}

template<Range bounds> std::optional<Number> consumeInteger(ValueRange& range) {
    const auto tokenIndex = detail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::TokenKind::Number)
        return std::nullopt;

    std::string_view token = range.stream.text(*tokenIndex);
    if (token.starts_with('+') || token.starts_with('-'))
        token.remove_prefix(1);
    if (token.empty())
        return std::nullopt;
    for (const char digit : token)
        if (digit < '0' || digit > '9')
            return std::nullopt;

    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !detail::inRange(*value, bounds))
        return std::nullopt;
    detail::consume(range, *tokenIndex);
    return Number {*value};
}

template<Range bounds> std::optional<Percentage> consumePercentage(ValueRange& range) {
    const std::optional<std::size_t> tokenIndex = detail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::TokenKind::Percentage)
        return std::nullopt;
    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !detail::inRange(*value, bounds))
        return std::nullopt;
    detail::consume(range, *tokenIndex);
    return Percentage {*value};
}

template<Range bounds> std::optional<Length> consumeLength(ValueRange& range) {
    const std::optional<std::size_t> tokenIndex = detail::nextToken(range);
    if (!tokenIndex)
        return std::nullopt;

    const detail::Token& token = range.stream.tokens()[*tokenIndex];
    if (token.kind == detail::TokenKind::Dimension) {
        const auto dimension = detail::parseDimension(range.stream, {*tokenIndex, *tokenIndex + 1});
        if (!dimension)
            return std::nullopt;
        const std::optional<float> value = token.numericValue;
        if (!value || !detail::inRange(*value, bounds))
            return std::nullopt;
        const std::optional<Length> length = detail::makeLength(dimension->unit, *value);
        if (!length)
            return std::nullopt;
        detail::consume(range, *tokenIndex);
        return length;
    }

    if (token.kind != detail::TokenKind::Number)
        return std::nullopt;
    const std::optional<float> value = token.numericValue;
    if (!value || *value != 0.f || !detail::inRange(*value, bounds))
        return std::nullopt;
    detail::consume(range, *tokenIndex);
    return Px();
}

template<Range bounds> std::optional<LengthPercentage> consumeLengthPercentage(ValueRange& range) {
    if (const auto percentage = consumePercentage<bounds>(range))
        return LengthPercentage {*percentage};
    if (const auto length = consumeLength<bounds>(range))
        return LengthPercentage {*length};
    return std::nullopt;
}
} // namespace Core::CSS
