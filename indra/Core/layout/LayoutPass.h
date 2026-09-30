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
#include "HTMLName.h"
#include "LayoutEngine.h"
#include "LayoutPrimitives.h"
#include "NativeAppearance.h"
#include "StylePass.h"
#include "Text.h"

namespace Core::Layout {
class Pass {
    friend class Engine;

    Pass(const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Direction direction = Direction::LeftToRight,
        ScrollLayoutOptions scrollOptions = {})
        : mOwnedStyles(std::in_place, styleSheet, textMetrics, direction, scrollOptions.nativeMetrics)
        , mStyles(*mOwnedStyles)
        , mScrollOptions(scrollOptions) {}
    explicit Pass(Style::Pass& styles, ScrollLayoutOptions scrollOptions = {})
        : mStyles(styles)
        , mScrollOptions(scrollOptions) {}
    Pass(const Pass&) = delete;
    Pass& operator=(const Pass&) = delete;
    Pass(Pass&&) = delete;
    Pass& operator=(Pass&&) = delete;

public:
    const CSS::StyleSheet& styleSheet() const { return mStyles.styleSheet(); }
    const TextMeasurer& textMetrics() const { return mStyles.textMetrics(); }
    Direction direction() const { return mStyles.direction(); }
    const ScrollLayoutOptions& scrollLayoutOptions() const { return mScrollOptions; }
    const NativeLayoutMetrics& nativeMetrics() const { return mScrollOptions.nativeMetrics; }
    const Rect& viewport() const { return mViewport; }
    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const { return nativeMetrics().scrollbarMetrics(mode); }
    void recordMeasured(bool constrained) {
        ++mStatistics.measuredNodes;
        if (constrained)
            ++mStatistics.constrainedRemeasures;
    }
    void recordArranged() { ++mStatistics.arrangedNodes; }
    void recordSkipped() { ++mStatistics.skippedNodes; }
    const Statistics& statistics() const { return mStatistics; }

    const Style::ComputedStyle& style(const Element& node) { return mStyles.style(node); }
    Style::ComputedStyle style(Style::PseudoElement& node) { return mStyles.style(node); }

private:
    void setViewport(const Rect& viewport) { mViewport = viewport; }

    Core::detail::LayoutContextKey contextKey() const {
        Core::detail::LayoutContextKey result = mStyles.contextKey();
        result.scrollbarMode = mScrollOptions.scrollbarMode;
        result.nativeMetrics = mScrollOptions.nativeMetrics;
        return result;
    }
    Style::Pass::OrderedChildSnapshot orderedChildrenForLayout(Element& parent) { return mStyles.orderedChildren(parent); }
    Style::Pass::OrderedChildSnapshot orderedChildrenForLayout(Style::PseudoElement& parent) { return mStyles.orderedChildren(parent); }

    Style::ComputedStyle style(const OrderedChildRef& node, const Style::ComputedStyle& parentStyle) {
        if (node.pseudoElement)
            return style(*node.pseudoElement);
        if (const Element* element = node.element())
            return mStyles.style(*element);
        if (Text* text = node.text()) {
            Style::ComputedStyle result = Text::styleForParent(parentStyle);
            result.direction = direction();
            text->setLayoutStyle(result);
            return result;
        }
        return Text::styleForParent(parentStyle);
    }

    bool preservesNormalFlowWhitespace(const std::vector<OrderedChildRef>& children, std::size_t index,
        const Style::ComputedStyle& parentStyle) {
        if (isOrderModifiedContainer(parentStyle.display()))
            return false;
        if (index == 0 || index + 1 >= children.size() || !detail::isWhitespaceOnlyText(children[index]))
            return false;

        const auto isDisplayedInline = [&](const OrderedChildRef& child) {
            const Element* element = child.element();
            const Style::ComputedStyle childStyle = style(child, parentStyle);
            if (element) {
                if (element->elementName() == HTMLTagName(HTMLTag::Br))
                    return false;
                return element->isDisplayed(childStyle) && detail::isInlineLevel(childStyle.display());
            }
            return child.text() && childStyle.display() != Style::Display::NoneValue;
        };
        return isDisplayedInline(children[index - 1]) && isDisplayedInline(children[index + 1]);
    }

private:
    std::optional<Style::Pass> mOwnedStyles;
    Style::Pass& mStyles;
    ScrollLayoutOptions mScrollOptions;
    Rect mViewport;
    Statistics mStatistics;
};
} // namespace Core::Layout
