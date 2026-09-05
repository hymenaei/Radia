/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <vector>
#include "llstring.h"
#include "style/computedstyle.h"

namespace radia::ui { class TextMetrics; }

namespace radia::ui::detail {
std::vector<std::size_t> graphemeBoundaries(const LLWString& value);

struct TextRun {
    std::string value;
    ComputedStyle style;
    Vec2 size;
};

using TextLine = std::vector<TextRun>;

float interRunSpacing(const TextRun& left, const TextRun& right, const TextMetrics& metrics);

struct LaidOutTextLine {
    TextLine runs;
    Vec2 size;
};

struct TextLayout {
    std::vector<LaidOutTextLine> lines;
    Vec2 size;
};

TextLayout layoutText(const std::vector<TextLine>& hardLines, const ComputedStyle& style, const TextMetrics& metrics,
                      std::optional<float> availableWidth, bool visualOrder, bool applyOverflow);
} // namespace radia::ui::detail
