/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/primitives.h"
#include <algorithm>
#include <cctype>

namespace radia::ui::layout_detail {
float styledDimension(const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
    const float resolved = value.resolve(fallback, reference);
    return minimum ? std::max(resolved, minimum->resolve(reference)) : resolved;
}

bool isInlineLevel(DisplayMode display) {
    return display == DisplayMode::Inline || display == DisplayMode::InlineBlock || display == DisplayMode::InlineGrid;
}

const Style& emptyChildStyle() {
    static const Style sEmpty;
    return sEmpty;
}

ChildLayout invalidChildLayout() {
    return {detail::NodeRef(), emptyChildStyle(), {}, {}};
}

bool isDisplayed(const ChildLayout& child) {
    if (!child.node) return false;
    if (const Element* element = child.node.element()) return element->isDisplayed(child.style);
    return child.style.display != DisplayMode::NoneValue;
}

bool isWhitespaceOnlyText(const detail::NodeRef& node) {
    const Text* text = node.text();
    if (!text || text->getData().empty()) return false;
    return std::all_of(text->getData().begin(), text->getData().end(), [](unsigned char character) { return std::isspace(character) != 0; });
}

bool flowBreakBefore(const ChildLayout& child) {
    const Node* node = child.node.get();
    return node && detail::NodeAccess::flowBreakBefore(*node);
}

void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children) {
    if (std::all_of(children.begin(), children.end(), [&parent](const ChildLayout& child) {
            const Node* node = child.node.get();
            return node && node->parentNode() == &parent && isDisplayed(child);
        }))
        return;
    std::vector<ChildLayout> attached;
    attached.reserve(children.size());
    for (const ChildLayout& child : children) {
        const Node* node = child.node.get();
        if (node && node->parentNode() == &parent && isDisplayed(child)) attached.push_back(child);
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

void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain,
                         bool allowGrowth, float& total) {
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

float justifySelfOffset(JustifySelf alignment, LayoutDirection direction, float freeSpace) {
    if (alignment == JustifySelf::Center) return freeSpace * .5f;
    if (alignment == JustifySelf::End) return direction == LayoutDirection::RightToLeft ? 0.f : freeSpace;
    if (alignment == JustifySelf::Start) return direction == LayoutDirection::RightToLeft ? freeSpace : 0.f;
    return 0.f;
}

GridTrackSizes gridTrackSizes(const std::vector<ChildLayout>& children, std::optional<float> availableWidth, std::optional<float> availableHeight) {
    std::size_t columnCount = 1;
    std::size_t rowCount = 1;
    for (const ChildLayout& child : children) {
        if (!isDisplayed(child)) continue;
        const GridArea area = child.style.gridArea.value_or(GridArea{});
        columnCount = std::max(columnCount, static_cast<std::size_t>(std::max(1, area.column)));
        rowCount = std::max(rowCount, static_cast<std::size_t>(std::max(1, area.row)));
    }

    GridTrackSizes result;
    result.columns.resize(columnCount);
    result.rows.resize(rowCount);
    for (const ChildLayout& child : children) {
        if (!isDisplayed(child)) continue;
        const GridArea area = child.style.gridArea.value_or(GridArea{});
        const std::size_t column = static_cast<std::size_t>(std::max(1, area.column)) - 1;
        const std::size_t row = static_cast<std::size_t>(std::max(1, area.row)) - 1;
        result.columns[column] = std::max(result.columns[column], child.measured.x + child.style.margin.horizontal());
        result.rows[row] = std::max(result.rows[row], child.measured.y + child.style.margin.vertical());
    }

    const auto distributeFreeSpace = [](std::vector<float>& tracks, std::optional<float> available) {
        if (!available || tracks.empty()) return;
        float used = 0.f;
        for (const float track : tracks) used += track;
        const float freeSpace = *available - used;
        if (freeSpace <= 0.f) return;
        const float extra = freeSpace / static_cast<float>(tracks.size());
        for (float& track : tracks) track += extra;
    };
    distributeFreeSpace(result.columns, availableWidth);
    distributeFreeSpace(result.rows, availableHeight);
    return result;
}

Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment) {
    const Element* node = child.node.element();
    const bool explicitRect = node && node->mRectExplicit;
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
    return {x, y, width, height};
}

Rect relativeRect(const ChildLayout& child, const Rect& rect, const Rect& containingBlock) {
    if (child.style.position != PositionMode::Relative) return rect;

    float x = rect.x;
    float y = rect.y;
    if (child.style.left) x += child.style.left->resolve(containingBlock.w);
    else if (child.style.right) x -= child.style.right->resolve(containingBlock.w);
    if (child.style.top) y -= child.style.top->resolve(containingBlock.h);
    else if (child.style.bottom) y += child.style.bottom->resolve(containingBlock.h);
    return {x, y, rect.w, rect.h};
}

Rect translatedRect(const ChildLayout& child, const Rect& rect) {
    return {rect.x + child.style.translate.x, rect.y + child.style.translate.y, rect.w, rect.h};
}

void setArrangedRect(Element& node, const Rect& rect) {
    const bool changed = node.mRect.x != rect.x || node.mRect.y != rect.y || node.mRect.w != rect.w || node.mRect.h != rect.h;
    if (!changed) return;
    node.mRect = rect;
    detail::ElementInternalAccess::layoutCache(node).arrangeValid = false;
    node.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    node.invalidatePaint();
}

std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const detail::NodeRef& firstRef, const detail::NodeRef& secondRef,
                                             const Style& parentStyle) {
    Element* parent = parentState.get();
    Node* firstNode = firstRef.get();
    Node* secondNode = secondRef.get();
    if (!parentState.valid() || !firstNode || !secondNode || firstNode->parentNode() != parent || secondNode->parentNode() != parent)
        return std::nullopt;
    Element* first = firstRef.element();
    Element* second = secondRef.element();
    if (!first || !second) return AdjacentLayout{true, 0.f};
    const ElementVisit firstState(*first);
    const ElementVisit secondState(*second);
    const bool hasGap = ElementLayoutAccess::hasGap(*parent, *first, *second);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.valid() || !parent || !firstState.validChildOf(*parent) || !secondState.validChildOf(*parent)) return std::nullopt;
    const float overlap = ElementLayoutAccess::overlap(*parent, *first, *second, parentStyle);
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
        if (index > lineStart && flowBreakBefore(children[index])) {
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
        if (!child.node) continue;
        const float width = child.measured.x + child.style.margin.horizontal();
        const float height = child.measured.y + child.style.margin.vertical();
        const bool block = !isInlineLevel(child.style.display);
        if (block) {
            finish();
            lines.push_back({index, index + 1, width, height, true});
            continue;
        }
        if (flowBreakBefore(child) && current) finish();
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

MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, const Style& parentStyle,
                                    FlexDirection flexDirection, float availableMain) {
    const ElementVisit parentState(parent);
    const auto invalidAllocation = [] {
        MainAxisAllocation invalid;
        invalid.valid = false;
        return invalid;
    };
    float total = 0.f;
    int autoMargins = 0;
    for (std::size_t index = begin; index < end; ++index) {
        const ChildLayout& child = children[index];
        total +=
            mainSize(child, flexDirection) + (flexDirection == FlexDirection::Row ? child.style.margin.horizontal() : child.style.margin.vertical());
        autoMargins += flexDirection == FlexDirection::Row ? child.style.margin.horizontalAutoCount() : child.style.margin.verticalAutoCount();
    }
    std::size_t gapCount = 0;
    float overlap = 0.f;
    for (std::size_t index = begin + 1; index < end; ++index) {
        const detail::NodeRef& previous = children[index - 1].node;
        const detail::NodeRef& current = children[index].node;
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
