/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <optional>
#include <utility>
#include <vector>
#include "layout/engine.h"
#include "layout/primitives.h"
#include "nativeappearance.h"
#include "style/stylepass.h"

namespace radia::ui {
class LayoutPass {
public:
    LayoutPass(const StyleSheet& styleSheet, const TextMetrics& textMetrics,
               LayoutDirection direction = LayoutDirection::LeftToRight, ScrollLayoutOptions scrollOptions = {})
        : mOwnedStyles(std::in_place, styleSheet, textMetrics, direction, scrollOptions.nativeAppearance),
          mStyles(*mOwnedStyles),
          mScrollOptions(scrollOptions) {}
    explicit LayoutPass(StylePass& styles, ScrollLayoutOptions scrollOptions = {}) : mStyles(styles), mScrollOptions(scrollOptions) {}
    LayoutPass(const LayoutPass&) = delete;
    LayoutPass& operator=(const LayoutPass&) = delete;
    LayoutPass(LayoutPass&&) = delete;
    LayoutPass& operator=(LayoutPass&&) = delete;

    LayoutContextKey contextKey() const {
        LayoutContextKey result = mStyles.contextKey();
        result.scrollbarMode = mScrollOptions.scrollbarMode;
        const NativeAppearance& appearance = nativeAppearance();
        result.nativeAppearance = &appearance;
        result.nativeAppearanceRevision = appearance.revision();
        return result;
    }
    const StyleSheet& styleSheet() const { return mStyles.styleSheet(); }
    const TextMetrics& textMetrics() const { return mStyles.textMetrics(); }
    LayoutDirection direction() const { return mStyles.direction(); }
    const ScrollLayoutOptions& scrollLayoutOptions() const { return mScrollOptions; }
    const NativeAppearance& nativeAppearance() const {
        return mScrollOptions.nativeAppearance ? *mScrollOptions.nativeAppearance : defaultNativeAppearance();
    }
    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const { return nativeAppearance().scrollbarMetrics(mode); }
    void recordMeasured(bool constrained) {
        ++mStatistics.measuredNodes;
        if (constrained) ++mStatistics.constrainedRemeasures;
    }
    void recordArranged() { ++mStatistics.arrangedNodes; }
    void recordSkipped() { ++mStatistics.skippedNodes; }
    const LayoutStatistics& statistics() const { return mStatistics; }

    const Style& style(const Element& node) { return mStyles.style(node); }
    StylePass::ChildSnapshot orderedChildren(Element& node) { return mStyles.orderedChildren(node); }

    std::vector<detail::NodeRef> orderedNodes(Element& parent) {
        std::vector<detail::NodeRef> result;
        result.reserve(detail::nodes(parent).size());
        for (detail::Node& node : detail::nodes(parent)) result.emplace_back(&node);
        std::stable_sort(result.begin(), result.end(), [this](const detail::NodeRef& left, const detail::NodeRef& right) {
            const Element* leftElement = left.element();
            const Element* rightElement = right.element();
            const int leftOrder = leftElement ? style(*leftElement).order : 0;
            const int rightOrder = rightElement ? style(*rightElement).order : 0;
            return leftOrder < rightOrder;
        });
        return result;
    }

    Style style(const detail::NodeRef& node, const Style& parentStyle) {
        if (const Element* element = node.element()) return mStyles.style(*element);
        Style result;
        inheritStyle(result, parentStyle);
        result.textOverflow = parentStyle.textOverflow;
        result.overflowX = parentStyle.overflowX;
        result.display = DisplayMode::Inline;
        result.displaySet = true;
        result.margin = {};
        result.padding = {};
        return result;
    }

