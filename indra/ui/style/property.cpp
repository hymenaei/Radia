/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/property.h"
#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <initializer_list>
#include <iterator>
#include <limits>
#include <string>
#include <type_traits>
#include <utility>
#include "ComputedStyleProperties.h"
#include "css/color.h"
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"
#include "CSSPropertyParsing.h"
#include "CSSValue.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::lower;
using detail::normalizeCSSKeyword;
using detail::startsWith;
using detail::StylePropertyImpact;
using detail::trim;

bool hasDimensionUnit(const detail::CSSTokenStream& stream, detail::CSSTokenRange range, std::string_view unit) {
    const auto dimension = detail::parseCSSDimension(stream, range);
    return dimension && dimension->unit == unit;
}

bool parseStrokeCap(detail::CSSValueRange value, StrokeCap& cap) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "butt") cap = StrokeCap::Butt;
    else if (token == "round") cap = StrokeCap::Round;
    else if (token == "square") cap = StrokeCap::Square;
    else return false;
    return true;
}

template<typename Visitor> bool visitKeywords(const CSSValue& value, Visitor&& visitor) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) return visitor(keyword->id);
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list || list->values.empty()) return false;
    for (const CSSValueComponent& component : list->values) {
        const auto* keyword = std::get_if<CSS::Keyword>(&component);
        if (!keyword || !visitor(keyword->id)) return false;
    }
    return true;
}

std::optional<SelfAlignmentData> parseSelfAlignment(const CSSValue& value, bool allowLegacy, bool& legacy) {
    std::optional<ItemPosition> position;
    std::optional<CSSKeyword> baselinePreference;
    OverflowAlignment overflow = OverflowAlignment::Default;
    bool baseline = false;
    legacy = false;
    const bool valid = visitKeywords(value, [&](CSSKeyword keyword) {
        switch (keyword) {
            case CSSKeyword::First:
            case CSSKeyword::Last:
                if (baselinePreference || position || overflow != OverflowAlignment::Default || legacy) return false;
                baselinePreference = keyword;
                return true;
            case CSSKeyword::Baseline:
                if (baseline || position || overflow != OverflowAlignment::Default || legacy) return false;
                baseline = true;
                return true;
            case CSSKeyword::Safe:
            case CSSKeyword::Unsafe:
                if (overflow != OverflowAlignment::Default || baseline || baselinePreference || position || legacy) return false;
                overflow = keyword == CSSKeyword::Safe ? OverflowAlignment::Safe : OverflowAlignment::Unsafe;
                return true;
            case CSSKeyword::Legacy:
                if (!allowLegacy || legacy || baseline || baselinePreference || overflow != OverflowAlignment::Default) return false;
                if (position && *position != ItemPosition::Center && *position != ItemPosition::Left && *position != ItemPosition::Right)
                    return false;
                legacy = true;
                return true;
            default: break;
        }

        if (position || baseline || baselinePreference) return false;
        switch (keyword) {
            case CSSKeyword::Auto: position = ItemPosition::Auto; break;
            case CSSKeyword::Normal: position = ItemPosition::Normal; break;
            case CSSKeyword::Stretch: position = ItemPosition::Stretch; break;
            case CSSKeyword::Center: position = ItemPosition::Center; break;
            case CSSKeyword::Start: position = ItemPosition::Start; break;
            case CSSKeyword::End: position = ItemPosition::End; break;
            case CSSKeyword::SelfStart: position = ItemPosition::SelfStart; break;
            case CSSKeyword::SelfEnd: position = ItemPosition::SelfEnd; break;
            case CSSKeyword::FlexStart: position = ItemPosition::FlexStart; break;
            case CSSKeyword::FlexEnd: position = ItemPosition::FlexEnd; break;
            case CSSKeyword::Left: position = ItemPosition::Left; break;
            case CSSKeyword::Right: position = ItemPosition::Right; break;
            case CSSKeyword::AnchorCenter: position = ItemPosition::AnchorCenter; break;
            default: return false;
        }
        return !legacy || *position == ItemPosition::Center || *position == ItemPosition::Left || *position == ItemPosition::Right;
    });

    if (!valid || (baselinePreference && !baseline) || (!position && !baseline && !legacy)) return std::nullopt;
    if (baseline) position = baselinePreference == CSSKeyword::Last ? ItemPosition::LastBaseline : ItemPosition::Baseline;
    return SelfAlignmentData{position.value_or(ItemPosition::Normal), overflow};
}

std::optional<GenericFontFamily> genericFontFamily(CSSKeyword keyword) {
    switch (keyword) {
        case CSSKeyword::Serif: return GenericFontFamily::Serif;
        case CSSKeyword::SansSerif: return GenericFontFamily::SansSerif;
        case CSSKeyword::SystemUi: return GenericFontFamily::SystemUI;
        case CSSKeyword::Cursive: return GenericFontFamily::Cursive;
        case CSSKeyword::Fantasy: return GenericFontFamily::Fantasy;
        case CSSKeyword::Math: return GenericFontFamily::Math;
        case CSSKeyword::Monospace: return GenericFontFamily::Monospace;
        case CSSKeyword::UiSerif: return GenericFontFamily::UISerif;
        case CSSKeyword::UiSansSerif: return GenericFontFamily::UISansSerif;
        case CSSKeyword::UiMonospace: return GenericFontFamily::UIMonospace;
        case CSSKeyword::UiRounded: return GenericFontFamily::UIRounded;
        case CSSKeyword::Fangsong: return GenericFontFamily::Fangsong;
        case CSSKeyword::Kai: return GenericFontFamily::Kai;
        case CSSKeyword::KhmerMul: return GenericFontFamily::KhmerMul;
        case CSSKeyword::Nastaliq: return GenericFontFamily::Nastaliq;
        default: return std::nullopt;
    }
}

} // namespace

template<> std::optional<SelfAlignmentData> toStyle<SelfAlignmentData>(const StyleBuilderContext&, const CSSValue& value) {
    bool legacy = false;
    return parseSelfAlignment(value, false, legacy);
}

template<> std::optional<VerticalAlignValue> toStyle<VerticalAlignValue>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        switch (keyword->id) {
            case CSSKeyword::Baseline: return VerticalAlignValue{VerticalAlign::Baseline, {}};
            case CSSKeyword::Sub: return VerticalAlignValue{VerticalAlign::Sub, {}};
            case CSSKeyword::Super: return VerticalAlignValue{VerticalAlign::Super, {}};
            case CSSKeyword::TextTop: return VerticalAlignValue{VerticalAlign::TextTop, {}};
            case CSSKeyword::TextBottom: return VerticalAlignValue{VerticalAlign::TextBottom, {}};
            case CSSKeyword::Top: return VerticalAlignValue{VerticalAlign::Top, {}};
            case CSSKeyword::Middle: return VerticalAlignValue{VerticalAlign::Middle, {}};
            case CSSKeyword::Bottom: return VerticalAlignValue{VerticalAlign::Bottom, {}};
            default: return std::nullopt;
        }
    }

    bool percentage = std::holds_alternative<CSS::Percentage>(value);
    if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value))
        percentage = std::holds_alternative<CSS::Percentage>(*lengthPercentage);
    const auto offset = toStyle<Length>(context, value);
    if (!offset) return std::nullopt;
    return VerticalAlignValue{percentage ? VerticalAlign::Percentage : VerticalAlign::Length, *offset};
}

template<> std::optional<Translate> toStyle<Translate>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::NoneValue) return Translate{};

    const auto translatedLength = [&context](const CSSValue& cssValue) { return toStyle<Length>(context, cssValue); };
    if (const auto* list = std::get_if<CSS::List>(&value)) {
        if (list->values.empty() || list->values.size() > 3) return std::nullopt;
        const auto valueOf = [](const CSSValueComponent& component) -> std::optional<CSSValue> {
            if (const auto* length = std::get_if<CSS::Length>(&component)) return CSSValue{*length};
            if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&component)) return CSSValue{*lengthPercentage};
            if (const auto* percentage = std::get_if<CSS::Percentage>(&component)) return CSSValue{*percentage};
            return std::nullopt;
        };
        const auto xValue = valueOf(list->values.front());
        if (!xValue) return std::nullopt;
        const auto x = translatedLength(*xValue);
        if (!x) return std::nullopt;
        if (list->values.size() == 1) return Translate{*x, {}, {}, false};
        const auto yValue = valueOf(list->values[1]);
        if (!yValue) return std::nullopt;
        const auto y = translatedLength(*yValue);
        if (!y) return std::nullopt;
        if (list->values.size() == 2) return Translate{*x, *y, {}, false};
        const auto zValue = valueOf(list->values[2]);
        if (!zValue) return std::nullopt;
        const auto z = translatedLength(*zValue);
        return z ? std::optional<Translate>(Translate{*x, *y, *z, false}) : std::nullopt;
    }

    const auto x = translatedLength(value);
    return x ? std::optional<Translate>(Translate{*x, {}, {}, false}) : std::nullopt;
}

template<> std::optional<JustifyItems> toStyle<JustifyItems>(const StyleBuilderContext& context, const CSSValue& value) {
    bool legacy = false;
    const auto alignment = parseSelfAlignment(value, true, legacy);
    if (!alignment) return std::nullopt;
    if (!legacy) return JustifyItems{alignment->position, alignment->overflow};
    if (alignment->position != ItemPosition::Normal) return JustifyItems{alignment->position, alignment->overflow, true};

    if (context.parentStyle && context.parentStyle->justifyItems().legacy) return context.parentStyle->justifyItems();
    return JustifyItems{};
}

template<> std::optional<FlexWrap> toStyle<FlexWrap>(const StyleBuilderContext&, const CSSValue& value) {
    FlexWrap result;
    bool modeSet = false;
    const bool valid = visitKeywords(value, [&](CSSKeyword keyword) {
        switch (keyword) {
            case CSSKeyword::Nowrap:
                if (modeSet || result.balance) return false;
                result.mode = FlexWrapMode::Nowrap;
                modeSet = true;
                return true;
            case CSSKeyword::Wrap:
            case CSSKeyword::WrapReverse:
                if (modeSet) return false;
                result.mode = keyword == CSSKeyword::Wrap ? FlexWrapMode::Wrap : FlexWrapMode::WrapReverse;
                modeSet = true;
                return true;
            case CSSKeyword::Balance:
                if (result.balance || (modeSet && result.mode == FlexWrapMode::Nowrap)) return false;
                result.balance = true;
                return true;
            default: return false;
        }
    });
    if (!valid || (!modeSet && !result.balance)) return std::nullopt;
    if (!modeSet) result.mode = FlexWrapMode::Wrap;
    return result;
}

