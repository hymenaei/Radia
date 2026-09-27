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
#include "css/color.h"
#include "css/syntax.h"
#include "CSSKeywords.h"
#include "platform/graphics/Color.h"

namespace radia::ui {
struct StyleBuilderContext;

namespace CSS {
struct String {
    std::string value;

    friend bool operator==(const String&, const String&) = default;
};

struct LightDarkColor;

class Color {
public:
    Color() = default;
    Color(radia::ui::Color value) : mValue(value) {}
    Color(Keyword value) : mValue(value) {}
    Color(LightDarkColor value);

    const radia::ui::Color* solidColor() const { return std::get_if<radia::ui::Color>(&mValue); }
    const Keyword* keyword() const { return std::get_if<Keyword>(&mValue); }
    const LightDarkColor* lightDark() const;

    friend bool operator==(const Color&, const Color&);

private:
    std::variant<radia::ui::Color, Keyword, std::shared_ptr<const LightDarkColor>> mValue;
};

struct LightDarkColor {
    Color light;
    Color dark;

    friend bool operator==(const LightDarkColor&, const LightDarkColor&) = default;
};

inline Color::Color(LightDarkColor value) : mValue(std::make_shared<const LightDarkColor>(std::move(value))) {}

inline const LightDarkColor* Color::lightDark() const {
    const auto* value = std::get_if<std::shared_ptr<const LightDarkColor>>(&mValue);
    return value ? value->get() : nullptr;
}

inline bool operator==(const Color& left, const Color& right) {
    if (left.mValue.index() != right.mValue.index()) return false;
    if (const auto* solid = left.solidColor()) return *solid == *right.solidColor();
    if (const auto* keyword = left.keyword()) return *keyword == *right.keyword();
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
    constexpr Length(float value, LengthUnit unit) : value(value), unit(unit) {}

    constexpr bool operator==(const Length&) const = default;
};

constexpr Length Px(float value = 0.f) {
    return {value, LengthUnit::Px};
}
constexpr Length Em(float value = 0.f) {
    return {value, LengthUnit::Em};
}
constexpr Length Rem(float value = 0.f) {
    return {value, LengthUnit::Rem};
}
constexpr Length Ch(float value = 0.f) {
    return {value, LengthUnit::Ch};
}
constexpr Length Ex(float value = 0.f) {
    return {value, LengthUnit::Ex};
}
constexpr Length Cap(float value = 0.f) {
    return {value, LengthUnit::Cap};
}
constexpr Length Ic(float value = 0.f) {
    return {value, LengthUnit::Ic};
}
constexpr Length Lh(float value = 0.f) {
    return {value, LengthUnit::Lh};
}
constexpr Length Rlh(float value = 0.f) {
    return {value, LengthUnit::Rlh};
}
constexpr Length Vw(float value = 0.f) {
    return {value, LengthUnit::Vw};
}
constexpr Length Vh(float value = 0.f) {
    return {value, LengthUnit::Vh};
}
constexpr Length Vmin(float value = 0.f) {
    return {value, LengthUnit::Vmin};
}
constexpr Length Vmax(float value = 0.f) {
    return {value, LengthUnit::Vmax};
}
constexpr Length Vi(float value = 0.f) {
    return {value, LengthUnit::Vi};
}
constexpr Length Vb(float value = 0.f) {
    return {value, LengthUnit::Vb};
}
constexpr Length Svw(float value = 0.f) {
    return {value, LengthUnit::Svw};
}
constexpr Length Svh(float value = 0.f) {
    return {value, LengthUnit::Svh};
}
constexpr Length Svmin(float value = 0.f) {
    return {value, LengthUnit::Svmin};
}
constexpr Length Svmax(float value = 0.f) {
    return {value, LengthUnit::Svmax};
}
constexpr Length Svi(float value = 0.f) {
    return {value, LengthUnit::Svi};
}
constexpr Length Svb(float value = 0.f) {
    return {value, LengthUnit::Svb};
}
constexpr Length Lvw(float value = 0.f) {
    return {value, LengthUnit::Lvw};
}
constexpr Length Lvh(float value = 0.f) {
    return {value, LengthUnit::Lvh};
}
constexpr Length Lvmin(float value = 0.f) {
    return {value, LengthUnit::Lvmin};
}
constexpr Length Lvmax(float value = 0.f) {
    return {value, LengthUnit::Lvmax};
}
constexpr Length Lvi(float value = 0.f) {
    return {value, LengthUnit::Lvi};
}
constexpr Length Lvb(float value = 0.f) {
    return {value, LengthUnit::Lvb};
}
constexpr Length Dvw(float value = 0.f) {
    return {value, LengthUnit::Dvw};
}
constexpr Length Dvh(float value = 0.f) {
    return {value, LengthUnit::Dvh};
}
constexpr Length Dvmin(float value = 0.f) {
    return {value, LengthUnit::Dvmin};
}
constexpr Length Dvmax(float value = 0.f) {
    return {value, LengthUnit::Dvmax};
}
constexpr Length Dvi(float value = 0.f) {
    return {value, LengthUnit::Dvi};
}
constexpr Length Dvb(float value = 0.f) {
    return {value, LengthUnit::Dvb};
}
constexpr Length Cm(float value = 0.f) {
    return {value, LengthUnit::Cm};
}
constexpr Length Mm(float value = 0.f) {
    return {value, LengthUnit::Mm};
}
constexpr Length Q(float value = 0.f) {
    return {value, LengthUnit::Q};
}
constexpr Length Inches(float value = 0.f) {
    return {value, LengthUnit::In};
}
constexpr Length Pt(float value = 0.f) {
    return {value, LengthUnit::Pt};
}
constexpr Length Pc(float value = 0.f) {
    return {value, LengthUnit::Pc};
}

struct Percentage {
    float value = 0.f;

