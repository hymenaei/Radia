/**
 * @file metrics.cpp
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
#include "text/metrics.h"
#include <algorithm>
#include <cmath>
#include "llstring.h"
#include "style/style.h"
#include "text/layout.h"

namespace rdui {
float TextMetrics::usedLetterSpacing(const Style& style) const {
    Style reference_style = style;
    reference_style.letter_spacing = {};
    reference_style.word_spacing = {};
    return style.letter_spacing.resolve(measureText(" ", reference_style).x);
}

Vec2 FixedTextMetrics::measureText(const std::string& text, const Style& style) const {
    const float line_height = std::ceil(style.line_height ? style.line_height->pixels : style.font_size);
    if (text.empty()) return {0.f, line_height};
    const LLWString wide = utf8str_to_wstring(text);
    const std::size_t characters = wide.size();
    const float weight_mix = std::clamp((static_cast<float>(style.font_weight) - 400.f) / 300.f, 0.f, 1.f);
    const float factor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weight_mix;
    const float character_width = style.font_size * factor;
    const float letter_spacing = style.letter_spacing.resolve(character_width);
    const float word_spacing = style.word_spacing.resolve(style.font_size);
    const std::vector<std::size_t> graphemes = detail::graphemeBoundaries(wide);
    const std::size_t grapheme_count = graphemes.empty() ? characters : graphemes.size() - 1;
    const float letter_spacing_width = grapheme_count > 1 ? letter_spacing * static_cast<float>(grapheme_count - 1) : 0.f;
    const std::size_t word_separators =
        static_cast<std::size_t>(std::count_if(wide.begin(), wide.end(), [](llwchar character) { return LLStringOps::isWordSeparator(character); }));
    const float word_spacing_width = word_spacing * static_cast<float>(word_separators);
    return {
        std::ceil(std::max(0.f, static_cast<float>(characters) * character_width + letter_spacing_width + word_spacing_width)),
        line_height,
    };
}

float FixedTextMetrics::usedLetterSpacing(const Style& style) const {
    const float weight_mix = std::clamp((static_cast<float>(style.font_weight) - 400.f) / 300.f, 0.f, 1.f);
    const float factor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weight_mix;
    return style.letter_spacing.resolve(style.font_size * factor);
}

const TextMetrics& fixedTextMetrics() {
    static const FixedTextMetrics metrics;
    return metrics;
}
} // namespace rdui