template<> std::optional<ContentAlignmentData> toStyle<ContentAlignmentData>(const StyleBuilderContext&, const CSSValue& value) {
    std::optional<ContentPosition> position;
    std::optional<ContentDistribution> distribution;
    std::optional<CSSKeyword> baselinePreference;
    OverflowAlignment overflow = OverflowAlignment::Default;
    bool baseline = false;
    const bool valid = visitKeywords(value, [&](CSSKeyword keyword) {
        switch (keyword) {
            case CSSKeyword::First:
            case CSSKeyword::Last:
                if (baselinePreference || position || distribution || overflow != OverflowAlignment::Default) return false;
                baselinePreference = keyword;
                return true;
            case CSSKeyword::Baseline:
                if (baseline || position || distribution || overflow != OverflowAlignment::Default) return false;
                baseline = true;
                return true;
            case CSSKeyword::Safe:
            case CSSKeyword::Unsafe:
                if (overflow != OverflowAlignment::Default || baseline || baselinePreference || position || distribution) return false;
                overflow = keyword == CSSKeyword::Safe ? OverflowAlignment::Safe : OverflowAlignment::Unsafe;
                return true;
            default: break;
        }

        if (position || distribution || baseline || baselinePreference) return false;
        switch (keyword) {
            case CSSKeyword::Normal: position = ContentPosition::Normal; break;
            case CSSKeyword::Center: position = ContentPosition::Center; break;
            case CSSKeyword::Start: position = ContentPosition::Start; break;
            case CSSKeyword::End: position = ContentPosition::End; break;
            case CSSKeyword::FlexStart: position = ContentPosition::FlexStart; break;
            case CSSKeyword::FlexEnd: position = ContentPosition::FlexEnd; break;
            case CSSKeyword::Left: position = ContentPosition::Left; break;
            case CSSKeyword::Right: position = ContentPosition::Right; break;
            case CSSKeyword::SpaceBetween: distribution = ContentDistribution::SpaceBetween; break;
            case CSSKeyword::SpaceAround: distribution = ContentDistribution::SpaceAround; break;
            case CSSKeyword::SpaceEvenly: distribution = ContentDistribution::SpaceEvenly; break;
            case CSSKeyword::Stretch: distribution = ContentDistribution::Stretch; break;
            default: return false;
        }
        return true;
    });

    if (!valid || (baselinePreference && !baseline) || (!position && !distribution && !baseline)) return std::nullopt;
    if (baseline) position = baselinePreference == CSSKeyword::Last ? ContentPosition::LastBaseline : ContentPosition::Baseline;
    return ContentAlignmentData{position.value_or(ContentPosition::Normal), distribution.value_or(ContentDistribution::Default), overflow};
}

template<> std::optional<Dimension> toStyle<Dimension>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        switch (keyword->id) {
            case CSSKeyword::Auto: return Dimension{};
            case CSSKeyword::Content: return Dimension::fromKeyword(DimensionKeyword::Content);
            case CSSKeyword::MinContent: return Dimension::fromKeyword(DimensionKeyword::MinContent);
            case CSSKeyword::MaxContent: return Dimension::fromKeyword(DimensionKeyword::MaxContent);
            case CSSKeyword::FitContent: return Dimension::fromKeyword(DimensionKeyword::FitContent);
            default: return std::nullopt;
        }
    }

    if (const auto* percentage = std::get_if<CSS::Percentage>(&value)) {
        if (!std::isfinite(percentage->value) || percentage->value < 0.f) return std::nullopt;
        return Dimension::fromPercentage(percentage->value / 100.f);
    }
    if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
        if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage)) {
            if (!std::isfinite(percentage->value) || percentage->value < 0.f) return std::nullopt;
            return Dimension::fromPercentage(percentage->value / 100.f);
        }
    }

    const auto length = toStyle<Length>(context, value);
    if (!length || !std::isfinite(length->pixels) || !std::isfinite(length->percent) || length->pixels < 0.f || length->percent < 0.f)
        return std::nullopt;
    return Dimension::fromLength(*length);
}

template<> std::optional<OptionalDimension> toStyle<OptionalDimension>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && (keyword->id == CSSKeyword::Auto || keyword->id == CSSKeyword::NoneValue))
        return std::optional<OptionalDimension>{std::in_place, std::nullopt};

    const auto dimension = toStyle<Dimension>(context, value);
    return dimension ? std::optional<OptionalDimension>{std::in_place, *dimension} : std::nullopt;
}

template<> std::optional<InsetEdge> toStyle<InsetEdge>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::Auto)
        return std::optional<InsetEdge>{std::in_place, std::nullopt};
    const auto length = toStyle<Length>(context, value);
    return length ? std::optional<InsetEdge>{std::in_place, *length} : std::nullopt;
}

template<> std::optional<LineWidth> toStyle<LineWidth>(const StyleBuilderContext&, const CSSValue& value) {
    return LineWidth::fromCSSValue(value);
}

template<> std::optional<Opacity> toStyle<Opacity>(const StyleBuilderContext&, const CSSValue& value) {
    float opacity;
    if (const auto* number = std::get_if<CSS::Number>(&value)) opacity = number->value;
    else if (const auto* percentage = std::get_if<CSS::Percentage>(&value)) opacity = percentage->value / 100.f;
    else return std::nullopt;
    return Opacity{std::clamp(opacity, 0.f, 1.f)};
}

template<> std::optional<CornerRadius> toStyle<CornerRadius>(const StyleBuilderContext&, const CSSValue& value) {
    const auto toLengthPercentage = [](const CSS::LengthPercentage& value) -> std::optional<LengthPercentage> {
        LengthPercentage result;
        if (const auto* percentage = std::get_if<CSS::Percentage>(&value)) result.percent = percentage->value / 100.f;
        else if (const auto* specifiedLength = std::get_if<CSS::Length>(&value)) {
            const auto pixels = Style::absolutePixels(*specifiedLength);
            if (!pixels) return std::nullopt;
            result.pixels = *pixels;
        } else return std::nullopt;
        return result;
    };
    const auto fromComponent = [&toLengthPercentage](const CSSValueComponent& component) -> std::optional<LengthPercentage> {
        const auto* value = std::get_if<CSS::LengthPercentage>(&component);
        return value ? toLengthPercentage(*value) : std::nullopt;
    };

    if (const auto* list = std::get_if<CSS::List>(&value)) {
        if (list->values.empty() || list->values.size() > 2) return std::nullopt;
        const auto horizontal = fromComponent(list->values[0]);
        const auto vertical = list->values.size() == 2 ? fromComponent(list->values[1]) : horizontal;
        if (!horizontal || !vertical) return std::nullopt;
        return CornerRadius{*horizontal, *vertical};
    }
    if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
        const auto radius = toLengthPercentage(*lengthPercentage);
        if (radius) return CornerRadius::uniform(*radius);
    }
    return std::nullopt;
}

template<> std::optional<StyleImage> toStyle<StyleImage>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::NoneValue) return StyleImage{};
    const auto* source = std::get_if<CSS::String>(&value);
    if (!source) return std::nullopt;

    detail::CSSTokenStream stream(source->value);
    const detail::CSSTokenRange range{0, stream.tokens().size()};
    if (const auto resource = detail::parseCSSUrl(stream, range)) return StyleImage{*resource};
    const auto gradient = StyleModel::parseGradient({stream, range}, context.style.usedColorScheme);
    return gradient ? std::optional<StyleImage>{StyleImage{*gradient}} : std::nullopt;
}

namespace {
std::optional<std::array<CSSValue, 4>> borderImageEdgeValues(const CSSValue& value) {
    std::vector<CSSValue> values;
    if (const auto* list = std::get_if<CSS::List>(&value)) {
        values.reserve(list->values.size());
        for (const CSSValueComponent& component : list->values) values.push_back(copyCSSValueComponent(component));
    } else values.push_back(value);
    return expandFourValues(std::move(values));
}

std::optional<BorderImageRepeatMode> borderImageRepeatMode(const CSSValue& value) {
    const auto* keyword = std::get_if<CSS::Keyword>(&value);
    if (!keyword) return std::nullopt;
    switch (keyword->id) {
        case CSSKeyword::Stretch: return BorderImageRepeatMode::Stretch;
        case CSSKeyword::Repeat: return BorderImageRepeatMode::Repeat;
        case CSSKeyword::Round: return BorderImageRepeatMode::Round;
        case CSSKeyword::Space: return BorderImageRepeatMode::Space;
        default: return std::nullopt;
    }
}
} // namespace

template<> std::optional<BorderImageSlice> toStyle<BorderImageSlice>(const StyleBuilderContext&, const CSSValue& value) {
    const auto* list = std::get_if<CSS::List>(&value);
    std::vector<CSSValue> edges;
    if (list) edges.reserve(list->values.size());
    bool fill = false;
    if (list) {
        for (const CSSValueComponent& component : list->values) {
            const CSSValue item = copyCSSValueComponent(component);
            if (const auto* keyword = std::get_if<CSS::Keyword>(&item); keyword && *keyword == CSSKeyword::Fill) {
                if (fill) return std::nullopt;
                fill = true;
            } else edges.push_back(item);
        }
    } else {
        edges.push_back(value);
    }
    const auto values = expandFourValues(std::move(edges));
    if (!values) return std::nullopt;

    const auto slice = [](const CSSValue& item) -> std::optional<BorderImageSliceValue> {
        if (const auto* number = std::get_if<CSS::Number>(&item); number && std::isfinite(number->value) && number->value >= 0.f)
            return BorderImageSliceValue{number->value, false};
        if (const auto* percentage = std::get_if<CSS::Percentage>(&item); percentage && std::isfinite(percentage->value) && percentage->value >= 0.f)
            return BorderImageSliceValue{percentage->value, true};
        return std::nullopt;
    };
    const auto top = slice((*values)[0]);
    const auto right = slice((*values)[1]);
    const auto bottom = slice((*values)[2]);
    const auto left = slice((*values)[3]);
    if (!top || !right || !bottom || !left) return std::nullopt;
    BorderImageSlice result{};
    result.edges = {*top, *right, *bottom, *left};
    result.fill = fill;
    return result;
}

template<> std::optional<BorderImageWidth> toStyle<BorderImageWidth>(const StyleBuilderContext& context, const CSSValue& value) {
    const auto values = borderImageEdgeValues(value);
    if (!values) return std::nullopt;
    const auto width = [&context](const CSSValue& item) -> std::optional<BorderImageWidthValue> {
        if (const auto* keyword = std::get_if<CSS::Keyword>(&item); keyword && *keyword == CSSKeyword::Auto)
            return BorderImageWidthValue{0.f, BorderImageValueUnit::Auto};
        if (const auto* number = std::get_if<CSS::Number>(&item); number && std::isfinite(number->value) && number->value >= 0.f)
            return BorderImageWidthValue{number->value, BorderImageValueUnit::Number};
        const auto length = toStyle<Length>(context, item);
        if (!length || length->pixels < 0.f || length->percent < 0.f) return std::nullopt;
        if (length->percent > 0.f) return BorderImageWidthValue{length->percent, BorderImageValueUnit::Percentage};
        if (std::holds_alternative<CSS::Percentage>(item)
            || (std::get_if<CSS::LengthPercentage>(&item) && std::holds_alternative<CSS::Percentage>(std::get<CSS::LengthPercentage>(item))))
            return BorderImageWidthValue{0.f, BorderImageValueUnit::Percentage};
        return BorderImageWidthValue{length->pixels, BorderImageValueUnit::Length};
    };
    const auto top = width((*values)[0]);
    const auto right = width((*values)[1]);
    const auto bottom = width((*values)[2]);
    const auto left = width((*values)[3]);
    if (!top || !right || !bottom || !left) return std::nullopt;
    BorderImageWidth result{};
    result.edges = {*top, *right, *bottom, *left};
    return result;
}

template<> std::optional<BorderImageOutset> toStyle<BorderImageOutset>(const StyleBuilderContext& context, const CSSValue& value) {
    const auto values = borderImageEdgeValues(value);
    if (!values) return std::nullopt;
    const auto outset = [&context](const CSSValue& item) -> std::optional<BorderImageOutsetValue> {
        if (const auto* number = std::get_if<CSS::Number>(&item); number && std::isfinite(number->value) && number->value >= 0.f)
            return BorderImageOutsetValue{number->value, true};
        if (!std::holds_alternative<CSS::Length>(item)) return std::nullopt;
        const auto length = toStyle<Length>(context, item);
        if (!length || length->pixels < 0.f || length->percent != 0.f) return std::nullopt;
        return BorderImageOutsetValue{length->pixels, false};
    };
    const auto top = outset((*values)[0]);
    const auto right = outset((*values)[1]);
    const auto bottom = outset((*values)[2]);
    const auto left = outset((*values)[3]);
    if (!top || !right || !bottom || !left) return std::nullopt;
    BorderImageOutset result{};
    result.edges = {*top, *right, *bottom, *left};
    return result;
}

