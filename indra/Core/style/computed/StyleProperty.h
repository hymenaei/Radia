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
#include "CSSRules.h"
#include "CSSValue.h"
#include "ComputedStyle.h"

namespace Core::Style {
template<typename T> struct ValueOrKeywordTraits {
    static constexpr bool isValueOrKeyword = false;
};

template<typename Value, typename Keyword> struct ValueOrKeywordTraits<ValueOrKeyword<Value, Keyword>> {
    static constexpr bool isValueOrKeyword = true;
    using value_type = Value;
    using keyword_type = Keyword;
};

template<typename T> constexpr std::optional<T> fromCSSKeyword(CSS::KeywordName) { return std::nullopt; }

namespace detail {
template<typename T> struct IsStyleLength : std::false_type {};
template<typename Constraint> struct IsStyleLength<LengthValue<Constraint, float>> : std::true_type {};
} // namespace detail

template<typename T> std::optional<T> toStyle(const BuilderContext&, const CSS::Value&);
template<typename T> std::optional<T> toStyle(const BuilderState&, const CSS::Value&);
template<> std::optional<float> toStyle<float>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Order> toStyle<Order>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Dimension> toStyle<Dimension>(const BuilderContext&, const CSS::Value&);
template<> std::optional<OptionalDimension> toStyle<OptionalDimension>(const BuilderContext&, const CSS::Value&);
template<> std::optional<InsetEdge> toStyle<InsetEdge>(const BuilderContext&, const CSS::Value&);
template<> std::optional<MarginEdge> toStyle<MarginEdge>(const BuilderContext&, const CSS::Value&);
template<> std::optional<PaddingEdge> toStyle<PaddingEdge>(const BuilderContext&, const CSS::Value&);
template<> std::optional<GapGutter> toStyle<GapGutter>(const BuilderContext&, const CSS::Value&);
template<> std::optional<ScrollbarGutter> toStyle<ScrollbarGutter>(const BuilderContext&, const CSS::Value&);
template<> std::optional<ScrollbarColor> toStyle<ScrollbarColor>(const BuilderContext&, const CSS::Value&);
template<> std::optional<FontWeight> toStyle<FontWeight>(const BuilderContext&, const CSS::Value&);
template<> std::optional<FontFamilies> toStyle<FontFamilies>(const BuilderContext&, const CSS::Value&);
template<> std::optional<FontWidth> toStyle<FontWidth>(const BuilderContext&, const CSS::Value&);
template<> std::optional<LineWidth> toStyle<LineWidth>(const BuilderContext&, const CSS::Value&);
template<> std::optional<CornerRadius> toStyle<CornerRadius>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Opacity> toStyle<Opacity>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Color> toStyle<Color>(const BuilderContext&, const CSS::Value&);
template<> std::optional<BoxShadows> toStyle<BoxShadows>(const BuilderContext&, const CSS::Value&);
template<> std::optional<LineHeight> toStyle<LineHeight>(const BuilderContext&, const CSS::Value&);
template<> std::optional<SelfAlignmentData> toStyle<SelfAlignmentData>(const BuilderContext&, const CSS::Value&);
template<> std::optional<ContentAlignmentData> toStyle<ContentAlignmentData>(const BuilderContext&, const CSS::Value&);
template<> std::optional<JustifyItems> toStyle<JustifyItems>(const BuilderContext&, const CSS::Value&);
template<> std::optional<FlexWrap> toStyle<FlexWrap>(const BuilderContext&, const CSS::Value&);
template<> std::optional<VerticalAlignValue> toStyle<VerticalAlignValue>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Translate> toStyle<Translate>(const BuilderContext&, const CSS::Value&);
template<> std::optional<Image> toStyle<Image>(const BuilderContext&, const CSS::Value&);
template<> std::optional<BorderImageSlice> toStyle<BorderImageSlice>(const BuilderContext&, const CSS::Value&);
template<> std::optional<BorderImageWidth> toStyle<BorderImageWidth>(const BuilderContext&, const CSS::Value&);
template<> std::optional<BorderImageOutset> toStyle<BorderImageOutset>(const BuilderContext&, const CSS::Value&);
template<> std::optional<BorderImageRepeat> toStyle<BorderImageRepeat>(const BuilderContext&, const CSS::Value&);

template<typename T> std::optional<T> toStyle(const BuilderContext& context, const CSS::Value& value) {
    using enum CSS::KeywordName;
    if constexpr (ValueOrKeywordTraits<T>::isValueOrKeyword) {
        using Traits = ValueOrKeywordTraits<T>;
        using Keyword = typename Traits::keyword_type;
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == Keyword::value)
            return T {Keyword {}};
        if (auto converted = toStyle<typename Traits::value_type>(context, value))
            return T {std::move(*converted)};
        return std::nullopt;
    }

