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

namespace rdui {
using namespace layout_detail;

Vec2 LayoutEngine::measure(Widget& node, LayoutPass& pass, std::optional<float> outer_width, std::optional<float> outer_height) {
    const LayoutContextKey context_key = pass.contextKey();
    const StyleSheet& theme = pass.theme();
    const TextMetrics& text_metrics = pass.textMetrics();
    if (node.mVisibility == Visibility::Collapsed) {
        node.mLayoutCache.measured_size = {};
        node.mLayoutCache.measured_width = outer_width.value_or(0.f);
        node.mLayoutCache.measured_height = outer_height.value_or(0.f);
        node.mLayoutCache.measured_width_set = outer_width.has_value();
        node.mLayoutCache.measured_height_set = outer_height.has_value();
        node.mLayoutCache.measured_rect_explicit = node.mRectExplicit;
        node.mLayoutCache.measured_rect_constraint_set = node.mRectExplicit;
        node.mLayoutCache.measured_rect_width = node.mRect.w;
        node.mLayoutCache.measured_rect_height = node.mRect.h;
        node.mLayoutCache.layout_context = context_key;
        node.mLayoutCache.measure_valid = true;
        if (!outer_width && !outer_height) node.mDesiredSize = {};
        node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
        return {};
    }

    const bool width_matches =
        node.mLayoutCache.measured_width_set == outer_width.has_value() && (!outer_width || node.mLayoutCache.measured_width == *outer_width);
    const bool height_matches =
        node.mLayoutCache.measured_height_set == outer_height.has_value() && (!outer_height || node.mLayoutCache.measured_height == *outer_height);
    const bool rect_mode_matches = node.mLayoutCache.measured_rect_explicit == node.mRectExplicit;
    const bool rect_constraint_matches = !node.mLayoutCache.measured_rect_constraint_set
        || (node.mRectExplicit && node.mLayoutCache.measured_rect_width == node.mRect.w && node.mLayoutCache.measured_rect_height == node.mRect.h);
    const bool cache_matches = node.mLayoutCache.measure_valid
        && width_matches
        && height_matches
        && rect_mode_matches
        && rect_constraint_matches
        && node.mLayoutCache.layout_context == context_key;
    const bool cache_context_matches = node.mLayoutCache.layout_context == context_key;
    if (node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) || !cache_context_matches || !rect_mode_matches || !rect_constraint_matches)
        node.mLayoutCache.intrinsic_valid = false;
    if (!node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) && cache_matches) {
        pass.recordSkipped();
        return node.mLayoutCache.measured_size;
    }

    pass.recordMeasured(outer_width.has_value() || outer_height.has_value());
    node.mLayoutCache.arrange_valid = false;

    const NodeSnapshot styled_state(node);
    const Style& style = pass.style(node);
    if (!styled_state.valid()) return {};
    const Flow flow = style.flow;
    std::optional<float> resolved_width = outer_width;
    if (!resolved_width && !style.width.isAuto() && !style.width.isPercentage()) resolved_width = styledDimension(style.width, style.min_width, 0.f);
    if (!resolved_width && node.mRectExplicit && style.width.isAuto()) resolved_width = std::max(0.f, node.mRect.w);
    if (!resolved_width && node.mRectExplicit && flow == Flow::Free && style.width.isPercentage()) resolved_width = std::max(0.f, node.mRect.w);
    std::optional<float> resolved_height = outer_height;
    if (!resolved_height && !style.height.isAuto() && !style.height.isPercentage())
        resolved_height = styledDimension(style.height, style.min_height, 0.f);
    if (!resolved_height && node.mRectExplicit && style.height.isAuto()) resolved_height = std::max(0.f, node.mRect.h);
    if (!resolved_height && node.mRectExplicit && flow == Flow::Free && style.height.isPercentage()) resolved_height = std::max(0.f, node.mRect.h);
    const IntrinsicSizeConstraints constraints{resolved_width, resolved_height};
    const WidgetRef<Widget> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Widget* parent = node.mParent;
    const std::uint64_t layout_revision = node.mLayoutInvalidationRevision;
    const Vec2 intrinsic = node.intrinsicSize(theme, style, text_metrics, constraints);
    Widget* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layout_revision) return {};
    Vec2 content;
    if (flow == Flow::Row) content = measureRow(node, style, intrinsic, resolved_width, resolved_height, pass);
    else if (flow == Flow::Column) content = measureColumn(node, style, intrinsic, resolved_width, resolved_height, pass);
    else content = measureFree(node, style, intrinsic, resolved_width, resolved_height, pass);
    current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layout_revision)
        return content;

    const Vec2 natural(content.x + style.padding.horizontal(), content.y + style.padding.vertical());
    const bool authored_width = !style.width.isAuto() && !style.width.isPercentage();
    const bool authored_height = !style.height.isAuto() && !style.height.isPercentage();
    const bool explicit_free_width = node.mRectExplicit && flow == Flow::Free && style.width.isPercentage();
    const bool explicit_free_height = node.mRectExplicit && flow == Flow::Free && style.height.isPercentage();
    float desired_width = styledDimension(style.width, style.min_width, natural.x, resolved_width.value_or(0.f));
    if (explicit_free_width) desired_width = styledDimension(style.width, style.min_width, natural.x, *resolved_width);
    else if (resolved_width && (outer_width || authored_width))
        desired_width = styledDimension(Dimension::fromPixels(*resolved_width), style.min_width, natural.x);
    float desired_height = styledDimension(style.height, style.min_height, natural.y, resolved_height.value_or(0.f));
    if (explicit_free_height) desired_height = styledDimension(style.height, style.min_height, natural.y, *resolved_height);
    else if (resolved_height && (outer_height || authored_height))
        desired_height = styledDimension(Dimension::fromPixels(*resolved_height), style.min_height, natural.y);
    const Vec2 desired = {desired_width, desired_height};
    node.mLayoutCache.measured_size = desired;
    node.mLayoutCache.measured_width = outer_width.value_or(0.f);
    node.mLayoutCache.measured_height = outer_height.value_or(0.f);
    node.mLayoutCache.measured_width_set = outer_width.has_value();
    node.mLayoutCache.measured_height_set = outer_height.has_value();
    node.mLayoutCache.measured_rect_explicit = node.mRectExplicit;
    node.mLayoutCache.measured_rect_constraint_set = node.mRectExplicit;
    node.mLayoutCache.measured_rect_width = node.mRect.w;
    node.mLayoutCache.measured_rect_height = node.mRect.h;
    node.mLayoutCache.layout_context = context_key;
    node.mLayoutCache.measure_valid = true;
    if (!outer_width && !outer_height) {
        node.mLayoutCache.intrinsic_size = desired;
        node.mLayoutCache.intrinsic_valid = true;
        node.mDesiredSize = desired;
    }
    node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
    return desired;
}

