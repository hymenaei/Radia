/**
 * @file arrange.cpp
 * @brief Flow arrangement and post-measure geometry.
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
#include "layout/layoutcontext.h"
#include "style/stylesheet.h"
#include "widgets/widget.h"

namespace rdui {
using layout_detail::AdjacentLayout;
using layout_detail::adjacentLayout;
using layout_detail::allocateMainAxis;
using layout_detail::applyCrossAxisSizing;
using layout_detail::ChildLayout;
using layout_detail::CrossAlignment;
using layout_detail::crossAlignment;
using layout_detail::MainAxisAllocation;
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
    const NodeSnapshot node_state(node);
    const auto node_valid = [&] { return node_state.valid(); };
    const float availableMain = panel.w - parentStyle.padding.horizontal();
    const float availableCross = panel.h - parentStyle.padding.vertical();
    prepareMainAxis(children, Flow::Row, availableMain);
    for (ChildLayout& child : children)
        child.measured.y = styledDimension(child.style.height, child.style.minHeight, child.measured.y, availableCross);

    RowSizing sizing;
    sizing.lines = rowLines(children);
    const auto& lines = sizing.lines;
    for (const auto& [begin, end] : lines) {
        float preliminary_height = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            preliminary_height = std::max(preliminary_height, children[index].measured.y + children[index].style.margin.vertical());
        if (lines.size() == 1 && availableCross >= 0.f) preliminary_height = availableCross;
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, Flow::Row, preliminary_height,
                                 crossAlignment(parentStyle, children[index].style, Flow::Row));
    }

    sizing.allocations.reserve(lines.size());
    for (const auto& [begin, end] : lines) {
        if (!node_valid()) {
            sizing.valid = false;
            return sizing;
        }
        const MainAxisAllocation allocation = allocateMainAxis(node, children, begin, end, parentStyle, Flow::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(node, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }

    if (!node_valid()) {
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
            applyCrossAxisSizing(children[index].measured, children[index].style, Flow::Row, sizing.lineHeights[line],
                                 crossAlignment(parentStyle, children[index].style, Flow::Row));
    }
    return sizing;
}

MainAxisAllocation LayoutEngine::resolveColumnSizes(Widget& node, const Style& parentStyle, const Rect& panel, std::vector<ChildLayout>& children,
                                                    LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    const auto node_valid = [&] { return node_state.valid(); };
    MainAxisAllocation allocation;
    const float availableMain = panel.h - parentStyle.padding.vertical();
    const float availableCross = panel.w - parentStyle.padding.horizontal();
    std::vector<Vec2> initialSizes;
    initialSizes.reserve(children.size());
    for (const ChildLayout& child : children) initialSizes.push_back(child.measured);
    for (ChildLayout& child : children) {
        if (!node_valid()) {
            allocation.valid = false;
            return allocation;
        }
        child.measured.x = styledDimension(child.style.width, child.style.minWidth, child.measured.x, availableCross);
        applyCrossAxisSizing(child.measured, child.style, Flow::Column, availableCross, crossAlignment(parentStyle, child.style, Flow::Column));
        if (child.style.height.isAuto() && !child.style.aspectRatio) {
            Widget* child_node = child.node.get();
            if (!child_node || child_node->parent() != &node) continue;
            const WidgetRef<Widget> child_lifetime(child_node);
            const std::uint64_t child_revision = child_node->mLayoutInvalidationRevision;
            child.measured.y = LayoutEngine::measure(*child_lifetime.get(), pass, child.measured.x).y;
            child_node = child_lifetime.get();
            if (!node_valid()
                || !child_node
                || child_node->parent() != node_state.get()
                || child_node->mLayoutInvalidationRevision != child_revision) {
                allocation.valid = false;
                return allocation;
            }
            child.fitSize.y = child.measured.y;
        }
    }
    prepareMainAxis(children, Flow::Column, availableMain);
    if (!node_valid()) {
        allocation.valid = false;
        return allocation;
    }
    allocation = allocateMainAxis(node, children, 0, children.size(), parentStyle, Flow::Column, availableMain);
    if (!allocation.valid || !remeasureColumnChildren(node, children, initialSizes, pass)) allocation.valid = false;
    return allocation;
}

std::vector<ChildLayout> LayoutEngine::flowChildren(Widget& parent, Flow flow, LayoutPass& pass) {
    const LayoutContextKey context_key = pass.contextKey();
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> result;
    result.reserve(parent.mChildren.size());
    const StylePass::ChildSnapshot children = orderedChildren(parent, pass);
    for (const WidgetRef<Widget>& child_ref : *children) {
        Widget* child = child_ref.get();
        if (!child || child->parent() != &parent || child->visibility() == Visibility::Collapsed) continue;
        const std::uint64_t child_revision = child->mLayoutInvalidationRevision;
        const Style& style = pass.style(*child);
        Widget* current_parent = parentState.get();
        child = child_ref.get();
        if (!parentState.valid()
            || !current_parent
            || !child
            || child->parent() != current_parent
            || child->mLayoutInvalidationRevision != child_revision)
            continue;
        warnIgnoredPosition(*child, style, flow);
        const bool cache_matches = child->mLayoutCache.intrinsicValid
            && !child->mInvalidationReasons.intersects(kMeasureInvalidationReasons)
            && child->mLayoutCache.layoutContext == context_key;
        const Vec2 measured = cache_matches ? child->mLayoutCache.intrinsicSize : LayoutEngine::measure(*child, pass);
        current_parent = parentState.get();
        child = child_ref.get();
        if (!parentState.valid() || !current_parent || !child || child->parent() != current_parent) continue;
        result.push_back({child_ref, style, measured, measured});
    }
    return result;
}

void LayoutEngine::arrangeNode(Widget& node, LayoutDirection direction, LayoutPass& pass) {
    const LayoutContextKey context_key = pass.contextKey();
    const bool cache_matches =
        node.mLayoutCache.arrangeValid && node.mLayoutCache.layoutContext == context_key && node.mLayoutCache.direction == direction;
    if (!node.mInvalidationReasons.intersects(kArrangeInvalidationReasons) && cache_matches) {
        pass.recordSkipped();
        return;
    }

    pass.recordArranged();
    const WidgetRef<Widget> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Widget* parent = node.mParent;
    const std::uint64_t layout_revision = node.mLayoutInvalidationRevision;
    const Style& parentStyle = pass.style(node);
    Widget* styled_node = lifetime.get();
    if (!styled_node
        || styled_node->mSurface != surface
        || styled_node->mParent != parent
        || styled_node->mLayoutInvalidationRevision != layout_revision)
        return;
    const Rect panel = node.mRect;
    const Rect content(panel.x + parentStyle.padding.left, panel.y + parentStyle.padding.bottom,
                       std::max(0.f, panel.w - parentStyle.padding.horizontal()), std::max(0.f, panel.h - parentStyle.padding.vertical()));
    const Flow flow = parentStyle.flow;
    std::vector<ChildLayout> children = flowChildren(node, flow, pass);
    if (flow == Flow::Row) arrangeRow(node, parentStyle, content, panel, children, direction, pass);
    else if (flow == Flow::Column) arrangeColumn(node, parentStyle, content, panel, children, direction, pass);
    else arrangeFree(node, parentStyle, content, children, direction, pass);

    Widget* arranged_node = lifetime.get();
    if (!arranged_node
        || arranged_node->mSurface != surface
        || arranged_node->mParent != parent
        || arranged_node->mLayoutInvalidationRevision != layout_revision)
        return;
    node.onArranged(parentStyle);
    Widget* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layout_revision) return;
    current->mLayoutCache.layoutContext = context_key;
    current->mLayoutCache.direction = direction;
    current->mLayoutCache.arrangeValid = true;
    current->mInvalidationReasons.remove(LayoutInvalidationReason::Arrange);
}

void LayoutEngine::arrangeRow(Widget& node, const Style& parentStyle, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                              LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    removeChildrenExcludedFromLayout(node, children);
    const float availableCross = panel.h - parentStyle.padding.vertical();
    const RowSizing sizing = resolveRowSizes(node, parentStyle, panel, children, pass);
    if (!sizing.valid) return;
    const auto& lines = sizing.lines;
    const auto& lineHeights = sizing.lineHeights;
    const float line_gap = parentStyle.gap.fixedPixels();
    float block_height = 0.f;
    for (float height : lineHeights) block_height += height;
    if (lineHeights.size() > 1) block_height += line_gap * static_cast<float>(lineHeights.size() - 1);
    float line_top = content.top() - verticalAlignmentOffset(parentStyle.verticalAlign, std::max(0.f, availableCross - block_height));

    for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
        const auto [begin, end] = sizing.lines[line];
        const float lineHeight = lineHeights[line];
        const float line_bottom = line_top - lineHeight;
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
            Widget* current_node = node_state.get();
            if (!node_state.valid()) return;
            if (!current || current->parent() != current_node || current->visibility() == Visibility::Collapsed) continue;
            Widget* next = index + 1 < end ? children[index + 1].node.get() : nullptr;
            if (next && (next->parent() != current_node || next->visibility() == Visibility::Collapsed)) next = nullptr;
            float adjacency_gap = 0.f;
            float adjacency_overlap = 0.f;
            if (next) {
                const std::optional<AdjacentLayout> adjacent = adjacentLayout(node_state, current, next, parentStyle);
                if (!adjacent) return;
                adjacency_gap = adjacent->hasGap ? gap : 0.f;
                adjacency_overlap = adjacent->overlap;
            }
            const MarginInsets& margin = child.style.margin;
            const float available_cross_space = lineHeight - child.measured.y - margin.vertical();
            const float cross_space = std::max(0.f, available_cross_space);
            const int cross_auto_count = margin.verticalAutoCount();
            const float cross_auto = cross_auto_count ? cross_space / static_cast<float>(cross_auto_count) : 0.f;
            float y = line_bottom + margin.bottom.fixedPixels();
            if (cross_auto_count) y += margin.bottom.isAuto() ? cross_auto : 0.f;
            else {
                const CrossAlignment alignment = crossAlignment(parentStyle, child.style, Flow::Row);
                if (alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch)
                    y = line_top - margin.top.fixedPixels() - child.measured.y;
                else if (alignment == CrossAlignment::Center) y += available_cross_space * .5f;
            }
            if (rtl) {
                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                setArrangedRect(*current, {x - child.measured.x, y, child.measured.x, child.measured.y});
                x -= child.measured.x + margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x -= adjacency_gap;
                    x += adjacency_overlap;
                }
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                setArrangedRect(*current, {x, y, child.measured.x, child.measured.y});
                x += child.measured.x + margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x += adjacency_gap;
                    x -= adjacency_overlap;
                }
            }
            arrangeNode(*current, direction, pass);
            current_node = node_state.get();
            if (!node_state.valid()) return;
        }
        line_top = line_bottom - line_gap;
    }
}

void LayoutEngine::arrangeColumn(Widget& node, const Style& parentStyle, const Rect& content, const Rect& panel, std::vector<ChildLayout>& children,
                                 LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
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
        Widget* current_node = node_state.get();
        if (!node_state.valid()) return;
        if (!current || current->parent() != current_node || current->visibility() == Visibility::Collapsed) continue;
        Widget* previous = i ? children[i - 1].node.get() : nullptr;
        if (previous && (previous->parent() != current_node || previous->visibility() == Visibility::Collapsed)) previous = nullptr;
        float adjacency_gap = 0.f;
        float adjacency_overlap = 0.f;
        if (previous) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(node_state, previous, current, parentStyle);
            if (!adjacent) return;
            adjacency_gap = adjacent->hasGap ? gap : 0.f;
            adjacency_overlap = adjacent->overlap;
        }
        const MarginInsets& margin = child.style.margin;
        if (previous) {
            y -= adjacency_gap;
            y += adjacency_overlap;
        }
        y -= margin.top.fixedPixels() + (margin.top.isAuto() ? autoMargin : 0.f);
        const int horizontal_auto_count = margin.horizontalAutoCount();
        const float width = child.measured.x;
        const float horizontalSpace = std::max(0.f, content.w - width - margin.horizontal());
        const float horizontal_auto = horizontal_auto_count ? horizontalSpace / static_cast<float>(horizontal_auto_count) : 0.f;
        float x = content.left() + margin.left.fixedPixels();
        if (horizontal_auto_count) x += margin.left.isAuto() ? horizontal_auto : 0.f;
        else {
            const CrossAlignment alignment = crossAlignment(parentStyle, child.style, Flow::Column);
            if (alignment == CrossAlignment::Center) x += horizontalSpace * .5f;
            else {
                const bool logical_start = alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch;
                const bool align_right = logical_start == (direction == LayoutDirection::RightToLeft);
                if (align_right) x = content.right() - margin.right.fixedPixels() - width;
            }
        }
        y -= child.measured.y;
        setArrangedRect(*current, {x, y, width, child.measured.y});
        arrangeNode(*current, direction, pass);
        current_node = node_state.get();
        if (!node_state.valid()) return;
        y -= margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? autoMargin : 0.f);
    }
}

void LayoutEngine::arrangeFree(Widget& node, const Style& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
                               LayoutDirection direction, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    removeChildrenExcludedFromLayout(node, children);
    for (ChildLayout& child : children) {
        Widget* child_node = child.node.get();
        Widget* current_node = node_state.get();
        if (!node_state.valid()) return;
        if (!child_node || child_node->parent() != current_node || child_node->visibility() == Visibility::Collapsed) continue;
        std::optional<float> width;
        std::optional<float> height;
        if (child.style.width.isPercentage()) width = styledDimension(child.style.width, child.style.minWidth, child.measured.x, content.w);
        if (child.style.height.isPercentage()) height = styledDimension(child.style.height, child.style.minHeight, child.measured.y, content.h);
        if (width || height) {
            const NodeSnapshot child_state(*child_node);
            const std::uint64_t child_revision = child_node->mLayoutInvalidationRevision;
            child.measured = measure(*child_state.get(), pass, width, height);
            child_node = child_state.get();
            current_node = node_state.get();
            if (!node_state.valid()
                || !child_state.valid()
                || !child_node
                || !current_node
                || child_node->parent() != current_node
                || child_node->mLayoutInvalidationRevision != child_revision)
                return;
        }
        setArrangedRect(*child_node, positionedRect(child, content, parentStyle.verticalAlign));
        arrangeNode(*child_node, direction, pass);
        current_node = node_state.get();
        if (!node_state.valid()) return;
    }
}
} // namespace rdui
