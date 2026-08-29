/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <utility>
#include "elements/element.h"
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "layout/layoutcontext.h"
#include "style/stylesheet.h"

namespace radia::ui {
using layout_detail::AdjacentLayout;
using layout_detail::adjacentLayout;
using layout_detail::allocateMainAxis;
using layout_detail::applyCrossAxisSizing;
using layout_detail::ChildLayout;
using layout_detail::CrossAlignment;
using layout_detail::crossAlignment;
using layout_detail::gridTrackSizes;
using layout_detail::isDisplayed;
using layout_detail::isWhitespaceOnlyText;
using layout_detail::justifySelfOffset;
using layout_detail::MainAxisAllocation;
using layout_detail::normalLines;
using layout_detail::positionedRect;
using layout_detail::prepareMainAxis;
using layout_detail::relativeRect;
using layout_detail::removeChildrenExcludedFromLayout;
using layout_detail::rowAlignmentOffset;
using layout_detail::rowLines;
using layout_detail::setArrangedRect;
using layout_detail::styledBoxDimension;
using layout_detail::textAlignmentOffset;
using layout_detail::translatedRect;
using layout_detail::verticalAlignmentOffset;

namespace {
constexpr float kScrollEpsilon = 1.0e-4f;
constexpr std::size_t kMaxScrollLayoutIterations = 4;

void includeOverflow(Rect& bounds, const Rect& candidate, bool includeX, bool includeY) {
    const float left = includeX ? std::min(bounds.left(), candidate.left()) : bounds.left();
    const float right = includeX ? std::max(bounds.right(), candidate.right()) : bounds.right();
    const float bottom = includeY ? std::min(bounds.bottom(), candidate.bottom()) : bounds.bottom();
    const float top = includeY ? std::max(bounds.top(), candidate.top()) : bounds.top();
    bounds = {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

Rect scrollableOverflow(Element& node, const Style& parentStyle, const Rect& scrollport, LayoutPass& pass) {
    Rect bounds = scrollport;
    const std::vector<detail::NodeRef> children = pass.orderedNodes(node);
    for (std::size_t index = 0; index < children.size(); ++index) {
        const detail::NodeRef& childRef = children[index];
        if (!childRef.get() || childRef.get()->parentNode() != &node) continue;
        const Style childStyle = pass.style(childRef, parentStyle);
        if (Element* child = childRef.element()) {
            if (!child->isDisplayed(childStyle)) continue;
            includeOverflow(bounds, child->rect(), true, true);
            const Rect& childOverflow = detail::ElementInternalAccess::scrollableOverflow(*child);
            if (!childOverflow.empty())
                includeOverflow(bounds, childOverflow, childStyle.overflowX == Overflow::Visible, childStyle.overflowY == Overflow::Visible);
        } else if (Text* text = childRef.text()) {
            if (isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle)) continue;
            includeOverflow(bounds, text->rect(), true, true);
        }
    }
    if (bounds.left() < scrollport.left()) {
        bounds.x -= parentStyle.padding.left;
        bounds.w += parentStyle.padding.left;
    }
    if (bounds.right() > scrollport.right()) bounds.w += parentStyle.padding.right;
    if (bounds.bottom() < scrollport.bottom()) {
        bounds.y -= parentStyle.padding.bottom;
        bounds.h += parentStyle.padding.bottom;
    }
    if (bounds.top() > scrollport.top()) bounds.h += parentStyle.padding.top;
    return bounds;
}

ScrollMetrics scrollMetrics(const Style& style, const Rect& scrollport, const Rect& overflow) {
    ScrollMetrics metrics;
    metrics.clientWidth = scrollport.w;
    metrics.clientHeight = scrollport.h;
    metrics.scrollWidth = std::max(metrics.clientWidth, overflow.w);
    metrics.scrollHeight = std::max(metrics.clientHeight, overflow.h);
    if (style.overflowX != Overflow::Visible) metrics.maxScrollLeft = metrics.scrollWidth - metrics.clientWidth;
    if (style.overflowY != Overflow::Visible) metrics.maxScrollTop = metrics.scrollHeight - metrics.clientHeight;
    return metrics;
}

bool needsScrollbar(Overflow overflow, float extent, float client) {
    if (overflow == Overflow::Scroll) return true;
    return overflow == Overflow::Auto && extent > client + kScrollEpsilon;
}

float scrollbarThicknessFor(ScrollbarWidth width, const NativeScrollbarMetrics& metrics) {
    const float thickness = std::max(0.f, metrics.thickness);
    if (width == ScrollbarWidth::NoneValue) return 0.f;
    if (width == ScrollbarWidth::Thin) return thickness * .5f;
    return thickness;
}

bool isScrollContainer(Overflow overflow) {
    return overflow != Overflow::Visible;
}
} // namespace

LayoutEngine::RowSizing LayoutEngine::resolveRowSizes(Element& node, const Style& parentStyle, const Rect& available,
                                                      std::vector<ChildLayout>& children, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeValid = [&] { return nodeState.valid(); };
    const float availableMain = available.w;
    const float availableCross = available.h;
    prepareMainAxis(children, FlexDirection::Row, availableMain);
    for (ChildLayout& child : children)
        child.measured.y = styledBoxDimension(child.style, false, child.style.height, child.style.minHeight, child.measured.y, availableCross);

    RowSizing sizing;
    sizing.lines = rowLines(children);
    const auto& lines = sizing.lines;
    for (const auto& [begin, end] : lines) {
        float preliminaryHeight = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            preliminaryHeight = std::max(preliminaryHeight, children[index].measured.y + children[index].style.margin.vertical());
        if (lines.size() == 1 && availableCross >= 0.f) preliminaryHeight = availableCross;
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, FlexDirection::Row, preliminaryHeight,
                                 crossAlignment(parentStyle, children[index].style, FlexDirection::Row));
    }