ChildLayout LayoutEngine::measureChild(Widget& parent, Widget& child, const Style& parent_style, Flow flow, std::optional<float> resolved_width,
                                       std::optional<float> resolved_height, LayoutPass& pass) {
    const NodeSnapshot parent_state(parent);
    const NodeSnapshot child_state(child);
    const auto valid = [&]() {
        Widget* current_parent = parent_state.get();
        Widget* current_child = child_state.get();
        return parent_state.valid()
            && child_state.valid()
            && current_parent
            && current_child
            && current_child->parent() == current_parent
            && current_child->mSurface == parent_state.surface;
    };

    const Style& child_style = pass.style(child);
    if (!valid()) return invalidChildLayout();
    std::optional<float> child_width;
    std::optional<float> child_height;
    if (resolved_width && child_style.width.isPercentage())
        child_width =
            styledDimension(child_style.width, child_style.min_width, 0.f, std::max(0.f, *resolved_width - parent_style.padding.horizontal()));
    if (resolved_height && child_style.height.isPercentage())
        child_height =
            styledDimension(child_style.height, child_style.min_height, 0.f, std::max(0.f, *resolved_height - parent_style.padding.vertical()));
    Vec2 child_size = measure(*child_state.get(), pass, child_width, child_height);
    if (!valid()) return invalidChildLayout();
    Widget* current_child = child_state.get();
    Widget* current_parent = parent_state.get();
    if (current_child->mRectExplicit) {
        if (child_style.width.isAuto()) child_size.x = current_child->mRect.w;
        if (child_style.height.isAuto()) child_size.y = current_child->mRect.h;
    }
    if ((flow == Flow::Row || flow == Flow::Column) && child_style.aspect_ratio) {
        std::optional<float> cross_size;
        if (flow == Flow::Row) {
            if (resolved_height) cross_size = resolved_height;
            else if (!parent_style.height.isAuto()) cross_size = parent_style.height.resolve(0.f, current_parent->mRect.h);
            else if (current_parent->mRectExplicit) cross_size = current_parent->mRect.h;
        } else {
            if (resolved_width) cross_size = resolved_width;
            else if (!parent_style.width.isAuto()) cross_size = parent_style.width.resolve(0.f, current_parent->mRect.w);
            else if (current_parent->mRectExplicit) cross_size = current_parent->mRect.w;
        }
        if (cross_size) {
            const float padding = flow == Flow::Row ? parent_style.padding.vertical() : parent_style.padding.horizontal();
            applyCrossAxisSizing(child_size, child_style, flow, std::max(0.f, *cross_size - padding),
                                 crossAlignment(parent_style, child_style, flow));
        }
    }

    Vec2 child_automatic_minimum = child_size;
    if (flow == Flow::Column && resolved_width) {
        const float available_cross = std::max(0.f, *resolved_width - parent_style.padding.horizontal());
        child_size.x = styledDimension(child_style.width, child_style.min_width, child_size.x, available_cross);
        applyCrossAxisSizing(child_size, child_style, flow, available_cross, crossAlignment(parent_style, child_style, flow));
        if (child_style.height.isAuto() && !child_style.aspect_ratio) {
            child_size.y = measure(*child_state.get(), pass, child_size.x).y;
            if (!valid()) return invalidChildLayout();
        }
        child_automatic_minimum = child_size;
    }

    if ((flow == Flow::Row || flow == Flow::Column) && !child_style.flex_basis.isAuto()) {
        const Dimension& parent_dimension = flow == Flow::Row ? parent_style.width : parent_style.height;
        const std::optional<float> resolved_parent = flow == Flow::Row ? resolved_width : resolved_height;
        const float rect_size = flow == Flow::Row ? current_parent->mRect.w : current_parent->mRect.h;
        const float padding = flow == Flow::Row ? parent_style.padding.horizontal() : parent_style.padding.vertical();
        const bool definite_parent = resolved_parent || !parent_dimension.isAuto() || parent.mRectExplicit;
        const bool percentage_is_auto = child_style.flex_basis.isPercentage() && !definite_parent;
        if (!percentage_is_auto) {
            const float parent_size = resolved_parent.value_or(parent_dimension.isAuto() ? rect_size : parent_dimension.resolve(0.f, rect_size));
            const float reference = std::max(0.f, parent_size - padding);
            const float basis = child_style.flex_basis.resolve(0.f, reference);
            const std::optional<Length>& authored_minimum = flow == Flow::Row ? child_style.min_width : child_style.min_height;
            const float automatic_minimum = flow == Flow::Row ? child_automatic_minimum.x : child_automatic_minimum.y;
            const float minimum = authored_minimum ? authored_minimum->resolve(reference) : std::min(automatic_minimum, basis);
            if (flow == Flow::Row) child_size.x = std::max(basis, minimum);
            else child_size.y = std::max(basis, minimum);
        }
    }
    current_child = child_state.get();
    if (!valid() || !current_child) return invalidChildLayout();
    return {child_state.lifetime, child_style, child_automatic_minimum, child_size};
}

