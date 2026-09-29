/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <utility>
#include <vector>
#include "LayoutGeometry.h"
#include "LayoutPrimitives.h"
#include "ScrollLayoutOptions.h"

namespace Core::Layout {
class Pass;

struct Statistics {
    std::size_t measuredNodes = 0;
    std::size_t constrainedRemeasures = 0;
    std::size_t arrangedNodes = 0;
    std::size_t skippedNodes = 0;
};

class Engine {
private:
    using ChildLayout = detail::ChildLayout;
    using MainAxisAllocation = detail::MainAxisAllocation;
    using NodeSnapshot = ElementVisit;

    struct RowSizing {
        std::vector<std::pair<std::size_t, std::size_t>> lines;
        std::vector<MainAxisAllocation> allocations;
        std::vector<float> lineHeights;
        float crossOffset = 0.f;
        float crossGap = 0.f;
        bool valid = true;
    };

    struct ColumnSizing {
        std::vector<std::pair<std::size_t, std::size_t>> lines;
        std::vector<MainAxisAllocation> allocations;
        std::vector<float> lineWidths;
        float crossOffset = 0.f;
        float crossGap = 0.f;
        bool valid = true;
    };

    static ChildLayout measureChild(Element& parent, OrderedChildRef child, const Style::ComputedStyle& parentStyle,
        Style::FlexDirection flexDirection, std::optional<float> resolvedWidth, std::optional<float> resolvedHeight, Pass& pass);
    static std::optional<std::vector<ChildLayout>> measureNormalChildren(Element& parent, std::optional<float> contentWidth,
        std::optional<float> contentHeight, Pass& pass);
    static std::optional<std::vector<ChildLayout>> measureGridChildren(Element& parent, std::optional<float> contentWidth,
        std::optional<float> contentHeight, Pass& pass);
    static Vec2 measureRow(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
        std::optional<float> resolvedHeight, Pass& pass);
    static Vec2 measureColumn(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
        std::optional<float> resolvedHeight, Pass& pass);
    static Vec2 measureGrid(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
        std::optional<float> resolvedHeight, Pass& pass);
    static Vec2 measureNormal(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
        std::optional<float> resolvedHeight, Pass& pass);
    static bool remeasureRowChildren(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, Pass& pass);
    static bool remeasureColumnChildren(Element& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes,
        Pass& pass);
    static RowSizing allocateRowLines(Element& parent, std::vector<ChildLayout>& children, const Style::ComputedStyle& parentStyle,
        float availableMain, Pass& pass);
    static RowSizing resolveRowSizes(Element& node, const Style::ComputedStyle& parentStyle, const Rect& available,
        std::vector<ChildLayout>& children, Pass& pass);
    static ColumnSizing resolveColumnSizes(Element& node, const Style::ComputedStyle& parentStyle, const Rect& available,
        std::vector<ChildLayout>& children, Pass& pass);
    static std::optional<std::vector<ChildLayout>> layoutChildren(Element& parent, Style::Display display, const Rect& content, Pass& pass);
    static Rect scrollableOverflow(Element& node, const Style::ComputedStyle& parentStyle, const Rect& scrollport, Pass& pass);

    static void arrangeNode(Element& node, Pass& pass);
    static void arrangeNode(const OrderedChildRef& node, Pass& pass);
    static void arrangeOutOfFlowChild(const OrderedChildRef& node, const Style::ComputedStyle& style, const Rect& containingBlock,
        Pass& pass);
    static void arrangeOutOfFlowChildren(Element& node, Pass& pass);
    static void arrangeOutOfFlowChildren(Style::PseudoElement& node, Pass& pass);
    static void arrangePseudoElement(Style::PseudoElement& node, Pass& pass);
    static void prepareTextPaint(Element& node, Pass& pass);
    static void setArrangedRect(const OrderedChildRef& node, const Rect& rect, Pass& pass);
    static void arrangeRow(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, const Rect& available,
        std::vector<ChildLayout>& children, Pass& pass);
    static void arrangeColumn(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, const Rect& available,
        std::vector<ChildLayout>& children, Pass& pass);
    static void arrangeGrid(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
        Pass& pass);
    static void arrangeNormal(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content,
        std::vector<ChildLayout>& children, Pass& pass);

    static Vec2 measure(Element& node, Pass& pass, std::optional<float> outerWidth = std::nullopt,
        std::optional<float> outerHeight = std::nullopt, bool intrinsicProbe = false);

public:
    static Vec2 measure(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics,
        std::optional<float> outerWidth = std::nullopt, std::optional<float> outerHeight = std::nullopt);
    static Statistics arrange(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics,
        Direction direction = Direction::LeftToRight, ScrollLayoutOptions scrollOptions = {});
    static Statistics layout(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics,
        Direction direction = Direction::LeftToRight, ScrollLayoutOptions scrollOptions = {});
    static Statistics layout(Element& node, Style::Pass& styles, ScrollLayoutOptions scrollOptions = {});

private:
    static Statistics run(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Direction direction,
        ScrollLayoutOptions scrollOptions);
    static Statistics run(Element& node, Style::Pass& styles, ScrollLayoutOptions scrollOptions);
    static Statistics runWithPass(Element& node, Pass& pass);
    static Vec2 measurePseudoElement(Style::PseudoElement& node, const Style::ComputedStyle& style, std::optional<float> outerWidth,
        std::optional<float> outerHeight, Pass& pass);
};
} // namespace Core::Layout