template<> std::optional<BorderImageRepeat> toStyle<BorderImageRepeat>(const StyleBuilderContext&, const CSSValue& value) {
    std::array<CSSValue, 2> values;
    std::size_t count = 0;
    if (const auto* list = std::get_if<CSS::List>(&value)) {
        if (list->values.empty() || list->values.size() > 2) return std::nullopt;
        count = list->values.size();
        for (std::size_t index = 0; index < count; ++index) values[index] = copyCSSValueComponent(list->values[index]);
    } else {
        values[0] = value;
        count = 1;
    }
    const auto horizontal = borderImageRepeatMode(values[0]);
    const auto vertical = borderImageRepeatMode(values[count - 1]);
    if (!horizontal || !vertical) return std::nullopt;
    return BorderImageRepeat{*horizontal, *vertical};
}

template<> std::optional<float> toStyle<float>(const StyleBuilderContext& context, const CSSValue& value) {
    const float parentFontSize = context.parentStyle ? context.parentStyle->fontSize() : ComputedStyle::initialStyle().fontSize();
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        constexpr float kMediumScale = FontDescription::cssMediumSize;
        switch (keyword->id) {
            case CSSKeyword::XxSmall: return kMediumScale * (3.f / 5.f);
            case CSSKeyword::XSmall: return kMediumScale * (3.f / 4.f);
            case CSSKeyword::Small: return kMediumScale * (8.f / 9.f);
            case CSSKeyword::Medium: return kMediumScale;
            case CSSKeyword::Large: return kMediumScale * (6.f / 5.f);
            case CSSKeyword::XLarge: return kMediumScale * (3.f / 2.f);
            case CSSKeyword::XxLarge: return kMediumScale * 2.f;
            case CSSKeyword::XxxLarge: return kMediumScale * 3.f;
            case CSSKeyword::Larger: return parentFontSize * 1.2f;
            case CSSKeyword::Smaller: return parentFontSize / 1.2f;
            default: return std::nullopt;
        }
    }

    if (const auto* length = std::get_if<CSS::Length>(&value)) {
        const auto pixels = Style::absolutePixels(*length);
        return pixels && std::isfinite(*pixels) && *pixels >= 0.f ? pixels : std::nullopt;
    }

    const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value);
    if (!lengthPercentage) return std::nullopt;
    if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage)) {
        if (std::isfinite(percentage->value) && percentage->value >= 0.f) return parentFontSize * percentage->value / 100.f;
        return std::nullopt;
    }

    const CSS::Length& length = std::get<CSS::Length>(*lengthPercentage);
    const bool supported = Style::absolutePixels(length).has_value() || length.unit == CSS::LengthUnit::Em || length.unit == CSS::LengthUnit::Rem;
    if (!supported || !std::isfinite(length.value) || length.value < 0.f) return std::nullopt;
    if (const auto pixels = Style::absolutePixels(length)) return pixels;
    if (length.unit == CSS::LengthUnit::Em) return parentFontSize * length.value;
    if (length.unit == CSS::LengthUnit::Rem) return context.rootStyle.fontSize() * length.value;
    return std::nullopt;
}

template<> std::optional<MarginEdge> toStyle<MarginEdge>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::Auto) return MarginEdge::automatic();
    const auto length = toStyle<Length>(context, value);
    return length ? std::optional<MarginEdge>(MarginEdge::fromLength(*length)) : std::nullopt;
}

template<> std::optional<PaddingEdge> toStyle<PaddingEdge>(const StyleBuilderContext& context, const CSSValue& value) {
    const auto length = toStyle<Length>(context, value);
    if (!length || length->pixels < 0.f || length->percent < 0.f) return std::nullopt;
    return PaddingEdge{length->pixels, length->percent};
}

template<> std::optional<Order> toStyle<Order>(const StyleBuilderContext&, const CSSValue& value) {
    const auto* number = std::get_if<CSS::Number>(&value);
    if (!number || !std::isfinite(number->value) || std::trunc(number->value) != number->value) return std::nullopt;

    const double numericValue = number->value;
    if (numericValue < static_cast<double>(std::numeric_limits<int>::min()) || numericValue > static_cast<double>(std::numeric_limits<int>::max()))
        return std::nullopt;

    return Order{static_cast<int>(number->value)};
}

template<> std::optional<GapGutter> toStyle<GapGutter>(const StyleBuilderContext&, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        if (*keyword == CSSKeyword::Normal) return GapGutter{};
        if (const auto lineWidth = LineWidth::fromCSSKeyword(keyword->id)) return GapGutter::fromPixels(lineWidth->pixels);
        return std::nullopt;
    }

    const auto toGap = [](CSS::Length length) -> std::optional<GapGutter> {
        const auto pixels = Style::absolutePixels(length);
        return pixels && std::isfinite(*pixels) && *pixels >= 0.f ? std::optional<GapGutter>(GapGutter::fromPixels(*pixels)) : std::nullopt;
    };
    if (const auto* length = std::get_if<CSS::Length>(&value)) return toGap(*length);
    if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
        if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage))
            return percentage->value == 0.f ? std::optional<GapGutter>(GapGutter{}) : std::nullopt;
        if (const auto* length = std::get_if<CSS::Length>(lengthPercentage)) return toGap(*length);
    }
    return std::nullopt;
}

template<> std::optional<ScrollbarGutter> toStyle<ScrollbarGutter>(const StyleBuilderContext&, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        if (*keyword == CSSKeyword::Auto) return ScrollbarGutter::Auto;
        if (*keyword == CSSKeyword::Stable) return ScrollbarGutter::Stable;
        return std::nullopt;
    }
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list || list->values.empty() || list->values.size() > 2) return std::nullopt;

    bool stable = false;
    bool bothEdges = false;
    for (const CSSValueComponent& component : list->values) {
        const auto* keyword = std::get_if<CSS::Keyword>(&component);
        if (!keyword) return std::nullopt;
        if (*keyword == CSSKeyword::Stable && !stable) stable = true;
        else if (*keyword == CSSKeyword::BothEdges && !bothEdges) bothEdges = true;
        else return std::nullopt;
    }
    if (!stable) return std::nullopt;
    return bothEdges ? ScrollbarGutter::StableBothEdges : ScrollbarGutter::Stable;
}

template<> std::optional<ScrollbarColor> toStyle<ScrollbarColor>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::Auto) return ScrollbarColor{};
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list || list->values.size() != 2) return std::nullopt;

    const auto color = [&context](const CSSValueComponent& component) -> std::optional<StyleColor> {
        const auto* cssColor = std::get_if<CSS::Color>(&component);
        return cssColor ? StyleColor::fromCSS(*cssColor, context.style.usedColorScheme) : std::nullopt;
    };
    const auto thumb = color(list->values[0]);
    const auto track = color(list->values[1]);
    if (!thumb || !track) return std::nullopt;
    return ScrollbarColor{false, *thumb, *track};
}

template<> std::optional<FontWeight> toStyle<FontWeight>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* number = std::get_if<CSS::Number>(&value);
        number && std::isfinite(number->value) && number->value >= 1.f && number->value <= 1000.f)
        return FontWeight{number->value};
    const auto* keyword = std::get_if<CSS::Keyword>(&value);
    if (!keyword) return std::nullopt;
    if (*keyword == CSSKeyword::Normal) return FontWeight{400.f};
    if (*keyword == CSSKeyword::Bold) return FontWeight{700.f};
    if (*keyword == CSSKeyword::Bolder || *keyword == CSSKeyword::Lighter) {
        const float inheritedWeight = context.parentStyle ? context.parentStyle->fontWeight().value : 400.f;
        if (*keyword == CSSKeyword::Bolder) {
            if (inheritedWeight < 350.f) return FontWeight{400.f};
            if (inheritedWeight < 550.f) return FontWeight{700.f};
            if (inheritedWeight < 900.f) return FontWeight{900.f};
            return FontWeight{inheritedWeight};
        }
        if (inheritedWeight < 100.f) return FontWeight{inheritedWeight};
        if (inheritedWeight < 550.f) return FontWeight{100.f};
        if (inheritedWeight < 750.f) return FontWeight{400.f};
        return FontWeight{700.f};
    }
    return std::nullopt;
}

template<> std::optional<FontFamilies> toStyle<FontFamilies>(const StyleBuilderContext&, const CSSValue& value) {
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list || list->values.empty()) return std::nullopt;

    FontFamilies families;
    families.reserve(list->values.size());
    for (const CSSValueComponent& component : list->values) {
        if (const auto* name = std::get_if<CSS::String>(&component)) {
            families.emplace_back(name->value);
            continue;
        }
        if (const auto* keyword = std::get_if<CSS::Keyword>(&component)) {
            const auto family = genericFontFamily(keyword->id);
            if (!family) return std::nullopt;
            families.emplace_back(*family);
            continue;
        }
        const auto* function = std::get_if<std::shared_ptr<const CSS::Function>>(&component);
        if (!function || !*function || lower((*function)->name) != "generic" || (*function)->arguments.values.size() != 1) return std::nullopt;
        const auto* keyword = std::get_if<CSS::Keyword>(&(*function)->arguments.values.front());
        if (!keyword) return std::nullopt;
        const auto family = genericFontFamily(keyword->id);
        if (!family
            || (*family != GenericFontFamily::Fangsong
                && *family != GenericFontFamily::Kai
                && *family != GenericFontFamily::KhmerMul
                && *family != GenericFontFamily::Nastaliq))
            return std::nullopt;
        families.emplace_back(*family);
    }
    return families;
}

template<> std::optional<FontWidth> toStyle<FontWidth>(const StyleBuilderContext&, const CSSValue& value) {
    if (const auto* percentage = std::get_if<CSS::Percentage>(&value)) {
        if (std::isfinite(percentage->value) && percentage->value >= 0.f) return FontWidth{percentage->value};
        return std::nullopt;
    }
    const auto* keyword = std::get_if<CSS::Keyword>(&value);
    if (!keyword) return std::nullopt;
    switch (keyword->id) {
        case CSSKeyword::Normal: return FontWidth{100.f};
        case CSSKeyword::UltraCondensed: return FontWidth{50.f};
        case CSSKeyword::ExtraCondensed: return FontWidth{62.5f};
        case CSSKeyword::Condensed: return FontWidth{75.f};
        case CSSKeyword::SemiCondensed: return FontWidth{87.5f};
        case CSSKeyword::SemiExpanded: return FontWidth{112.5f};
        case CSSKeyword::Expanded: return FontWidth{125.f};
        case CSSKeyword::ExtraExpanded: return FontWidth{150.f};
        case CSSKeyword::UltraExpanded: return FontWidth{200.f};
        default: return std::nullopt;
    }
}

template<> std::optional<LineHeight> toStyle<LineHeight>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value)) {
        if (*keyword == CSSKeyword::Normal) return LineHeight{CSS::Keyword::Normal{}};
        return std::nullopt;
    }

    if (const auto* number = std::get_if<CSS::Number>(&value)) return LineHeight{LineHeight::Number{number->value}};

    if (const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value)) {
        if (const auto* length = std::get_if<CSS::Length>(lengthPercentage)) {
            if (length->unit == CSS::LengthUnit::Px) return LineHeight{LineHeight::Length{length->value}};
        }
        if (const auto* percentage = std::get_if<CSS::Percentage>(lengthPercentage))
            return LineHeight{LineHeight::Length{context.style.fontSize() * percentage->value / 100.f}};
    }

    return std::nullopt;
}

template<> std::optional<StyleColor> toStyle<StyleColor>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* color = std::get_if<CSS::Color>(&value)) return StyleColor::fromCSS(*color, context.style.usedColorScheme);
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::CurrentColor) return StyleColor::current();
    return std::nullopt;
}

