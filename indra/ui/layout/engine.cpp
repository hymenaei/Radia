/**
 * @file engine.cpp
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
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "layout/engineinternal.h"
#include "layout/layoutcontext.h"
#include "text/metrics.h"

namespace radia::ui {

Style resolveWidgetStyle(const StyleSheet& theme, const Widget& node) {
    LayoutPass pass(theme, fixedTextMetrics());
    return pass.style(node);
}

Vec2 LayoutEngine::measure(Widget& node, const StyleSheet& theme, const TextMetrics& textMetrics, std::optional<float> outerWidth,
                           std::optional<float> outerHeight) {
    LayoutPass pass(theme, textMetrics);
    return measure(node, pass, outerWidth, outerHeight);
}

LayoutStatistics LayoutEngine::arrange(Widget& node, const StyleSheet& theme, const TextMetrics& textMetrics, LayoutDirection direction) {
    return run(node, theme, textMetrics, direction);
}

LayoutStatistics LayoutEngine::layout(Widget& node, const StyleSheet& theme, const TextMetrics& textMetrics, LayoutDirection direction) {
    return run(node, theme, textMetrics, direction);
}

LayoutStatistics LayoutEngine::layout(Widget& node, StylePass& styles, LayoutDirection direction) {
    return run(node, styles, direction);
}

LayoutStatistics LayoutEngine::run(Widget& node, const StyleSheet& theme, const TextMetrics& textMetrics, LayoutDirection direction) {
    LayoutPass pass(theme, textMetrics);
    return runWithPass(node, direction, pass);
}

LayoutStatistics LayoutEngine::run(Widget& node, StylePass& styles, LayoutDirection direction) {
    LayoutPass pass(styles);
    return runWithPass(node, direction, pass);
}

LayoutStatistics LayoutEngine::runWithPass(Widget& node, LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot state(node);
    measure(node, pass);
    Widget* current = state.get();
    if (!state.valid()) return pass.statistics();
    if (current->mVisibility != Visibility::Collapsed) arrangeNode(*current, direction, pass);
    return pass.statistics();
}

Vec2 measureWidget(const Widget& node, const StyleSheet& theme, const TextMetrics& textMetrics) {
    return LayoutEngine::measure(const_cast<Widget&>(node), theme, textMetrics);
}

void measureTree(Widget& root, const StyleSheet& theme, const TextMetrics& textMetrics) {
    LayoutEngine::measure(root, theme, textMetrics);
}

void arrangeTree(Widget& root, const StyleSheet& theme, const TextMetrics& textMetrics, LayoutDirection direction) {
    LayoutEngine::arrange(root, theme, textMetrics, direction);
}

LayoutStatistics layoutTree(Widget& root, const StyleSheet& theme, const TextMetrics& textMetrics, LayoutDirection direction) {
    return LayoutEngine::layout(root, theme, textMetrics, direction);
}

LayoutStatistics layoutTreeUsingStylePass(Widget& root, StylePass& styles, LayoutDirection direction) {
    return LayoutEngine::layout(root, styles, direction);
}
} // namespace radia::ui