    constexpr Percentage() = default;
    constexpr explicit Percentage(float value) : value(value) {}

    constexpr bool operator==(const Percentage&) const = default;
};

struct Number {
    float value = 0.f;

    constexpr Number() = default;
    constexpr explicit Number(float value) : value(value) {}

    constexpr bool operator==(const Number&) const = default;
};

using LengthPercentage = std::variant<Length, Percentage>;

struct Function;
struct Block;
struct List;
} // namespace CSS

using CSSValueComponent = std::variant<CSS::Keyword, CSS::String, CSS::Number, CSS::Length, CSS::Percentage, CSS::Color, CSS::LengthPercentage,
                                       std::shared_ptr<const CSS::Function>, std::shared_ptr<const CSS::Block>, std::shared_ptr<const CSS::List>>;

namespace CSS {
struct List {
    std::vector<CSSValueComponent> values;
};

enum class BlockType { Parentheses, Brackets, Braces };

struct Function {
    std::string name;
    List arguments;
};

struct Block {
    BlockType type;
    List values;
};
} // namespace CSS

using CSSValue = std::variant<CSS::Keyword, CSS::String, CSS::Number, CSS::Length, CSS::Percentage, CSS::Color, CSS::LengthPercentage, CSS::List,
                              std::shared_ptr<const CSS::Function>, std::shared_ptr<const CSS::Block>>;
using CSSValueRange = detail::CSSValueRange;

template<typename T, typename Variant> struct VariantContains : std::false_type {};

template<typename T, typename... Values> struct VariantContains<T, std::variant<Values...>> : std::bool_constant<(std::is_same_v<T, Values> || ...)> {
};

struct Range {
    static constexpr float infinity = std::numeric_limits<float>::infinity();