template<> std::optional<BoxShadows> toStyle<BoxShadows>(const StyleBuilderContext& context, const CSSValue& value) {
    if (const auto* keyword = std::get_if<CSS::Keyword>(&value); keyword && *keyword == CSSKeyword::NoneValue) return BoxShadows{};
    const auto* list = std::get_if<CSS::List>(&value);
    if (!list) return std::nullopt;

    const auto componentValue = [](const CSSValueComponent& component) -> std::optional<CSSValue> {
        return std::visit(
            [](const auto& item) -> std::optional<CSSValue> {
                using Item = std::remove_cvref_t<decltype(item)>;
                if constexpr (std::is_same_v<Item, std::shared_ptr<const CSS::List>>) return std::nullopt;
                else return CSSValue{item};
            },
            component);
    };

    BoxShadows shadows;
    shadows.reserve(list->values.size());
    for (const CSSValueComponent& entry : list->values) {
        const auto* group = std::get_if<std::shared_ptr<const CSS::List>>(&entry);
        if (!group || !*group) return std::nullopt;

        BoxShadow shadow;
        std::vector<float> lengths;
        bool hasColor = false;
        for (const CSSValueComponent& component : (*group)->values) {
            if (const auto* keyword = std::get_if<CSS::Keyword>(&component); keyword && *keyword == CSSKeyword::Inset) {
                if (shadow.inset) return std::nullopt;
                shadow.inset = true;
                continue;
            }

            const auto parsedComponent = componentValue(component);
            if (!parsedComponent) return std::nullopt;
            if (const auto color = toStyle<StyleColor>(context, *parsedComponent)) {
                if (hasColor) return std::nullopt;
                shadow.color = *color;
                hasColor = true;
                continue;
            }

            if (const auto* number = std::get_if<CSS::Number>(&*parsedComponent)) {
                if (number->value != 0.f) return std::nullopt;
                lengths.push_back(0.f);
                continue;
            }
            const auto length = toStyle<Length>(context, *parsedComponent);
            if (!length || length->percent != 0.f) return std::nullopt;
            lengths.push_back(length->pixels);
        }

        if (lengths.size() < 2 || lengths.size() > 4) return std::nullopt;
        shadow.horizontal = lengths[0];
        shadow.vertical = lengths[1];
        if (lengths.size() > 2) shadow.blur = lengths[2];
        if (lengths.size() > 3) shadow.spread = lengths[3];
        if (shadow.blur < 0.f) return std::nullopt;
        shadows.push_back(std::move(shadow));
    }
    return shadows;
}

namespace {
StyleDeclaration makeDeclaration(std::string_view name, StyleValue value) {
    return {name, std::move(value)};
}

std::optional<StyleColor> styleColorValue(const ComputedStyle& style, const StyleValue& value) {
    if (const auto* color = std::get_if<StyleColor>(&value)) return *color;
    if (const auto* cssValue = std::get_if<CSSValue>(&value)) {
        if (const auto* color = std::get_if<CSS::Color>(cssValue)) return StyleColor::fromCSS(*color, style.usedColorScheme);
        if (const auto* keyword = std::get_if<CSS::Keyword>(cssValue); keyword && *keyword == CSSKeyword::CurrentColor) return StyleColor::current();
    }
    return std::nullopt;
}

std::vector<StyleDeclaration> makeDeclarations(std::initializer_list<std::pair<std::string_view, StyleValue>> values) {
    std::vector<StyleDeclaration> declarations;
    declarations.reserve(values.size());
    for (auto& value : values) declarations.push_back(makeDeclaration(value.first, std::move(value.second)));
    return declarations;
}

std::vector<StyleDeclaration> makeDeclarations(const std::vector<ParsedLonghand>& values) {
    std::vector<StyleDeclaration> declarations;
    declarations.reserve(values.size());
    for (const ParsedLonghand& value : values)
        if (value.property) declarations.emplace_back(cssPropertyName(*value.property), value.value);
    return declarations;
}

std::vector<StyleDeclaration> makeDeclarations(std::string_view name, StyleValue value) {
    if (const auto id = findProperty(name)) {
        if (const auto shorthandValue = shorthand(*id)) {
            std::vector<StyleDeclaration> declarations;
            for (const CSSProperty property : shorthandValue->properties) {
                auto expanded = makeDeclarations(cssPropertyName(property), value);
                declarations.insert(declarations.end(), std::make_move_iterator(expanded.begin()), std::make_move_iterator(expanded.end()));
            }
            return declarations;
        }
    }
    return {{name, std::move(value)}};
}
} // namespace

namespace {
void initializeStyleProperty(StyleBuilderState& builderState, std::string_view name) {
    ComputedStyle& style = builderState.style;
    if (const auto id = findProperty(name); id && applyProperty(*id, builderState, ApplyType::Initial)) {
        if (*id == CSSProperty::DisplayValue) style.displaySet = true;
        return;
    }

    const detail::StylePropertyDefinition* property = detail::findLegacyProperty(name);
    if (!property || !property->initial) {
        LL_ERRS("UI") << "Attempted to initialize a style property without an initial function: " << name << LL_ENDL;
        return;
    }
    property->initial(style);
}

void clearExplicitInheritance(ComputedStyle& style, std::string_view propertyName) {
    auto& properties = style.explicitlyInheritedProperties;
    properties.erase(std::remove(properties.begin(), properties.end(), propertyName), properties.end());
}

void clearSpecifiedProperty(ComputedStyle& style, std::string_view propertyName) {
    auto& properties = style.specifiedProperties;
    properties.erase(std::remove(properties.begin(), properties.end(), propertyName), properties.end());
}

void markSpecifiedProperty(ComputedStyle& style, std::string_view propertyName) {
    if (std::find(style.specifiedProperties.begin(), style.specifiedProperties.end(), propertyName) == style.specifiedProperties.end())
        style.specifiedProperties.emplace_back(propertyName);
}

void markExplicitInheritance(ComputedStyle& style, std::string_view propertyName) {
    if (std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        == style.explicitlyInheritedProperties.end())
        style.explicitlyInheritedProperties.emplace_back(propertyName);
}

} // namespace

void detail::applyStyleDeclaration(StyleBuilderState& builderState, const StyleDeclaration& declaration) {
    ComputedStyle& style = builderState.style;
    const std::string_view name = declaration.property;
    const bool styleWide =
        std::holds_alternative<InitialStyleValue>(declaration.value) || std::holds_alternative<StyleWideKeyword>(declaration.value);
    if (styleWide) {
        if (const auto id = findProperty(name); id) {
            if (const auto shorthandValue = shorthand(*id)) {
                for (const CSSProperty longhand : shorthandValue->properties)
                    applyStyleDeclaration(builderState, {cssPropertyName(longhand), declaration.value});
                return;
            }
        }
    }

    const detail::StylePropertyDefinition* property = detail::findLegacyProperty(name);
    if (std::holds_alternative<InitialStyleValue>(declaration.value)) {
        clearExplicitInheritance(style, name);
        initializeStyleProperty(builderState, name);
        if (property && property->specify) property->specify(style);
        markSpecifiedProperty(style, name);
        return;
    }
    if (const auto keyword = std::get_if<StyleWideKeyword>(&declaration.value)) {
        clearExplicitInheritance(style, name);
        initializeStyleProperty(builderState, name);
        const bool inherit = *keyword == StyleWideKeyword::Inherit || (*keyword == StyleWideKeyword::Unset && detail::isPropertyInherited(name));
        if (inherit) {
            clearSpecifiedProperty(style, name);
            markExplicitInheritance(style, name);
        } else {
            if (property && property->specify) property->specify(style);
            markSpecifiedProperty(style, name);
        }
        return;
    }
    clearExplicitInheritance(style, name);
    if (const auto id = findProperty(name)) {
        if (!applyProperty(*id, builderState, ApplyType::Value, &declaration.value)) {
            if (!property || !property->set || std::holds_alternative<CSSValue>(declaration.value)) {
                applyInvalidStyleDeclaration(builderState, name);
                return;
            }
            property->set(style, declaration.value);
        }
        if (*id == CSSProperty::DisplayValue) style.displaySet = true;
        if (property && property->specify) property->specify(style);
        markSpecifiedProperty(style, name);
        return;
    }
    if (!property || !property->set) {
        LL_ERRS("UI") << "Attempted to apply a stylesheet declaration without an applicable property definition." << LL_ENDL;
        return;
    }
    property->set(style, declaration.value);
    if (property->specify) property->specify(style);
    markSpecifiedProperty(style, name);
}

void detail::applyInvalidStyleDeclaration(StyleBuilderState& builderState, std::string_view name) {
    if (const auto id = findProperty(name)) {
        if (const auto shorthandValue = shorthand(*id)) {
            for (const CSSProperty property : shorthandValue->properties) applyInvalidStyleDeclaration(builderState, cssPropertyName(property));
            return;
        }
    }

    applyStyleDeclaration(builderState, StyleDeclaration{name, StyleWideKeyword::Unset});
}

namespace { using CompileResult = detail::StyleCompileResult; } // namespace

struct StyleCompileValue {
    std::string text;
    const detail::CSSTokenStream& stream;
    detail::CSSTokenRange range;
    ColorSchemeMode scheme = ColorSchemeMode::Dark;
};

struct detail::StyleCompileContext {
    std::string_view property;
    StyleCompileValue value;
    const std::string& selector;
    StyleSheetLoadResult& result;
    const std::string& sourceName;

    CompileResult invalid() const {
        const std::size_t offset =
            value.range.begin < value.stream.tokens().size() ? value.stream.tokens()[value.range.begin].begin : value.stream.source().size();
        const auto [line, column] = detail::cssSourcePosition(value.stream.source(), offset);
        result.warning("stylesheet.property.value_invalid", "Invalid value for " + std::string(property) + ": " + value.text + ".", sourceName, line,
                       column);
        return std::nullopt;
    }

    CompileResult compiled(StyleValue parsed) const { return std::vector<StyleDeclaration>{makeDeclaration(property, std::move(parsed))}; }

    std::optional<CSS::Color> specifiedColorValue() const { return StyleModel::consumeCSSColor({value.stream, value.range}); }

    std::optional<CSS::Color> specifiedColorValue(detail::CSSValueRange raw) const { return StyleModel::consumeCSSColor(raw); }