    bool preservesNormalFlowWhitespace(const std::vector<detail::NodeRef>& children, std::size_t index, const Style& parentStyle) {
        if (parentStyle.display == DisplayMode::Flex || parentStyle.display == DisplayMode::Grid || parentStyle.display == DisplayMode::InlineGrid)
            return false;
        if (index == 0 || index + 1 >= children.size() || !layout_detail::isWhitespaceOnlyText(children[index])) return false;

        const auto isDisplayedInline = [&](const detail::NodeRef& child) {
            const Element* element = child.element();
            if (element && element->elementName() == "br") return false;
            const Style childStyle = style(child, parentStyle);
            if (element) return element->isDisplayed(childStyle) && layout_detail::isInlineLevel(childStyle.display);
            return child.text() && childStyle.display != DisplayMode::NoneValue;
        };
        return isDisplayedInline(children[index - 1]) && isDisplayedInline(children[index + 1]);
    }

private:
    std::optional<StylePass> mOwnedStyles;
    StylePass& mStyles;
    ScrollLayoutOptions mScrollOptions;
    LayoutStatistics mStatistics;
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

    static ChildLayout measureChild(Element& parent, detail::NodeRef child, const Style& parentStyle, FlexDirection flexDirection,
                                    std::optional<float> resolvedWidth, std::optional<float> resolvedHeight, LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> measureNormalChildren(Element& parent, std::optional<float> contentWidth,
                                                                         std::optional<float> contentHeight, LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> measureGridChildren(Element& parent, std::optional<float> contentWidth,
                                                                       std::optional<float> contentHeight, LayoutPass& pass);
    static Vec2 measureRow(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                           std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureColumn(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureGrid(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                            std::optional<float> resolvedHeight, LayoutPass& pass);
    static Vec2 measureNormal(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass);
    static bool remeasureRowChildren(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass);
    static bool remeasureColumnChildren(Element& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes, LayoutPass& pass);
    static RowSizing allocateRowLines(Element& parent, std::vector<ChildLayout>& children, const Style& parentStyle, float availableMain,
                                      LayoutPass& pass);
    static RowSizing resolveRowSizes(Element& node, const Style& parentStyle, const Rect& available, std::vector<ChildLayout>& children,
                                     LayoutPass& pass);
    static MainAxisAllocation resolveColumnSizes(Element& node, const Style& parentStyle, const Rect& available, std::vector<ChildLayout>& children,
                                                 LayoutPass& pass);
    static std::optional<std::vector<ChildLayout>> layoutChildren(Element& parent, DisplayMode display, const Rect& content, LayoutPass& pass);

    static void arrangeNode(Element& node, LayoutPass& pass);
    static void arrangeNode(detail::NodeRef node, LayoutPass& pass);
    static void setArrangedRect(detail::NodeRef node, const Rect& rect);
    static void arrangeRow(Element& node, const Style& parentStyle, const Rect& content, const Rect& available, std::vector<ChildLayout>& children,
                           LayoutPass& pass);
    static void arrangeColumn(Element& node, const Style& parentStyle, const Rect& content, const Rect& available, std::vector<ChildLayout>& children,
                              LayoutPass& pass);
    static void arrangeGrid(Element& node, const Style& parentStyle, const Rect& content, std::vector<ChildLayout>& children, LayoutPass& pass);
    static void arrangeNormal(Element& node, const Style& parentStyle, const Rect& content, std::vector<ChildLayout>& children, LayoutPass& pass);

public:
    static Vec2 measure(Element& node, LayoutPass& pass, std::optional<float> outerWidth = std::nullopt,
                        std::optional<float> outerHeight = std::nullopt);
    static Vec2 measure(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, std::optional<float> outerWidth = std::nullopt,
                        std::optional<float> outerHeight = std::nullopt);
    static LayoutStatistics arrange(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                    ScrollLayoutOptions scrollOptions = {});
    static LayoutStatistics layout(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                   ScrollLayoutOptions scrollOptions = {});
    static LayoutStatistics layout(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions = {});

private:
    static LayoutStatistics run(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                ScrollLayoutOptions scrollOptions);
    static LayoutStatistics run(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions);
    static LayoutStatistics runWithPass(Element& node, LayoutPass& pass);
    static std::vector<detail::NodeRef> orderedNodes(Element& node, LayoutPass& pass) { return pass.orderedNodes(node); }
};
} // namespace radia::ui
