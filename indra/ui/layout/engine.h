/**
 * @file engine.h
 * @brief Coordinates layout transactions and exposes public layout statistics.
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

#ifndef RD_LAYOUT_ENGINE_H
#define RD_LAYOUT_ENGINE_H

#include <cstddef>
#include "types.h"

namespace radia::ui {
class Widget;
struct Style;
class StyleSheet;
class TextMetrics;

struct LayoutStatistics {
    std::size_t measuredNodes = 0;
    std::size_t constrainedRemeasures = 0;
    std::size_t arrangedNodes = 0;
    std::size_t skippedNodes = 0;
};

Style resolveWidgetStyle(const StyleSheet& styleSheet, const Widget& node);
Vec2 measureWidget(const Widget& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics);
void measureTree(Widget& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics);
void arrangeTree(Widget& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                 LayoutDirection direction = LayoutDirection::LeftToRight);
LayoutStatistics layoutTree(Widget& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                            LayoutDirection direction = LayoutDirection::LeftToRight);
} // namespace radia::ui
#endif // RD_LAYOUT_ENGINE_H
