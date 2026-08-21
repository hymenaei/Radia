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

namespace radia::ui::layout_detail {
float styledDimension(const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
    const float resolved = value.resolve(fallback, reference);
    return minimum ? std::max(resolved, minimum->resolve(reference)) : resolved;
}

void warnIgnoredPosition(const Widget& child, const Style& style, FlexDirection flexDirection) {
    const char* property = style.left ? "left" : style.right ? "right" : style.top ? "top" : style.bottom ? "bottom" : nullptr;
    if (!property) return;
    LL_WARNS("UI")
        << "Ignoring '"
        << property
        << "' on <"
        << child.elementName()
        << (child.id().empty() ? "" : " id=\"" + child.id() + "\"")
        << ">: parent flex direction is '"
        << (flexDirection == FlexDirection::Row ? "row" : "column")
        << "'."
        << LL_ENDL;
}

const Style& emptyChildStyle() {
    static const Style sEmpty;
    return sEmpty;
}

ChildLayout invalidChildLayout() {
    return {WidgetRef<Widget>(), emptyChildStyle(), {}, {}};
}

void removeChildrenExcludedFromLayout(Widget& parent, std::vector<ChildLayout>& children) {
    if (std::all_of(children.begin(), children.end(), [&parent](const ChildLayout& child) {
            const Widget* node = child.node.get();
            return node && node->parent() == &parent && node->isDisplayed(child.style);
        }))
        return;
    std::vector<ChildLayout> attached;
    attached.reserve(children.size());
    for (const ChildLayout& child : children) {
        const Widget* node = child.node.get();
        if (node && node->parent() == &parent && node->isDisplayed(child.style)) attached.push_back(child);
    }
    children.swap(attached);
}

float& mainSize(ChildLayout& child, FlexDirection flexDirection) {
    return flexDirection == FlexDirection::Row ? child.measured.x : child.measured.y;
}
float mainSize(const ChildLayout& child, FlexDirection flexDirection) {
    return flexDirection == FlexDirection::Row ? child.measured.x : child.measured.y;
}

float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase) {
    const std::optional<Length>& minimum = flexDirection == FlexDirection::Row ? child.style.minWidth : child.style.minHeight;
    if (minimum) return minimum->resolve(availableMain);
    const float automatic = flexDirection == FlexDirection::Row ? child.fitSize.x : child.fitSize.y;
    return std::min(automatic, flexBase);
}

void applyFlexBasis(ChildLayout& child, FlexDirection flexDirection, float availableMain) {
    if (child.style.flexBasis.isAuto()) return;
    const float basis = child.style.flexBasis.resolve(0.f, availableMain);
    mainSize(child, flexDirection) = std::max(basis, mainMinimum(child, flexDirection, availableMain, basis));
}

void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain, bool allowGrowth,
                         float& total) {
    const float freeSpace = availableMain - total;
    if (freeSpace > 0.f && allowGrowth) {
        float totalGrow = 0.f;
        for (std::size_t index = begin; index < end; ++index) totalGrow += children[index].style.flexGrow;
        if (totalGrow <= 0.f) return;
        for (std::size_t index = begin; index < end; ++index)
            mainSize(children[index], flexDirection) += freeSpace * children[index].style.flexGrow / totalGrow;
        total += freeSpace;
        return;
    }
    if (freeSpace >= 0.f) return;

    float deficit = -freeSpace;
    std::vector<float> baseSizes;
    std::vector<bool> active;
    baseSizes.reserve(end - begin);
    active.reserve(end - begin);
    for (std::size_t index = begin; index < end; ++index) {
        const ChildLayout& child = children[index];
        const float base = mainSize(child, flexDirection);
        baseSizes.push_back(base);
        active.push_back(child.style.flexShrink > 0.f && base > mainMinimum(child, flexDirection, availableMain, base));
    }

    constexpr float kFlexEpsilon = 1.0e-4f;
    while (deficit > kFlexEpsilon) {
        float totalWeight = 0.f;
        for (std::size_t offset = 0; offset < active.size(); ++offset)
            if (active[offset]) totalWeight += children[begin + offset].style.flexShrink * baseSizes[offset];
        if (totalWeight <= kFlexEpsilon) break;

        const float roundDeficit = deficit;
        float reduced = 0.f;
        for (std::size_t offset = 0; offset < active.size(); ++offset) {
            if (!active[offset]) continue;
            ChildLayout& child = children[begin + offset];
            float& size = mainSize(child, flexDirection);
            const float minimum = mainMinimum(child, flexDirection, availableMain, baseSizes[offset]);
            const float share = roundDeficit * child.style.flexShrink * baseSizes[offset] / totalWeight;
            const float reduction = std::min(share, size - minimum);
            size -= reduction;
            reduced += reduction;
        if (size <= minimum + kFlexEpsilon) active[offset] = false;
        }
        if (reduced <= kFlexEpsilon) break;
        deficit -= reduced;
        total -= reduced;
    }
}

