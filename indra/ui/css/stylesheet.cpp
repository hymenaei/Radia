/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "css/stylesheet.h"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <tuple>
#include <utility>
#include <variant>
#include "ComputedStyleProperties.h"
#include "css/rules.h"
#include "style/property.h"

namespace radia::ui {
namespace {
bool explicitlyInherits(const ComputedStyle& style, std::string_view propertyName) {
    return std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        != style.explicitlyInheritedProperties.end();
}

bool specifiedHere(const ComputedStyle& style, std::string_view propertyName) {
    return std::find(style.specifiedProperties.begin(), style.specifiedProperties.end(), propertyName) != style.specifiedProperties.end();
}

bool shouldInherit(const ComputedStyle& style, std::string_view property) {
    return explicitlyInherits(style, property) || (detail::isPropertyInherited(property) && !specifiedHere(style, property));
}
} // namespace

void resolveStyleColors(ComputedStyle& style, const Color& inheritedColor) {
    StyleColor color = style.color();
    color.resolve(inheritedColor, style.usedColorScheme);
    style.setColor(std::move(color));
    const Color currentColor = style.color().resolvedColor();
    StyleColor backgroundColor = style.backgroundColor();
    backgroundColor.resolve(currentColor, style.usedColorScheme);
    style.setBackgroundColor(std::move(backgroundColor));
    RectEdges<StyleColor> borderColors = style.borderColor();
    borderColors.top.resolve(currentColor, style.usedColorScheme);
    borderColors.right.resolve(currentColor, style.usedColorScheme);
    borderColors.bottom.resolve(currentColor, style.usedColorScheme);
    borderColors.left.resolve(currentColor, style.usedColorScheme);
    style.setBorderColor(std::move(borderColors));
    if (auto* color = std::get_if<StyleColor>(&style.stroke)) color->resolve(currentColor, style.usedColorScheme);
    AccentColor accent = style.accentColor();
    if (!accent.isKeyword()) accent.value().resolve(currentColor, style.usedColorScheme);
    style.setAccentColor(std::move(accent));
    ScrollbarColor scrollbarColor = style.scrollbarColor();
    scrollbarColor.thumb.resolve(currentColor, style.usedColorScheme);
    scrollbarColor.track.resolve(currentColor, style.usedColorScheme);
    style.setScrollbarColor(std::move(scrollbarColor));
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (Gradient* gradient = layer.image.gradient())
            for (GradientStop& stop : gradient->stops) stop.color.resolve(currentColor, style.usedColorScheme);
    for (MaskLayer& layer : style.maskLayers)
        if (Gradient* gradient = layer.image.image.gradient())
            for (GradientStop& stop : gradient->stops) stop.color.resolve(currentColor, style.usedColorScheme);
    if (auto* image = std::get_if<StyleImage>(&style.stroke); image && image->gradient()) {
        Gradient* gradient = image->gradient();
        for (GradientStop& stop : gradient->stops) stop.color.resolve(currentColor, style.usedColorScheme);
    }
    BoxShadows shadows = style.boxShadow();
    for (BoxShadow& shadow : shadows) shadow.color.resolve(currentColor, style.usedColorScheme);
    style.setBoxShadow(std::move(shadows));
    style.outline().color.resolve(currentColor, style.usedColorScheme);
}

void normalizeOverflow(ComputedStyle& style) {
    const auto scrollable = [](Overflow value) { return value == Overflow::Hidden || value == Overflow::Scroll || value == Overflow::Auto; };
    if (scrollable(style.overflowY())) {
        if (style.overflowX() == Overflow::Visible) style.setOverflowX(Overflow::Auto);
        else if (style.overflowX() == Overflow::Clip) style.setOverflowX(Overflow::Hidden);
    }
    if (scrollable(style.overflowX())) {
        if (style.overflowY() == Overflow::Visible) style.setOverflowY(Overflow::Auto);
        else if (style.overflowY() == Overflow::Clip) style.setOverflowY(Overflow::Hidden);
    }
}

void inheritStyle(ComputedStyle& style, const ComputedStyle& parent) {
    StyleBuilderState builderState{style, &parent};
    for (const detail::StylePropertyDefinition* property = detail::legacyPropertyBegin(); property != detail::legacyPropertyEnd(); ++property)
        if (property->inherit && shouldInherit(style, property->name)) property->inherit(style, parent);

    for (std::size_t index = 0; index < static_cast<std::size_t>(CSSProperty::Count); ++index) {
        const CSSProperty property = static_cast<CSSProperty>(index);
        const std::string_view name = cssPropertyName(property);
        const auto* legacy = detail::findLegacyProperty(name);
        if ((legacy && legacy->inherit) || !shouldInherit(style, name)) continue;
        applyProperty(property, builderState, ApplyType::Inherit);
    }
    style.specifiedProperties.clear();
    style.explicitlyInheritedProperties.clear();
    style.textDecorationPropagation =
        static_cast<TextDecoration>(static_cast<unsigned>(parent.textDecorationPropagation) | static_cast<unsigned>(style.textDecoration()));
}

void applyOpacity(ComputedStyle& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity().value;
    auto applyAlpha = [opacity](StyleColor color) {
        color.resolvedColor().a *= opacity;
        return color;
    };
    style.setBackgroundColor(applyAlpha(style.backgroundColor()));
    RectEdges<StyleColor> borderColors = style.borderColor();
    borderColors.top.resolvedColor().a *= opacity;
    borderColors.right.resolvedColor().a *= opacity;
    borderColors.bottom.resolvedColor().a *= opacity;
    borderColors.left.resolvedColor().a *= opacity;
    style.setBorderColor(std::move(borderColors));
    StyleColor color = style.color();
    color.resolvedColor().a *= opacity;
    style.setColor(std::move(color));
    if (auto* color = std::get_if<StyleColor>(&style.stroke)) color->resolvedColor().a *= opacity;
    style.outline().color.resolvedColor().a *= opacity;
    ScrollbarColor scrollbarColor = style.scrollbarColor();
    if (!scrollbarColor.automatic) {
        scrollbarColor.thumb.resolvedColor().a *= opacity;
        scrollbarColor.track.resolvedColor().a *= opacity;
        style.setScrollbarColor(std::move(scrollbarColor));
    }
    BoxShadows shadows = style.boxShadow();
    for (BoxShadow& shadow : shadows) shadow.color.resolvedColor().a *= opacity;
    style.setBoxShadow(std::move(shadows));
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (Gradient* gradient = layer.image.gradient())
            for (GradientStop& stop : gradient->stops) stop.color.resolvedColor().a *= opacity;
    if (auto* image = std::get_if<StyleImage>(&style.stroke); image && image->gradient()) {
        Gradient* gradient = image->gradient();
        for (GradientStop& stop : gradient->stops) stop.color.resolvedColor().a *= opacity;
    }
    style.setOpacity(Opacity{opacity});
}

std::shared_ptr<StyleSheet::Impl> StyleSheet::makeEmptyImpl() {
    return std::make_shared<Impl>();
}

StyleSheet::StyleSheet() : mImpl(makeEmptyImpl()) {}
StyleSheet::~StyleSheet() = default;
StyleSheet::StyleSheet(const StyleSheet& other) : mImpl(other.mImpl ? other.mImpl : makeEmptyImpl()), mFontFaces(other.mFontFaces) {}
StyleSheet& StyleSheet::operator=(const StyleSheet& other) {
    if (this != &other) {
        auto replacement = other.mImpl ? std::make_shared<Impl>(*other.mImpl) : makeEmptyImpl();
        auto replacementFontFaces = other.mFontFaces;
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
        mFontFaces = std::move(replacementFontFaces);
    }
    return *this;
}
StyleSheet::StyleSheet(StyleSheet&& other) noexcept : mImpl(std::move(other.mImpl)) {
    if (!mImpl) mImpl = makeEmptyImpl();
    mFontFaces = std::move(other.mFontFaces);
    other.mImpl = makeEmptyImpl();
}
StyleSheet& StyleSheet::operator=(StyleSheet&& other) noexcept {
    if (this != &other) {
        auto replacement = std::move(other.mImpl);
        if (!replacement) replacement = makeEmptyImpl();
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
        mFontFaces = std::move(other.mFontFaces);
        other.mImpl = makeEmptyImpl();
    }
    return *this;
}

std::uint64_t StyleSheet::generation() const {
    return mImpl->generation;
}

const std::vector<FontFace>& StyleSheet::fontFaces() const {
    return mFontFaces;
}

std::vector<const FontFace*> fontFacesInMatchOrder(const std::vector<FontFace>& faces, const std::string& family,
                                                   const FontSelectionRequest& request) {
    using Rank = std::tuple<int, float, int, int, float>;
    const auto widthRank = [&request](float width) {
        const bool preferred = request.width.percentage <= 100.f ? width <= request.width.percentage : width >= request.width.percentage;
        return std::pair{preferred ? 0 : 1, std::abs(width - request.width.percentage)};
    };
    const auto styleRank = [&request](FontStyle style) {
        if (style == request.style) return 0;
        if (request.style == FontStyle::Normal) return style == FontStyle::Oblique ? 1 : 2;
        if (request.style == FontStyle::Italic) return style == FontStyle::Oblique ? 1 : 2;
        return style == FontStyle::Italic ? 1 : 2;
    };
    const auto weightRank = [&request](float weight) {
        const float desired = request.weight.value;
        if (desired >= 400.f && desired <= 500.f) {
            if (weight == desired) return std::pair{0, 0.f};
            if (weight > desired && weight <= 500.f) return std::pair{1, weight - desired};
            if (weight < desired) return std::pair{2, desired - weight};
            return std::pair{3, weight - 500.f};
        }
        if (desired < 400.f) return weight <= desired ? std::pair{0, desired - weight} : std::pair{1, weight - desired};
        return weight >= desired ? std::pair{0, weight - desired} : std::pair{1, desired - weight};
    };

    const auto rank = [&](const FontFace& face) {
        const auto [widthDirection, widthDistance] = widthRank(face.selection.width.percentage);
        const auto [weightDirection, weightDistance] = weightRank(face.selection.weight.value);
        return Rank{widthDirection, widthDistance, styleRank(face.selection.style), weightDirection, weightDistance};
    };

    std::vector<const FontFace*> candidates;
    for (auto face = faces.rbegin(); face != faces.rend(); ++face) {
        if (LLStringUtil::compareInsensitive(face->family, family) != 0) continue;
        candidates.push_back(&*face);
    }
    std::stable_sort(candidates.begin(), candidates.end(), [&](const FontFace* left, const FontFace* right) { return rank(*left) < rank(*right); });
    return candidates;
}

const StyleRuleSet* StyleSheet::ruleSetIdentity() const {
    return &mImpl->ruleSet;
}

const StyleSheet::DependencyMap& StyleSheet::dependencies() const {
    return mImpl->ruleSet.dependencies();
}

const std::vector<StyleResourceReference>& StyleSheet::resourceReferences() const {
    return mImpl->ruleSet.resourceReferences();
}

bool StyleSheet::pseudoClassAffectsLayout(CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsLayout(pseudoClass);
}

bool StyleSheet::pseudoClassAffectsLayout(const Element& element, CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsLayout(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsHitTesting(CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsHitTesting(pseudoClass);
}

bool StyleSheet::pseudoClassAffectsHitTesting(const Element& element, CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsHitTesting(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsDescendants(const Element& element, CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsDescendants(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsFollowingSiblings(const Element& element, CSSPseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsFollowingSiblings(element, pseudoClass);
}

StyleRuleSet StyleModel::build() && {
    return StyleRuleSet(std::move(*this));
}
} // namespace radia::ui
