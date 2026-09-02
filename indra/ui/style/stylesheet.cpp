/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/stylesheet.h"
#include <algorithm>
#include "style/defaults.inc"
#include "style/model.h"

namespace radia::ui {
namespace {
Color resolveLightDarkColor(const LightDarkColor& colors, ColorScheme scheme) {
    return scheme == ColorScheme::Light ? colors.light : colors.dark;
}

void resolveLightDarkColor(Color& color, std::optional<LightDarkColor>& colors, ColorScheme scheme) {
    if (colors) color = resolveLightDarkColor(*colors, scheme);
}

bool explicitlyInherits(const Style& style, std::string_view propertyName) {
    return std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        != style.explicitlyInheritedProperties.end();
}

} // namespace

void resolveLightDarkColors(Style& style) {
    resolveLightDarkColor(style.backgroundColor, style.backgroundColorLightDark, style.colorScheme);
    resolveLightDarkColor(style.borderColor, style.borderColorLightDark, style.colorScheme);
    resolveLightDarkColor(style.color, style.colorLightDark, style.colorScheme);
    resolveLightDarkColor(style.iconStrokeColor, style.iconStrokeColorLightDark, style.colorScheme);
    if (style.accentColor.lightDarkColor) style.accentColor.color = resolveLightDarkColor(*style.accentColor.lightDarkColor, style.colorScheme);
    if (style.scrollbarColor.thumbLightDarkColor)
        style.scrollbarColor.thumb = resolveLightDarkColor(*style.scrollbarColor.thumbLightDarkColor, style.colorScheme);
    if (style.scrollbarColor.trackLightDarkColor)
        style.scrollbarColor.track = resolveLightDarkColor(*style.scrollbarColor.trackLightDarkColor, style.colorScheme);
    if (style.backgroundGradient)
        for (GradientStop& stop : style.backgroundGradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    if (style.borderGradient)
        for (GradientStop& stop : style.borderGradient->stops) resolveLightDarkColor(stop.color, stop.lightDarkColor, style.colorScheme);
    for (BoxShadow& shadow : style.shadows) resolveLightDarkColor(shadow.color, shadow.lightDarkColor, style.colorScheme);
    resolveLightDarkColor(style.outline.color, style.outline.lightDarkColor, style.colorScheme);
}

void resolveCurrentColors(Style& style) {
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
}

void normalizeOverflow(Style& style) {
    const auto scrollable = [](Overflow value) { return value == Overflow::Hidden || value == Overflow::Scroll || value == Overflow::Auto; };
    if (style.overflowX == Overflow::Visible && scrollable(style.overflowY)) style.overflowX = Overflow::Auto;
    if (style.overflowY == Overflow::Visible && scrollable(style.overflowX)) style.overflowY = Overflow::Auto;
}

void inheritStyle(Style& style, const Style& parent) {
    for (const detail::StylePropertyDefinition* property = detail::stylePropertyBegin(); property != detail::stylePropertyEnd(); ++property)
        if (property->inherit && (property->isInherited() || explicitlyInherits(style, property->name))) property->inherit(style, parent);
    style.specifiedInheritedProperties |= parent.specifiedInheritedProperties;
    style.explicitlyInheritedProperties.clear();
}

void applyOpacity(Style& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity;
    style.backgroundColor.a *= opacity;
    style.borderColor.a *= opacity;
    style.color.a *= opacity;
    style.iconStrokeColor.a *= opacity;
    style.outline.color.a *= opacity;
    if (!style.scrollbarColor.automatic) {
        style.scrollbarColor.thumb.a *= opacity;
        style.scrollbarColor.track.a *= opacity;
    }
    for (BoxShadow& shadow : style.shadows) shadow.color.a *= opacity;
    if (style.backgroundGradient)
        for (GradientStop& stop : style.backgroundGradient->stops) stop.color.a *= opacity;
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
const StyleSheet::DependencyMap& StyleSheet::dependencies() const {
    return mImpl->dependencies;
}

bool StyleSheet::stateAffectsLayout(ElementState state) const {
    return mImpl->stateAffectsLayout(state);
}

bool StyleSheet::stateAffectsLayout(const Element& element, ElementState state) const {
    return mImpl->stateAffectsLayout(element, state);
}

bool StyleSheet::stateAffectsHitTesting(ElementState state) const {
    return mImpl->stateAffectsHitTesting(state);
}

bool StyleSheet::stateAffectsHitTesting(const Element& element, ElementState state) const {
    return mImpl->stateAffectsHitTesting(element, state);
}

bool StyleSheet::stateAffectsDescendants(const Element& element, ElementState state) const {
    return mImpl->stateAffectsDescendants(element, state);
}

void StyleModel::setColorToken(const std::string& name, const Color& color) {
    colorTokens[name] = color;
}
void StyleModel::setNumberToken(const std::string& name, float value) {
    numberTokens[name] = value;
}

Color StyleModel::colorToken(const std::string& name, const Color& fallback) const {
    const auto found = colorTokens.find(name);
    return found == colorTokens.end() ? fallback : found->second;
}

float StyleModel::numberToken(const std::string& name, float fallback) const {
    const auto found = numberTokens.find(name);
    return found == numberTokens.end() ? fallback : found->second;
}
} // namespace radia::ui