    sizing.allocations.reserve(lines.size());
    for (const auto& [begin, end] : lines) {
        if (!nodeValid()) {
            sizing.valid = false;
            return sizing;
        }
        const MainAxisAllocation allocation = allocateMainAxis(node, children, begin, end, parentStyle, FlexDirection::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(node, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }

    if (!nodeValid()) {
        sizing.valid = false;
        return sizing;
    }
    sizing.lineHeights.reserve(lines.size());
    for (const auto& [begin, end] : lines) {
        float height = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            height = std::max(height, children[index].measured.y + children[index].style.margin.vertical());
        sizing.lineHeights.push_back(lines.size() == 1 && availableCross >= 0.f ? availableCross : height);
    }
    for (std::size_t line = 0; line < lines.size(); ++line) {
        const auto [begin, end] = lines[line];
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, FlexDirection::Row, sizing.lineHeights[line],
                                 crossAlignment(parentStyle, children[index].style, FlexDirection::Row));
    }
    return sizing;
}

MainAxisAllocation LayoutEngine::resolveColumnSizes(Element& node, const Style& parentStyle, const Rect& available,
                                                    std::vector<ChildLayout>& children, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeValid = [&] { return nodeState.valid(); };
    MainAxisAllocation allocation;
    const float availableMain = available.h;
    const float availableCross = available.w;
    std::vector<Vec2> initialSizes;
    initialSizes.reserve(children.size());
    for (const ChildLayout& child : children) initialSizes.push_back(child.measured);
    for (ChildLayout& child : children) {
        if (!nodeValid()) {
            allocation.valid = false;
            return allocation;
        }
        child.measured.x = styledBoxDimension(child.style, true, child.style.width, child.style.minWidth, child.measured.x, availableCross);
        applyCrossAxisSizing(child.measured, child.style, FlexDirection::Column, availableCross,
                             crossAlignment(parentStyle, child.style, FlexDirection::Column));
        if (child.style.height.isAuto() && !child.style.aspectRatio) {
            Node* rawChild = child.node.get();
            if (!rawChild || rawChild->parentNode() != &node) continue;
            Element* childNode = child.node.element();
            const ElementRef<Element> childLifetime(childNode);
            const std::uint64_t childRevision = childNode ? childNode->mLayoutInvalidationRevision : 0;
            if (childNode) child.measured.y = LayoutEngine::measure(*childNode, pass, child.measured.x).y;
            else if (Text* text = child.node.text())
                child.measured.y = text->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, std::nullopt}).y;
            childNode = childLifetime.get();
            rawChild = child.node.get();
            if (!nodeValid()
                || !rawChild
                || rawChild->parentNode() != nodeState.get()
                || (childNode && (childNode->mLayoutInvalidationRevision != childRevision || childNode->mSurface != nodeState.surface))) {
                allocation.valid = false;
                return allocation;
            }
            child.fitSize.y = child.measured.y;
        }
    }
    prepareMainAxis(children, FlexDirection::Column, availableMain);
    if (!nodeValid()) {
        allocation.valid = false;
        return allocation;
    }
    allocation = allocateMainAxis(node, children, 0, children.size(), parentStyle, FlexDirection::Column, availableMain);
    if (!allocation.valid || !remeasureColumnChildren(node, children, initialSizes, pass)) allocation.valid = false;
    return allocation;
}

