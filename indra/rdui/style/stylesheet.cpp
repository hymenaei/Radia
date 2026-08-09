/**
 * @file stylesheet.cpp
 * @brief Owns the public stylesheet lifecycle and immutable generation metadata.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "style/stylesheet.h"
#include <algorithm>
#include "style/model.h"

namespace rdui {
void inheritStyle(Style& style, const Style& parent) {
    for (const detail::StylePropertyDefinition* property = detail::stylePropertyBegin(); property != detail::stylePropertyEnd(); ++property)
        if (property->inherit) property->inherit(style, parent);
    style.specified_inherited |= parent.specified_inherited;
}

StyleSheet::StyleSheet() : mImpl(std::make_shared<Impl>()) {}
StyleSheet::~StyleSheet() = default;
StyleSheet::StyleSheet(const StyleSheet& other) : mImpl(other.mImpl ? other.mImpl : std::make_shared<Impl>()) {}
StyleSheet& StyleSheet::operator=(const StyleSheet& other) {
    if (this != &other) {
        auto replacement = other.mImpl ? std::make_shared<Impl>(*other.mImpl) : std::make_shared<Impl>();
        const std::uint64_t current_generation = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(current_generation, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
    }
    return *this;
}
StyleSheet::StyleSheet(StyleSheet&& other) noexcept : mImpl(std::move(other.mImpl)) {
    if (!mImpl) mImpl = std::make_shared<Impl>();
    other.mImpl = std::make_shared<Impl>();
}
StyleSheet& StyleSheet::operator=(StyleSheet&& other) noexcept {
    if (this != &other) {
        auto replacement = std::move(other.mImpl);
        if (!replacement) replacement = std::make_shared<Impl>();
        const std::uint64_t current_generation = mImpl ? mImpl->generation : std::uint64_t{0};
        replacement->generation = std::max(current_generation, replacement->generation) + std::uint64_t{1};
        mImpl = std::move(replacement);
        other.mImpl = std::make_shared<Impl>();
    }
    return *this;
}

std::uint64_t StyleSheet::generation() const {
    return mImpl->generation;
}
const StyleSheet::DependencyMap& StyleSheet::dependencies() const {
    return mImpl->dependencies;
}

bool StyleSheet::stateAffectsLayout(WidgetState state) const {
    return mImpl->stateAffectsLayout(state);
}

bool StyleSheet::stateAffectsLayout(const Widget& widget, WidgetState state) const {
    return mImpl->stateAffectsLayout(widget, state);
}

bool StyleSheet::stateAffectsHitTesting(WidgetState state) const {
    return mImpl->stateAffectsHitTesting(state);
}

bool StyleSheet::stateAffectsHitTesting(const Widget& widget, WidgetState state) const {
    return mImpl->stateAffectsHitTesting(widget, state);
}

bool StyleSheet::stateAffectsDescendants(const Widget& widget, WidgetState state) const {
    return mImpl->stateAffectsDescendants(widget, state);
}

void StyleModel::setColorToken(const std::string& name, const Color& color) {
    color_tokens[name] = color;
}
void StyleModel::setNumberToken(const std::string& name, float value) {
    number_tokens[name] = value;
}

Color StyleModel::colorToken(const std::string& name, const Color& fallback) const {
    const auto found = color_tokens.find(name);
    return found == color_tokens.end() ? fallback : found->second;
}

float StyleModel::numberToken(const std::string& name, float fallback) const {
    const auto found = number_tokens.find(name);
    return found == number_tokens.end() ? fallback : found->second;
}
} // namespace rdui
