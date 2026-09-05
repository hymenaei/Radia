/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <utility>
#include <vector>
#include "layout/primitives.h"
#include "types.h"

namespace radia::ui {
class LayoutPass;
class StyleSheet;
class TextMetrics;
class StylePass;

struct LayoutStatistics {
    std::size_t measuredNodes = 0;
    std::size_t constrainedRemeasures = 0;
    std::size_t arrangedNodes = 0;
    std::size_t skippedNodes = 0;
};

class LayoutEngine {
private:
    using ChildLayout = layout_detail::ChildLayout;
    using MainAxisAllocation = layout_detail::MainAxisAllocation;
    using NodeSnapshot = ElementVisit;

    struct RowSizing {
        std::vector<std::pair<std::size_t, std::size_t>> lines;
        std::vector<MainAxisAllocation> allocations;
        std::vector<float> lineHeights;
        bool valid = true;
    };

    static ChildLayout measureChild(Element& parent, layout_detail::LayoutChildRef child, const ComputedStyle& parentStyle,
                                    FlexDirection flexDirection, std::optional<float> resolvedWidth, std::optional<float> resolvedHeight,
                                    LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> measureNormalChildren(Element& parent, std::optional<float> contentWidth,
                                                                         std::optional<float> contentHeight, LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> measureGridChildren(Element& parent, std::optional<float> contentWidth,
                                                                       std::optional<float> contentHeight, LayoutPass& pass);
    static Vec2 measureRow(Element& node, const ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                           std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureColumn(Element& node, const ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureGrid(Element& node, const ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                            std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureNormal(Element& node, const ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass);
    static bool remeasureRowChildren(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass);
    static bool remeasureColumnChildren(Element& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes, LayoutPass& pass);
    static RowSizing allocateRowLines(Element& parent, std::vector<ChildLayout>& children, const ComputedStyle& parentStyle, float availableMain,
                                      LayoutPass& pass);
    static RowSizing resolveRowSizes(Element& node, const ComputedStyle& parentStyle, const Rect& available, std::vector<ChildLayout>& children,
                                     LayoutPass& pass);
    static MainAxisAllocation resolveColumnSizes(Element& node, const ComputedStyle& parentStyle, const Rect& available,
                                                 std::vector<ChildLayout>& children, LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> layoutChildren(Element& parent, DisplayMode display, const Rect& content, LayoutPass& pass);
    static Rect scrollableOverflow(Element& node, const ComputedStyle& parentStyle, const Rect& scrollport, LayoutPass& pass);

    static void arrangeNode(Element& node, LayoutPass& pass);
    static void arrangeNode(const layout_detail::LayoutChildRef& node, LayoutPass& pass);
    static void arrangePseudoElement(PseudoElement& node, LayoutPass& pass);
    static void setArrangedRect(const layout_detail::LayoutChildRef& node, const Rect& rect);
    static void arrangeRow(Element& node, const ComputedStyle& parentStyle, const Rect& content, const Rect& available,
                           std::vector<ChildLayout>& children, LayoutPass& pass);
    static void arrangeColumn(Element& node, const ComputedStyle& parentStyle, const Rect& content, const Rect& available,
                              std::vector<ChildLayout>& children, LayoutPass& pass);
    static void arrangeGrid(Element& node, const ComputedStyle& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
                            LayoutPass& pass);
    static void arrangeNormal(Element& node, const ComputedStyle& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
                              LayoutPass& pass);

    static Vec2 measure(Element& node, LayoutPass& pass, std::optional<float> outerWidth = std::nullopt,
                        std::optional<float> outerHeight = std::nullopt);

public:
    static Vec2 measure(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, std::optional<float> outerWidth = std::nullopt,
                        std::optional<float> outerHeight = std::nullopt);
    static LayoutStatistics arrange(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                                    LayoutDirection direction = LayoutDirection::LeftToRight, ScrollLayoutOptions scrollOptions = {});
    static LayoutStatistics layout(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                                   LayoutDirection direction = LayoutDirection::LeftToRight, ScrollLayoutOptions scrollOptions = {});
    static LayoutStatistics layout(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions = {});

private:
    static LayoutStatistics run(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                ScrollLayoutOptions scrollOptions);
    static LayoutStatistics run(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions);
    static LayoutStatistics runWithPass(Element& node, LayoutPass& pass);
    static Vec2 measurePseudoElement(PseudoElement& node, const ComputedStyle& style, std::optional<float> outerWidth,
                                     std::optional<float> outerHeight, LayoutPass& pass);
};
} // namespace radia::ui