    float minimum;
    float maximum;
};

inline constexpr Range AnyRange{-Range::infinity, Range::infinity};
inline constexpr Range Nonnegative{0.f, Range::infinity};

namespace CSSValueDetail {
inline std::optional<std::size_t> nextToken(const CSSValueRange& value) {
    std::size_t token = value.range.begin;
    while (token < value.range.end && detail::isCSSTrivia(value.stream.tokens()[token].kind)) ++token;
    if (token == value.range.end) return std::nullopt;
    return token;
}

inline bool inRange(float value, Range bounds) {
    return value >= bounds.minimum && value <= bounds.maximum;
}

inline void consume(CSSValueRange& range, detail::CSSTokenRange component) {
    range.range.begin = component.end;
    while (range.range.begin < range.range.end && detail::isCSSTrivia(range.stream.tokens()[range.range.begin].kind)) ++range.range.begin;
}

inline void consume(CSSValueRange& range, std::size_t token) {
    consume(range, {token, token + 1});
}

inline std::optional<CSS::Color> consumeColor(CSSValueRange& range);

inline std::optional<CSS::Color> consumeColorArm(const detail::CSSTokenStream& stream, detail::CSSTokenRange component) {
    CSSValueRange range{stream, component};
    const auto color = consumeColor(range);
    if (!color || nextToken(range)) return std::nullopt;
    return color;
}

inline bool isDelimiter(const CSSValueRange& range, std::size_t token, char delimiter) {
    const auto kind = range.stream.tokens()[token].kind;
    if (delimiter == ',' && kind == detail::CSSTokenKind::Comma) return true;
    if (delimiter == ':' && kind == detail::CSSTokenKind::Colon) return true;
    if (delimiter == ';' && kind == detail::CSSTokenKind::Semicolon) return true;
    return kind == detail::CSSTokenKind::Delim && range.stream.text(token).size() == 1 && range.stream.text(token)[0] == delimiter;
}

template<typename Value> void appendListValue(CSS::List& list, Value&& value) {
    using Type = std::remove_cvref_t<Value>;
    if constexpr (std::is_same_v<Type, CSSValue> || std::is_same_v<Type, CSSValueComponent>)
        std::visit([&list](auto&& item) { appendListValue(list, std::forward<decltype(item)>(item)); }, std::forward<Value>(value));
    else if constexpr (std::is_same_v<Type, CSS::List>)
        list.values.insert(list.values.end(), std::make_move_iterator(value.values.begin()), std::make_move_iterator(value.values.end()));
    else list.values.emplace_back(std::forward<Value>(value));
}

inline std::optional<CSS::Color> consumeColor(CSSValueRange& range) {
    const std::optional<std::size_t> tokenIndex = nextToken(range);
    if (!tokenIndex) return std::nullopt;

    const detail::CSSTokenRange tokenRange{*tokenIndex, *tokenIndex + 1};
    if (range.stream.tokens()[*tokenIndex].kind == detail::CSSTokenKind::Ident) {
        const auto keyword = findCSSKeyword(detail::normalizeCSSKeyword(range.stream, tokenRange));
        if (keyword && (*keyword == CSSKeyword::CurrentColor || isSystemColorKeyword(*keyword))) {
            consume(range, *tokenIndex);
            return CSS::Color{CSS::Keyword{*keyword}};
        }
    }

    const std::size_t close = range.stream.tokens()[*tokenIndex].matching;
    const detail::CSSTokenRange functionRange{*tokenIndex, close == detail::kNoMatchingCSSToken ? *tokenIndex + 1 : close + 1};
    const auto function = detail::parseCSSFunction(range.stream, functionRange);
    if (function && function->name == "light-dark") {
        const std::vector<detail::CSSTokenRange> choices = detail::splitCSSOnDelimiter(range.stream, function->body, ',');
        if (choices.size() != 2) return std::nullopt;
        const auto light = consumeColorArm(range.stream, choices[0]);
        const auto dark = consumeColorArm(range.stream, choices[1]);
        if (!light || !dark) return std::nullopt;
        consume(range, functionRange);
        return CSS::Color{CSS::LightDarkColor{*light, *dark}};
    }

    const auto color = radia::ui::consumeColor(range.stream, function ? functionRange : tokenRange);
    if (!color) return std::nullopt;
    consume(range, function ? functionRange : tokenRange);
    return CSS::Color{*color};
}

inline std::optional<CSS::Length> makeLength(std::string_view unit, float value) {
    if (unit == "px") return CSS::Px(value);
    if (unit == "em") return CSS::Em(value);
    if (unit == "rem") return CSS::Rem(value);
    if (unit == "ch") return CSS::Ch(value);
    if (unit == "ex") return CSS::Ex(value);
    if (unit == "cap") return CSS::Cap(value);
    if (unit == "ic") return CSS::Ic(value);
    if (unit == "lh") return CSS::Lh(value);
    if (unit == "rlh") return CSS::Rlh(value);
    if (unit == "vw") return CSS::Vw(value);
    if (unit == "vh") return CSS::Vh(value);
    if (unit == "vmin") return CSS::Vmin(value);
    if (unit == "vmax") return CSS::Vmax(value);
    if (unit == "vi") return CSS::Vi(value);
    if (unit == "vb") return CSS::Vb(value);
    if (unit == "svw") return CSS::Svw(value);
    if (unit == "svh") return CSS::Svh(value);
    if (unit == "svmin") return CSS::Svmin(value);
    if (unit == "svmax") return CSS::Svmax(value);
    if (unit == "svi") return CSS::Svi(value);
    if (unit == "svb") return CSS::Svb(value);
    if (unit == "lvw") return CSS::Lvw(value);
    if (unit == "lvh") return CSS::Lvh(value);
    if (unit == "lvmin") return CSS::Lvmin(value);
    if (unit == "lvmax") return CSS::Lvmax(value);
    if (unit == "lvi") return CSS::Lvi(value);
    if (unit == "lvb") return CSS::Lvb(value);
    if (unit == "dvw") return CSS::Dvw(value);
    if (unit == "dvh") return CSS::Dvh(value);
    if (unit == "dvmin") return CSS::Dvmin(value);
    if (unit == "dvmax") return CSS::Dvmax(value);
    if (unit == "dvi") return CSS::Dvi(value);
    if (unit == "dvb") return CSS::Dvb(value);
    if (unit == "cm") return CSS::Cm(value);
    if (unit == "mm") return CSS::Mm(value);
    if (unit == "q") return CSS::Q(value);
    if (unit == "in") return CSS::Inches(value);
    if (unit == "pt") return CSS::Pt(value);
    if (unit == "pc") return CSS::Pc(value);
    return std::nullopt;
}
} // namespace CSSValueDetail

inline std::optional<CSSValue> consumeColor(CSSValueRange& range) {
    auto color = CSSValueDetail::consumeColor(range);
    if (!color) return std::nullopt;
    return CSSValue{std::move(*color)};
}

template<CSSKeyword... allowed> std::optional<CSS::Keyword> consumeKeyword(CSSValueRange& range) {
    const std::optional<std::size_t> tokenIndex = CSSValueDetail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::CSSTokenKind::Ident) return std::nullopt;

