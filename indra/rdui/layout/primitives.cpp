/**
 * @file primitives.cpp
 * @brief Shared private layout math and geometry primitives.
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
#include "layout/primitives.h"
#include <algorithm>

namespace rdui::layout_detail {
float styledDimension(const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
    const float resolved = value.resolve(fallback, reference);
    return minimum ? std::max(resolved, minimum->resolve(reference)) : resolved;
}

void warnIgnoredPosition(const Widget& child, const Style& style, Flow flow) {
    if (flow == Flow::Free) return;
    const char* property = style.left ? "left" : style.right ? "right" : style.top ? "top" : style.bottom ? "bottom" : nullptr;
    if (!property) return;
    LL_WARNS("rdui")
        << "Ignoring '"
        << property
        << "' on <"
        << child.element()
        << (child.id().empty() ? "" : " id=\"" + child.id() + "\"")
        << ">: parent flow is '"
        << (flow == Flow::Row ? "row" : "column")
        << "'."
        << LL_ENDL;
}

const Style& emptyChildStyle() {
    static const Style empty;
    return empty;
}

ChildLayout invalidChildLayout() {
    return {WidgetRef<Widget>(), emptyChildStyle(), {}, {}};
}

void removeDetachedChildren(Widget& parent, std::vector<ChildLayout>& children) {
    if (std::all_of(children.begin(), children.end(), [&parent](const ChildLayout& child) {
            const Widget* node = child.node.get();
            return node && node->parent() == &parent && node->visibility() != Visibility::Collapsed;
        }))
        return;
    std::vector<ChildLayout> attached;
    attached.reserve(children.size());
    for (const ChildLayout& child : children) {
        const Widget* node = child.node.get();
        if (node && node->parent() == &parent && node->visibility() != Visibility::Collapsed) attached.push_back(child);
    }
    children.swap(attached);
}

float& mainSize(ChildLayout& child, Flow flow) {
    return flow == Flow::Row ? child.measured.x : child.measured.y;
}
float mainSize(const ChildLayout& child, Flow flow) {
    return flow == Flow::Row ? child.measured.x : child.measured.y;
}

float mainMinimum(const ChildLayout& child, Flow flow, float available_main, float flex_base) {
    const std::optional<Length>& minimum = flow == Flow::Row ? child.style.min_width : child.style.min_height;
    if (minimum) return minimum->resolve(available_main);
    const float automatic = flow == Flow::Row ? child.fit_size.x : child.fit_size.y;
    return std::min(automatic, flex_base);
}

void applyFlexBasis(ChildLayout& child, Flow flow, float available_main) {
    if (child.style.flex_basis.isAuto()) return;
    const float basis = child.style.flex_basis.resolve(0.f, available_main);
    mainSize(child, flow) = std::max(basis, mainMinimum(child, flow, available_main, basis));
}

void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, Flow flow, float available_main, bool allow_growth,
                         float& total) {
    const float free_space = available_main - total;
    if (free_space > 0.f && allow_growth) {
        float total_grow = 0.f;
        for (std::size_t index = begin; index < end; ++index) total_grow += children[index].style.flex_grow;
        if (total_grow <= 0.f) return;
        for (std::size_t index = begin; index < end; ++index)
            mainSize(children[index], flow) += free_space * children[index].style.flex_grow / total_grow;
        total += free_space;
        return;
    }
    if (free_space >= 0.f) return;

    float deficit = -free_space;
    std::vector<float> base_sizes;
    std::vector<bool> active;
    base_sizes.reserve(end - begin);
    active.reserve(end - begin);
    for (std::size_t index = begin; index < end; ++index) {
        const ChildLayout& child = children[index];
        const float base = mainSize(child, flow);
        base_sizes.push_back(base);
        active.push_back(child.style.flex_shrink > 0.f && base > mainMinimum(child, flow, available_main, base));
    }

    constexpr float epsilon = 1.0e-4f;
    while (deficit > epsilon) {
        float total_weight = 0.f;
        for (std::size_t offset = 0; offset < active.size(); ++offset)
            if (active[offset]) total_weight += children[begin + offset].style.flex_shrink * base_sizes[offset];
        if (total_weight <= epsilon) break;

        const float round_deficit = deficit;
        float reduced = 0.f;
        for (std::size_t offset = 0; offset < active.size(); ++offset) {
            if (!active[offset]) continue;
            ChildLayout& child = children[begin + offset];
            float& size = mainSize(child, flow);
            const float minimum = mainMinimum(child, flow, available_main, base_sizes[offset]);
            const float share = round_deficit * child.style.flex_shrink * base_sizes[offset] / total_weight;
            const float reduction = std::min(share, size - minimum);
            size -= reduction;
            reduced += reduction;
            if (size <= minimum + epsilon) active[offset] = false;
        }
        if (reduced <= epsilon) break;
        deficit -= reduced;
        total -= reduced;
    }
}

float verticalAlignmentOffset(VerticalAlign alignment, float free_space) {
    if (alignment == VerticalAlign::Middle) return free_space * .5f;
    if (alignment == VerticalAlign::Bottom) return free_space;
    return 0.f;
}

CrossAlignment crossAlignment(const Style& parent, const Style& child, Flow flow) {
    if (child.align_self != AlignSelf::Auto) {
        if (child.align_self == AlignSelf::Start) return CrossAlignment::Start;
        if (child.align_self == AlignSelf::Center) return CrossAlignment::Center;
        if (child.align_self == AlignSelf::End) return CrossAlignment::End;
        return CrossAlignment::Stretch;
    }
    if (parent.align_items == AlignItems::Start) return CrossAlignment::Start;
    if (parent.align_items == AlignItems::Center) return CrossAlignment::Center;
    if (parent.align_items == AlignItems::End) return CrossAlignment::End;
    if (parent.align_items == AlignItems::Stretch) return CrossAlignment::Stretch;
    if (flow == Flow::Column) return CrossAlignment::Stretch;
    if (parent.vertical_align == VerticalAlign::Middle) return CrossAlignment::Center;
    if (parent.vertical_align == VerticalAlign::Bottom) return CrossAlignment::End;
    return CrossAlignment::Start;
}

void applyCrossAxisSizing(Vec2& size, const Style& style, Flow flow, float available_cross, CrossAlignment alignment) {
    if (alignment != CrossAlignment::Stretch) return;
    if (flow == Flow::Row) {
        if (!style.height.isAuto() || style.margin.verticalAutoCount()) return;
        const float height = std::max(0.f, available_cross - style.margin.vertical());
        size.y = styledDimension(style.height, style.min_height, height, available_cross);
        if (style.aspect_ratio && style.width.isAuto() && *style.aspect_ratio > 0.f) size.x = size.y * *style.aspect_ratio;
    } else if (flow == Flow::Column) {
        if (!style.width.isAuto() || style.margin.horizontalAutoCount()) return;
        const float width = std::max(0.f, available_cross - style.margin.horizontal());
        size.x = styledDimension(style.width, style.min_width, width, available_cross);
        if (style.aspect_ratio && style.height.isAuto() && *style.aspect_ratio > 0.f) size.y = size.x / *style.aspect_ratio;
    }
}

float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float free_space) {
    if (alignment == JustifyContent::Center) return free_space * .5f;
    if (alignment == JustifyContent::End) return free_space;
    if (alignment == JustifyContent::Left) return direction == LayoutDirection::RightToLeft ? free_space : 0.f;
    if (alignment == JustifyContent::Right) return direction == LayoutDirection::RightToLeft ? 0.f : free_space;
    return 0.f;
}

Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign vertical_alignment) {
    const Widget* node = child.node.get();
    llassert(node);
    if (!node) return {};
    const bool explicit_rect = node->mRectExplicit;
    const float width = explicit_rect && child.style.width.isAuto()
        ? child.style.min_width ? std::max(node->mRect.w, child.style.min_width->resolve(parent.w)) : node->mRect.w
        : styledDimension(child.style.width, child.style.min_width, child.measured.x > 0.f ? child.measured.x : parent.w, parent.w);
    const float height = explicit_rect && child.style.height.isAuto()
        ? child.style.min_height ? std::max(node->mRect.h, child.style.min_height->resolve(parent.h)) : node->mRect.h
        : styledDimension(child.style.height, child.style.min_height, child.measured.y, parent.h);
    const MarginInsets& margin = child.style.margin;
    const float horizontal_space = std::max(0.f, parent.w - width - margin.horizontal());
    float x = explicit_rect ? node->mRect.x : margin.left.isAuto() ? parent.left() + horizontal_space : parent.left() + margin.left.fixedPixels();
    if (margin.left.isAuto() && margin.right.isAuto()) x = parent.left() + horizontal_space * .5f;
    const float vertical_space = std::max(0.f, parent.h - height - margin.vertical());
    float y = explicit_rect ? node->mRect.y
                            : parent.top() - margin.top.fixedPixels() - height - verticalAlignmentOffset(vertical_alignment, vertical_space);
    if (child.style.left) x = parent.left() + child.style.left->resolve(parent.w) + margin.left.fixedPixels();
    else if (child.style.right) x = parent.right() - child.style.right->resolve(parent.w) - margin.right.fixedPixels() - width;
    if (child.style.top) y = parent.top() - child.style.top->resolve(parent.h) - margin.top.fixedPixels() - height;
    else if (child.style.bottom) y = parent.bottom() + child.style.bottom->resolve(parent.h) + margin.bottom.fixedPixels();
    return {x, y, width, height};
}

void setArrangedRect(Widget& node, const Rect& rect) {
    const bool changed = node.mRect.x != rect.x || node.mRect.y != rect.y || node.mRect.w != rect.w || node.mRect.h != rect.h;
    if (!changed) return;
    node.mRect = rect;
    node.mLayoutCache.arrange_valid = false;
    node.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    node.invalidatePaint();
}

std::optional<AdjacentLayout> adjacentLayout(const WidgetVisit& parent_state, Widget* first, Widget* second, const Style& parent_style) {
    Widget* parent = parent_state.get();
    if (!parent_state.valid() || !first || !second || first->parent() != parent || second->parent() != parent) return std::nullopt;
    const WidgetVisit first_state(*first);
    const WidgetVisit second_state(*second);
    const bool has_gap = WidgetLayoutAccess::hasGap(*parent, *first, *second);
    parent = parent_state.get();
    first = first_state.get();
    second = second_state.get();
    if (!parent_state.valid() || !parent || !first_state.validChildOf(*parent) || !second_state.validChildOf(*parent)) return std::nullopt;
    const float overlap = WidgetLayoutAccess::overlap(*parent, *first, *second, parent_style);
    parent = parent_state.get();
    first = first_state.get();
    second = second_state.get();
    if (!parent_state.valid() || !parent || !first_state.validChildOf(*parent) || !second_state.validChildOf(*parent)) return std::nullopt;
    return AdjacentLayout{has_gap, overlap};
}

std::vector<std::pair<std::size_t, std::size_t>> rowLines(const std::vector<ChildLayout>& children) {
    std::vector<std::pair<std::size_t, std::size_t>> lines;
    std::size_t line_start = 0;
    for (std::size_t index = 0; index < children.size(); ++index) {
        const Widget* child = children[index].node.get();
        if (index > line_start && child && child->flowBreakBefore()) {
            lines.emplace_back(line_start, index);
            line_start = index;
        }
    }
    if (line_start < children.size()) lines.emplace_back(line_start, children.size());
    return lines;
}

MainAxisAllocation allocateMainAxis(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, const Style& parent_style,
                                    Flow flow, float available_main) {
    const WidgetVisit parent_state(parent);
    const auto invalid_allocation = [] {
        MainAxisAllocation invalid;
        invalid.valid = false;
        return invalid;
    };
    float total = 0.f;
    int auto_margins = 0;
    for (std::size_t index = begin; index < end; ++index) {
        const ChildLayout& child = children[index];
        total += mainSize(child, flow) + (flow == Flow::Row ? child.style.margin.horizontal() : child.style.margin.vertical());
        auto_margins += flow == Flow::Row ? child.style.margin.horizontalAutoCount() : child.style.margin.verticalAutoCount();
    }
    std::size_t gap_count = 0;
    float overlap = 0.f;
    for (std::size_t index = begin + 1; index < end; ++index) {
        Widget* previous = children[index - 1].node.get();
        Widget* current = children[index].node.get();
        if (!parent_state.valid()) return invalid_allocation();
        const std::optional<AdjacentLayout> adjacent = adjacentLayout(parent_state, previous, current, parent_style);
        if (!adjacent) return invalid_allocation();
        if (adjacent->has_gap) ++gap_count;
        overlap += adjacent->overlap;
    }
    MainAxisAllocation allocation;
    allocation.gap = parent_style.gap.fixedPixels();
    total += allocation.gap * static_cast<float>(gap_count) - overlap;
    distributeFlexSpace(children, begin, end, flow, available_main, !auto_margins && !parent_style.gap.isAuto(), total);
    allocation.free_space = available_main - total;
    if (!auto_margins && parent_style.gap.isAuto() && gap_count) {
        allocation.gap = std::max(0.f, allocation.free_space) / static_cast<float>(gap_count);
        total += allocation.gap * static_cast<float>(gap_count);
        allocation.free_space = available_main - total;
    }
    allocation.has_auto_margins = auto_margins != 0;
    if (auto_margins) allocation.auto_margin = std::max(0.f, allocation.free_space) / static_cast<float>(auto_margins);
    return allocation;
}

void prepareMainAxis(std::vector<ChildLayout>& children, Flow flow, float available_main) {
    for (ChildLayout& child : children) {
        if (flow == Flow::Row) child.measured.x = styledDimension(child.style.width, child.style.min_width, child.measured.x, available_main);
        else child.measured.y = styledDimension(child.style.height, child.style.min_height, child.measured.y, available_main);
        applyFlexBasis(child, flow, available_main);
    }
}
} // namespace rdui::layout_detail