std::optional<std::vector<ChildLayout>> LayoutEngine::layoutChildren(Element& parent, DisplayMode display, const Rect& content, LayoutPass& pass) {
    if (display == DisplayMode::Grid || display == DisplayMode::InlineGrid) {
        const std::optional<float> contentWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
        const std::optional<float> contentHeight = content.h >= 0.f ? std::optional<float>(content.h) : std::nullopt;
        return measureGridChildren(parent, contentWidth, contentHeight, pass);
    }
    if (display != DisplayMode::Flex) {
        const std::optional<float> contentWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
        const std::optional<float> contentHeight = content.h >= 0.f ? std::optional<float>(content.h) : std::nullopt;
        std::optional<std::vector<ChildLayout>> children = measureNormalChildren(parent, contentWidth, contentHeight, pass);
        if (!children) return std::nullopt;
        return std::move(*children);
    }

    const LayoutContextKey contextKey = pass.contextKey();
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> result;
    result.reserve(parent.mChildren.size());
    const Style parentStyle = pass.style(parent);
    const std::vector<detail::NodeRef> children = orderedNodes(parent, pass);
    for (const detail::NodeRef& childRef : children) {
        Node* rawChild = childRef.get();
        Element* child = childRef.element();
        if (!rawChild || rawChild->parentNode() != &parent) continue;
        if (isWhitespaceOnlyText(childRef)) continue;
        if (child && child->elementName() == "br") continue;
        const std::uint64_t childRevision = child ? child->mLayoutInvalidationRevision : 0;
        const Style style = pass.style(childRef, parentStyle);
        if (child ? !child->isDisplayed(style) : style.display == DisplayMode::NoneValue) continue;
        Element* currentParent = parentState.get();
        rawChild = childRef.get();
        child = childRef.element();
        if (!parentState.valid()
            || !currentParent
            || !rawChild
            || rawChild->parentNode() != currentParent
            || (child && child->mLayoutInvalidationRevision != childRevision))
            continue;
        Vec2 measured;
        if (child) {
            const bool cacheMatches = detail::ElementInternalAccess::layoutCache(*child).intrinsicValid
                && !child->mInvalidationReasons.intersects(kMeasureInvalidationReasons)
                && detail::ElementInternalAccess::layoutCache(*child).layoutContext == contextKey;
            measured = cacheMatches ? detail::ElementInternalAccess::layoutCache(*child).intrinsicSize : LayoutEngine::measure(*child, pass);
        } else if (Text* text = childRef.text()) {
            measured = text->intrinsicSize(pass.styleSheet(), style, pass.textMetrics());
        }
        currentParent = parentState.get();
        rawChild = childRef.get();
        if (!parentState.valid() || !currentParent || !rawChild || rawChild->parentNode() != currentParent) continue;
        result.push_back({childRef, style, measured, measured});
    }
    return result;
}

