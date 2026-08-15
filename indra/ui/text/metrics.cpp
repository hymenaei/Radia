/**
 * @file metrics.cpp
 * @brief Implements fixed and adapter-backed text measurement behavior.
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

namespace radia::ui {
float TextMetrics::usedLetterSpacing(const Style& style) const {
    Style unspacedStyle = style;
    unspacedStyle.letterSpacing = {};
    unspacedStyle.wordSpacing = {};
    return style.letterSpacing.resolve(measureText(" ", unspacedStyle).x);
}

Vec2 FixedTextMetrics::measureText(const std::string& text, const Style& style) const {
    const float lineHeight = std::ceil(style.lineHeight ? style.lineHeight->pixels : style.fontSize);
    if (text.empty()) return {0.f, lineHeight};
    const LLWString wide = utf8str_to_wstring(text);
    const std::size_t codepointCount = wide.size();
    const float weightBlend = std::clamp((static_cast<float>(style.fontWeight) - 400.f) / 300.f, 0.f, 1.f);
    const float widthFactor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weightBlend;
    const float averageCharacterWidth = style.fontSize * widthFactor;
    const float letterSpacing = style.letterSpacing.resolve(averageCharacterWidth);
    const float wordSpacing = style.wordSpacing.resolve(style.fontSize);
    const std::vector<std::size_t> graphemeBoundaries = detail::graphemeBoundaries(wide);
    const std::size_t graphemeCount = graphemeBoundaries.empty() ? codepointCount : graphemeBoundaries.size() - 1;
    const float letterSpacingWidth = graphemeCount > 1 ? letterSpacing * static_cast<float>(graphemeCount - 1) : 0.f;
    const std::size_t wordSeparatorCount =
        static_cast<std::size_t>(std::count_if(wide.begin(), wide.end(), [](llwchar character) { return LLStringOps::isWordSeparator(character); }));
    const float wordSpacingWidth = wordSpacing * static_cast<float>(wordSeparatorCount);
    return {
        std::ceil(std::max(0.f, static_cast<float>(codepointCount) * averageCharacterWidth + letterSpacingWidth + wordSpacingWidth)),
        lineHeight,
    };
}

float FixedTextMetrics::usedLetterSpacing(const Style& style) const {
    const float weightBlend = std::clamp((static_cast<float>(style.fontWeight) - 400.f) / 300.f, 0.f, 1.f);
    const float widthFactor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weightBlend;
    return style.letterSpacing.resolve(style.fontSize * widthFactor);
}

const TextMetrics& fixedTextMetrics() {
    static const FixedTextMetrics metrics;
    return metrics;
}
} // namespace radia::ui