Vec2 LayoutEngine::measureRow(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                              std::optional<float> resolved_height, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    Vec2 content;
    float row_width = intrinsic.x;
    float row_height = intrinsic.y;
    std::size_t row_children = 0;
    std::size_t row_lines = 0;
    std::vector<ChildLayout> row_layouts;
    WidgetRef<Widget> previous_child;
    const float fixed_gap = style.gap.fixedPixels();
    const auto finishRow = [&] {
        if (!row_children && intrinsic.x == 0.f && intrinsic.y == 0.f) return;
        content.x = std::max(content.x, row_width);
        if (row_lines) content.y += fixed_gap;
        content.y += row_height;
        ++row_lines;
        row_width = 0.f;
        row_height = 0.f;
        row_children = 0;
        previous_child.set(nullptr);
    };

    const StylePass::ChildSnapshot children = orderedChildren(node, pass);
    for (const WidgetRef<Widget>& child_ref : *children) {
        Widget* child_ptr = child_ref.get();
        if (!child_ptr || child_ptr->parent() != &node || child_ptr->visibility() == Visibility::Collapsed) continue;
        ChildLayout measured = measureChild(node, *child_ptr, style, Flow::Row, resolved_width, resolved_height, pass);
        Widget* current_node = node_state.get();
        Widget* measured_child = measured.node.get();
        if (!node_state.valid()) return content;
        if (!measured_child || measured_child->parent() != current_node || measured_child->visibility() == Visibility::Collapsed) continue;
        const float child_outer_width = measured.measured.x + measured.style.margin.horizontal();
        const float outer_height = measured.measured.y + measured.style.margin.vertical();
        row_layouts.push_back(measured);
        if (measured_child->flowBreakBefore() && row_children) finishRow();
        if (Widget* previous = previous_child.get(); previous && previous->parent() == current_node) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(node_state, previous, measured_child, style);
            if (!adjacent) return content;
            if (adjacent->has_gap) row_width += fixed_gap;
            row_width -= adjacent->overlap;
        }
        row_width += child_outer_width;
        row_height = std::max(row_height, outer_height);
        ++row_children;
        previous_child.set(measured_child);
    }

    if (row_children || !row_lines) finishRow();
    if (resolved_width && !row_layouts.empty()) {
        const float available_main = std::max(0.f, *resolved_width - style.padding.horizontal());
        prepareMainAxis(row_layouts, Flow::Row, available_main);
        const RowSizing sizing = allocateRowLines(node, row_layouts, style, available_main, pass);
        content.y = 0.f;
        for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
            if (line) content.y += fixed_gap;
            float height = line == 0 ? intrinsic.y : 0.f;
            const auto [begin, end] = sizing.lines[line];
            for (std::size_t index = begin; index < end; ++index)
                height = std::max(height, row_layouts[index].measured.y + row_layouts[index].style.margin.vertical());
            content.y += height;
        }
    }
    return content;
}