float verticalAlignmentOffset(VerticalAlign alignment, float freeSpace) {
    if (alignment == VerticalAlign::Middle) return freeSpace * .5f;
    if (alignment == VerticalAlign::Bottom) return freeSpace;
    return 0.f;
}

CrossAlignment crossAlignment(const Style& parent, const Style& child, FlexDirection flexDirection) {
    if (child.alignSelf != AlignSelf::Auto) {
        if (child.alignSelf == AlignSelf::Start) return CrossAlignment::Start;
        if (child.alignSelf == AlignSelf::Center) return CrossAlignment::Center;
        if (child.alignSelf == AlignSelf::End) return CrossAlignment::End;
        return CrossAlignment::Stretch;
    }
    if (parent.alignItems == AlignItems::Start) return CrossAlignment::Start;
    if (parent.alignItems == AlignItems::Center) return CrossAlignment::Center;
    if (parent.alignItems == AlignItems::End) return CrossAlignment::End;
    if (parent.alignItems == AlignItems::Stretch) return CrossAlignment::Stretch;
    if (flexDirection == FlexDirection::Column) return CrossAlignment::Stretch;
    if (parent.verticalAlign == VerticalAlign::Middle) return CrossAlignment::Center;
    if (parent.verticalAlign == VerticalAlign::Bottom) return CrossAlignment::End;
    return CrossAlignment::Start;
}

void applyCrossAxisSizing(Vec2& size, const Style& style, FlexDirection flexDirection, float availableCross, CrossAlignment alignment) {
    if (alignment != CrossAlignment::Stretch) return;
    if (flexDirection == FlexDirection::Row) {
        if (!style.height.isAuto() || style.margin.verticalAutoCount()) return;
        const float height = std::max(0.f, availableCross - style.margin.vertical());
        size.y = styledDimension(style.height, style.minHeight, height, availableCross);
        if (style.aspectRatio && style.width.isAuto() && *style.aspectRatio > 0.f) size.x = size.y * *style.aspectRatio;
    } else if (flexDirection == FlexDirection::Column) {
        if (!style.width.isAuto() || style.margin.horizontalAutoCount()) return;
        const float width = std::max(0.f, availableCross - style.margin.horizontal());
        size.x = styledDimension(style.width, style.minWidth, width, availableCross);
        if (style.aspectRatio && style.height.isAuto() && *style.aspectRatio > 0.f) size.y = size.x / *style.aspectRatio;
    }
}

float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float freeSpace) {
    if (alignment == JustifyContent::Center) return freeSpace * .5f;
    if (alignment == JustifyContent::End) return freeSpace;
    if (alignment == JustifyContent::Left) return direction == LayoutDirection::RightToLeft ? freeSpace : 0.f;
    if (alignment == JustifyContent::Right) return direction == LayoutDirection::RightToLeft ? 0.f : freeSpace;
    return 0.f;
}

Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment) {
    const Widget* node = child.node.get();
    llassert(node);
    if (!node) return {};
    const bool explicitRect = node->mRectExplicit;
    const float width = explicitRect && child.style.width.isAuto()
        ? child.style.minWidth ? std::max(node->mRect.w, child.style.minWidth->resolve(parent.w)) : node->mRect.w
        : styledDimension(child.style.width, child.style.minWidth, child.measured.x > 0.f ? child.measured.x : parent.w, parent.w);
    const float height = explicitRect && child.style.height.isAuto()
        ? child.style.minHeight ? std::max(node->mRect.h, child.style.minHeight->resolve(parent.h)) : node->mRect.h
        : styledDimension(child.style.height, child.style.minHeight, child.measured.y, parent.h);
    const MarginInsets& margin = child.style.margin;
    const float horizontalSpace = std::max(0.f, parent.w - width - margin.horizontal());
    float x = explicitRect ? node->mRect.x : margin.left.isAuto() ? parent.left() + horizontalSpace : parent.left() + margin.left.fixedPixels();
    if (margin.left.isAuto() && margin.right.isAuto()) x = parent.left() + horizontalSpace * .5f;
    const float verticalSpace = std::max(0.f, parent.h - height - margin.vertical());
    float y =
        explicitRect ? node->mRect.y : parent.top() - margin.top.fixedPixels() - height - verticalAlignmentOffset(verticalAlignment, verticalSpace);
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
    node.mLayoutCache.arrangeValid = false;
    node.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    node.invalidatePaint();
}