    const auto keyword = findCSSKeyword(detail::normalizeCSSKeyword(range.stream, {*tokenIndex, *tokenIndex + 1}));
    if (!keyword || !((*keyword == allowed) || ...)) return std::nullopt;
    CSSValueDetail::consume(range, *tokenIndex);
    return CSS::Keyword{*keyword};
}

template<Range bounds> std::optional<CSS::Number> consumeNumber(CSSValueRange& range) {
    const std::optional<std::size_t> tokenIndex = CSSValueDetail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::CSSTokenKind::Number) return std::nullopt;
    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !CSSValueDetail::inRange(*value, bounds)) return std::nullopt;
    CSSValueDetail::consume(range, *tokenIndex);
    return CSS::Number{*value};
}

template<Range bounds> std::optional<CSS::Number> consumeInteger(CSSValueRange& range) {
    const auto tokenIndex = CSSValueDetail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::CSSTokenKind::Number) return std::nullopt;

    std::string_view token = range.stream.text(*tokenIndex);
    if (token.starts_with('+') || token.starts_with('-')) token.remove_prefix(1);
    if (token.empty()) return std::nullopt;
    for (const char digit : token)
        if (digit < '0' || digit > '9') return std::nullopt;

    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !CSSValueDetail::inRange(*value, bounds)) return std::nullopt;
    CSSValueDetail::consume(range, *tokenIndex);
    return CSS::Number{*value};
}

template<Range bounds> std::optional<CSS::Percentage> consumePercentage(CSSValueRange& range) {
    const std::optional<std::size_t> tokenIndex = CSSValueDetail::nextToken(range);
    if (!tokenIndex || range.stream.tokens()[*tokenIndex].kind != detail::CSSTokenKind::Percentage) return std::nullopt;
    const std::optional<float> value = range.stream.tokens()[*tokenIndex].numericValue;
    if (!value || !CSSValueDetail::inRange(*value, bounds)) return std::nullopt;
    CSSValueDetail::consume(range, *tokenIndex);
    return CSS::Percentage{*value};
}

template<Range bounds> std::optional<CSS::Length> consumeLength(CSSValueRange& range) {
    const std::optional<std::size_t> tokenIndex = CSSValueDetail::nextToken(range);
    if (!tokenIndex) return std::nullopt;

    const detail::CSSToken& token = range.stream.tokens()[*tokenIndex];
    if (token.kind == detail::CSSTokenKind::Dimension) {
        const auto dimension = detail::parseCSSDimension(range.stream, {*tokenIndex, *tokenIndex + 1});
        if (!dimension) return std::nullopt;
        const std::optional<float> value = token.numericValue;
        if (!value || !CSSValueDetail::inRange(*value, bounds)) return std::nullopt;
        const std::optional<CSS::Length> length = CSSValueDetail::makeLength(dimension->unit, *value);
        if (!length) return std::nullopt;
        CSSValueDetail::consume(range, *tokenIndex);
        return length;
    }

    if (token.kind != detail::CSSTokenKind::Number) return std::nullopt;
    const std::optional<float> value = token.numericValue;
    if (!value || *value != 0.f || !CSSValueDetail::inRange(*value, bounds)) return std::nullopt;
    CSSValueDetail::consume(range, *tokenIndex);
    return CSS::Px();
}

template<Range bounds> std::optional<CSS::LengthPercentage> consumeLengthPercentage(CSSValueRange& range) {
    if (const auto percentage = consumePercentage<bounds>(range)) return CSS::LengthPercentage{*percentage};
    if (const auto length = consumeLength<bounds>(range)) return CSS::LengthPercentage{*length};
    return std::nullopt;
}
} // namespace radia::ui