Vec2 LayoutEngine::measureColumn(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                                 std::optional<float> resolved_height, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    Vec2 content = intrinsic;
    WidgetRef<Widget> previous_child;
    const float fixed_gap = style.gap.fixedPixels();
    const StylePass::ChildSnapshot children = orderedChildren(node, pass);
    for (const WidgetRef<Widget>& child_ref : *children) {
        Widget* child_ptr = child_ref.get();
        if (!child_ptr || child_ptr->parent() != &node || child_ptr->visibility() == Visibility::Collapsed) continue;
        ChildLayout measured = measureChild(node, *child_ptr, style, Flow::Column, resolved_width, resolved_height, pass);
        Widget* current_node = node_state.get();
        Widget* measured_child = measured.node.get();
        if (!node_state.valid()) return content;
        if (!measured_child || measured_child->parent() != current_node || measured_child->visibility() == Visibility::Collapsed) continue;
        const float child_outer_width = measured.measured.x + measured.style.margin.horizontal();
        const float outer_height = measured.measured.y + measured.style.margin.vertical();
        if (Widget* previous = previous_child.get(); previous && previous->parent() == current_node) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(node_state, previous, measured_child, style);
            if (!adjacent) return content;
            if (adjacent->has_gap) content.y += fixed_gap;
            content.y -= adjacent->overlap;
        }
        content.x = std::max(content.x, child_outer_width);
        content.y += outer_height;
        previous_child.set(measured_child);
    }
    return content;
}