    std::optional<float> number() const {
        const float parsed = StyleModel::parseNumberValue({value.stream, value.range}, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    }

    std::optional<float> number(detail::CSSValueRange raw) const {
        const float parsed = StyleModel::parseNumberValue(raw, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    }

    std::optional<Length> length() const { return StyleModel::parseLengthValue({value.stream, value.range}); }

    std::optional<Length> length(detail::CSSValueRange raw) const { return StyleModel::parseLengthValue(raw); }

    std::vector<detail::CSSTokenRange> commaSeparatedRanges() const { return detail::splitCSSOnDelimiter(value.stream, value.range, ','); }

    std::optional<Length> nonnegativeLength() const {
        const std::optional<Length> parsed = length();
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    }

    std::optional<Length> nonnegativeLength(detail::CSSValueRange raw) const {
        const std::optional<Length> parsed = length(raw);
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    }

    std::vector<detail::CSSTokenRange> ranges(bool splitSlash = true) const {
        return detail::splitCSSComponents(value.stream, value.range, splitSlash);
    }

    std::string keyword() const { return normalizeCSSKeyword(value.stream, value.range); }
};

namespace {
struct ParsedContent {
    bool valid = false;
    std::optional<std::string> value;
};

std::optional<std::string> decodeCSSStringRange(const detail::CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = detail::trimCSSRange(stream, range);
    if (range.end != range.begin + 1 || stream.tokens()[range.begin].kind != detail::CSSTokenKind::String) return std::nullopt;
    return detail::decodeCSSString(stream.text(range.begin));
}

ParsedContent parseContent(const StyleCompileValue& raw) {
    const auto& stream = raw.stream;
    const std::vector<detail::CSSTokenRange> components = detail::splitCSSComponents(stream, raw.range, true);
    if (components.size() == 1) {
        const std::string keyword = normalizeCSSKeyword(stream, components.front());
        if (keyword == "none" || keyword == "normal") return {true, std::nullopt};
        if (const std::optional<std::string> primary = decodeCSSStringRange(stream, components.front())) return {true, *primary};
    }
    if (components.size() != 3 || normalizeCSSKeyword(stream, components[1]) != "/") return {};
    const std::optional<std::string> primary = decodeCSSStringRange(stream, components[0]);
    const std::optional<std::string> alternative = decodeCSSStringRange(stream, components[2]);
    return primary && alternative ? ParsedContent{true, *primary} : ParsedContent{};
}

CompileResult compileContent(detail::StyleCompileContext& context) {
    const ParsedContent parsed = parseContent(context.value);
    return parsed.valid ? context.compiled(parsed.value) : context.invalid();
}

CompileResult compileFilter(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseFilter({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOutline(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseOutline({value.stream, value.range}, value.scheme);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

std::optional<BackgroundLayer> parseBackgroundImage(detail::CSSValueRange value, ColorSchemeMode scheme);
template<typename Parse> std::optional<std::vector<BackgroundLayer>> parseBackgroundLayerList(detail::CSSValueRange value, Parse parse);

std::optional<BackgroundLayer> parseBackgroundImage(detail::CSSValueRange value, ColorSchemeMode scheme) {
    BackgroundLayer layer;
    if (normalizeCSSKeyword(value.stream, value.range) == "none") return layer;
    if (const std::optional<std::string> url = detail::parseCSSUrl(value.stream, value.range)) {
        layer.image = StyleImage{*url};
        return layer;
    }
    if (const std::optional<Gradient> gradient = StyleModel::parseGradient(value, scheme)) {
        layer.image = StyleImage{*gradient};
        return layer;
    }
    return std::nullopt;
}

template<typename Parse> std::optional<std::vector<BackgroundLayer>> parseBackgroundLayerList(detail::CSSValueRange value, Parse parse) {
    const std::vector<detail::CSSTokenRange> values = detail::splitCSSOnDelimiter(value.stream, value.range, ',');
    if (values.empty()) return std::nullopt;
    std::vector<BackgroundLayer> result;
    result.reserve(values.size());
    for (const detail::CSSTokenRange range : values) {
        const std::optional<BackgroundLayer> layer = parse({value.stream, range});
        if (!layer) return std::nullopt;
        result.push_back(*layer);
    }
    return result;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundImages(detail::CSSValueRange value, ColorSchemeMode scheme) {
    return parseBackgroundLayerList(value, [scheme](detail::CSSValueRange range) { return parseBackgroundImage(range, scheme); });
}

std::optional<Length> parsePositionHorizontal(detail::CSSValueRange value);
std::optional<Length> parsePositionVertical(detail::CSSValueRange value);
std::optional<BackgroundPosition> parseBackgroundPosition(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundPositions(detail::CSSValueRange value);
std::optional<BackgroundSize> parseBackgroundSize(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundSizes(detail::CSSValueRange value);
std::optional<BackgroundRepeat> parseBackgroundRepeat(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundRepeats(detail::CSSValueRange value);
std::optional<BackgroundBox> parseBackgroundBox(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundBoxes(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundAttachments(detail::CSSValueRange value);
std::optional<BackgroundAttachment> parseBackgroundAttachment(detail::CSSValueRange value);

std::optional<Length> parsePositionHorizontal(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "left") return Length{0.f};
    if (token == "center") return Length{0.f, .5f};
    if (token == "right") return Length{0.f, 1.f};
    return StyleModel::parseLengthValue(value);
}

std::optional<Length> parsePositionVertical(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "bottom") return Length{0.f};
    if (token == "center") return Length{0.f, .5f};
    if (token == "top") return Length{0.f, 1.f};
    const std::optional<Length> length = StyleModel::parseLengthValue(value);
    if (!length) return std::nullopt;
    return Length{-length->pixels, 1.f - length->percent};
}

bool isPositionToken(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    const auto dimension = detail::parseCSSDimension(value.stream, value.range);
    const auto& tokens = value.stream.tokens();
    const detail::CSSTokenRange trimmed = detail::trimCSSRange(value.stream, value.range);
    const bool scalar = trimmed.end == trimmed.begin + 1
        && trimmed.begin < tokens.size()
        && (tokens[trimmed.begin].kind == detail::CSSTokenKind::Number || tokens[trimmed.begin].kind == detail::CSSTokenKind::Percentage);
    return token == "left"
        || token == "right"
        || token == "top"
        || token == "bottom"
        || token == "center"
        || (dimension && dimension->unit == "px")
        || scalar;
}

std::optional<BackgroundPosition> parseBackgroundPosition(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "top" || token == "bottom") return BackgroundPosition{{0.f, .5f}, *parsePositionVertical({value.stream, tokens.front()})};
        const std::optional<Length> x = parsePositionHorizontal({value.stream, tokens.front()});
        return x ? std::optional<BackgroundPosition>(BackgroundPosition{*x, {0.f, .5f}}) : std::nullopt;
    }

    std::optional<Length> x = parsePositionHorizontal({value.stream, tokens[0]});
    std::optional<Length> y = parsePositionVertical({value.stream, tokens[1]});
    if (!x || !y) {
        x = parsePositionHorizontal({value.stream, tokens[1]});
        y = parsePositionVertical({value.stream, tokens[0]});
    }
    return x && y ? std::optional<BackgroundPosition>(BackgroundPosition{*x, *y}) : std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundPositions(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundPosition> position = parseBackgroundPosition(range);
        if (!position) return std::nullopt;
        BackgroundLayer layer;
        layer.position = *position;
        return layer;
    });
}

std::optional<BackgroundSize> parseBackgroundSize(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "cover") return BackgroundSize{BackgroundSizeType::Cover};
        if (token == "contain") return BackgroundSize{BackgroundSizeType::Contain};
        if (token == "auto") return BackgroundSize{};
    }
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "auto" && normalizeCSSKeyword(value.stream, tokens[1]) == "auto")
        return BackgroundSize{};
    const auto parse = [&value](detail::CSSTokenRange token) -> std::optional<Length> {
        if (normalizeCSSKeyword(value.stream, token) == "auto") return std::optional<Length>{};
        const std::optional<Length> length = StyleModel::parseLengthValue({value.stream, token});
        return length && length->pixels >= 0.f && length->percent >= 0.f ? length : std::nullopt;
    };
    const std::optional<Length> width = parse(tokens[0]);
    const bool heightAuto = tokens.size() == 1 || normalizeCSSKeyword(value.stream, tokens[1]) == "auto";
    const std::optional<Length> height = tokens.size() == 1 ? std::optional<Length>{} : parse(tokens[1]);
    if ((!width && normalizeCSSKeyword(value.stream, tokens[0]) != "auto") || (!height && !heightAuto)) return std::nullopt;
    return BackgroundSize{BackgroundSizeType::Explicit, width, height};
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundSizes(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundSize> size = parseBackgroundSize(range);
        if (!size) return std::nullopt;
        BackgroundLayer layer;
        layer.size = *size;
        return layer;
    });
}

std::optional<BackgroundRepeat> parseBackgroundRepeat(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "repeat") return BackgroundRepeat::Repeat;
        if (token == "no-repeat") return BackgroundRepeat::NoRepeat;
        if (token == "repeat-x") return BackgroundRepeat::RepeatX;
        if (token == "repeat-y") return BackgroundRepeat::RepeatY;
    }
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "repeat" && normalizeCSSKeyword(value.stream, tokens[1]) == "no-repeat")
        return BackgroundRepeat::RepeatX;
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "no-repeat" && normalizeCSSKeyword(value.stream, tokens[1]) == "repeat")
        return BackgroundRepeat::RepeatY;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundRepeats(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat(range);
        if (!repeat) return std::nullopt;
        BackgroundLayer layer;
        layer.repeat = *repeat;
        return layer;
    });
}

std::optional<BackgroundBox> parseBackgroundBox(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "border-box") return BackgroundBox::BorderBox;
    if (token == "padding-box") return BackgroundBox::PaddingBox;
    if (token == "content-box") return BackgroundBox::ContentBox;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundBoxes(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundBox> box = parseBackgroundBox(range);
        if (!box) return std::nullopt;
        BackgroundLayer layer;
        layer.origin = *box;
        layer.clip = *box;
        return layer;
    });
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundAttachments(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::string token = normalizeCSSKeyword(range.stream, range.range);
        BackgroundLayer layer;
        if (token == "scroll") layer.attachment = BackgroundAttachment::Scroll;
        else if (token == "fixed") layer.attachment = BackgroundAttachment::Fixed;
        else if (token == "local") layer.attachment = BackgroundAttachment::Local;
        else return std::nullopt;
        return layer;
    });
}

std::optional<BackgroundAttachment> parseBackgroundAttachment(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "scroll") return BackgroundAttachment::Scroll;
    if (token == "fixed") return BackgroundAttachment::Fixed;
    if (token == "local") return BackgroundAttachment::Local;
    return std::nullopt;
}

void copyLayerComponent(BackgroundLayer& destination, const BackgroundLayer& source, StyleImageComponent component) {
    switch (component) {
        case StyleImageComponent::Image: destination.image = source.image; break;
        case StyleImageComponent::Position: destination.position = source.position; break;
        case StyleImageComponent::Size: destination.size = source.size; break;
        case StyleImageComponent::Repeat: destination.repeat = source.repeat; break;
        case StyleImageComponent::Origin: destination.origin = source.origin; break;
        case StyleImageComponent::Clip: destination.clip = source.clip; break;
        case StyleImageComponent::Attachment: destination.attachment = source.attachment; break;
        case StyleImageComponent::All:
        case StyleImageComponent::Mode:
        case StyleImageComponent::Composite:
        case StyleImageComponent::Type: break;
    }
}

void copyLayerComponent(MaskLayer& destination, const MaskLayer& source, StyleImageComponent component) {
    switch (component) {
        case StyleImageComponent::Image: destination.image.image = source.image.image; break;
        case StyleImageComponent::Position: destination.image.position = source.image.position; break;
        case StyleImageComponent::Size: destination.image.size = source.image.size; break;
        case StyleImageComponent::Repeat: destination.image.repeat = source.image.repeat; break;
        case StyleImageComponent::Origin: destination.image.origin = source.image.origin; break;
        case StyleImageComponent::Clip: destination.image.clip = source.image.clip; break;
        case StyleImageComponent::Attachment: destination.image.attachment = source.image.attachment; break;
        case StyleImageComponent::Mode: destination.mode = source.mode; break;
        case StyleImageComponent::Composite: destination.composite = source.composite; break;
        case StyleImageComponent::Type: destination.type = source.type; break;
        case StyleImageComponent::All: break;
    }
}

template<typename Layer, typename Value>
void applyLayerComponent(std::vector<Layer>& destination, const Value& value, StyleImageComponent component) {
    const auto& source = value.layers;
    if (component == StyleImageComponent::All) {
        destination = source;
        return;
    }
    if (source.empty()) return;
    if (destination.empty()) destination.resize(source.size());
    else if (component == StyleImageComponent::Image && destination.size() != source.size()) {
        const std::size_t previousSize = destination.size();
        std::vector<Layer> normalized(source.size());
        for (std::size_t index = 0; index < normalized.size() && previousSize > 0; ++index) normalized[index] = destination[index % previousSize];
        destination = std::move(normalized);
    } else if (destination.size() < source.size()) {
        const std::size_t previousSize = destination.size();
        destination.resize(source.size());
        for (std::size_t index = previousSize; index < destination.size(); ++index) destination[index] = destination[index % previousSize];
    }
    for (std::size_t index = 0; index < destination.size(); ++index) copyLayerComponent(destination[index], source[index % source.size()], component);
}

void applyBackgroundImages(ComputedStyle& style, const StyleValue& value) {
    const StyleImageLayers& layers = std::get<StyleImageLayers>(value);
    applyLayerComponent(style.backgroundLayers, layers, layers.component);
}

