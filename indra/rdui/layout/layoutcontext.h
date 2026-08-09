/**
 * @file layoutcontext.h
 * @brief Shared private layout context and phase declarations.
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

#ifndef RD_LAYOUT_LAYOUTCONTEXT_H
#define RD_LAYOUT_LAYOUTCONTEXT_H

#include <optional>
#include <utility>
#include <vector>
#include "layout/engine.h"
#include "layout/primitives.h"
#include "style/stylepass.h"

namespace rdui {
using layout_detail::ChildLayout;

class LayoutPass {
public:
    LayoutPass(const StyleSheet& theme, const TextMetrics& text_metrics) : mOwnedStyles(std::in_place, theme, text_metrics), mStyles(*mOwnedStyles) {}
    explicit LayoutPass(StylePass& styles) : mStyles(styles) {}
    LayoutPass(const LayoutPass&) = delete;
    LayoutPass& operator=(const LayoutPass&) = delete;
    LayoutPass(LayoutPass&&) = delete;
    LayoutPass& operator=(LayoutPass&&) = delete;

    LayoutContextKey contextKey() const { return mStyles.contextKey(); }
    const StyleSheet& theme() const { return mStyles.styleSheet(); }
    const TextMetrics& textMetrics() const { return mStyles.textMetrics(); }
    void recordMeasured(bool constrained) {
        ++mStatistics.measured_nodes;
        if (constrained) ++mStatistics.constrained_remeasures;
    }
    void recordArranged() { ++mStatistics.arranged_nodes; }
    void recordSkipped() { ++mStatistics.skipped_nodes; }
    const LayoutStatistics& statistics() const { return mStatistics; }

    const Style& style(const Widget& node) { return mStyles.style(node); }
    StylePass::ChildSnapshot orderedChildren(const Widget& node) { return mStyles.orderedChildren(node); }

private:
    std::optional<StylePass> mOwnedStyles;
    StylePass& mStyles;
    LayoutStatistics mStatistics;
};

class LayoutEngine {
private:
    using NodeSnapshot = WidgetVisit;

    struct RowSizing {
        std::vector<std::pair<std::size_t, std::size_t>> lines;
        std::vector<layout_detail::MainAxisAllocation> allocations;
        std::vector<float> line_heights;
        bool valid = true;
    };

    static ChildLayout measureChild(Widget& parent, Widget& child, const Style& parent_style, Flow flow, std::optional<float> resolved_width,
                                    std::optional<float> resolved_height, LayoutPass& pass);
    static Vec2 measureRow(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                           std::optional<float> resolved_height, LayoutPass& pass);
    static Vec2 measureColumn(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                              std::optional<float> resolved_height, LayoutPass& pass);
    static Vec2 measureFree(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                            std::optional<float> resolved_height, LayoutPass& pass);
    static bool remeasureRowChildren(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass);
    static bool remeasureColumnChildren(Widget& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initial_sizes, LayoutPass& pass);
    static RowSizing allocateRowLines(Widget& parent, std::vector<ChildLayout>& children, const Style& parent_style, float available_main,
                                      LayoutPass& pass);
    static RowSizing resolveRowSizes(Widget& node, const Style& parent_style, const Rect& panel, std::vector<ChildLayout>& children,
                                     LayoutPass& pass);
    static layout_detail::MainAxisAllocation resolveColumnSizes(Widget& node, const Style& parent_style, const Rect& panel,
                                                                std::vector<ChildLayout>& children, LayoutPass& pass);
    static std::vector<ChildLayout> flowChildren(Widget& parent, Flow flow, LayoutPass& pass);

    static void arrangeNode(Widget& node, LayoutDirection direction, LayoutPass& pass);
    static void arrangeRow(Widget& node, const Style& parent_style, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                           LayoutDirection direction, LayoutPass& pass);
    static void arrangeColumn(Widget& node, const Style& parent_style, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                              LayoutDirection direction, LayoutPass& pass);
    static void arrangeFree(Widget& node, const Style& parent_style, const Rect& content, std::vector<ChildLayout>& children,
                            LayoutDirection direction, LayoutPass& pass);

public:
    static Vec2 measure(Widget& node, LayoutPass& pass, std::optional<float> outer_width = std::nullopt,
                        std::optional<float> outer_height = std::nullopt);
    static Vec2 measure(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, std::optional<float> outer_width = std::nullopt,
                        std::optional<float> outer_height = std::nullopt);
    static LayoutStatistics arrange(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction);
    static LayoutStatistics layout(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction);
    static LayoutStatistics layout(Widget& node, StylePass& styles, LayoutDirection direction);

private:
    static LayoutStatistics run(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction);
    static LayoutStatistics run(Widget& node, StylePass& styles, LayoutDirection direction);
    static LayoutStatistics runWithPass(Widget& node, LayoutDirection direction, LayoutPass& pass);
    static StylePass::ChildSnapshot orderedChildren(const Widget& node, LayoutPass& pass) { return pass.orderedChildren(node); }
};
} // namespace rdui
#endif // RD_LAYOUT_LAYOUTCONTEXT_H