void LayoutEngine::arrangeNode(Element& node, LayoutPass& pass) {
    const LayoutContextKey contextKey = pass.contextKey();
    const bool cacheMatches =
        detail::ElementInternalAccess::layoutCache(node).arrangeValid && detail::ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    if (!node.mInvalidationReasons.intersects(kArrangeInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return;
    }

    pass.recordArranged();
    const ElementRef<Element> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Element* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Style& parentStyle = pass.style(node);
    Element* styledNode = lifetime.get();
    if (!styledNode || styledNode->mSurface != surface || styledNode->mParent != parent || styledNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    const Rect panel = node.mRect;
    const Rect paddingBox{
        panel.x + parentStyle.borderWidth.left,
        panel.y + parentStyle.borderWidth.bottom,
        std::max(0.f, panel.w - parentStyle.borderWidth.horizontal()),
        std::max(0.f, panel.h - parentStyle.borderWidth.vertical()),
    };
    const ScrollLayoutOptions& scrollOptions = pass.scrollLayoutOptions();
    const ScrollbarMode scrollbarMode = parentStyle.scrollbarModeSet ? parentStyle.scrollbarMode : scrollOptions.scrollbarMode;
    const bool classicScrollbars = scrollbarMode == ScrollbarMode::Classic;
    const bool scrollbarSpaceAvailable = classicScrollbars && parentStyle.scrollbarWidth != ScrollbarWidth::NoneValue;
    const float scrollbarThickness = scrollbarThicknessFor(parentStyle.scrollbarWidth, pass.scrollbarMetrics(scrollbarMode));
    const bool stableGutter = parentStyle.scrollbarGutter != ScrollbarGutter::Auto;
    const bool reservesVerticalGutter = scrollbarSpaceAvailable && stableGutter && isScrollContainer(parentStyle.overflowY);
    const float verticalGutter = parentStyle.scrollbarGutter == ScrollbarGutter::StableBothEdges ? scrollbarThickness * 2.f : scrollbarThickness;
    bool verticalScrollbar = false;
    bool horizontalScrollbar = false;
    Rect scrollport;
    Rect content;
    Rect available;
    Rect overflow;
    ScrollMetrics metrics;
    const bool flexParent = parentStyle.display == DisplayMode::Flex;
    const bool gridParent = parentStyle.display == DisplayMode::Grid || parentStyle.display == DisplayMode::InlineGrid;
    const std::vector<detail::NodeRef> children = pass.orderedNodes(node);
    for (std::size_t index = 0; index < children.size(); ++index) {
        const detail::NodeRef& childRef = children[index];
        if (Text* text = childRef.text(); text && isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle))
            text->setRect({});
    }
    for (std::size_t iteration = 0; iteration < kMaxScrollLayoutIterations; ++iteration) {
        const bool reserveVerticalSpace = scrollbarSpaceAvailable && (verticalScrollbar || reservesVerticalGutter);
        const bool reserveHorizontalSpace = scrollbarSpaceAvailable && horizontalScrollbar;
        const bool rightToLeft = pass.direction() == LayoutDirection::RightToLeft;
        const bool stableBothEdges = parentStyle.scrollbarGutter == ScrollbarGutter::StableBothEdges && reservesVerticalGutter;
        const float inlineStartGutter = stableBothEdges ? scrollbarThickness : (rightToLeft && reserveVerticalSpace ? scrollbarThickness : 0.f);
        const float blockEndGutter = reserveHorizontalSpace ? scrollbarThickness : 0.f;
        scrollport = {paddingBox.x + inlineStartGutter, paddingBox.y + blockEndGutter,
                      std::max(0.f, paddingBox.w - (reserveVerticalSpace ? verticalGutter : 0.f)),
                      std::max(0.f, paddingBox.h - (reserveHorizontalSpace ? scrollbarThickness : 0.f))};
        available = {scrollport.x + parentStyle.padding.left, scrollport.y + parentStyle.padding.bottom,
                     scrollport.w - parentStyle.padding.horizontal(), scrollport.h - parentStyle.padding.vertical()};
        content = {available.x, available.y, std::max(0.f, available.w), std::max(0.f, available.h)};
        std::optional<std::vector<ChildLayout>> childrenResult = layoutChildren(node, parentStyle.display, content, pass);
        if (!childrenResult) return;
        std::vector<ChildLayout> children = std::move(*childrenResult);
        if (flexParent && parentStyle.flexDirection == FlexDirection::Row) arrangeRow(node, parentStyle, content, available, children, pass);
        else if (flexParent) arrangeColumn(node, parentStyle, content, available, children, pass);
        else if (gridParent) arrangeGrid(node, parentStyle, content, children, pass);
        else arrangeNormal(node, parentStyle, content, children, pass);

        Element* currentNode = lifetime.get();
        if (!currentNode
            || currentNode->mSurface != surface
            || currentNode->mParent != parent
            || currentNode->mLayoutInvalidationRevision != layoutRevision)
            return;
        overflow = scrollableOverflow(*currentNode, parentStyle, scrollport, pass);
        metrics = scrollMetrics(parentStyle, scrollport, overflow);
        const bool nextVerticalScrollbar =
            scrollbarSpaceAvailable && needsScrollbar(parentStyle.overflowY, metrics.scrollHeight, metrics.clientHeight);
        const bool nextHorizontalScrollbar =
            scrollbarSpaceAvailable && needsScrollbar(parentStyle.overflowX, metrics.scrollWidth, metrics.clientWidth);
        if (nextVerticalScrollbar == verticalScrollbar && nextHorizontalScrollbar == horizontalScrollbar) break;
        verticalScrollbar = nextVerticalScrollbar;
        horizontalScrollbar = nextHorizontalScrollbar;
    }

    Element* arrangedNode = lifetime.get();
    if (!arrangedNode
        || arrangedNode->mSurface != surface
        || arrangedNode->mParent != parent
        || arrangedNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    node.onArranged(parentStyle);
    Element* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision) return;
    overflow = scrollableOverflow(*current, parentStyle, scrollport, pass);
    metrics = scrollMetrics(parentStyle, scrollport, overflow);
    current->setScrollMetrics(metrics, overflow, scrollport);
    detail::ElementInternalAccess::layoutCache(*current).layoutContext = contextKey;
    detail::ElementInternalAccess::layoutCache(*current).arrangeValid = true;
    current->mInvalidationReasons.remove(LayoutInvalidationReason::Arrange);
}

void LayoutEngine::setArrangedRect(detail::NodeRef node, const Rect& rect) {
    if (Element* element = node.element()) layout_detail::setArrangedRect(*element, rect);
    else if (Text* text = node.text()) text->setRect(rect);
}

void LayoutEngine::arrangeNode(detail::NodeRef node, LayoutPass& pass) {
    if (Element* element = node.element()) arrangeNode(*element, pass);
}

void LayoutEngine::arrangeRow(Element& node, const Style& parentStyle, const Rect& content, const Rect& available, std::vector<ChildLayout>& children,
                              LayoutPass& pass) {
    const LayoutDirection direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const float availableCross = available.h;
    const RowSizing sizing = resolveRowSizes(node, parentStyle, available, children, pass);
    if (!sizing.valid) return;
    const auto& lines = sizing.lines;
    const auto& lineHeights = sizing.lineHeights;
    const float lineGap = parentStyle.gap.fixedPixels();
    float blockHeight = 0.f;
    for (float height : lineHeights) blockHeight += height;
    if (lineHeights.size() > 1) blockHeight += lineGap * static_cast<float>(lineHeights.size() - 1);
    float lineTop = content.top() - verticalAlignmentOffset(parentStyle.verticalAlign, std::max(0.f, availableCross - blockHeight));

    for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
        const auto [begin, end] = sizing.lines[line];
        const float lineHeight = lineHeights[line];
        const float lineBottom = lineTop - lineHeight;
        const MainAxisAllocation& allocation = sizing.allocations[line];
        const float gap = allocation.gap;
        const float freeSpace = allocation.freeSpace;
        const float autoMargin = allocation.autoMargin;
        const bool rtl = direction == LayoutDirection::RightToLeft;
        float x = rtl ? content.right() : content.left();
        if (!allocation.hasAutoMargins) {
            const float offset = rowAlignmentOffset(parentStyle.justifyContent, direction, freeSpace);
            x += rtl ? -offset : offset;
        }
        for (std::size_t index = begin; index < end; ++index) {
            ChildLayout& child = children[index];
            detail::NodeRef current = child.node;
            Element* currentNode = nodeState.get();
            if (!nodeState.valid()) return;
            if (!current || current.get()->parentNode() != currentNode || !isDisplayed(child)) continue;
            detail::NodeRef next = index + 1 < end ? children[index + 1].node : detail::NodeRef();
            if (next && (next.get()->parentNode() != currentNode || !isDisplayed(children[index + 1]))) next.set(nullptr);
            float adjacencyGap = 0.f;
            float adjacencyOverlap = 0.f;
            if (next) {
                const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, current, next, parentStyle);
                if (!adjacent) return;
                adjacencyGap = adjacent->hasGap ? gap : 0.f;
                adjacencyOverlap = adjacent->overlap;
            }
            const MarginInsets& margin = child.style.margin;
            const float availableCrossSpace = lineHeight - child.measured.y - margin.vertical();
            const float crossSpace = std::max(0.f, availableCrossSpace);
            const int crossAutoCount = margin.verticalAutoCount();
            const float crossAuto = crossAutoCount ? crossSpace / static_cast<float>(crossAutoCount) : 0.f;
            float y = lineBottom + margin.bottom.fixedPixels();
            if (crossAutoCount) y += margin.bottom.isAuto() ? crossAuto : 0.f;
            else {
                const CrossAlignment alignment = crossAlignment(parentStyle, child.style, FlexDirection::Row);
                if (alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch)
                    y = lineTop - margin.top.fixedPixels() - child.measured.y;
                else if (alignment == CrossAlignment::Center) y += availableCrossSpace * .5f;
            }
            if (rtl) {
                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                const Rect base{x - child.measured.x, y, child.measured.x, child.measured.y};
                setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)));
                x -= child.measured.x + margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x -= adjacencyGap;
                    x += adjacencyOverlap;
                }
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                const Rect base{x, y, child.measured.x, child.measured.y};
                setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)));
                x += child.measured.x + margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x += adjacencyGap;
                    x -= adjacencyOverlap;
                }
            }
            arrangeNode(child.node, pass);
            currentNode = nodeState.get();
            if (!nodeState.valid()) return;
        }
        lineTop = lineBottom - lineGap;
    }
}