void applyMaskComponent(std::vector<MaskLayer>& destination, const StyleMaskLayers& value) {
    applyLayerComponent(destination, value, value.component);
}

MaskLayer makeMaskLayer(BackgroundLayer image) {
    MaskLayer result;
    result.image = std::move(image);
    result.image.origin = BackgroundBox::BorderBox;
    return result;
}

std::vector<MaskLayer> makeMaskLayers(const std::vector<BackgroundLayer>& images) {
    std::vector<MaskLayer> result;
    result.reserve(images.size());
    for (const BackgroundLayer& image : images) result.push_back(makeMaskLayer(image));
    return result;
}

std::optional<std::vector<MaskLayer>> parseMaskImages(detail::CSSValueRange value, ColorSchemeMode scheme) {
    const std::optional<std::vector<BackgroundLayer>> images =
        parseBackgroundLayerList(value, [scheme](detail::CSSValueRange range) { return parseBackgroundImage(range, scheme); });
    if (!images) return std::nullopt;
    return makeMaskLayers(*images);
}

CompileResult compileBackgroundImage(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundImages({context.value.stream, context.value.range}, context.value.scheme);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileBackgroundPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Position});
}

CompileResult compileBackgroundSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Size});
}

CompileResult compileBackgroundRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Repeat});
}

CompileResult compileBackgroundBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, component});
}

CompileResult compileBackgroundOrigin(detail::StyleCompileContext& context) {
    return compileBackgroundBox(context, StyleImageComponent::Origin);
}

CompileResult compileBackgroundClip(detail::StyleCompileContext& context) {
    return compileBackgroundBox(context, StyleImageComponent::Clip);
}

CompileResult compileBackgroundAttachment(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundAttachments({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Attachment});
}

CompileResult compileMaskImage(detail::StyleCompileContext& context) {
    const auto parsed = parseMaskImages({context.value.stream, context.value.range}, context.value.scheme);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileMaskPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Position});
}

CompileResult compileMaskSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Size});
}

CompileResult compileMaskRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Repeat});
}

CompileResult compileMaskBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), component});
}

CompileResult compileMaskOrigin(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Origin);
}

CompileResult compileMaskClip(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Clip);
}

std::optional<MaskMode> parseMaskMode(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "match-source") return MaskMode::MatchSource;
    if (token == "alpha") return MaskMode::Alpha;
    if (token == "luminance") return MaskMode::Luminance;
    return std::nullopt;
}

std::optional<MaskComposite> parseMaskComposite(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "add") return MaskComposite::Add;
    if (token == "subtract") return MaskComposite::Subtract;
    if (token == "intersect") return MaskComposite::Intersect;
    if (token == "exclude") return MaskComposite::Exclude;
    return std::nullopt;
}

std::optional<MaskType> parseMaskType(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "luminance") return MaskType::Luminance;
    if (token == "alpha") return MaskType::Alpha;
    return std::nullopt;
}

template<typename Enum> CompileResult compileMaskEnum(detail::StyleCompileContext& context, StyleImageComponent component,
                                                      std::optional<Enum> (*parse)(detail::CSSValueRange), Enum MaskLayer::* member) {
    const std::vector<detail::CSSTokenRange> values = context.commaSeparatedRanges();
    if (values.empty()) return context.invalid();
    std::vector<MaskLayer> layers;
    layers.reserve(values.size());
    for (const detail::CSSTokenRange range : values) {
        MaskLayer layer;
        const std::optional<Enum> parsed = parse({context.value.stream, range});
        if (!parsed) return context.invalid();
        layer.*member = *parsed;
        layers.push_back(std::move(layer));
    }
    return context.compiled(StyleMaskLayers{std::move(layers), component});
}

CompileResult compileMaskMode(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Mode, parseMaskMode, &MaskLayer::mode);
}

CompileResult compileMaskComposite(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Composite, parseMaskComposite, &MaskLayer::composite);
}

CompileResult compileMaskType(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Type, parseMaskType, &MaskLayer::type);
}

struct ParsedImageLayer {
    BackgroundLayer image;
    std::vector<BackgroundBox> boxes;
    std::vector<detail::CSSTokenRange> extras;
    bool repeatSpecified = false;
    bool attachmentSpecified = false;
};

std::optional<ParsedImageLayer> parseImageLayer(detail::CSSValueRange value, ColorSchemeMode scheme);

std::optional<ParsedImageLayer> parseImageLayer(detail::CSSValueRange value, ColorSchemeMode scheme) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range, true);
    if (tokens.empty()) return std::nullopt;

    ParsedImageLayer result;
    std::vector<detail::CSSTokenRange> positions;
    std::vector<detail::CSSTokenRange> sizes;
    bool afterSlash = false;
    bool sawImage = false;
    for (std::size_t index = 0; index < tokens.size(); ++index) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens[index]);
        const auto parseRepeat = [&]() -> std::optional<BackgroundRepeat> {
            if (index + 1 < tokens.size() && normalizeCSSKeyword(value.stream, tokens[index + 1]) != "/") {
                const detail::CSSTokenRange combined{tokens[index].begin, tokens[index + 1].end};
                if (const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat({value.stream, combined})) {
                    ++index;
                    return repeat;
                }
            }
            return parseBackgroundRepeat({value.stream, tokens[index]});
        };
        if (token == "/") {
            if (afterSlash) return std::nullopt;
            afterSlash = true;
        } else if (afterSlash) {
            if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
                if (result.repeatSpecified) return std::nullopt;
                result.image.repeat = *repeat;
                result.repeatSpecified = true;
            } else if (const std::optional<BackgroundBox> box = parseBackgroundBox({value.stream, tokens[index]})) result.boxes.push_back(*box);
            else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment({value.stream, tokens[index]})) {
                if (result.attachmentSpecified) return std::nullopt;
                result.image.attachment = *attachment;
                result.attachmentSpecified = true;
            } else if (parseBackgroundSize({value.stream, tokens[index]})) sizes.push_back(tokens[index]);
            else result.extras.push_back(tokens[index]);
        } else if (const std::optional<BackgroundLayer> image = parseBackgroundImage({value.stream, tokens[index]}, scheme)) {
            if (sawImage) return std::nullopt;
            result.image.image = image->image;
            sawImage = true;
        } else if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
            if (result.repeatSpecified) return std::nullopt;
            result.image.repeat = *repeat;
            result.repeatSpecified = true;
        } else if (const std::optional<BackgroundBox> box = parseBackgroundBox({value.stream, tokens[index]})) result.boxes.push_back(*box);
        else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment({value.stream, tokens[index]})) {
            if (result.attachmentSpecified) return std::nullopt;
            result.image.attachment = *attachment;
            result.attachmentSpecified = true;
        } else if (isPositionToken(detail::CSSValueRange{value.stream, tokens[index]})) positions.push_back(tokens[index]);
        else result.extras.push_back(tokens[index]);
    }
    if (positions.size() > 2 || sizes.size() > 2 || (afterSlash && sizes.empty())) return std::nullopt;
    if (!positions.empty()) {
        const detail::CSSTokenRange positionRange = positions.size() > 1 ? detail::CSSTokenRange{positions[0].begin, positions[1].end} : positions[0];
        const std::optional<BackgroundPosition> position = parseBackgroundPosition({value.stream, positionRange});
        if (!position) return std::nullopt;
        result.image.position = *position;
    }
    if (!sizes.empty()) {
        const detail::CSSTokenRange sizeRange = sizes.size() > 1 ? detail::CSSTokenRange{sizes[0].begin, sizes[1].end} : sizes[0];
        const std::optional<BackgroundSize> size = parseBackgroundSize({value.stream, sizeRange});
        if (!size) return std::nullopt;
        result.image.size = *size;
    }
    if (result.boxes.size() > 2) return std::nullopt;
    return result;
}

bool applyImageBoxes(BackgroundLayer& image, const std::vector<BackgroundBox>& boxes, BackgroundBox defaultOrigin, BackgroundBox defaultClip) {
    if (boxes.empty()) {
        image.origin = defaultOrigin;
        image.clip = defaultClip;
    } else if (boxes.size() == 1) image.origin = image.clip = boxes.front();
    else if (boxes.size() == 2) {
        image.origin = boxes[0];
        image.clip = boxes[1];
    } else return false;
    return true;
}

CompileResult compileBackground(detail::StyleCompileContext& context) {
    const std::vector<detail::CSSTokenRange> layers = context.commaSeparatedRanges();
    if (layers.empty()) return context.invalid();
    std::vector<BackgroundLayer> parsed;
    std::optional<CSS::Color> color;
    for (std::size_t layerIndex = 0; layerIndex < layers.size(); ++layerIndex) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer({context.value.stream, layers[layerIndex]}, context.value.scheme);
        if (!parsedLayer || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::PaddingBox, BackgroundBox::BorderBox))
            return context.invalid();
        for (const detail::CSSTokenRange token : parsedLayer->extras) {
            const detail::CSSValueRange value{context.value.stream, token};
            if (const auto parsedColor = context.specifiedColorValue(value)) {
                if (layerIndex + 1 != layers.size() || color) return context.invalid();
                color = *parsedColor;
            } else return context.invalid();
        }
        parsed.push_back(std::move(parsedLayer->image));
    }
    std::vector<StyleDeclaration> declarations;
    declarations.push_back(makeDeclaration("background-color", CSSValue{color.value_or(CSS::Color{Color(0.f, 0.f, 0.f, 0.f)})}));
    const auto imageValue = [&parsed](StyleImageComponent component) { return StyleImageLayers{parsed, component}; };
    declarations.push_back(makeDeclaration("background-image", imageValue(StyleImageComponent::Image)));
    declarations.push_back(makeDeclaration("background-position", imageValue(StyleImageComponent::Position)));
    declarations.push_back(makeDeclaration("background-size", imageValue(StyleImageComponent::Size)));
    declarations.push_back(makeDeclaration("background-repeat", imageValue(StyleImageComponent::Repeat)));
    declarations.push_back(makeDeclaration("background-origin", imageValue(StyleImageComponent::Origin)));
    declarations.push_back(makeDeclaration("background-clip", imageValue(StyleImageComponent::Clip)));
    declarations.push_back(makeDeclaration("background-attachment", imageValue(StyleImageComponent::Attachment)));
    return declarations;
}

CompileResult compileMask(detail::StyleCompileContext& context) {
    const std::vector<detail::CSSTokenRange> layers = context.commaSeparatedRanges();
    if (layers.empty()) return context.invalid();
    std::vector<MaskLayer> parsed;
    for (const detail::CSSTokenRange rawLayer : layers) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer({context.value.stream, rawLayer}, context.value.scheme);
        if (!parsedLayer
            || parsedLayer->attachmentSpecified
            || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::BorderBox, BackgroundBox::BorderBox))
            return context.invalid();
        MaskLayer layer;
        layer.image = std::move(parsedLayer->image);
        bool modeSpecified = false;
        bool compositeSpecified = false;
        for (const detail::CSSTokenRange token : parsedLayer->extras) {
            const detail::CSSValueRange value{context.value.stream, token};
            if (const std::optional<MaskMode> mode = parseMaskMode(value)) {
                if (modeSpecified) return context.invalid();
                layer.mode = *mode;
                modeSpecified = true;
            } else if (const std::optional<MaskComposite> composite = parseMaskComposite(value)) {
                if (compositeSpecified) return context.invalid();
                layer.composite = *composite;
                compositeSpecified = true;
            } else return context.invalid();
        }
        parsed.push_back(std::move(layer));
    }
    std::vector<StyleDeclaration> declarations;
    const auto imageValue = [&parsed](StyleImageComponent component) { return StyleMaskLayers{parsed, component}; };
    declarations.push_back(makeDeclaration("mask-image", imageValue(StyleImageComponent::Image)));
    declarations.push_back(makeDeclaration("mask-mode", imageValue(StyleImageComponent::Mode)));
    declarations.push_back(makeDeclaration("mask-position", imageValue(StyleImageComponent::Position)));
    declarations.push_back(makeDeclaration("mask-size", imageValue(StyleImageComponent::Size)));
    declarations.push_back(makeDeclaration("mask-repeat", imageValue(StyleImageComponent::Repeat)));
    declarations.push_back(makeDeclaration("mask-origin", imageValue(StyleImageComponent::Origin)));
    declarations.push_back(makeDeclaration("mask-clip", imageValue(StyleImageComponent::Clip)));
    declarations.push_back(makeDeclaration("mask-composite", imageValue(StyleImageComponent::Composite)));
    declarations.push_back(makeDeclaration("mask-type", imageValue(StyleImageComponent::Type)));
    return declarations;
}

