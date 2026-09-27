/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <cstddef>
#include <optional>
#include <utility>
#include <vector>
#include "dom/text.h"
#include "html/elementnames.h"
#include "layout/engine.h"
#include "layout/primitives.h"
#include "paint/nativeappearance.h"
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

public:
    const StyleSheet& styleSheet() const { return mStyles.styleSheet(); }
    const TextMetrics& textMetrics() const { return mStyles.textMetrics(); }
    LayoutDirection direction() const { return mStyles.direction(); }
    const ScrollLayoutOptions& scrollLayoutOptions() const { return mScrollOptions; }
    const NativeLayoutMetrics& nativeMetrics() const { return mScrollOptions.nativeMetrics; }
    const Rect& viewport() const { return mViewport; }
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
    void setViewport(const Rect& viewport) { mViewport = viewport; }

    detail::LayoutContextKey contextKey() const {
        detail::LayoutContextKey result = mStyles.contextKey();
        result.scrollbarMode = mScrollOptions.scrollbarMode;
        result.nativeMetrics = mScrollOptions.nativeMetrics;
        return result;
    }
    StylePass::OrderedChildSnapshot orderedChildrenForLayout(Element& parent) { return mStyles.orderedChildren(parent); }
    StylePass::OrderedChildSnapshot orderedChildrenForLayout(PseudoElement& parent) { return mStyles.orderedChildren(parent); }

    ComputedStyle style(const OrderedChildRef& node, const ComputedStyle& parentStyle) {
        if (node.pseudoElement) return style(*node.pseudoElement);
        if (const Element* element = node.element()) return mStyles.style(*element);
        if (Text* text = node.text()) {
            ComputedStyle result = Text::styleForParent(parentStyle);
            result.direction = direction();
            text->setLayoutStyle(result);
            return result;
        }
        return Text::styleForParent(parentStyle);
    }

    bool preservesNormalFlowWhitespace(const std::vector<OrderedChildRef>& children, std::size_t index, const ComputedStyle& parentStyle) {
        if (isOrderModifiedContainer(parentStyle.display())) return false;
        if (index == 0 || index + 1 >= children.size() || !layout_detail::isWhitespaceOnlyText(children[index])) return false;

        const auto isDisplayedInline = [&](const OrderedChildRef& child) {
            const Element* element = child.element();
            const ComputedStyle childStyle = style(child, parentStyle);
            if (element) {
                if (element->elementName() == HTMLTagName(HTMLTag::Br)) return false;
                return element->isDisplayed(childStyle) && layout_detail::isInlineLevel(childStyle.display());
            }
            return child.text() && childStyle.display() != Display::NoneValue;
        };
        return isDisplayedInline(children[index - 1]) && isDisplayedInline(children[index + 1]);
    }

private:
    std::optional<StylePass> mOwnedStyles;
    StylePass& mStyles;
    ScrollLayoutOptions mScrollOptions;
    Rect mViewport;
    LayoutStatistics mStatistics;
};
} // namespace radia::ui