void LayoutEngine::arrangeColumn(Element& node, const Style& parentStyle, const Rect& content, const Rect& available,
                                 std::vector<ChildLayout>& children, LayoutPass& pass) {
    const LayoutDirection direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const MainAxisAllocation allocation = resolveColumnSizes(node, parentStyle, available, children, pass);
    if (!allocation.valid) return;
    const float gap = allocation.gap;
    const float freeSpace = allocation.freeSpace;
    const float autoMargin = allocation.autoMargin;
    float y = content.top();
    if (!allocation.hasAutoMargins) {
        const JustifyContent alignment = parentStyle.justifyContent;
        if (alignment == JustifyContent::Center) y -= freeSpace * .5f;
        else if (alignment == JustifyContent::End || alignment == JustifyContent::Right) y -= freeSpace;
        else if (alignment == JustifyContent::Start) y -= verticalAlignmentOffset(parentStyle.verticalAlign, freeSpace);
    }
    for (std::size_t i = 0; i < children.size(); ++i) {
        ChildLayout& child = children[i];
        detail::NodeRef current = child.node;
        Element* currentNode = nodeState.get();
        if (!nodeState.valid()) return;
        if (!current || current.get()->parentNode() != currentNode || !isDisplayed(child)) continue;
        detail::NodeRef previous = i ? children[i - 1].node : detail::NodeRef();
        if (previous && (previous.get()->parentNode() != currentNode || !isDisplayed(children[i - 1]))) previous.set(nullptr);
        float adjacencyGap = 0.f;
        float adjacencyOverlap = 0.f;
        if (previous) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previous, current, parentStyle);
            if (!adjacent) return;
            adjacencyGap = adjacent->hasGap ? gap : 0.f;
            adjacencyOverlap = adjacent->overlap;
        }
        const MarginInsets& margin = child.style.margin;
        if (previous) {
            y -= adjacencyGap;
            y += adjacencyOverlap;
        }
        y -= margin.top.fixedPixels() + (margin.top.isAuto() ? autoMargin : 0.f);
        const int horizontalAutoCount = margin.horizontalAutoCount();
        const float width = child.measured.x;
        const float horizontalSpace = std::max(0.f, content.w - width - margin.horizontal());
        const float horizontalAuto = horizontalAutoCount ? horizontalSpace / static_cast<float>(horizontalAutoCount) : 0.f;
        float x = content.left() + margin.left.fixedPixels();
        if (horizontalAutoCount) x += margin.left.isAuto() ? horizontalAuto : 0.f;
        else {
            const CrossAlignment alignment = crossAlignment(parentStyle, child.style, FlexDirection::Column);
            if (alignment == CrossAlignment::Center) x += horizontalSpace * .5f;
            else {
                const bool logicalStart = alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch;
                const bool alignRight = logicalStart == (direction == LayoutDirection::RightToLeft);
                if (alignRight) x = content.right() - margin.right.fixedPixels() - width;
            }
        }
        y -= child.measured.y;
        const Rect base{x, y, width, child.measured.y};
        setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)));
        arrangeNode(child.node, pass);
        currentNode = nodeState.get();
        if (!nodeState.valid()) return;
        y -= margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? autoMargin : 0.f);
    }
}

