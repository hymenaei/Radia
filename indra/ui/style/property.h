/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cmath>
#include <cstdint>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>
#include "css/rules.h"
#include "CSSValue.h"
#include "style/computedstyle.h"

namespace radia::ui {
template<typename T> struct ValueOrKeywordTraits {
    static constexpr bool isValueOrKeyword = false;
};

template<typename Value, typename Keyword> struct ValueOrKeywordTraits<ValueOrKeyword<Value, Keyword>> {
    static constexpr bool isValueOrKeyword = true;
    using value_type = Value;
    using keyword_type = Keyword;
};

template<typename T> constexpr std::optional<T> fromCSSKeyword(CSSKeyword) {
    return std::nullopt;
}

namespace detail {
template<typename T> struct IsStyleLength : std::false_type {};
template<typename Constraint> struct IsStyleLength<Style::Length<Constraint, float>> : std::true_type {};
} // namespace detail

template<typename T> std::optional<T> toStyle(const StyleBuilderContext&, const CSSValue&);
template<typename T> std::optional<T> toStyle(const StyleBuilderState&, const CSSValue&);
template<> std::optional<float> toStyle<float>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<Order> toStyle<Order>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<Dimension> toStyle<Dimension>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<OptionalDimension> toStyle<OptionalDimension>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<InsetEdge> toStyle<InsetEdge>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<MarginEdge> toStyle<MarginEdge>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<PaddingEdge> toStyle<PaddingEdge>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<GapGutter> toStyle<GapGutter>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<ScrollbarGutter> toStyle<ScrollbarGutter>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<ScrollbarColor> toStyle<ScrollbarColor>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<FontWeight> toStyle<FontWeight>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<FontFamilies> toStyle<FontFamilies>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<FontWidth> toStyle<FontWidth>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<LineWidth> toStyle<LineWidth>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<CornerRadius> toStyle<CornerRadius>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<Opacity> toStyle<Opacity>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<StyleColor> toStyle<StyleColor>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<BoxShadows> toStyle<BoxShadows>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<LineHeight> toStyle<LineHeight>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<SelfAlignmentData> toStyle<SelfAlignmentData>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<ContentAlignmentData> toStyle<ContentAlignmentData>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<JustifyItems> toStyle<JustifyItems>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<FlexWrap> toStyle<FlexWrap>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<VerticalAlignValue> toStyle<VerticalAlignValue>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<Translate> toStyle<Translate>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<StyleImage> toStyle<StyleImage>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<BorderImageSlice> toStyle<BorderImageSlice>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<BorderImageWidth> toStyle<BorderImageWidth>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<BorderImageOutset> toStyle<BorderImageOutset>(const StyleBuilderContext&, const CSSValue&);
template<> std::optional<BorderImageRepeat> toStyle<BorderImageRepeat>(const StyleBuilderContext&, const CSSValue&);

template<typename T> std::optional<T> toStyle(const StyleBuilderContext& context, const CSSValue& value) {
    if constexpr (ValueOrKeywordTraits<T>::isValueOrKeyword) {
        using Traits = ValueOrKeywordTraits<T>;
        using Keyword = typename Traits::keyword_type;
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == Keyword::value) return T{Keyword{}};
        if (auto converted = toStyle<typename Traits::value_type>(context, value)) return T{std::move(*converted)};
        return std::nullopt;
    }

    if constexpr (std::is_same_v<T, float>)
        if (const auto* number = std::get_if<CSS::Number>(&value)) return number->value;

    if constexpr (detail::IsStyleLength<T>::value) {
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::Normal) return T{};

        const auto toPixels = [&context](CSS::Length length) -> std::optional<float> {
            if (!std::isfinite(length.value)) return std::nullopt;
            auto pixels = Style::absolutePixels(length);
            if (!pixels && length.unit == CSS::LengthUnit::Em) pixels = context.style.fontSize() * length.value;
            if (!pixels && length.unit == CSS::LengthUnit::Rem) pixels = context.rootStyle.fontSize() * length.value;
            return pixels && std::isfinite(*pixels) ? pixels : std::nullopt;
        };

