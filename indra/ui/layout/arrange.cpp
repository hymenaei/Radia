/**
 * @file arrange.cpp
 * @brief Normal and flex arrangement and post-measure geometry.
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
#include <algorithm>
#include <cmath>
#include <utility>
#include "layout/layoutcontext.h"
#include "style/stylesheet.h"
#include "widgets/widget.h"

namespace radia::ui {
using layout_detail::AdjacentLayout;
using layout_detail::adjacentLayout;
using layout_detail::allocateMainAxis;
using layout_detail::applyCrossAxisSizing;
using layout_detail::ChildLayout;
using layout_detail::CrossAlignment;
using layout_detail::crossAlignment;
using layout_detail::MainAxisAllocation;
using layout_detail::normalLines;
using layout_detail::positionedRect;
using layout_detail::prepareMainAxis;
using layout_detail::removeChildrenExcludedFromLayout;
using layout_detail::rowAlignmentOffset;
using layout_detail::rowLines;
using layout_detail::setArrangedRect;
using layout_detail::styledDimension;
using layout_detail::verticalAlignmentOffset;
using layout_detail::warnIgnoredPosition;

LayoutEngine::RowSizing LayoutEngine::resolveRowSizes(Widget& node, const Style& parentStyle, const Rect& panel, std::vector<ChildLayout>& children,
                                                      LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeValid = [&] { return nodeState.valid(); };
    const float availableMain = panel.w - parentStyle.padding.horizontal();
    const float availableCross = panel.h - parentStyle.padding.vertical();
    prepareMainAxis(children, FlexDirection::Row, availableMain);
    for (ChildLayout& child : children)
        child.measured.y = styledDimension(child.style.height, child.style.minHeight, child.measured.y, availableCross);

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

MainAxisAllocation LayoutEngine::resolveColumnSizes(Widget& node, const Style& parentStyle, const Rect& panel, std::vector<ChildLayout>& children,
                                                    LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeValid = [&] { return nodeState.valid(); };
    MainAxisAllocation allocation;
    const float availableMain = panel.h - parentStyle.padding.vertical();
    const float availableCross = panel.w - parentStyle.padding.horizontal();
    std::vector<Vec2> initialSizes;
    initialSizes.reserve(children.size());
    for (const ChildLayout& child : children) initialSizes.push_back(child.measured);
    for (ChildLayout& child : children) {
        if (!nodeValid()) {
            allocation.valid = false;
            return allocation;
        }
        child.measured.x = styledDimension(child.style.width, child.style.minWidth, child.measured.x, availableCross);
        applyCrossAxisSizing(child.measured, child.style, FlexDirection::Column, availableCross,
                             crossAlignment(parentStyle, child.style, FlexDirection::Column));
        if (child.style.height.isAuto() && !child.style.aspectRatio) {
            Widget* childNode = child.node.get();
            if (!childNode || childNode->parent() != &node) continue;
            const WidgetRef<Widget> childLifetime(childNode);
            const std::uint64_t childRevision = childNode->mLayoutInvalidationRevision;
            child.measured.y = LayoutEngine::measure(*childLifetime.get(), pass, child.measured.x).y;
            childNode = childLifetime.get();
            if (!nodeValid()
                || !childNode
                || childNode->parent() != nodeState.get()
                || childNode->mLayoutInvalidationRevision != childRevision) {
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

std::optional<std::vector<ChildLayout>> LayoutEngine::layoutChildren(Widget& parent, bool flexParent, const Rect& content, LayoutPass& pass) {
    if (!flexParent) {
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
    const StylePass::ChildSnapshot children = orderedChildren(parent, pass);
    for (const WidgetRef<Widget>& childRef : *children) {
        Widget* child = childRef.get();
        if (!child || child->parent() != &parent) continue;
        const std::uint64_t childRevision = child->mLayoutInvalidationRevision;
        const Style& style = pass.style(*child);
        if (!child->isDisplayed(style)) continue;
        Widget* currentParent = parentState.get();
        child = childRef.get();
        if (!parentState.valid()
            || !currentParent
            || !child
            || child->parent() != currentParent
            || child->mLayoutInvalidationRevision != childRevision)
            continue;
        warnIgnoredPosition(*child, style, pass.style(parent).flexDirection);
        const bool cacheMatches = child->mLayoutCache.intrinsicValid
            && !child->mInvalidationReasons.intersects(kMeasureInvalidationReasons)
            && child->mLayoutCache.layoutContext == contextKey;
        Vec2 measured = cacheMatches ? child->mLayoutCache.intrinsicSize : LayoutEngine::measure(*child, pass);
        currentParent = parentState.get();
        child = childRef.get();
        if (!parentState.valid() || !currentParent || !child || child->parent() != currentParent) continue;
        result.push_back({childRef, style, measured, measured});
    }
    return result;
}

void LayoutEngine::arrangeNode(Widget& node, LayoutDirection direction, LayoutPass& pass) {
    const LayoutContextKey contextKey = pass.contextKey();
    const bool cacheMatches =
        node.mLayoutCache.arrangeValid && node.mLayoutCache.layoutContext == contextKey && node.mLayoutCache.direction == direction;
    if (!node.mInvalidationReasons.intersects(kArrangeInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return;
    }

    pass.recordArranged();
    const WidgetRef<Widget> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Widget* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Style& parentStyle = pass.style(node);
    Widget* styledNode = lifetime.get();
    if (!styledNode || styledNode->mSurface != surface || styledNode->mParent != parent || styledNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    const Rect panel = node.mRect;
    const Rect content(panel.x + parentStyle.padding.left, panel.y + parentStyle.padding.bottom,
                       std::max(0.f, panel.w - parentStyle.padding.horizontal()), std::max(0.f, panel.h - parentStyle.padding.vertical()));
    const bool flexParent = parentStyle.display == DisplayMode::Flex;
    std::optional<std::vector<ChildLayout>> childrenResult = layoutChildren(node, flexParent, content, pass);
    if (!childrenResult) return;
    std::vector<ChildLayout> children = std::move(*childrenResult);
    if (flexParent && parentStyle.flexDirection == FlexDirection::Row)
        arrangeRow(node, parentStyle, content, panel, children, direction, pass);
    else if (flexParent)
        arrangeColumn(node, parentStyle, content, panel, children, direction, pass);
    else
        arrangeNormal(node, parentStyle, content, children, direction, pass);

    Widget* arrangedNode = lifetime.get();
    if (!arrangedNode
        || arrangedNode->mSurface != surface
        || arrangedNode->mParent != parent
        || arrangedNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    node.onArranged(parentStyle);
    Widget* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision) return;
    current->mLayoutCache.layoutContext = contextKey;
    current->mLayoutCache.direction = direction;
    current->mLayoutCache.arrangeValid = true;
    current->mInvalidationReasons.remove(LayoutInvalidationReason::Arrange);
}

void LayoutEngine::arrangeRow(Widget& node, const Style& parentStyle, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                              LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const float availableCross = panel.h - parentStyle.padding.vertical();
    const RowSizing sizing = resolveRowSizes(node, parentStyle, panel, children, pass);
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
            Widget* current = child.node.get();
            Widget* currentNode = nodeState.get();
            if (!nodeState.valid()) return;
            if (!current || current->parent() != currentNode || !current->isDisplayed(child.style)) continue;
            Widget* next = index + 1 < end ? children[index + 1].node.get() : nullptr;
            if (next && (next->parent() != currentNode || !next->isDisplayed(children[index + 1].style))) next = nullptr;
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
                setArrangedRect(*current, {x - child.measured.x, y, child.measured.x, child.measured.y});
                x -= child.measured.x + margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x -= adjacencyGap;
                    x += adjacencyOverlap;
                }
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                setArrangedRect(*current, {x, y, child.measured.x, child.measured.y});
                x += child.measured.x + margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x += adjacencyGap;
                    x -= adjacencyOverlap;
                }
            }
            arrangeNode(*current, direction, pass);
            currentNode = nodeState.get();
            if (!nodeState.valid()) return;
        }
        lineTop = lineBottom - lineGap;
    }
}

void LayoutEngine::arrangeColumn(Widget& node, const Style& parentStyle, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                                 LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const MainAxisAllocation allocation = resolveColumnSizes(node, parentStyle, panel, children, pass);
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
        Widget* current = child.node.get();
        Widget* currentNode = nodeState.get();
        if (!nodeState.valid()) return;
        if (!current || current->parent() != currentNode || !current->isDisplayed(child.style)) continue;
        Widget* previous = i ? children[i - 1].node.get() : nullptr;
        if (previous && (previous->parent() != currentNode || !previous->isDisplayed(children[i - 1].style))) previous = nullptr;
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
        setArrangedRect(*current, {x, y, width, child.measured.y});
        arrangeNode(*current, direction, pass);
        currentNode = nodeState.get();
        if (!nodeState.valid()) return;
        y -= margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? autoMargin : 0.f);
    }
}

void LayoutEngine::arrangeNormal(Widget& node, const Style& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
                                 LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const std::optional<float> availableWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
    const std::vector<layout_detail::NormalLine> lines = normalLines(children, availableWidth);
    float lineTop = content.top();
    const bool rtl = direction == LayoutDirection::RightToLeft;

    for (const layout_detail::NormalLine& line : lines) {
        const float lineBottom = lineTop - line.height;
        float x = rtl ? content.right() : content.left();
        for (std::size_t index = line.begin; index < line.end; ++index) {
            ChildLayout& child = children[index];
            Widget* childNode = child.node.get();
            Widget* currentNode = nodeState.get();
            if (!nodeState.valid()) return;
            if (!childNode || childNode->parent() != currentNode || !childNode->isDisplayed(child.style)) continue;

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
            const bool positioned = child.style.left || child.style.right || child.style.top || child.style.bottom;
            if (positioned) {
                setArrangedRect(*childNode, positionedRect(child, content, parentStyle.verticalAlign));
                arrangeNode(*childNode, direction, pass);
                currentNode = nodeState.get();
                if (!nodeState.valid()) return;
                continue;
            }

            float childY = lineBottom + margin.bottom.fixedPixels();
            if (line.block || child.style.verticalAlign == VerticalAlign::Top) {
                childY = lineTop - margin.top.fixedPixels() - height;
            } else {
                const float freeSpace = std::max(0.f, line.height - height - margin.vertical());
                childY = lineBottom + margin.bottom.fixedPixels() + verticalAlignmentOffset(child.style.verticalAlign, freeSpace);
            }
            if (child.style.top) childY -= child.style.top->resolve(content.h);
            else if (child.style.bottom) childY += child.style.bottom->resolve(content.h);

            if (childNode->mRectExplicit) {
                setArrangedRect(*childNode, positionedRect(child, content, parentStyle.verticalAlign));
            } else {
                setArrangedRect(*childNode, {childX, childY, width, height});
            }
            arrangeNode(*childNode, direction, pass);
            currentNode = nodeState.get();
            if (!nodeState.valid()) return;
        }
        lineTop = lineBottom;
    }
}
} // namespace radia::ui
