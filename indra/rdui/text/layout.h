/**
 * @file layout.h
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

#ifndef RD_TEXT_LAYOUT_H
#define RD_TEXT_LAYOUT_H

#include <optional>
#include <string>
#include <vector>
#include "llstring.h"
#include "style/style.h"

namespace rdui { class TextMetrics; }

namespace rdui::detail {
std::vector<std::size_t> graphemeBoundaries(const LLWString& value);

struct TextRun {
    struct Key {
        std::string value;
        Style style;
        Vec2 text_size;
        Vec2 box_size;
        Vec2 size;
    };

    std::string value;
    Style style;
    Vec2 size;
    Vec2 box_size;
    std::vector<Key> keys;

    bool keybinding() const { return !keys.empty(); }
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

TextLayout layoutText(std::vector<TextLine> hard_lines, const Style& style, const TextMetrics& metrics, std::optional<float> available_width,
                      bool visual_order, bool apply_overflow);
} // namespace rdui::detail
#endif // RD_TEXT_LAYOUT_H