std::optional<AdjacentLayout> adjacentLayout(const WidgetVisit& parentState, Widget* first, Widget* second, const Style& parentStyle) {
    Widget* parent = parentState.get();
    if (!parentState.valid() || !first || !second || first->parent() != parent || second->parent() != parent) return std::nullopt;
    const WidgetVisit firstState(*first);
    const WidgetVisit secondState(*second);
    const bool hasGap = WidgetLayoutAccess::hasGap(*parent, *first, *second);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.valid() || !parent || !firstState.validChildOf(*parent) || !secondState.validChildOf(*parent)) return std::nullopt;
    const float overlap = WidgetLayoutAccess::overlap(*parent, *first, *second, parentStyle);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.valid() || !parent || !firstState.validChildOf(*parent) || !secondState.validChildOf(*parent)) return std::nullopt;
    return AdjacentLayout{hasGap, overlap};
}

std::vector<std::pair<std::size_t, std::size_t>> rowLines(const std::vector<ChildLayout>& children) {
    std::vector<std::pair<std::size_t, std::size_t>> lines;
    std::size_t lineStart = 0;
    for (std::size_t index = 0; index < children.size(); ++index) {
        const Widget* child = children[index].node.get();
        if (index > lineStart && child && child->flowBreakBefore()) {
            lines.emplace_back(lineStart, index);
            lineStart = index;
        }
    }
    if (lineStart < children.size()) lines.emplace_back(lineStart, children.size());
    return lines;
}

std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth) {
    std::vector<NormalLine> lines;
    std::optional<NormalLine> current;
    const auto finish = [&] {
        if (current) lines.push_back(*current);
        current.reset();
    };

    for (std::size_t index = 0; index < children.size(); ++index) {
        const ChildLayout& child = children[index];
        const Widget* node = child.node.get();
        if (!node) continue;
        const float width = child.measured.x + child.style.margin.horizontal();
        const float height = child.measured.y + child.style.margin.vertical();
        const bool block = child.style.display != DisplayMode::Inline;
        if (block) {
            finish();
            lines.push_back({index, index + 1, width, height, true});
            continue;
        }
        if (node->flowBreakBefore() && current) finish();
        if (availableWidth && current && current->width > 0.f && current->width + width > *availableWidth) finish();
        if (!current) current = NormalLine{index, index + 1, width, height, false};
        else {
            current->end = index + 1;
            current->width += width;
            current->height = std::max(current->height, height);
        }
    }
    finish();
    return lines;
}

MainAxisAllocation allocateMainAxis(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, const Style& parentStyle,
                                    FlexDirection flexDirection, float availableMain) {
    const WidgetVisit parentState(parent);
    const auto invalidAllocation = [] {
        MainAxisAllocation invalid;
        invalid.valid = false;
        return invalid;
    };
    float total = 0.f;
    int autoMargins = 0;
    for (std::size_t index = begin; index < end; ++index) {
        const ChildLayout& child = children[index];
        total += mainSize(child, flexDirection)
            + (flexDirection == FlexDirection::Row ? child.style.margin.horizontal() : child.style.margin.vertical());
        autoMargins += flexDirection == FlexDirection::Row ? child.style.margin.horizontalAutoCount() : child.style.margin.verticalAutoCount();
    }
    std::size_t gapCount = 0;
    float overlap = 0.f;
    for (std::size_t index = begin + 1; index < end; ++index) {
        Widget* previous = children[index - 1].node.get();
        Widget* current = children[index].node.get();
        if (!parentState.valid()) return invalidAllocation();
        const std::optional<AdjacentLayout> adjacent = adjacentLayout(parentState, previous, current, parentStyle);
        if (!adjacent) return invalidAllocation();
        if (adjacent->hasGap) ++gapCount;
        overlap += adjacent->overlap;
    }
    MainAxisAllocation allocation;
    allocation.gap = parentStyle.gap.fixedPixels();
    total += allocation.gap * static_cast<float>(gapCount) - overlap;
    distributeFlexSpace(children, begin, end, flexDirection, availableMain, !autoMargins && !parentStyle.gap.isAuto(), total);
    allocation.freeSpace = availableMain - total;
    if (!autoMargins && parentStyle.gap.isAuto() && gapCount) {
        allocation.gap = std::max(0.f, allocation.freeSpace) / static_cast<float>(gapCount);
        total += allocation.gap * static_cast<float>(gapCount);
        allocation.freeSpace = availableMain - total;
    }
    allocation.hasAutoMargins = autoMargins != 0;
    if (autoMargins) allocation.autoMargin = std::max(0.f, allocation.freeSpace) / static_cast<float>(autoMargins);
    return allocation;
}

void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain) {
    for (ChildLayout& child : children) {
        if (flexDirection == FlexDirection::Row)
            child.measured.x = styledDimension(child.style.width, child.style.minWidth, child.measured.x, availableMain);
        else child.measured.y = styledDimension(child.style.height, child.style.minHeight, child.measured.y, availableMain);
        applyFlexBasis(child, flexDirection, availableMain);
    }
}
} // namespace radia::ui::layout_detail
