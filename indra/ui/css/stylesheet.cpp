/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "css/stylesheet.h"
#include <algorithm>
#include <utility>
#include "css/defaults.inc"
#include "css/rules.h"
#include "style/property.h"

namespace radia::ui {
namespace {
Color resolveLightDarkColor(const LightDarkColor& colors, ColorScheme scheme) {
    return scheme == ColorScheme::Light ? colors.light : colors.dark;
}

void resolveLightDarkColor(Color& color, std::optional<LightDarkColor>& colors, ColorScheme scheme) {
    if (colors) color = resolveLightDarkColor(*colors, scheme);
}

bool explicitlyInherits(const ComputedStyle& style, std::string_view propertyName) {
    return std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        != style.explicitlyInheritedProperties.end();
}
} // namespace

void resolveLightDarkColors(ComputedStyle& style) {
    resolveLightDarkColor(style.backgroundColor, style.backgroundColorLightDark, style.colorScheme);
    resolveLightDarkColor(style.borderColor, style.borderColorLightDark, style.colorScheme);
    resolveLightDarkColor(style.color, style.colorLightDark, style.colorScheme);
    resolveLightDarkColor(style.strokeColor, style.strokeColorLightDark, style.colorScheme);
    if (style.accentColor.lightDarkColor) style.accentColor.color = resolveLightDarkColor(*style.accentColor.lightDarkColor, style.colorScheme);
    if (style.scrollbarColor.thumbLightDarkColor)
        style.scrollbarColor.thumb = resolveLightDarkColor(*style.scrollbarColor.thumbLightDarkColor, style.colorScheme);
    if (style.scrollbarColor.trackLightDarkColor)
        style.scrollbarColor.track = resolveLightDarkColor(*style.scrollbarColor.trackLightDarkColor, style.colorScheme);
    if (style.backgroundGradient)
        for (GradientStop& stop : style.backgroundGradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    if (style.strokeGradient)
        for (GradientStop& stop : style.strokeGradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (layer.gradient)
            for (GradientStop& stop : layer.gradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    for (MaskLayer& layer : style.maskLayers)
        if (layer.image.gradient)
            for (GradientStop& stop : layer.image.gradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    if (style.borderGradient)
        for (GradientStop& stop : style.borderGradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    for (BoxShadow& shadow : style.shadows) resolveLightDarkColor(shadow.color, shadow.lightDarkColor, style.colorScheme);
    resolveLightDarkColor(style.outline.color, style.outline.lightDarkColor, style.colorScheme);
}

void resolveCurrentColors(ComputedStyle& style) {
    if (style.backgroundColorCurrent) {
        style.backgroundColor = style.color;
        style.backgroundColorLightDark.reset();
        style.backgroundGradient.reset();
    }
    if (style.borderColorCurrent) {
        style.borderColor = style.color;
        style.borderColorLightDark.reset();
        style.borderGradient.reset();
    }
    if (style.strokeColorCurrent) {
        style.strokeColor = style.color;
        style.strokeColorLightDark.reset();
        style.strokeGradient.reset();
    }
    for (BoxShadow& shadow : style.shadows)
        if (shadow.currentColor) shadow.color = style.color;
}

void normalizeOverflow(ComputedStyle& style) {
    const auto scrollable = [](Overflow value) { return value == Overflow::Hidden || value == Overflow::Scroll || value == Overflow::Auto; };
    if (style.overflowX == Overflow::Visible && scrollable(style.overflowY)) style.overflowX = Overflow::Auto;
    if (style.overflowY == Overflow::Visible && scrollable(style.overflowX)) style.overflowY = Overflow::Auto;
}

void resolveRelativeFontWeight(ComputedStyle& style, U16 inheritedWeight) {
    if (!style.fontWeightAdjustment) return;
    const bool lighter = style.fontWeightAdjustment->lighter;
    const U16 weight = inheritedWeight;
    if (lighter) style.fontWeight = weight < 100 ? weight : weight < 550 ? 100 : weight < 750 ? 400 : 700;
    else style.fontWeight = weight < 350 ? 400 : weight < 550 ? 700 : weight < 900 ? 900 : weight;
    style.fontWeightAdjustment.reset();
}

void resolvePercentageLineHeight(ComputedStyle& style) {
    if (style.lineHeight.kind != LineHeight::Kind::Percentage) return;
    style.lineHeight = {LineHeight::Kind::Length, style.fontSize * style.lineHeight.value};
}

void inheritStyle(ComputedStyle& style, const ComputedStyle& parent) {
    for (const detail::StylePropertyDefinition* property = detail::stylePropertyBegin(); property != detail::stylePropertyEnd(); ++property)
        if (property->inherit && (property->isInherited() || explicitlyInherits(style, property->name))) property->inherit(style, parent);
    style.specifiedInheritedProperties |= parent.specifiedInheritedProperties;
    style.explicitlyInheritedProperties.clear();
    style.textDecorationPropagation =
        static_cast<TextDecoration>(static_cast<unsigned>(parent.textDecorationPropagation) | static_cast<unsigned>(style.textDecoration));
    resolveRelativeFontWeight(style, parent.fontWeight);
}

void applyOpacity(ComputedStyle& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity;
    style.backgroundColor.a *= opacity;
    style.borderColor.a *= opacity;
    style.color.a *= opacity;
    style.strokeColor.a *= opacity;
    style.outline.color.a *= opacity;
    if (!style.scrollbarColor.automatic) {
        style.scrollbarColor.thumb.a *= opacity;
        style.scrollbarColor.track.a *= opacity;
    }
    for (BoxShadow& shadow : style.shadows) shadow.color.a *= opacity;
    if (style.backgroundGradient)
        for (GradientStop& stop : style.backgroundGradient->stops) stop.color.a *= opacity;
    if (style.strokeGradient)
        for (GradientStop& stop : style.strokeGradient->stops) stop.color.a *= opacity;
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (layer.gradient)
            for (GradientStop& stop : layer.gradient->stops) stop.color.a *= opacity;
    if (style.borderGradient)
        for (GradientStop& stop : style.borderGradient->stops) stop.color.a *= opacity;
    style.opacity = opacity;
}

std::shared_ptr<StyleSheet::Impl> StyleSheet::makeEmptyImpl() {
    return std::make_shared<Impl>();
}

StyleSheet::StyleSheet() : mImpl(makeEmptyImpl()) {}
StyleSheet::~StyleSheet() = default;
StyleSheet::StyleSheet(const StyleSheet& other) : mImpl(other.mImpl ? other.mImpl : makeEmptyImpl()) {}
StyleSheet& StyleSheet::operator=(const StyleSheet& other) {
    if (this != &other) {
        auto replacement = other.mImpl ? std::make_shared<Impl>(*other.mImpl) : makeEmptyImpl();
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
    }
    return *this;
}
StyleSheet::StyleSheet(StyleSheet&& other) noexcept : mImpl(std::move(other.mImpl)) {
    if (!mImpl) mImpl = makeEmptyImpl();
    other.mImpl = makeEmptyImpl();
}
StyleSheet& StyleSheet::operator=(StyleSheet&& other) noexcept {
    if (this != &other) {
        auto replacement = std::move(other.mImpl);
        if (!replacement) replacement = makeEmptyImpl();
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
        other.mImpl = makeEmptyImpl();
    }
    return *this;
}

std::uint64_t StyleSheet::generation() const {
    return mImpl->generation;
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

bool StyleSheet::stateAffectsLayout(ElementState state) const {
    return mImpl->ruleSet.stateAffectsLayout(state);
}

bool StyleSheet::stateAffectsLayout(const Element& element, ElementState state) const {
    return mImpl->ruleSet.stateAffectsLayout(element, state);
}

bool StyleSheet::stateAffectsHitTesting(ElementState state) const {
    return mImpl->ruleSet.stateAffectsHitTesting(state);
}

bool StyleSheet::stateAffectsHitTesting(const Element& element, ElementState state) const {
    return mImpl->ruleSet.stateAffectsHitTesting(element, state);
}

bool StyleSheet::stateAffectsDescendants(const Element& element, ElementState state) const {
    return mImpl->ruleSet.stateAffectsDescendants(element, state);
}

bool StyleSheet::stateAffectsFollowingSiblings(const Element& element, ElementState state) const {
    return mImpl->ruleSet.stateAffectsFollowingSiblings(element, state);
}

StyleRuleSet StyleModel::build() && {
    return StyleRuleSet(std::move(*this));
}
} // namespace radia::ui