    if constexpr (std::is_same_v<T, float>)
        if (const auto* number = std::get_if<CSS::Number>(&value))
            return number->value;

    if constexpr (detail::IsStyleLength<T>::value) {
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == KeywordNormal)
            return T {};

        const auto toPixels = [&context](CSS::Length length) -> std::optional<float> {
            if (!std::isfinite(length.value))
                return std::nullopt;
            auto pixels = absolutePixels(length);
            if (!pixels && length.unit == CSS::LengthUnit::Em)
                pixels = context.style.fontSize() * length.value;
            if (!pixels && length.unit == CSS::LengthUnit::Rem)
                pixels = context.rootStyle.fontSize() * length.value;
            return pixels && std::isfinite(*pixels) ? pixels : std::nullopt;
        };

        if (const auto* percentage = std::get_if<CSS::Percentage>(&value))
            return std::isfinite(percentage->value) ? std::optional<T>(T {0.f, percentage->value / 100.f}) : std::nullopt;

        const CSS::Length* specifiedLength = std::get_if<CSS::Length>(&value);
        if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
            if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage))
                return std::isfinite(percentage->value) ? std::optional<T>(T {0.f, percentage->value / 100.f}) : std::nullopt;
            specifiedLength = std::get_if<CSS::Length>(lengthPercentage);
        }
        if (!specifiedLength)
            return std::nullopt;
        const auto pixels = toPixels(*specifiedLength);
        return pixels ? std::optional<T>(T {*pixels}) : std::nullopt;
    }

    if constexpr (std::is_constructible_v<T, CSS::Number>)
        if (const auto* number = std::get_if<CSS::Number>(&value))
            return T {*number};

    if constexpr (std::is_enum_v<T>)
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value))
            return fromCSSKeyword<T>(keyword->keyword);

    if constexpr (CSS::VariantContains<T, CSS::Value>::value)
        if (const auto* result = std::get_if<T>(&value))
            return *result;
    return std::nullopt;
}

template<typename T> std::optional<T> toStyle(const BuilderState& builderState, const CSS::Value& value) {
    return toStyle<T>(builderState.context, value);
}

template<typename T> std::optional<T> toStyle(const BuilderState& builderState, const CSS::StyleValue& value) {
    if constexpr (CSS::VariantContains<T, CSS::StyleValue>::value)
        if (const auto* storedValue = std::get_if<T>(&value))
            return *storedValue;
    if (const auto* cssValue = std::get_if<CSS::Value>(&value))
        return toStyle<T>(builderState, *cssValue);
    return std::nullopt;
}

namespace detail {
struct CompileContext;
using CompileResult = std::optional<std::vector<CSS::StyleDeclaration>>;
using CompileFunction = CompileResult (*)(CompileContext&);
using SetFunction = void (*)(ComputedStyle&, const CSS::StyleValue&);
using InitialFunction = void (*)(ComputedStyle&);
using SpecifyFunction = void (*)(ComputedStyle&);
using InheritFunction = void (*)(ComputedStyle&, const ComputedStyle&);

enum class PropertyImpact : std::uint8_t {
    Layout = 1 << 0,
    Paint = 1 << 1,
    HitTest = 1 << 2,
    Descendant = 1 << 3
};

inline constexpr PropertyImpact operator|(PropertyImpact left, PropertyImpact right) {
    return static_cast<PropertyImpact>(static_cast<std::uint8_t>(left) | static_cast<std::uint8_t>(right));
}

inline constexpr bool hasImpact(PropertyImpact value, PropertyImpact flag) {
    return (static_cast<std::uint8_t>(value) & static_cast<std::uint8_t>(flag)) != 0;
}

struct PropertyDefinition {
    std::string_view name;
    CompileFunction compile = nullptr;
    SetFunction set = nullptr;
    InitialFunction initial = nullptr;
    SpecifyFunction specify = nullptr;
    InheritFunction inherit = nullptr;
    PropertyImpact impact = PropertyImpact::Layout;
    bool userAgentOnly = false;

    bool isPaintOnly() const { return hasImpact(impact, PropertyImpact::Paint) && !hasImpact(impact, PropertyImpact::Layout); }
    bool affectsHitTesting() const { return hasImpact(impact, PropertyImpact::HitTest); }
    bool inherited = false;
};

const PropertyDefinition* findLegacyProperty(std::string_view name);
const PropertyDefinition* legacyPropertyBegin();
const PropertyDefinition* legacyPropertyEnd();
bool isPropertyInherited(std::string_view name);
bool isPropertyPaintOnly(std::string_view name);
bool propertyAffectsHitTesting(std::string_view name);
bool propertyPropagatesToDescendants(std::string_view name);
void applyStyleDeclaration(BuilderState& builderState, const CSS::StyleDeclaration& declaration);
void applyInvalidStyleDeclaration(BuilderState& builderState, std::string_view property);
} // namespace detail
} // namespace Core::Style