void LayoutEngine::arrangeGrid(Element& node, const Style&, const Rect& content, std::vector<ChildLayout>& children, LayoutPass& pass) {
    const LayoutDirection direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const layout_detail::GridTrackSizes tracks = gridTrackSizes(children, content.w, content.h);
    const auto sumBefore = [](const std::vector<float>& sizes, std::size_t end) {
        float result = 0.f;
        for (std::size_t index = 0; index < end; ++index) result += sizes[index];
        return result;
    };
    for (ChildLayout& child : children) {
        Element* currentNode = nodeState.get();
        if (!nodeState.valid()) return;
        if (!child.node || child.node.get()->parentNode() != currentNode || !isDisplayed(child)) continue;

        const GridArea area = child.style.gridArea.value_or(GridArea{});
        const std::size_t column = static_cast<std::size_t>(std::max(1, area.column)) - 1;
        const std::size_t row = static_cast<std::size_t>(std::max(1, area.row)) - 1;
        const float cellLeft = direction == LayoutDirection::RightToLeft ? content.right() - sumBefore(tracks.columns, column + 1)
                                                                         : content.left() + sumBefore(tracks.columns, column);
        const float cellTop = content.top() - sumBefore(tracks.rows, row);
        const float cellWidth = tracks.columns[column];
        const float cellHeight = tracks.rows[row];
        const MarginInsets& margin = child.style.margin;
        const float availableWidth = std::max(0.f, cellWidth - margin.horizontal());
        const float availableHeight = std::max(0.f, cellHeight - margin.vertical());
        const bool stretch = child.style.justifySelf == JustifySelf::Auto || child.style.justifySelf == JustifySelf::Stretch;
        const float widthFallback = child.style.width.isAuto() ? (stretch ? availableWidth : child.measured.x) : child.measured.x;
        const float heightFallback = child.style.height.isAuto() ? availableHeight : child.measured.y;
        const float width = styledBoxDimension(child.style, true, child.style.width, child.style.minWidth, widthFallback, content.w);
        const float height = styledBoxDimension(child.style, false, child.style.height, child.style.minHeight, heightFallback, content.h);

        const float freeSpace = std::max(0.f, availableWidth - width);
        const float x = cellLeft + margin.left.fixedPixels() + justifySelfOffset(child.style.justifySelf, direction, freeSpace);
        const Rect item{x, cellTop - margin.top.fixedPixels() - height, width, height};
        setArrangedRect(child.node, translatedRect(child, relativeRect(child, item, content)));
        arrangeNode(child.node, pass);
        currentNode = nodeState.get();
        if (!nodeState.valid()) return;
    }
}

