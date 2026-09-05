/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <optional>
#include <utility>
#include <vector>
#include "dom/text.h"
#include "html/elementnames.h"
#include "layout/engine.h"
#include "layout/primitives.h"
#include "nativeappearance.h"
#include "style/stylepass.h"

namespace radia::ui {
class LayoutPass {
    friend class LayoutEngine;

    LayoutPass(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction = LayoutDirection::LeftToRight,
               ScrollLayoutOptions scrollOptions = {})
        : mOwnedStyles(std::in_place, styleSheet, textMetrics, direction, scrollOptions.nativeMetrics), mStyles(*mOwnedStyles),
          mScrollOptions(scrollOptions) {}
    explicit LayoutPass(StylePass& styles, ScrollLayoutOptions scrollOptions = {}) : mStyles(styles), mScrollOptions(scrollOptions) {}
    LayoutPass(const LayoutPass&) = delete;
    LayoutPass& operator=(const LayoutPass&) = delete;
    LayoutPass(LayoutPass&&) = delete;
    LayoutPass& operator=(LayoutPass&&) = delete;

    const StyleSheet& styleSheet() const { return mStyles.styleSheet(); }
    const TextMetrics& textMetrics() const { return mStyles.textMetrics(); }
    LayoutDirection direction() const { return mStyles.direction(); }
    const ScrollLayoutOptions& scrollLayoutOptions() const { return mScrollOptions; }
    const NativeLayoutMetrics& nativeMetrics() const { return mScrollOptions.nativeMetrics; }
    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const { return nativeMetrics().scrollbarMetrics(mode); }
    void recordMeasured(bool constrained) {
        ++mStatistics.measuredNodes;
        if (constrained) ++mStatistics.constrainedRemeasures;
    }
    void recordArranged() { ++mStatistics.arrangedNodes; }
    void recordSkipped() { ++mStatistics.skippedNodes; }
    const LayoutStatistics& statistics() const { return mStatistics; }

    const ComputedStyle& style(const Element& node) { return mStyles.style(node); }
    ComputedStyle style(PseudoElement& node) { return mStyles.style(node); }

private:
    detail::LayoutContextKey contextKey() const {
        detail::LayoutContextKey result = mStyles.contextKey();
        result.scrollbarMode = mScrollOptions.scrollbarMode;
        result.nativeMetrics = mScrollOptions.nativeMetrics;
        return result;
    }
    std::vector<layout_detail::LayoutChildRef> orderedChildrenForLayout(Element& parent) {
        std::vector<layout_detail::LayoutChildRef> result;
        const ComputedStyle& parentStyle = style(parent);
        const bool includesPseudoElements = parentStyle.appearance == AppearanceMode::Base;
        result.reserve(detail::nodes(parent).size() + (includesPseudoElements ? parent.generatedPseudoElements().size() : 0));
        for (detail::Node& node : detail::nodes(parent)) result.emplace_back(&node);
        if (includesPseudoElements)
            for (PseudoElement* pseudoElement : parent.generatedPseudoElements())
                if (pseudoElement) result.emplace_back(pseudoElement);
        std::stable_sort(result.begin(), result.end(), [this](const layout_detail::LayoutChildRef& left, const layout_detail::LayoutChildRef& right) {
            const int leftOrder = left.pseudoElement ? style(*left.pseudoElement).order : left.element() ? style(*left.element()).order : 0;
            const int rightOrder = right.pseudoElement ? style(*right.pseudoElement).order : right.element() ? style(*right.element()).order : 0;
            return leftOrder < rightOrder;
        });
        return result;
    }

    std::vector<layout_detail::LayoutChildRef> orderedChildrenForLayout(PseudoElement& parent) {
        std::vector<layout_detail::LayoutChildRef> result;
        result.reserve(parent.generatedPseudoElements().size());
        for (PseudoElement* pseudoElement : parent.generatedPseudoElements())
            if (pseudoElement) result.emplace_back(pseudoElement);
        std::stable_sort(result.begin(), result.end(), [this](const layout_detail::LayoutChildRef& left, const layout_detail::LayoutChildRef& right) {
            return style(*left.pseudoElement).order < style(*right.pseudoElement).order;
        });
        return result;
    }

    ComputedStyle style(const layout_detail::LayoutChildRef& node, const ComputedStyle& parentStyle) {
        if (node.pseudoElement) return style(*node.pseudoElement);
        if (const Element* element = node.element()) return mStyles.style(*element);
        return Text::styleForParent(parentStyle);
    }

    bool preservesNormalFlowWhitespace(const std::vector<layout_detail::LayoutChildRef>& children, std::size_t index,
                                       const ComputedStyle& parentStyle) {
        if (isFlexDisplay(parentStyle.display) || parentStyle.display == DisplayMode::Grid || parentStyle.display == DisplayMode::InlineGrid)
            return false;
        if (index == 0 || index + 1 >= children.size() || !layout_detail::isWhitespaceOnlyText(children[index])) return false;

        const auto isDisplayedInline = [&](const layout_detail::LayoutChildRef& child) {
            const Element* element = child.element();
            const ComputedStyle childStyle = style(child, parentStyle);
            if (element) {
                if (element->elementName() == kBrTag.localName) return false;
                return element->isDisplayed(childStyle) && layout_detail::isInlineLevel(childStyle.display);
            }
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
} // namespace radia::ui
