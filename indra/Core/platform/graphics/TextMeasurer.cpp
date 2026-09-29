/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "TextMeasurer.h"
#include <algorithm>
#include <cmath>
#include <variant>
#include "ComputedStyle.h"
#include "TextLayout.h"
#include "llstring.h"

namespace Core {
float TextMeasurer::usedLetterSpacing(const Style::ComputedStyle& style) const { return style.letterSpacing().resolve(style.fontSize()); }

Layout::Vec2 FixedTextMeasurer::measureText(const std::string& text, const Style::ComputedStyle& style) const {
    const auto& lineHeightValue = style.lineHeight().mValue;
    const auto* length = std::get_if<Style::LineHeight::Length>(&lineHeightValue);
    const auto* number = std::get_if<Style::LineHeight::Number>(&lineHeightValue);
    const float lineHeight = std::ceil(length ? length->pixels : number ? style.fontSize() * number->value : style.fontSize());
    if (text.empty())
        return {0.f, lineHeight};
    const LLWString wide = utf8str_to_wstring(text);
    const std::size_t codepointCount = wide.size();
    const float weightBlend = std::clamp((style.fontWeight().value - 400.f) / 300.f, 0.f, 1.f);
    const float widthFactor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weightBlend;
    const float averageCharacterWidth = style.fontSize() * widthFactor;
    const float letterSpacing = style.letterSpacing().resolve(style.fontSize());
    const float wordSpacing = style.wordSpacing().resolve(style.fontSize());
    const std::vector<std::size_t> graphemeBoundaries = Layout::detail::graphemeBoundaries(wide);
    const std::size_t graphemeCount = graphemeBoundaries.empty() ? codepointCount : graphemeBoundaries.size() - 1;
    const float letterSpacingWidth = graphemeCount > 1 ? letterSpacing * static_cast<float>(graphemeCount - 1) : 0.f;
    const std::size_t wordSeparatorCount = static_cast<std::size_t>(std::count_if(wide.begin(), wide.end(), [](llwchar character) {
        return LLStringOps::isWordSeparator(character);
    }));
    const float wordSpacingWidth = wordSpacing * static_cast<float>(wordSeparatorCount);
    return {
        std::ceil(std::max(0.f, static_cast<float>(codepointCount) * averageCharacterWidth + letterSpacingWidth + wordSpacingWidth)),
        lineHeight,
    };
}

float FixedTextMeasurer::usedLetterSpacing(const Style::ComputedStyle& style) const {
    const float weightBlend = std::clamp((style.fontWeight().value - 400.f) / 300.f, 0.f, 1.f);
    const float widthFactor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weightBlend;
    return style.letterSpacing().resolve(style.fontSize() * widthFactor);
}

const TextMeasurer& fixedTextMeasurer() {
    static const FixedTextMeasurer sMetrics;
    return sMetrics;
}
} // namespace Core