Vec2 LayoutEngine::measureFree(Widget& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolved_width,
                               std::optional<float> resolved_height, LayoutPass& pass) {
    const NodeSnapshot node_state(node);
    Vec2 content = intrinsic;
    std::vector<WidgetRef<Widget>> children;
    children.reserve(node.mChildren.size());
    for (const auto& child_ptr : node.mChildren) children.emplace_back(child_ptr.get());
    const std::optional<float> content_width =
        resolved_width ? std::optional<float>(std::max(0.f, *resolved_width - style.padding.horizontal())) : std::nullopt;
    const std::optional<float> content_height =
        resolved_height ? std::optional<float>(std::max(0.f, *resolved_height - style.padding.vertical())) : std::nullopt;
    for (const WidgetRef<Widget>& child_ref : children) {
        Widget* child_ptr = child_ref.get();
        if (!child_ptr || child_ptr->parent() != &node) continue;
        if (child_ptr->visibility() == Visibility::Collapsed) continue;
        const WidgetRef<Widget> child_lifetime(child_ptr);
        const std::uint64_t child_revision = child_ptr->mLayoutInvalidationRevision;
        const Style& child_style = pass.style(*child_ptr);
        if (!node_state.valid()
            || !child_lifetime
            || child_lifetime.get()->parent() != node_state.get()
            || child_lifetime.get()->mLayoutInvalidationRevision != child_revision)
            return content;
        const std::optional<float> child_width = content_width && child_style.width.isPercentage()
            ? std::optional<float>(styledDimension(child_style.width, child_style.min_width, 0.f, *content_width))
            : std::nullopt;
        const std::optional<float> child_height = content_height && child_style.height.isPercentage()
            ? std::optional<float>(styledDimension(child_style.height, child_style.min_height, 0.f, *content_height))
            : std::nullopt;
        Vec2 child_size = measure(*child_lifetime.get(), pass, child_width, child_height);
        Widget* current_node = node_state.get();
        Widget* current_child = child_lifetime.get();
        if (!node_state.valid()
            || !current_node
            || !current_child
            || current_child->parent() != current_node
            || current_child->mLayoutInvalidationRevision != child_revision)
            return content;
        if (current_child->mRectExplicit) {
            if (child_style.width.isAuto()) child_size.x = current_child->mRect.w;
            if (child_style.height.isAuto()) child_size.y = current_child->mRect.h;
        }
        const float child_outer_width = child_size.x + child_style.margin.horizontal();
        const float outer_height = child_size.y + child_style.margin.vertical();
        const float horizontal_reference = content_width.value_or(0.f);
        const float vertical_reference = content_height.value_or(0.f);
        const float horizontal_offset = child_style.left ? child_style.left->resolve(horizontal_reference)
            : child_style.right                          ? child_style.right->resolve(horizontal_reference)
            : current_child->mRectExplicit               ? current_child->mRect.x
                                                         : child_style.margin.left.fixedPixels();
        const float vertical_offset = child_style.top ? child_style.top->resolve(vertical_reference)
            : child_style.bottom                      ? child_style.bottom->resolve(vertical_reference)
            : current_child->mRectExplicit            ? current_child->mRect.y
                                                      : child_style.margin.top.fixedPixels();
        content.x = std::max(content.x, horizontal_offset + child_outer_width);
        content.y = std::max(content.y, vertical_offset + outer_height);
    }
    return content;
}

bool LayoutEngine::remeasureRowChildren(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass) {
    const NodeSnapshot parent_state(parent);
    for (std::size_t index = begin; index < end; ++index) {
        ChildLayout& child = children[index];
        if (!child.style.height.isAuto() || child.style.aspect_ratio) continue;
        Widget* node = child.node.get();
        if (!node || node->parent() != &parent) continue;
        const WidgetRef<Widget> node_lifetime(node);
        const std::uint64_t node_revision = node->mLayoutInvalidationRevision;
        child.measured.y = measure(*node_lifetime.get(), pass, child.measured.x).y;
        Widget* current_parent = parent_state.get();
        node = node_lifetime.get();
        if (!parent_state.valid()
            || !node
            || !current_parent
            || node->parent() != current_parent
            || node->mSurface != parent_state.surface
            || node->mLayoutInvalidationRevision != node_revision)
            return false;
        child.fit_size.y = child.measured.y;
    }
    return true;
}

bool LayoutEngine::remeasureColumnChildren(Widget& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initial_sizes,
                                           LayoutPass& pass) {
    const NodeSnapshot parent_state(parent);
    for (std::size_t index = 0; index < children.size(); ++index) {
        ChildLayout& child = children[index];
        if (std::abs(child.measured.x - initial_sizes[index].x) <= 1.0e-4f && std::abs(child.measured.y - initial_sizes[index].y) <= 1.0e-4f)
            continue;
        Widget* node = child.node.get();
        if (!node || node->parent() != &parent) continue;
        const WidgetRef<Widget> node_lifetime(node);
        const std::uint64_t node_revision = node->mLayoutInvalidationRevision;
        const Vec2 constrained = measure(*node_lifetime.get(), pass, child.measured.x, child.measured.y);
        Widget* current_parent = parent_state.get();
        node = node_lifetime.get();
        if (!parent_state.valid()
            || !node
            || !current_parent
            || node->parent() != current_parent
            || node->mSurface != parent_state.surface
            || node->mLayoutInvalidationRevision != node_revision)
            return false;
        child.measured = constrained;
    }
    return true;
}

LayoutEngine::RowSizing LayoutEngine::allocateRowLines(Widget& parent, std::vector<ChildLayout>& children, const Style& parent_style,
                                                       float available_main, LayoutPass& pass) {
    RowSizing sizing;
    sizing.lines = rowLines(children);
    sizing.allocations.reserve(sizing.lines.size());
    for (const auto& [begin, end] : sizing.lines) {
        const MainAxisAllocation allocation = allocateMainAxis(parent, children, begin, end, parent_style, Flow::Row, available_main);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(parent, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }
    return sizing;
}
} // namespace rdui