void LayoutEngine::arrangeNormal(Element& node, const Style& parentStyle, const Rect& content, std::vector<ChildLayout>& children, LayoutPass& pass) {
    const LayoutDirection direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const std::optional<float> availableWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
    const std::vector<layout_detail::NormalLine> lines = normalLines(children, availableWidth);
    float contentHeight = 0.f;
    for (const layout_detail::NormalLine& line : lines) contentHeight += line.height;
    const float blockAlignmentOffset = parentStyle.alignContentBlockCenter ? (content.h - contentHeight) * .5f : 0.f;
    float lineTop = content.top() - blockAlignmentOffset;
    const bool rtl = direction == LayoutDirection::RightToLeft;

    for (const layout_detail::NormalLine& line : lines) {
        const float lineBottom = lineTop - line.height;
        const float freeSpace = std::max(0.f, content.w - line.width);
        const float alignmentOffset = line.block ? 0.f : textAlignmentOffset(parentStyle.textAlign, direction, freeSpace);
        float x = rtl ? content.right() - alignmentOffset : content.left() + alignmentOffset;
        for (std::size_t index = line.begin; index < line.end; ++index) {
            ChildLayout& child = children[index];
            detail::NodeRef childNode = child.node;
            Element* currentNode = nodeState.get();
            if (!nodeState.valid()) return;
            if (!childNode || childNode.get()->parentNode() != currentNode || !isDisplayed(child)) continue;

            const MarginInsets& margin = child.style.margin;
            const float width = child.measured.x;
            const float height = child.measured.y;
            const float horizontalSpace = std::max(0.f, content.w - width - margin.horizontal());
            float childX;
            if (rtl) {
                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? horizontalSpace : 0.f);
                childX = x - width;
                x = childX - margin.left.fixedPixels();
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? horizontalSpace : 0.f);
                childX = x;
                x += width + margin.right.fixedPixels();
            }
            float childY = lineBottom + margin.bottom.fixedPixels();
            if (line.block || child.style.verticalAlign == VerticalAlign::Top) {
                childY = lineTop - margin.top.fixedPixels() - height;
            } else {
                const float freeSpace = std::max(0.f, line.height - height - margin.vertical());
                childY = lineBottom + margin.bottom.fixedPixels() + verticalAlignmentOffset(child.style.verticalAlign, freeSpace);
            }
            Rect base;
            if (Element* element = childNode.element(); element && element->mRectExplicit)
                base = positionedRect(child, content, parentStyle.verticalAlign);
            else base = {childX, childY, width, height};
            setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)));
            arrangeNode(child.node, pass);
            currentNode = nodeState.get();
            if (!nodeState.valid()) return;
        }
        lineTop = lineBottom;
    }
}
} // namespace radia::ui
