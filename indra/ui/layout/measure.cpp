/**
 * @file measure.cpp
 * @brief Intrinsic measurement and constrained child sizing.
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

namespace radia::ui {
using layout_detail::AdjacentLayout;
using layout_detail::adjacentLayout;
using layout_detail::allocateMainAxis;
using layout_detail::applyCrossAxisSizing;
using layout_detail::ChildLayout;
using layout_detail::crossAlignment;
using layout_detail::invalidChildLayout;
using layout_detail::MainAxisAllocation;
using layout_detail::prepareMainAxis;
using layout_detail::rowLines;
using layout_detail::styledDimension;

Vec2 LayoutEngine::measure(Widget& node, LayoutPass& pass, std::optional<float> outerWidth, std::optional<float> outerHeight) {
    const LayoutContextKey contextKey = pass.contextKey();
    const StyleSheet& styleSheet = pass.styleSheet();
    const TextMetrics& textMetrics = pass.textMetrics();
    if (node.mVisibility == Visibility::Collapsed) {
        node.mLayoutCache.measuredSize = {};
        node.mLayoutCache.measuredWidth = outerWidth.value_or(0.f);
        node.mLayoutCache.measuredHeight = outerHeight.value_or(0.f);
        node.mLayoutCache.measuredWidthSet = outerWidth.has_value();
        node.mLayoutCache.measuredHeightSet = outerHeight.has_value();
        node.mLayoutCache.measuredRectExplicit = node.mRectExplicit;
        node.mLayoutCache.measuredRectConstraintSet = node.mRectExplicit;
        node.mLayoutCache.measuredRectWidth = node.mRect.w;
        node.mLayoutCache.measuredRectHeight = node.mRect.h;
        node.mLayoutCache.layoutContext = contextKey;
        node.mLayoutCache.measureValid = true;
        if (!outerWidth && !outerHeight) node.mDesiredSize = {};
        node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
        return {};
    }

    const bool widthMatches =
        node.mLayoutCache.measuredWidthSet == outerWidth.has_value() && (!outerWidth || node.mLayoutCache.measuredWidth == *outerWidth);
    const bool heightMatches =
        node.mLayoutCache.measuredHeightSet == outerHeight.has_value() && (!outerHeight || node.mLayoutCache.measuredHeight == *outerHeight);
    const bool rectModeMatches = node.mLayoutCache.measuredRectExplicit == node.mRectExplicit;
    const bool rectConstraintMatches = !node.mLayoutCache.measuredRectConstraintSet
        || (node.mRectExplicit && node.mLayoutCache.measuredRectWidth == node.mRect.w && node.mLayoutCache.measuredRectHeight == node.mRect.h);
    const bool cacheMatches = node.mLayoutCache.measureValid
        && widthMatches
        && heightMatches
        && rectModeMatches
        && rectConstraintMatches
        && node.mLayoutCache.layoutContext == contextKey;
    const bool cacheContextMatches = node.mLayoutCache.layoutContext == contextKey;
    if (node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) || !cacheContextMatches || !rectModeMatches || !rectConstraintMatches)
        node.mLayoutCache.intrinsicValid = false;
    if (!node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return node.mLayoutCache.measuredSize;
    }

    pass.recordMeasured(outerWidth.has_value() || outerHeight.has_value());
    node.mLayoutCache.arrangeValid = false;

    const NodeSnapshot styledState(node);
    const Style& style = pass.style(node);
    if (!styledState.valid()) return {};
    const Flow flow = style.flow;
    std::optional<float> resolvedWidth = outerWidth;
    if (!resolvedWidth && !style.width.isAuto() && !style.width.isPercentage()) resolvedWidth = styledDimension(style.width, style.minWidth, 0.f);
    if (!resolvedWidth && node.mRectExplicit && style.width.isAuto()) resolvedWidth = std::max(0.f, node.mRect.w);
    if (!resolvedWidth && node.mRectExplicit && flow == Flow::Free && style.width.isPercentage()) resolvedWidth = std::max(0.f, node.mRect.w);
    std::optional<float> resolvedHeight = outerHeight;
    if (!resolvedHeight && !style.height.isAuto() && !style.height.isPercentage())
        resolvedHeight = styledDimension(style.height, style.minHeight, 0.f);
    if (!resolvedHeight && node.mRectExplicit && style.height.isAuto()) resolvedHeight = std::max(0.f, node.mRect.h);
    if (!resolvedHeight && node.mRectExplicit && flow == Flow::Free && style.height.isPercentage()) resolvedHeight = std::max(0.f, node.mRect.h);
    const IntrinsicSizeConstraints constraints{resolvedWidth, resolvedHeight};
    const WidgetRef<Widget> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Widget* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Vec2 intrinsic = node.intrinsicSize(styleSheet, style, textMetrics, constraints);
    Widget* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision) return {};
    Vec2 content;
    if (flow == Flow::Row) content = measureRow(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else if (flow == Flow::Column) content = measureColumn(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else content = measureFree(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision)
        return content;

    const Vec2 natural(content.x + style.padding.horizontal(), content.y + style.padding.vertical());
    const bool authoredWidth = !style.width.isAuto() && !style.width.isPercentage();
    const bool authoredHeight = !style.height.isAuto() && !style.height.isPercentage();
    const bool explicitFreeWidth = node.mRectExplicit && flow == Flow::Free && style.width.isPercentage();
    const bool explicitFreeHeight = node.mRectExplicit && flow == Flow::Free && style.height.isPercentage();
    float desiredWidth = styledDimension(style.width, style.minWidth, natural.x, resolvedWidth.value_or(0.f));
    if (explicitFreeWidth) desiredWidth = styledDimension(style.width, style.minWidth, natural.x, *resolvedWidth);
    else if (resolvedWidth && (outerWidth || authoredWidth))
        desiredWidth = styledDimension(Dimension::fromPixels(*resolvedWidth), style.minWidth, natural.x);
    float desiredHeight = styledDimension(style.height, style.minHeight, natural.y, resolvedHeight.value_or(0.f));
    if (explicitFreeHeight) desiredHeight = styledDimension(style.height, style.minHeight, natural.y, *resolvedHeight);
    else if (resolvedHeight && (outerHeight || authoredHeight))
        desiredHeight = styledDimension(Dimension::fromPixels(*resolvedHeight), style.minHeight, natural.y);
    const Vec2 desired = {desiredWidth, desiredHeight};
    node.mLayoutCache.measuredSize = desired;
    node.mLayoutCache.measuredWidth = outerWidth.value_or(0.f);
    node.mLayoutCache.measuredHeight = outerHeight.value_or(0.f);
    node.mLayoutCache.measuredWidthSet = outerWidth.has_value();
    node.mLayoutCache.measuredHeightSet = outerHeight.has_value();
    node.mLayoutCache.measuredRectExplicit = node.mRectExplicit;
    node.mLayoutCache.measuredRectConstraintSet = node.mRectExplicit;
    node.mLayoutCache.measuredRectWidth = node.mRect.w;
    node.mLayoutCache.measuredRectHeight = node.mRect.h;
    node.mLayoutCache.layoutContext = contextKey;
    node.mLayoutCache.measureValid = true;
    if (!outerWidth && !outerHeight) {
        node.mLayoutCache.intrinsicSize = desired;
        node.mLayoutCache.intrinsicValid = true;
        node.mDesiredSize = desired;
    }
    node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
    return desired;
}

ChildLayout LayoutEngine::measureChild(Widget& parent, Widget& child, const Style& parentStyle, Flow flow, std::optional<float> resolvedWidth,
                                       std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    const NodeSnapshot childState(child);
    const auto valid = [&]() {
        Widget* currentParent = parentState.get();
        Widget* currentChild = childState.get();
        return parentState.valid()
            && childState.valid()
            && currentParent
            && currentChild
            && currentChild->parent() == currentParent
            && currentChild->mSurface == parentState.surface;
    };

    const Style& childStyle = pass.style(child);
    if (!valid()) return invalidChildLayout();
    std::optional<float> childWidth;
    std::optional<float> childHeight;
    if (resolvedWidth && childStyle.width.isPercentage())
        childWidth = styledDimension(childStyle.width, childStyle.minWidth, 0.f, std::max(0.f, *resolvedWidth - parentStyle.padding.horizontal()));
    if (resolvedHeight && childStyle.height.isPercentage())
        childHeight = styledDimension(childStyle.height, childStyle.minHeight, 0.f, std::max(0.f, *resolvedHeight - parentStyle.padding.vertical()));
    Vec2 childSize = measure(*childState.get(), pass, childWidth, childHeight);
    if (!valid()) return invalidChildLayout();
    Widget* currentChild = childState.get();
    Widget* currentParent = parentState.get();
    if (currentChild->mRectExplicit) {
        if (childStyle.width.isAuto()) childSize.x = currentChild->mRect.w;
        if (childStyle.height.isAuto()) childSize.y = currentChild->mRect.h;
    }
    if ((flow == Flow::Row || flow == Flow::Column) && childStyle.aspectRatio) {
        std::optional<float> crossSize;
        if (flow == Flow::Row) {
            if (resolvedHeight) crossSize = resolvedHeight;
            else if (!parentStyle.height.isAuto()) crossSize = parentStyle.height.resolve(0.f, currentParent->mRect.h);
            else if (currentParent->mRectExplicit) crossSize = currentParent->mRect.h;
        } else {
            if (resolvedWidth) crossSize = resolvedWidth;
            else if (!parentStyle.width.isAuto()) crossSize = parentStyle.width.resolve(0.f, currentParent->mRect.w);
            else if (currentParent->mRectExplicit) crossSize = currentParent->mRect.w;
        }
        if (crossSize) {
            const float padding = flow == Flow::Row ? parentStyle.padding.vertical() : parentStyle.padding.horizontal();
            applyCrossAxisSizing(childSize, childStyle, flow, std::max(0.f, *crossSize - padding), crossAlignment(parentStyle, childStyle, flow));
        }
    }

    Vec2 childAutomaticMinimum = childSize;
    if (flow == Flow::Column && resolvedWidth) {
        const float availableCross = std::max(0.f, *resolvedWidth - parentStyle.padding.horizontal());
        childSize.x = styledDimension(childStyle.width, childStyle.minWidth, childSize.x, availableCross);
        applyCrossAxisSizing(childSize, childStyle, flow, availableCross, crossAlignment(parentStyle, childStyle, flow));
        if (childStyle.height.isAuto() && !childStyle.aspectRatio) {
            childSize.y = measure(*childState.get(), pass, childSize.x).y;
            if (!valid()) return invalidChildLayout();
        }
        childAutomaticMinimum = childSize;
    }

    if ((flow == Flow::Row || flow == Flow::Column) && !childStyle.flexBasis.isAuto()) {
        const Dimension& parentDimension = flow == Flow::Row ? parentStyle.width : parentStyle.height;
        const std::optional<float> resolvedParent = flow == Flow::Row ? resolvedWidth : resolvedHeight;
        const float rectSize = flow == Flow::Row ? currentParent->mRect.w : currentParent->mRect.h;
        const float padding = flow == Flow::Row ? parentStyle.padding.horizontal() : parentStyle.padding.vertical();
        const bool definiteParent = resolvedParent || !parentDimension.isAuto() || parent.mRectExplicit;
        const bool percentageIsAuto = childStyle.flexBasis.isPercentage() && !definiteParent;
        if (!percentageIsAuto) {
            const float parentSize = resolvedParent.value_or(parentDimension.isAuto() ? rectSize : parentDimension.resolve(0.f, rectSize));
            const float reference = std::max(0.f, parentSize - padding);
            const float basis = childStyle.flexBasis.resolve(0.f, reference);
            const std::optional<Length>& authoredMinimum = flow == Flow::Row ? childStyle.minWidth : childStyle.minHeight;
            const float automaticMinimum = flow == Flow::Row ? childAutomaticMinimum.x : childAutomaticMinimum.y;
            const float minimum = authoredMinimum ? authoredMinimum->resolve(reference) : std::min(automaticMinimum, basis);
            if (flow == Flow::Row) childSize.x = std::max(basis, minimum);
            else childSize.y = std::max(basis, minimum);
        }
    }
    currentChild = childState.get();
    if (!valid() || !currentChild) return invalidChildLayout();
    return {childState.lifetime, childStyle, childAutomaticMinimum, childSize};
}

Vec2 LayoutEngine::measureRow(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content;
    float rowWidth = intrinsic.x;
    float rowHeight = intrinsic.y;
    std::size_t rowChildren = 0;
    std::size_t rowLines = 0;
    std::vector<ChildLayout> rowLayouts;
    WidgetRef<Widget> previousChild;
    const float fixedGap = style.gap.fixedPixels();
    const auto finishRow = [&] {
        if (!rowChildren && intrinsic.x == 0.f && intrinsic.y == 0.f) return;
        content.x = std::max(content.x, rowWidth);
        if (rowLines) content.y += fixedGap;
        content.y += rowHeight;
        ++rowLines;
        rowWidth = 0.f;
        rowHeight = 0.f;
        rowChildren = 0;
        previousChild.set(nullptr);
    };

    const StylePass::ChildSnapshot children = orderedChildren(node, pass);
    for (const WidgetRef<Widget>& childRef : *children) {
        Widget* childPtr = childRef.get();
        if (!childPtr || childPtr->parent() != &node || childPtr->visibility() == Visibility::Collapsed) continue;
        ChildLayout measured = measureChild(node, *childPtr, style, Flow::Row, resolvedWidth, resolvedHeight, pass);
        Widget* currentNode = nodeState.get();
        Widget* measuredChild = measured.node.get();
        if (!nodeState.valid()) return content;
        if (!measuredChild || measuredChild->parent() != currentNode || measuredChild->visibility() == Visibility::Collapsed) continue;
        const float childOuterWidth = measured.measured.x + measured.style.margin.horizontal();
        const float outerHeight = measured.measured.y + measured.style.margin.vertical();
        rowLayouts.push_back(measured);
        if (measuredChild->flowBreakBefore() && rowChildren) finishRow();
        if (Widget* previous = previousChild.get(); previous && previous->parent() == currentNode) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previous, measuredChild, style);
            if (!adjacent) return content;
            if (adjacent->hasGap) rowWidth += fixedGap;
            rowWidth -= adjacent->overlap;
        }
        rowWidth += childOuterWidth;
        rowHeight = std::max(rowHeight, outerHeight);
        ++rowChildren;
        previousChild.set(measuredChild);
    }

    if (rowChildren || !rowLines) finishRow();
    if (resolvedWidth && !rowLayouts.empty()) {
        const float availableMain = std::max(0.f, *resolvedWidth - style.padding.horizontal());
        prepareMainAxis(rowLayouts, Flow::Row, availableMain);
        const RowSizing sizing = allocateRowLines(node, rowLayouts, style, availableMain, pass);
        content.y = 0.f;
        for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
            if (line) content.y += fixedGap;
            float height = line == 0 ? intrinsic.y : 0.f;
            const auto [begin, end] = sizing.lines[line];
            for (std::size_t index = begin; index < end; ++index)
                height = std::max(height, rowLayouts[index].measured.y + rowLayouts[index].style.margin.vertical());
            content.y += height;
        }
    }
    return content;
}

Vec2 LayoutEngine::measureColumn(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                                 std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content = intrinsic;
    WidgetRef<Widget> previousChild;
    const float fixedGap = style.gap.fixedPixels();
    const StylePass::ChildSnapshot children = orderedChildren(node, pass);
    for (const WidgetRef<Widget>& childRef : *children) {
        Widget* childPtr = childRef.get();
        if (!childPtr || childPtr->parent() != &node || childPtr->visibility() == Visibility::Collapsed) continue;
        ChildLayout measured = measureChild(node, *childPtr, style, Flow::Column, resolvedWidth, resolvedHeight, pass);
        Widget* currentNode = nodeState.get();
        Widget* measuredChild = measured.node.get();
        if (!nodeState.valid()) return content;
        if (!measuredChild || measuredChild->parent() != currentNode || measuredChild->visibility() == Visibility::Collapsed) continue;
        const float childOuterWidth = measured.measured.x + measured.style.margin.horizontal();
        const float outerHeight = measured.measured.y + measured.style.margin.vertical();
        if (Widget* previous = previousChild.get(); previous && previous->parent() == currentNode) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previous, measuredChild, style);
            if (!adjacent) return content;
            if (adjacent->hasGap) content.y += fixedGap;
            content.y -= adjacent->overlap;
        }
        content.x = std::max(content.x, childOuterWidth);
        content.y += outerHeight;
        previousChild.set(measuredChild);
    }
    return content;
}

Vec2 LayoutEngine::measureFree(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                               std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content = intrinsic;
    std::vector<WidgetRef<Widget>> children;
    children.reserve(node.mChildren.size());
    for (const auto& childPtr : node.mChildren) children.emplace_back(childPtr.get());
    const std::optional<float> contentWidth =
        resolvedWidth ? std::optional<float>(std::max(0.f, *resolvedWidth - style.padding.horizontal())) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(std::max(0.f, *resolvedHeight - style.padding.vertical())) : std::nullopt;
    for (const WidgetRef<Widget>& childRef : children) {
        Widget* childPtr = childRef.get();
        if (!childPtr || childPtr->parent() != &node) continue;
        if (childPtr->visibility() == Visibility::Collapsed) continue;
        const WidgetRef<Widget> childLifetime(childPtr);
        const std::uint64_t childRevision = childPtr->mLayoutInvalidationRevision;
        const Style& childStyle = pass.style(*childPtr);
        if (!nodeState.valid()
            || !childLifetime
            || childLifetime.get()->parent() != nodeState.get()
            || childLifetime.get()->mLayoutInvalidationRevision != childRevision)
            return content;
        const std::optional<float> childWidth = contentWidth && childStyle.width.isPercentage()
            ? std::optional<float>(styledDimension(childStyle.width, childStyle.minWidth, 0.f, *contentWidth))
            : std::nullopt;
        const std::optional<float> childHeight = contentHeight && childStyle.height.isPercentage()
            ? std::optional<float>(styledDimension(childStyle.height, childStyle.minHeight, 0.f, *contentHeight))
            : std::nullopt;
        Vec2 childSize = measure(*childLifetime.get(), pass, childWidth, childHeight);
        Widget* currentNode = nodeState.get();
        Widget* currentChild = childLifetime.get();
        if (!nodeState.valid()
            || !currentNode
            || !currentChild
            || currentChild->parent() != currentNode
            || currentChild->mLayoutInvalidationRevision != childRevision)
            return content;
        if (currentChild->mRectExplicit) {
            if (childStyle.width.isAuto()) childSize.x = currentChild->mRect.w;
            if (childStyle.height.isAuto()) childSize.y = currentChild->mRect.h;
        }
        const float childOuterWidth = childSize.x + childStyle.margin.horizontal();
        const float outerHeight = childSize.y + childStyle.margin.vertical();
        const float horizontalReference = contentWidth.value_or(0.f);
        const float verticalReference = contentHeight.value_or(0.f);
        const float horizontalOffset = childStyle.left ? childStyle.left->resolve(horizontalReference)
            : childStyle.right                         ? childStyle.right->resolve(horizontalReference)
            : currentChild->mRectExplicit              ? currentChild->mRect.x
                                                       : childStyle.margin.left.fixedPixels();
        const float verticalOffset = childStyle.top ? childStyle.top->resolve(verticalReference)
            : childStyle.bottom                     ? childStyle.bottom->resolve(verticalReference)
            : currentChild->mRectExplicit           ? currentChild->mRect.y
                                                    : childStyle.margin.top.fixedPixels();
        content.x = std::max(content.x, horizontalOffset + childOuterWidth);
        content.y = std::max(content.y, verticalOffset + outerHeight);
    }
    return content;
}

bool LayoutEngine::remeasureRowChildren(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = begin; index < end; ++index) {
        ChildLayout& child = children[index];
        if (!child.style.height.isAuto() || child.style.aspectRatio) continue;
        Widget* node = child.node.get();
        if (!node || node->parent() != &parent) continue;
        const WidgetRef<Widget> nodeLifetime(node);
        const std::uint64_t nodeRevision = node->mLayoutInvalidationRevision;
        child.measured.y = measure(*nodeLifetime.get(), pass, child.measured.x).y;
        Widget* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.valid()
            || !node
            || !currentParent
            || node->parent() != currentParent
            || node->mSurface != parentState.surface
            || node->mLayoutInvalidationRevision != nodeRevision)
            return false;
        child.fitSize.y = child.measured.y;
    }
    return true;
}

bool LayoutEngine::remeasureColumnChildren(Widget& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes,
                                           LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = 0; index < children.size(); ++index) {
        ChildLayout& child = children[index];
        if (std::abs(child.measured.x - initialSizes[index].x) <= 1.0e-4f && std::abs(child.measured.y - initialSizes[index].y) <= 1.0e-4f) continue;
        Widget* node = child.node.get();
        if (!node || node->parent() != &parent) continue;
        const WidgetRef<Widget> nodeLifetime(node);
        const std::uint64_t nodeRevision = node->mLayoutInvalidationRevision;
        const Vec2 constrained = measure(*nodeLifetime.get(), pass, child.measured.x, child.measured.y);
        Widget* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.valid()
            || !node
            || !currentParent
            || node->parent() != currentParent
            || node->mSurface != parentState.surface
            || node->mLayoutInvalidationRevision != nodeRevision)
            return false;
        child.measured = constrained;
    }
    return true;
}

LayoutEngine::RowSizing LayoutEngine::allocateRowLines(Widget& parent, std::vector<ChildLayout>& children, const Style& parentStyle,
                                                       float availableMain, LayoutPass& pass) {
    RowSizing sizing;
    sizing.lines = rowLines(children);
    sizing.allocations.reserve(sizing.lines.size());
    for (const auto& [begin, end] : sizing.lines) {
        const MainAxisAllocation allocation = allocateMainAxis(parent, children, begin, end, parentStyle, Flow::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(parent, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }
    return sizing;
}
} // namespace radia::ui
