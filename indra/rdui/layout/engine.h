/**
 * @file engine.h
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

#ifndef RD_LAYOUT_ENGINE_H
#define RD_LAYOUT_ENGINE_H

#include "types.h"

namespace rdui {
class Widget;
struct Style;
class StyleSheet;
class TextMetrics;

Style resolveWidgetStyle(const StyleSheet& theme, const Widget& node);
Vec2 measureWidget(const Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics);
void measureTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics);
void arrangeTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction = LayoutDirection::LeftToRight);
void layoutTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction = LayoutDirection::LeftToRight);
} // namespace rdui
#endif // RD_LAYOUT_ENGINE_H
