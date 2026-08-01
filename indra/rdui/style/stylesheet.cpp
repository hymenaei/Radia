/**
 * @file stylesheet.cpp
 * @brief
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
#include "style/compiler.h"

namespace rdui {
void markSpecified(Style& style, InheritedStyleProperty property) {
    style.specified_inherited |= static_cast<InheritedStyleProperties>(property);
}

void inheritStyle(Style& style, const Style& parent) {
    const auto missing = [&style](InheritedStyleProperty property) {
        return (style.specified_inherited & static_cast<InheritedStyleProperties>(property)) == 0;
    };
    if (missing(InheritedStyleProperty::FontFamily)) style.font_family = parent.font_family;
    if (missing(InheritedStyleProperty::FontSize)) style.font_size = parent.font_size;
    if (missing(InheritedStyleProperty::FontWeight)) style.font_weight = parent.font_weight;
    if (missing(InheritedStyleProperty::FontStyle)) style.font_italic = parent.font_italic;
    if (missing(InheritedStyleProperty::LineHeight)) style.line_height = parent.line_height;
    if (missing(InheritedStyleProperty::TextColor)) style.text_color = parent.text_color;
    if (missing(InheritedStyleProperty::TextAlign)) style.text_align = parent.text_align;
    if (missing(InheritedStyleProperty::LetterSpacing)) style.letter_spacing = parent.letter_spacing;
    if (missing(InheritedStyleProperty::WordSpacing)) style.word_spacing = parent.word_spacing;
    if (missing(InheritedStyleProperty::TextWrap)) style.text_wrap = parent.text_wrap;
    if (missing(InheritedStyleProperty::Cursor)) style.cursor = parent.cursor;
    style.specified_inherited |= parent.specified_inherited;
}

StyleSheet::StyleSheet() : mImpl(std::make_unique<Impl>()) {}
StyleSheet::~StyleSheet() = default;
StyleSheet::StyleSheet(const StyleSheet& other) : mImpl(other.mImpl ? std::make_unique<Impl>(*other.mImpl) : std::make_unique<Impl>()) {}
StyleSheet& StyleSheet::operator=(const StyleSheet& other) {
    if (this != &other) mImpl = other.mImpl ? std::make_unique<Impl>(*other.mImpl) : std::make_unique<Impl>();
    return *this;
}
StyleSheet::StyleSheet(StyleSheet&& other) noexcept = default;
StyleSheet& StyleSheet::operator=(StyleSheet&& other) noexcept = default;

std::uint64_t StyleSheet::generation() const {
    return mImpl->generation;
}
const StyleSheet::DependencyMap& StyleSheet::dependencies() const {
    return mImpl->dependencies;
}

void StyleSheet::Impl::setColorToken(const std::string& name, const Color& color) {
    color_tokens[name] = color;
}
void StyleSheet::Impl::setNumberToken(const std::string& name, float value) {
    number_tokens[name] = value;
}

Color StyleSheet::Impl::colorToken(const std::string& name, const Color& fallback) const {
    const auto found = color_tokens.find(name);
    return found == color_tokens.end() ? fallback : found->second;
}

float StyleSheet::Impl::numberToken(const std::string& name, float fallback) const {
    const auto found = number_tokens.find(name);
    return found == number_tokens.end() ? fallback : found->second;
}
} // namespace rdui