        if (const auto* percentage = std::get_if<CSS::Percentage>(&value))
            return std::isfinite(percentage->value) ? std::optional<T>(T{0.f, percentage->value / 100.f}) : std::nullopt;

        const CSS::Length* specifiedLength = std::get_if<CSS::Length>(&value);
        if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
            if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage))
                return std::isfinite(percentage->value) ? std::optional<T>(T{0.f, percentage->value / 100.f}) : std::nullopt;
            specifiedLength = std::get_if<CSS::Length>(lengthPercentage);
        }
        if (!specifiedLength) return std::nullopt;
        const auto pixels = toPixels(*specifiedLength);
        return pixels ? std::optional<T>(T{*pixels}) : std::nullopt;
    }

    if constexpr (std::is_constructible_v<T, CSS::Number>)
        if (const auto* number = std::get_if<CSS::Number>(&value)) return T{*number};

    if constexpr (std::is_enum_v<T>)
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) return fromCSSKeyword<T>(keyword->id);

    if constexpr (VariantContains<T, CSSValue>::value)
        if (const auto* result = std::get_if<T>(&value)) return *result;
    return std::nullopt;
}

template<typename T> std::optional<T> toStyle(const StyleBuilderState& builderState, const CSSValue& value) {
    return toStyle<T>(builderState.context, value);
}

template<typename T> std::optional<T> toStyle(const StyleBuilderState& builderState, const StyleValue& value) {
    if constexpr (VariantContains<T, StyleValue>::value)
        if (const auto* storedValue = std::get_if<T>(&value)) return *storedValue;
    if (const auto* cssValue = std::get_if<CSSValue>(&value)) return toStyle<T>(builderState, *cssValue);
    return std::nullopt;
}

namespace detail {
struct StyleCompileContext;
using StyleCompileResult = std::optional<std::vector<StyleDeclaration>>;
using StyleCompileFunction = StyleCompileResult (*)(StyleCompileContext&);
using StyleSetFunction = void (*)(ComputedStyle&, const StyleValue&);
using StyleInitialFunction = void (*)(ComputedStyle&);
using StyleSpecifyFunction = void (*)(ComputedStyle&);
using StyleInheritFunction = void (*)(ComputedStyle&, const ComputedStyle&);

enum class StylePropertyImpact : std::uint8_t { Layout = 1 << 0, Paint = 1 << 1, HitTest = 1 << 2, Descendant = 1 << 3 };

inline constexpr StylePropertyImpact operator|(StylePropertyImpact left, StylePropertyImpact right) {
    return static_cast<StylePropertyImpact>(static_cast<std::uint8_t>(left) | static_cast<std::uint8_t>(right));
}

inline constexpr bool hasImpact(StylePropertyImpact value, StylePropertyImpact flag) {
    return (static_cast<std::uint8_t>(value) & static_cast<std::uint8_t>(flag)) != 0;
}

struct StylePropertyDefinition {
    std::string_view name;
    StyleCompileFunction compile = nullptr;
    StyleSetFunction set = nullptr;
    StyleInitialFunction initial = nullptr;
    StyleSpecifyFunction specify = nullptr;
    StyleInheritFunction inherit = nullptr;
    StylePropertyImpact impact = StylePropertyImpact::Layout;
    bool userAgentOnly = false;

    bool isPaintOnly() const { return hasImpact(impact, StylePropertyImpact::Paint) && !hasImpact(impact, StylePropertyImpact::Layout); }
    bool affectsHitTesting() const { return hasImpact(impact, StylePropertyImpact::HitTest); }
    bool inherited = false;
};

const StylePropertyDefinition* findLegacyProperty(std::string_view name);
const StylePropertyDefinition* legacyPropertyBegin();
const StylePropertyDefinition* legacyPropertyEnd();
bool isPropertyInherited(std::string_view name);
bool isPropertyPaintOnly(std::string_view name);
bool propertyAffectsHitTesting(std::string_view name);
bool propertyPropagatesToDescendants(std::string_view name);
void applyStyleDeclaration(StyleBuilderState& builderState, const StyleDeclaration& declaration);
void applyInvalidStyleDeclaration(StyleBuilderState& builderState, std::string_view property);
} // namespace detail
} // namespace radia::ui