CompileResult compilePaint(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    if (const std::optional<Gradient> gradient = StyleModel::parseGradient({value.stream, value.range}, value.scheme))
        return context.compiled(StyleImage{*gradient});
    const auto parsed = context.specifiedColorValue();
    if (!parsed) return context.invalid();
    return context.compiled(CSSValue{*parsed});
}

CompileResult compileStrokeLinecap(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    StrokeCap cap;
    return parseStrokeCap({context.value.stream, context.value.range}, cap) ? context.compiled(cap) : context.invalid();
}

CompileResult compileInternalAlignContentBlock(detail::StyleCompileContext& context) {
    const std::string alignment = context.keyword();
    if (alignment == "normal") return context.compiled(false);
    if (alignment == "center") return context.compiled(true);
    return context.invalid();
}

CompileResult compileColorScheme(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty()) return context.invalid();
    if (tokens.size() == 1 && normalizeCSSKeyword(value.stream, tokens.front()) == "normal") return context.compiled(ColorScheme{});

    ColorScheme parsed;
    parsed.normal = false;
    const auto isCustomIdentifier = [&value](detail::CSSTokenRange token) {
        token = detail::trimCSSRange(value.stream, token);
        return token.end == token.begin + 1 && value.stream.tokens()[token.begin].kind == detail::CSSTokenKind::Ident;
    };
    for (const detail::CSSTokenRange token : tokens) {
        const std::string scheme = normalizeCSSKeyword(value.stream, token);
        if (scheme == "only") {
            if (parsed.only) return context.invalid();
            parsed.only = true;
        } else if (scheme == "light") {
            if (std::find(parsed.schemes.begin(), parsed.schemes.end(), ColorSchemeMode::Light) == parsed.schemes.end())
                parsed.schemes.push_back(ColorSchemeMode::Light);
        } else if (scheme == "dark") {
            if (std::find(parsed.schemes.begin(), parsed.schemes.end(), ColorSchemeMode::Dark) == parsed.schemes.end())
                parsed.schemes.push_back(ColorSchemeMode::Dark);
        } else if (scheme == "normal"
                   || scheme == "default"
                   || scheme == "inherit"
                   || scheme == "initial"
                   || scheme == "unset"
                   || scheme == "revert"
                   || scheme == "revert-layer")
            return context.invalid();
        else {
            if (!isCustomIdentifier(token)) return context.invalid();
            parsed.customIdentifiers.push_back(detail::decodeCSSIdentifier(value.stream.text(token.begin)));
        }
    }
    if (parsed.schemes.empty() && parsed.customIdentifiers.empty()) return context.invalid();
    return context.compiled(parsed);
}

CompileResult compileGridArea(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges(true);
    if (tokens.size() != 3 || normalizeCSSKeyword(value.stream, tokens[1]) != "/") return context.invalid();
    const auto line = [&context, &value](detail::CSSTokenRange raw) -> std::optional<int> {
        const std::string token = normalizeCSSKeyword(value.stream, raw);
        if (hasDimensionUnit(value.stream, raw, "px") || endsWith(token, "%")) return std::nullopt;
        const auto parsed = context.number({value.stream, raw});
        if (!parsed || *parsed < 1.f || std::floor(*parsed) != *parsed || *parsed > static_cast<float>(std::numeric_limits<int>::max()))
            return std::nullopt;
        return static_cast<int>(*parsed);
    };
    const auto row = line(tokens[0]);
    const auto column = line(tokens[2]);
    return row && column ? context.compiled(GridArea{*row, *column}) : context.invalid();
}

CompileResult compileCursor(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, CursorStyle>, 35> sCursorValues{{
        {"auto", CursorStyle::Auto},
        {"default", CursorStyle::Default},
        {"pointer", CursorStyle::Pointer},
        {"progress", CursorStyle::Progress},
        {"wait", CursorStyle::Wait},
        {"crosshair", CursorStyle::Crosshair},
        {"text", CursorStyle::Text},
        {"vertical-text", CursorStyle::VerticalText},
        {"alias", CursorStyle::Alias},
        {"copy", CursorStyle::Copy},
        {"move", CursorStyle::Move},
        {"no-drop", CursorStyle::NoDrop},
        {"not-allowed", CursorStyle::NotAllowed},
        {"grab", CursorStyle::Grab},
        {"grabbing", CursorStyle::Grabbing},
        {"col-resize", CursorStyle::ColumnResize},
        {"row-resize", CursorStyle::RowResize},
        {"ew-resize", CursorStyle::EastWestResize},
        {"e-resize", CursorStyle::EastWestResize},
        {"w-resize", CursorStyle::EastWestResize},
        {"ns-resize", CursorStyle::NorthSouthResize},
        {"n-resize", CursorStyle::NorthSouthResize},
        {"s-resize", CursorStyle::NorthSouthResize},
        {"nesw-resize", CursorStyle::NortheastSouthwestResize},
        {"ne-resize", CursorStyle::NortheastSouthwestResize},
        {"sw-resize", CursorStyle::NortheastSouthwestResize},
        {"nwse-resize", CursorStyle::NorthwestSoutheastResize},
        {"nw-resize", CursorStyle::NorthwestSoutheastResize},
        {"se-resize", CursorStyle::NorthwestSoutheastResize},
        {"all-scroll", CursorStyle::AllScroll},
        {"zoom-in", CursorStyle::ZoomIn},
        {"zoom-out", CursorStyle::ZoomOut},
        {"help", CursorStyle::Help},
        {"context-menu", CursorStyle::ContextMenu},
        {"cell", CursorStyle::Cell},
    }};
    const auto consumeCursorKeyword = [](std::string_view normalized) -> std::optional<CursorStyle> {
        const auto found =
            std::find_if(sCursorValues.begin(), sCursorValues.end(), [&normalized](const auto& entry) { return entry.first == normalized; });
        return found == sCursorValues.end() ? std::nullopt : std::optional<CursorStyle>(found->second);
    };

    const std::vector<detail::CSSTokenRange> candidates = context.commaSeparatedRanges();
    if (candidates.size() == 1) {
        const std::optional<CursorStyle> keyword = consumeCursorKeyword(normalizeCSSKeyword(value.stream, candidates.front()));
        return keyword ? context.compiled(CursorValue{*keyword, {}}) : context.invalid();
    }
    const auto parseHotspot = [&value](detail::CSSTokenRange range) -> std::optional<float> {
        range = detail::trimCSSRange(value.stream, range);
        if (range.end != range.begin + 1 || value.stream.tokens()[range.begin].kind != detail::CSSTokenKind::Number) return std::nullopt;
        const float parsed = StyleModel::parseNumberValue({value.stream, range}, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    };
    std::vector<CursorImage> images;
    for (std::size_t index = 0; index + 1 < candidates.size(); ++index) {
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, candidates[index]);
        if (tokens.size() != 1 && tokens.size() != 3) return context.invalid();
        const std::optional<std::string> resource = detail::parseCSSUrl(value.stream, tokens.front());
        if (!resource) return context.invalid();
        CursorImage image;
        image.resource = *resource;
        if (tokens.size() > 1) image.hotspotX = parseHotspot(tokens[1]);
        if (tokens.size() > 2) image.hotspotY = parseHotspot(tokens[2]);
        if ((tokens.size() > 1 && !image.hotspotX) || (tokens.size() > 2 && !image.hotspotY)) return context.invalid();
        images.push_back(std::move(image));
    }
    const std::optional<CursorStyle> fallback = consumeCursorKeyword(normalizeCSSKeyword(value.stream, candidates.back()));
    return fallback && !images.empty() ? context.compiled(CursorValue{*fallback, std::move(images)}) : context.invalid();
}

CompileResult compileNonnegativeLength(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.nonnegativeLength();
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileUnitlessNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string raw = context.keyword();
    if (hasDimensionUnit(value.stream, value.range, "px") || endsWith(raw, "%")) return context.invalid();
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileStrokeWidth(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(Length{*parsed}) : context.invalid();
}

} // namespace

namespace {
bool containsLightDark(const detail::CSSTokenStream& stream, detail::CSSTokenRange range) {
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const detail::CSSToken& token = stream.tokens()[index];
        if (token.kind != detail::CSSTokenKind::Function || token.matching == detail::kNoMatchingCSSToken || token.matching >= range.end) continue;
        const auto function = detail::parseCSSFunction(stream, {index, token.matching + 1});
        if (function && function->name == "light-dark") return true;
    }
    return false;
}
} // namespace

std::optional<std::vector<StyleDeclaration>> StyleModel::compileDeclaration(std::string_view property, const detail::CSSTokenStream& stream,
                                                                            detail::CSSTokenRange valueRange, const std::string& selector,
                                                                            StyleSheetLoadResult& result, const std::string& sourceName,
                                                                            ColorSchemeMode scheme, bool resolveColorFunctions) {
    valueRange = detail::trimCSSRange(stream, valueRange);
    const std::string value = detail::trim(detail::serializeCSSRange(stream, valueRange));
    const std::string normalizedValue = detail::normalizeCSSKeyword(stream, valueRange);
    if (normalizedValue == "initial") return makeDeclarations(property, InitialStyleValue{});
    if (normalizedValue == "inherit" || normalizedValue == "unset") {
        const StyleWideKeyword keyword = normalizedValue == "inherit" ? StyleWideKeyword::Inherit : StyleWideKeyword::Unset;
        return makeDeclarations(property, keyword);
    }
    if (!resolveColorFunctions && containsLightDark(stream, valueRange)) {
        if (!compileDeclaration(property, stream, valueRange, selector, result, sourceName, scheme, true)) return std::nullopt;
        return std::vector<StyleDeclaration>{makeDeclaration(property, DeferredStyleValue{value})};
    }
    detail::StyleCompileContext context{property, StyleCompileValue{value, stream, valueRange, scheme}, selector, result, sourceName};
    if (const auto id = findProperty(property)) {
        detail::CSSValueRange range{stream, valueRange};
        ParsedProperties parsed;
        if (parseProperty(*id, range, parsed)) {
            std::vector<StyleDeclaration> declarations;
            declarations.reserve(parsed.size());
            for (const ParsedLonghand& longhand : parsed) {
                if (!longhand.property) return context.invalid();
                if (*longhand.property == CSSProperty::BorderImageSource) {
                    ComputedStyle validationStyle = ComputedStyle::initialStyle();
                    validationStyle.usedColorScheme = scheme;
                    const StyleBuilderContext conversionContext{validationStyle, nullptr, ComputedStyle::initialStyle()};
                    const auto image = toStyle<StyleImage>(conversionContext, longhand.value);
                    if (!image) return context.invalid();
                    declarations.emplace_back(cssPropertyName(*longhand.property), *image);
                    continue;
                }
                if (*longhand.property == CSSProperty::RowGap || *longhand.property == CSSProperty::ColumnGap) {
                    const ComputedStyle& initialStyle = ComputedStyle::initialStyle();
                    const StyleBuilderContext conversionContext{initialStyle};
                    const auto gap = toStyle<GapGutter>(conversionContext, longhand.value);
                    if (!gap) return context.invalid();
                    declarations.emplace_back(cssPropertyName(*longhand.property), *gap);
                    continue;
                }
                declarations.emplace_back(cssPropertyName(*longhand.property), longhand.value);
            }
            return declarations;
        }
    }

    const detail::StylePropertyDefinition* legacy = detail::findLegacyProperty(property);
    if (legacy && legacy->compile) return legacy->compile(context);
    return context.invalid();
}

namespace {

template<auto Member> void applyMember(ComputedStyle& style, const StyleValue& value) {
    using Value = std::decay_t<decltype(style.*Member)>;
    style.*Member = std::get<Value>(value);
}

template<auto Member> void copyMember(ComputedStyle& style, const ComputedStyle& parent) {
    style.*Member = parent.*Member;
}

void specifyPointerEvents(ComputedStyle& style) {
    style.pointerEventsSpecified = true;
}

void copyPointerEvents(ComputedStyle& style, const ComputedStyle& parent) {
    style.setPointerEvents(parent.pointerEvents());
    style.pointerEventsSpecified = parent.pointerEventsSpecified;
}

void copyCursor(ComputedStyle& style, const ComputedStyle& parent) {
    style.cursor = parent.cursor;
    style.cursorImages = parent.cursorImages;
}

void copyStrokeLinecap(ComputedStyle& style, const ComputedStyle& parent) {
    style.svgStrokeCap = parent.svgStrokeCap;
    style.svgStrokeCapSet = parent.svgStrokeCapSet;
}

template<auto Member> void resetMember(ComputedStyle& style) {
    const ComputedStyle initial;
    style.*Member = initial.*Member;
}

void resetBackgroundImages(ComputedStyle& style) {
    style.backgroundLayers = {BackgroundLayer{}};
}

void resetBackgroundPositions(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.position = {};
}

void resetBackgroundSizes(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.size = {};
}

void resetBackgroundRepeats(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.repeat = BackgroundRepeat::Repeat;
}

void resetBackgroundOrigins(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.origin = BackgroundBox::PaddingBox;
}

void resetBackgroundClips(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.clip = BackgroundBox::BorderBox;
}

void resetBackgroundAttachments(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.attachment = BackgroundAttachment::Scroll;
}

void resetMaskImages(ComputedStyle& style) {
    style.maskLayers = {MaskLayer{}};
}

void resetMaskPositions(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.position = {};
}

void resetMaskSizes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.size = {};
}

void resetMaskRepeats(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.repeat = BackgroundRepeat::Repeat;
}

void resetMaskOrigins(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.origin = BackgroundBox::BorderBox;
}

void resetMaskClips(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.clip = BackgroundBox::BorderBox;
}

void resetMaskModes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.mode = MaskMode::MatchSource;
}

void resetMaskComposites(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.composite = MaskComposite::Add;
}

void resetMaskTypes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.type = MaskType::Alpha;
}

void resetCursor(ComputedStyle& style) {
    style.cursor = CursorStyle::Auto;
    style.cursorImages.clear();
}

void resetStrokeLinecap(ComputedStyle& style) {
    const ComputedStyle initial;
    style.svgStrokeCap = initial.svgStrokeCap;
    style.svgStrokeCapSet = true;
}

void applyMask(ComputedStyle& style, const StyleValue& value) {
    applyMaskComponent(style.maskLayers, std::get<StyleMaskLayers>(value));
}
void applyCursor(ComputedStyle& style, const StyleValue& value) {
    const CursorValue& cursor = std::get<CursorValue>(value);
    style.cursor = cursor.style;
    style.cursorImages = cursor.images;
}
void applyOutline(ComputedStyle& style, const StyleValue& value) {
    const Outline& outline = std::get<Outline>(value);
    Outline& target = style.outline();
    target.width = outline.width;
    target.color = outline.color;
    target.style = outline.style;
}
void resetOutline(ComputedStyle& style) {
    style.outline() = Outline{.offset = ComputedStyle::initialOutlineOffset()};
}
void inheritOutline(ComputedStyle& style, const ComputedStyle& parent) {
    style.outline() = parent.outline();
}
void applyStrokeLinecap(ComputedStyle& style, const StyleValue& value) {
    style.svgStrokeCap = std::get<StrokeCap>(value);
    style.svgStrokeCapSet = true;
}
void applyStrokeWidth(ComputedStyle& style, const StyleValue& value) {
    style.svgStrokeWidth = std::get<Length>(value);
}

void applyStroke(ComputedStyle& style, const StyleValue& value) {
    if (const auto color = styleColorValue(style, value)) style.stroke = *color;
    else if (const auto* image = std::get_if<StyleImage>(&value)) style.stroke = *image;
}

const detail::StylePropertyDefinition kLegacyPropertyDefinitions[] = {
    {"accent-color", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"appearance", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"color-scheme", compileColorScheme, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"box-sizing", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"background", compileBackground, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint, false, false},
    {"background-color", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"background-image", compileBackgroundImage, applyBackgroundImages, resetBackgroundImages, nullptr, copyMember<&ComputedStyle::backgroundLayers>,
     StylePropertyImpact::Paint},
    {"background-position", compileBackgroundPosition, applyBackgroundImages, resetBackgroundPositions, nullptr,
     copyMember<&ComputedStyle::backgroundLayers>, StylePropertyImpact::Paint},
    {"background-size", compileBackgroundSize, applyBackgroundImages, resetBackgroundSizes, nullptr, copyMember<&ComputedStyle::backgroundLayers>,
     StylePropertyImpact::Paint},
    {"background-repeat", compileBackgroundRepeat, applyBackgroundImages, resetBackgroundRepeats, nullptr,
     copyMember<&ComputedStyle::backgroundLayers>, StylePropertyImpact::Paint},
    {"background-origin", compileBackgroundOrigin, applyBackgroundImages, resetBackgroundOrigins, nullptr,
     copyMember<&ComputedStyle::backgroundLayers>, StylePropertyImpact::Paint},
    {"background-clip", compileBackgroundClip, applyBackgroundImages, resetBackgroundClips, nullptr, copyMember<&ComputedStyle::backgroundLayers>,
     StylePropertyImpact::Paint},
    {"background-attachment", compileBackgroundAttachment, applyBackgroundImages, resetBackgroundAttachments, nullptr,
     copyMember<&ComputedStyle::backgroundLayers>, StylePropertyImpact::Paint},
    {"border", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-bottom", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-bottom-left-radius", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-bottom-right-radius", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-color", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image-outset", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image-repeat", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image-slice", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image-source", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-image-width", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-left", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-right", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-radius", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-top", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-top-left-radius", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-top-right-radius", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-style", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-width", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"bottom", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"cursor", compileCursor, applyCursor, resetCursor, nullptr, copyCursor, StylePropertyImpact::Paint, false, true},
    {"display", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"backdrop-filter", compileFilter, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"filter", compileFilter, applyMember<&ComputedStyle::filter>, resetMember<&ComputedStyle::filter>, nullptr, copyMember<&ComputedStyle::filter>,
     StylePropertyImpact::Paint},
    {"left", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"mask", compileMask, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint, false, false},
    {"mask-image", compileMaskImage, applyMask, resetMaskImages, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-mode", compileMaskMode, applyMask, resetMaskModes, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-position", compileMaskPosition, applyMask, resetMaskPositions, nullptr, copyMember<&ComputedStyle::maskLayers>,
     StylePropertyImpact::Paint},
    {"mask-size", compileMaskSize, applyMask, resetMaskSizes, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-repeat", compileMaskRepeat, applyMask, resetMaskRepeats, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-origin", compileMaskOrigin, applyMask, resetMaskOrigins, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-clip", compileMaskClip, applyMask, resetMaskClips, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"mask-composite", compileMaskComposite, applyMask, resetMaskComposites, nullptr, copyMember<&ComputedStyle::maskLayers>,
     StylePropertyImpact::Paint},
    {"mask-type", compileMaskType, applyMask, resetMaskTypes, nullptr, copyMember<&ComputedStyle::maskLayers>, StylePropertyImpact::Paint},
    {"outline", compileOutline, applyOutline, resetOutline, nullptr, inheritOutline, StylePropertyImpact::Paint},
    {"outline-offset", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"overflow", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-x", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-y", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"pointer-events", nullptr, nullptr, nullptr, specifyPointerEvents, copyPointerEvents, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"position", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"right", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-color", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"scrollbar-gutter", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-width", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"box-shadow", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint},
    {"top", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"translate", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"align-items"},
    {"align-content"},
    {"place-items", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"place-self", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"flex-direction", nullptr, nullptr, nullptr, nullptr, nullptr},
    {"flex-wrap", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"flex-flow", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"gap", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"row-gap", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"column-gap", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"grid-area", compileGridArea, [](ComputedStyle& style, const StyleValue& value) { style.gridArea = std::get<GridArea>(value); },
     resetMember<&ComputedStyle::gridArea>, nullptr, copyMember<&ComputedStyle::gridArea>,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"justify-content"},
    {"justify-self", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"justify-items", nullptr, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"-internal-align-content-block", compileInternalAlignContentBlock, applyMember<&ComputedStyle::alignContentBlockCenter>,
     resetMember<&ComputedStyle::alignContentBlockCenter>, nullptr, copyMember<&ComputedStyle::alignContentBlockCenter>, StylePropertyImpact::Layout,
     true},
    {"align-self"},
    {"flex", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"text-decoration", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint | StylePropertyImpact::Descendant},
    {"content", compileContent, applyMember<&ComputedStyle::content>, resetMember<&ComputedStyle::content>, nullptr,
     copyMember<&ComputedStyle::content>, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"text-align", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"color", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint, false, true},
    {"text-overflow", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"text-wrap-mode", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"text-wrap-style", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout},
    {"visibility", nullptr, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"stroke", compilePaint, applyStroke, resetMember<&ComputedStyle::stroke>, nullptr, copyMember<&ComputedStyle::stroke>,
     StylePropertyImpact::Paint},
    {"stroke-linecap", compileStrokeLinecap, applyStrokeLinecap, resetStrokeLinecap, nullptr, copyStrokeLinecap, StylePropertyImpact::Paint},
    {"stroke-width", compileStrokeWidth, applyStrokeWidth, resetMember<&ComputedStyle::svgStrokeWidth>, nullptr,
     copyMember<&ComputedStyle::svgStrokeWidth>, StylePropertyImpact::Paint},
};
} // namespace

namespace detail {
const StylePropertyDefinition* findLegacyProperty(std::string_view name) {
    const auto found = std::find_if(std::begin(kLegacyPropertyDefinitions), std::end(kLegacyPropertyDefinitions),
                                    [name](const StylePropertyDefinition& property) { return property.name == name; });
    return found == std::end(kLegacyPropertyDefinitions) ? nullptr : found;
}

const StylePropertyDefinition* legacyPropertyBegin() {
    return std::begin(kLegacyPropertyDefinitions);
}
const StylePropertyDefinition* legacyPropertyEnd() {
    return std::end(kLegacyPropertyDefinitions);
}

bool isPropertyInherited(std::string_view name) {
    if (const auto id = findProperty(name)) return isInheritedProperty(*id);
    if (const auto* property = findLegacyProperty(name)) return property->inherited;
    return false;
}

bool isPropertyPaintOnly(std::string_view name) {
    if (const auto* property = findLegacyProperty(name)) return property->isPaintOnly();
    return false;
}

bool propertyAffectsHitTesting(std::string_view name) {
    if (const auto* property = findLegacyProperty(name)) return property->affectsHitTesting();
    return false;
}

bool propertyPropagatesToDescendants(std::string_view name) {
    if (isPropertyInherited(name)) return true;
    if (const auto* property = findLegacyProperty(name)) return hasImpact(property->impact, StylePropertyImpact::Descendant);
    return false;
}
} // namespace detail
} // namespace radia::ui
