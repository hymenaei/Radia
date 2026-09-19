/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/primitives.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <limits>

namespace radia::ui::layout_detail {
using radia::ui::detail::ElementInternalAccess;

namespace {
float boxSizingExtra(const ComputedStyle& style, bool horizontal) {
    if (style.boxSizing == BoxSizing::BorderBox) return 0.f;
    return (horizontal ? style.padding.horizontal() : style.padding.vertical())
        + (horizontal ? style.borderWidth.horizontal() : style.borderWidth.vertical());
}
} // namespace

namespace {
float resolveBoxDimension(const ComputedStyle& style, bool horizontal, const Dimension& value, float fallback, float reference, float minContent) {
    const float extra = boxSizingExtra(style, horizontal);
    float contentSize = value.resolve(fallback, reference);
    if (value.isIntrinsic()) {
        switch (value.intrinsicKeyword()) {
            case DimensionKeyword::Content: contentSize = fallback; break;
            case DimensionKeyword::MaxContent: contentSize = fallback; break;
            case DimensionKeyword::MinContent: contentSize = minContent; break;
            case DimensionKeyword::FitContent:
                if (reference > 0.f) contentSize = std::min(fallback, std::max(minContent, reference - extra));
                break;
        }
    }
    const float resolved = contentSize + (value.isAuto() ? 0.f : extra);
    return resolved;
}

float resolveConstraint(const ComputedStyle& style, bool horizontal, const Dimension& value, float fallback, float reference, float minContent) {
    if (value.isAuto()) return 0.f;
    return resolveBoxDimension(style, horizontal, value, fallback, reference, minContent);
}
} // namespace

float styledBoxDimension(const ComputedStyle& style, bool horizontal, const Dimension& value, const std::optional<Dimension>& minimum,
                         const std::optional<Dimension>& maximum, float fallback, float reference, float minContent) {
    const float resolved = resolveBoxDimension(style, horizontal, value, fallback, reference, minContent);
    return clampBoxDimension(style, horizontal, resolved, minimum, maximum, reference, minContent);
}

float clampBoxDimension(const ComputedStyle& style, bool horizontal, float size, const std::optional<Dimension>& minimum,
                        const std::optional<Dimension>& maximum, float reference, float minContent) {
    const float minimumSize = minimum ? resolveConstraint(style, horizontal, *minimum, size, reference, minContent) : 0.f;
    const float maximumSize =
        maximum ? resolveConstraint(style, horizontal, *maximum, size, reference, minContent) : std::numeric_limits<float>::infinity();
    return std::min(std::max(size, minimumSize), std::max(maximumSize, minimumSize));
}

float minimumBoxDimension(const ComputedStyle& style, bool horizontal, const std::optional<Dimension>& minimum, float reference, float fallback,
                          float minContent) {
    return minimum ? resolveConstraint(style, horizontal, *minimum, fallback, reference, minContent) : 0.f;
}

float maximumBoxDimension(const ComputedStyle& style, bool horizontal, const std::optional<Dimension>& maximum, float reference, float fallback,
                          float minContent) {
    return maximum ? resolveConstraint(style, horizontal, *maximum, fallback, reference, minContent) : std::numeric_limits<float>::infinity();
}

float contentBoxDimension(const ComputedStyle& style, bool horizontal, float borderBoxSize) {
    return std::max(
        0.f,
        borderBoxSize
            - (horizontal ? style.padding.horizontal() + style.borderWidth.horizontal() : style.padding.vertical() + style.borderWidth.vertical()));
}

bool isInlineLevel(DisplayMode display) {
    return display == DisplayMode::Inline
        || display == DisplayMode::InlineBlock
        || display == DisplayMode::InlineFlex
        || display == DisplayMode::InlineGrid;
}

const ComputedStyle& emptyChildStyle() {
    static const ComputedStyle sEmpty;
    return sEmpty;
}

ChildLayout invalidChildLayout() {
    return {OrderedChildRef(), emptyChildStyle(), {}, {}, {}};
}

bool isDisplayed(const ChildLayout& child) {
    if (!child.node) return false;
    if (child.node.pseudoElement) return child.style.display != DisplayMode::NoneValue;
    if (const Element* element = child.node.element()) return element->isDisplayed(child.style);
    return child.style.display != DisplayMode::NoneValue;
}

bool isWhitespaceOnlyText(const detail::NodeRef& node) {
    const Text* text = node.text();
    if (!text || text->data().empty()) return false;
    return std::all_of(text->data().begin(), text->data().end(), [](unsigned char character) { return std::isspace(character) != 0; });
}

bool isWhitespaceOnlyText(const OrderedChildRef& node) {
    return !node.pseudoElement && isWhitespaceOnlyText(node.node);
}

bool flowBreakBefore(const ChildLayout& child) {
    if (child.node.pseudoElement) return false;
    const Node* node = child.node.get();
    return node && detail::NodeAccess::flowBreakBefore(*node);
}

void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children) {
    if (std::all_of(children.begin(), children.end(),
                    [&parent](const ChildLayout& child) { return child.node.attachedTo(parent) && isDisplayed(child); }))
        return;
    std::vector<ChildLayout> attached;
    attached.reserve(children.size());
    for (const ChildLayout& child : children)
        if (child.node.attachedTo(parent) && isDisplayed(child)) attached.push_back(child);
    children.swap(attached);
}

float& mainSize(ChildLayout& child, FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.measured.x : child.measured.y;
}
float mainSize(const ChildLayout& child, FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.measured.x : child.measured.y;
}

float& flexBaseSize(ChildLayout& child, FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.flexBase.x : child.flexBase.y;
}

float flexBaseSize(const ChildLayout& child, FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.flexBase.x : child.flexBase.y;
}

float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase) {
    const std::optional<Dimension>& minimum = isRowFlexDirection(flexDirection) ? child.style.minWidth : child.style.minHeight;
    if (minimum)
        return minimumBoxDimension(child.style, isRowFlexDirection(flexDirection), minimum, availableMain, flexBase,
                                   isRowFlexDirection(flexDirection) ? child.minContent.x : child.minContent.y);
    const float automatic = isRowFlexDirection(flexDirection) ? child.fitSize.x : child.fitSize.y;
    return std::min(automatic, flexBase);
}

void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain,
                         float& total) {
    struct FlexItem {
        std::size_t index = 0;
        float base = 0.f;
        float hypothetical = 0.f;
        float target = 0.f;
        float minimum = 0.f;
        float maximum = 0.f;
        bool frozen = false;
    };

    constexpr float kFlexEpsilon = 1.0e-4f;
    const bool growing = availableMain > total;
    std::vector<FlexItem> items;
    items.reserve(end - begin);
    float hypotheticalTotal = 0.f;
    for (std::size_t index = begin; index < end; ++index) {
        ChildLayout& child = children[index];
        const bool horizontal = isRowFlexDirection(flexDirection);
        const float base = flexBaseSize(child, flexDirection);
        const float minimum = mainMinimum(child, flexDirection, availableMain, base);
        const float maximum = std::max(maximumBoxDimension(child.style, horizontal, horizontal ? child.style.maxWidth : child.style.maxHeight,
                                                           availableMain, base, horizontal ? child.minContent.x : child.minContent.y),
                                       minimum);
        items.push_back({index, base, mainSize(child, flexDirection), base, minimum, maximum, false});
        hypotheticalTotal += mainSize(child, flexDirection);
    }

    const float nonFlexTotal = total - hypotheticalTotal;
    for (FlexItem& item : items) {
        const ChildLayout& child = children[item.index];
        const bool hasFlexFactor = growing ? child.style.flexGrow > 0.f : child.style.flexShrink > 0.f;
        const bool violatesHypothetical = growing ? item.base > item.hypothetical : item.base < item.hypothetical;
        if (!hasFlexFactor || violatesHypothetical) {
            item.frozen = true;
            item.target = item.hypothetical;
        }
    }

    while (true) {
        bool hasUnfrozenItems = false;
        float used = nonFlexTotal;
        float factorTotal = 0.f;
        for (const FlexItem& item : items) {
            if (item.frozen) used += item.target;
            else {
                hasUnfrozenItems = true;
                used += item.base;
                const ChildLayout& child = children[item.index];
                factorTotal += growing ? child.style.flexGrow : child.style.flexShrink * item.base;
            }
        }
        if (!hasUnfrozenItems) break;

        const float freeSpace = availableMain - used;
        if ((growing && freeSpace <= kFlexEpsilon) || (!growing && freeSpace >= -kFlexEpsilon) || factorTotal <= kFlexEpsilon) {
            for (FlexItem& item : items)
                if (!item.frozen) {
                    item.target = item.base;
                    item.frozen = true;
                }
            break;
        }

        const float adjustedFreeSpace = growing && factorTotal < 1.f ? freeSpace * factorTotal : freeSpace;
        bool frozeItem = false;
        for (FlexItem& item : items) {
            if (item.frozen) continue;
            const ChildLayout& child = children[item.index];
            const float proposed = growing ? item.base + adjustedFreeSpace * child.style.flexGrow / factorTotal
                                           : item.base + adjustedFreeSpace * child.style.flexShrink * item.base / factorTotal;
            item.target = std::min(std::max(proposed, item.minimum), item.maximum);
            if (std::abs(item.target - proposed) > kFlexEpsilon) {
                item.frozen = true;
                frozeItem = true;
            }
        }
        if (!frozeItem) break;
    }

    total = nonFlexTotal;
    for (const FlexItem& item : items) {
        mainSize(children[item.index], flexDirection) = item.target;
        total += item.target;
    }
}

float verticalAlignmentOffset(const ComputedStyle& style, float freeSpace) {
    if (style.verticalAlign == VerticalAlign::Middle) return freeSpace * .5f;
    if (style.verticalAlign == VerticalAlign::Bottom) return freeSpace;
    if (style.verticalAlign == VerticalAlign::Sub) return -style.fontSize * .2f;
    if (style.verticalAlign == VerticalAlign::Super) return style.fontSize * .4f;
    if (style.verticalAlign == VerticalAlign::TextTop) return freeSpace;
    if (style.verticalAlign == VerticalAlign::TextBottom) return 0.f;
    if (style.verticalAlign == VerticalAlign::Length || style.verticalAlign == VerticalAlign::Percentage)
        return style.verticalAlignOffset.resolve(style.lineHeight.kind == LineHeight::Kind::Length ? style.lineHeight.value : freeSpace);
    return 0.f;
}

ItemPosition crossAlignment(const ComputedStyle& parent, const ComputedStyle& child) {
    if (child.alignSelf.position != ItemPosition::Auto) {
        if (child.alignSelf.position == ItemPosition::Start || child.alignSelf.position == ItemPosition::SelfStart) return ItemPosition::Start;
        if (child.alignSelf.position == ItemPosition::FlexStart) return ItemPosition::FlexStart;
        if (child.alignSelf.position == ItemPosition::Center) return ItemPosition::Center;
        if (child.alignSelf.position == ItemPosition::End || child.alignSelf.position == ItemPosition::SelfEnd) return ItemPosition::End;
        if (child.alignSelf.position == ItemPosition::FlexEnd) return ItemPosition::FlexEnd;
        if (child.alignSelf.position == ItemPosition::Baseline || child.alignSelf.position == ItemPosition::LastBaseline)
            return child.alignSelf.position;
        if (child.alignSelf.position == ItemPosition::AnchorCenter) return ItemPosition::Center;
        return ItemPosition::Stretch;
    }
    if (parent.alignItems.position == ItemPosition::Start || parent.alignItems.position == ItemPosition::SelfStart) return ItemPosition::Start;
    if (parent.alignItems.position == ItemPosition::FlexStart) return ItemPosition::FlexStart;
    if (parent.alignItems.position == ItemPosition::Center) return ItemPosition::Center;
    if (parent.alignItems.position == ItemPosition::End || parent.alignItems.position == ItemPosition::SelfEnd) return ItemPosition::End;
    if (parent.alignItems.position == ItemPosition::FlexEnd) return ItemPosition::FlexEnd;
    if (parent.alignItems.position == ItemPosition::Baseline || parent.alignItems.position == ItemPosition::LastBaseline)
        return parent.alignItems.position;
    if (parent.alignItems.position == ItemPosition::AnchorCenter) return ItemPosition::Center;
    if (parent.alignItems.position == ItemPosition::Stretch) return ItemPosition::Stretch;
    return ItemPosition::Stretch;
}

OverflowAlignment crossAlignmentSafety(const ComputedStyle& parent, const ComputedStyle& child) {
    return child.alignSelf.position == ItemPosition::Auto ? parent.alignItems.overflow : child.alignSelf.overflow;
}

void applyCrossAxisSizing(Vec2& size, const ComputedStyle& style, FlexDirection flexDirection, float availableCross, ItemPosition alignment) {
    if (alignment != ItemPosition::Stretch) return;
    if (isRowFlexDirection(flexDirection)) {
        if (!style.height.isAuto() || style.margin.verticalAutoCount()) return;
        const float height = std::max(0.f, availableCross - style.margin.vertical());
        size.y = styledBoxDimension(style, false, style.height, style.minHeight, style.maxHeight, height, availableCross);
        if (style.aspectRatio && style.width.isAuto() && *style.aspectRatio > 0.f) size.x = size.y * *style.aspectRatio;
    } else {
        if (!style.width.isAuto() || style.margin.horizontalAutoCount()) return;
        const float width = std::max(0.f, availableCross - style.margin.horizontal());
        size.x = styledBoxDimension(style, true, style.width, style.minWidth, style.maxWidth, width, availableCross);
        if (style.aspectRatio && style.height.isAuto() && *style.aspectRatio > 0.f) size.y = size.x / *style.aspectRatio;
    }
}

float textAlignmentOffset(TextAlign alignment, LayoutDirection direction, float freeSpace) {
    if (alignment == TextAlign::Center) return freeSpace * .5f;
    if (alignment == TextAlign::End) return freeSpace;
    if (alignment == TextAlign::Left) return direction == LayoutDirection::RightToLeft ? freeSpace : 0.f;
    if (alignment == TextAlign::Right) return direction == LayoutDirection::RightToLeft ? 0.f : freeSpace;
    if (alignment == TextAlign::MatchParent) return direction == LayoutDirection::RightToLeft ? freeSpace : 0.f;
    return 0.f;
}

float justifySelfOffset(ItemPosition alignment, OverflowAlignment safety, LayoutDirection direction, float freeSpace) {
    const float distributable = safety == OverflowAlignment::Safe ? std::max(0.f, freeSpace) : freeSpace;
    if (alignment == ItemPosition::Center || alignment == ItemPosition::AnchorCenter) return distributable * .5f;
    if (alignment == ItemPosition::End || alignment == ItemPosition::SelfEnd || alignment == ItemPosition::FlexEnd)
        return direction == LayoutDirection::RightToLeft ? 0.f : distributable;
    if (alignment == ItemPosition::Start || alignment == ItemPosition::SelfStart || alignment == ItemPosition::FlexStart)
        return direction == LayoutDirection::RightToLeft ? distributable : 0.f;
    if (alignment == ItemPosition::Right) return direction == LayoutDirection::RightToLeft ? 0.f : distributable;
    if (alignment == ItemPosition::Left) return direction == LayoutDirection::RightToLeft ? distributable : 0.f;
    if (alignment == ItemPosition::Baseline) {
        const float safeSpace = std::max(0.f, freeSpace);
        return direction == LayoutDirection::RightToLeft ? safeSpace : 0.f;
    }
    if (alignment == ItemPosition::LastBaseline) return direction == LayoutDirection::RightToLeft ? 0.f : std::max(0.f, freeSpace);
    return 0.f;
}

float alignSelfOffset(ItemPosition alignment, OverflowAlignment safety, float freeSpace) {
    const float distributable = safety == OverflowAlignment::Safe ? std::max(0.f, freeSpace) : freeSpace;
    if (alignment == ItemPosition::Center || alignment == ItemPosition::AnchorCenter) return distributable * .5f;
    if (alignment == ItemPosition::End || alignment == ItemPosition::SelfEnd || alignment == ItemPosition::FlexEnd) return distributable;
    if (alignment == ItemPosition::LastBaseline) return std::max(0.f, freeSpace);
    return 0.f;
}

GridTrackSizes gridTrackSizes(const std::vector<ChildLayout>& children, std::optional<float> availableWidth, std::optional<float> availableHeight,
                              float columnGap, float rowGap) {
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

    const auto distributeFreeSpace = [](std::vector<float>& tracks, std::optional<float> available, float gap) {
        if (!available || tracks.empty()) return;
        float used = 0.f;
        for (const float track : tracks) used += track;
        const float freeSpace = *available - used - gap * static_cast<float>(tracks.size() - 1);
        if (freeSpace <= 0.f) return;
        const float extra = freeSpace / static_cast<float>(tracks.size());
        for (float& track : tracks) track += extra;
    };
    distributeFreeSpace(result.columns, availableWidth, columnGap);
    distributeFreeSpace(result.rows, availableHeight, rowGap);
    return result;
}

Rect positionedRect(const ChildLayout& child, const Rect& parent) {
    const Element* node = child.node.element();
    const bool explicitRect = node && node->mRectExplicit;
    const float width = explicitRect && child.style.width.isAuto()
        ? clampBoxDimension(child.style, true, node->mRect.w, child.style.minWidth, child.style.maxWidth, parent.w)
        : styledBoxDimension(child.style, true, child.style.width, child.style.minWidth, child.style.maxWidth,
                             child.measured.x > 0.f ? child.measured.x : parent.w, parent.w, child.minContent.x);
    const float height = explicitRect && child.style.height.isAuto()
        ? clampBoxDimension(child.style, false, node->mRect.h, child.style.minHeight, child.style.maxHeight, parent.h)
        : styledBoxDimension(child.style, false, child.style.height, child.style.minHeight, child.style.maxHeight, child.measured.y, parent.h,
                             child.minContent.y);
    const MarginInsets& margin = child.style.margin;
    const float horizontalSpace = std::max(0.f, parent.w - width - margin.horizontal());
    float x = explicitRect ? node->mRect.x : margin.left.isAuto() ? parent.left() + horizontalSpace : parent.left() + margin.left.fixedPixels();
    if (margin.left.isAuto() && margin.right.isAuto()) x = parent.left() + horizontalSpace * .5f;
    const float y = explicitRect ? node->mRect.y : parent.top() - margin.top.fixedPixels() - height;
    return {x, y, width, height};
}

Rect outOfFlowRect(const ChildLayout& child, const Rect& containingBlock) {
    const ComputedStyle& style = child.style;
    const MarginInsets& margin = style.margin;
    const float leftInset = style.left ? style.left->resolve(containingBlock.w) : 0.f;
    const float rightInset = style.right ? style.right->resolve(containingBlock.w) : 0.f;
    const float topInset = style.top ? style.top->resolve(containingBlock.h) : 0.f;
    const float bottomInset = style.bottom ? style.bottom->resolve(containingBlock.h) : 0.f;
    float width =
        styledBoxDimension(style, true, style.width, style.minWidth, style.maxWidth, child.measured.x, containingBlock.w, child.minContent.x);
    float height =
        styledBoxDimension(style, false, style.height, style.minHeight, style.maxHeight, child.measured.y, containingBlock.h, child.minContent.y);
    if (style.width.isAuto() && style.left && style.right) width = std::max(0.f, containingBlock.w - leftInset - rightInset - margin.horizontal());
    if (style.height.isAuto() && style.top && style.bottom) height = std::max(0.f, containingBlock.h - topInset - bottomInset - margin.vertical());
    const float x = style.left ? containingBlock.left() + leftInset + margin.left.fixedPixels()
        : style.right          ? containingBlock.right() - rightInset - margin.right.fixedPixels() - width
                               : containingBlock.left() + margin.left.fixedPixels();
    const float y = style.top ? containingBlock.top() - topInset - margin.top.fixedPixels() - height
        : style.bottom        ? containingBlock.bottom() + bottomInset + margin.bottom.fixedPixels()
                              : containingBlock.top() - margin.top.fixedPixels() - height;
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
    ElementInternalAccess::layoutCache(node).arrangeValid = false;
    node.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    node.invalidatePaint();
}

std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const OrderedChildRef& firstRef, const OrderedChildRef& secondRef,
                                             const ComputedStyle& parentStyle) {
    Element* parent = parentState.get();
    if (!parentState.layoutValid() || !firstRef.attachedTo(*parent) || !secondRef.attachedTo(*parent)) return std::nullopt;
    if (firstRef.isPseudoElement() || secondRef.isPseudoElement()) return AdjacentLayout{true, 0.f};
    Node* firstNode = firstRef.get();
    Node* secondNode = secondRef.get();
    if (!firstNode || !secondNode) return std::nullopt;
    Element* first = firstRef.element();
    Element* second = secondRef.element();
    if (!first || !second) return AdjacentLayout{true, 0.f};
    const ElementVisit firstState(*first);
    const ElementVisit secondState(*second);
    const bool hasGap = ElementLayoutAccess::hasGap(*parent, *first, *second);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.layoutValid()
        || !parent
        || !firstState.layoutValid()
        || !firstState.attachedTo(*parent)
        || !secondState.layoutValid()
        || !secondState.attachedTo(*parent))
        return std::nullopt;
    const float overlap = ElementLayoutAccess::overlap(*parent, *first, *second, parentStyle);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.layoutValid()
        || !parent
        || !firstState.layoutValid()
        || !firstState.attachedTo(*parent)
        || !secondState.layoutValid()
        || !secondState.attachedTo(*parent))
        return std::nullopt;
    return AdjacentLayout{hasGap, overlap};
}

std::vector<std::pair<std::size_t, std::size_t>> flexLines(Element& parent, const std::vector<ChildLayout>& children,
                                                           const ComputedStyle& parentStyle, FlexDirection flexDirection, float availableMain) {
    std::vector<std::pair<std::size_t, std::size_t>> lines;
    if (children.empty()) return lines;
    if (!isFlexWrapMultiLine(parentStyle.flexWrap) || availableMain < 0.f) {
        lines.emplace_back(0, children.size());
        return lines;
    }

    const ElementVisit parentState(parent);
    const float gap = isRowFlexDirection(flexDirection) ? parentStyle.columnGap.fixedPixels() : parentStyle.rowGap.fixedPixels();
    const auto outerSize = [&](std::size_t index) {
        return std::max(
            0.f,
            mainSize(children[index], flexDirection)
                + (isRowFlexDirection(flexDirection) ? children[index].style.margin.horizontal() : children[index].style.margin.vertical()));
    };
    std::vector<float> itemSizes(children.size());
    std::vector<float> spacing(children.size());
    for (std::size_t index = 0; index < children.size(); ++index) itemSizes[index] = outerSize(index);
    for (std::size_t index = 1; index < children.size(); ++index) {
        const std::optional<AdjacentLayout> adjacent = adjacentLayout(parentState, children[index - 1].node, children[index].node, parentStyle);
        if (!adjacent) return {{0, children.size()}};
        spacing[index] = (adjacent->hasGap ? gap : 0.f) - adjacent->overlap;
    }

    const auto greedyLines = [&] {
        std::vector<std::pair<std::size_t, std::size_t>> result;
        std::size_t lineStart = 0;
        float lineSize = itemSizes.front();
        for (std::size_t index = 1; index < children.size(); ++index)
            if (index > lineStart && lineSize + spacing[index] + itemSizes[index] > availableMain) {
                result.emplace_back(lineStart, index);
                lineStart = index;
                lineSize = itemSizes[index];
            } else lineSize += spacing[index] + itemSizes[index];
        result.emplace_back(lineStart, children.size());
        return result;
    };
    lines = greedyLines();
    if (!parentStyle.flexWrapBalance || lines.size() < 2) return lines;

    std::vector<float> itemPrefix(children.size() + 1);
    std::vector<float> spacingPrefix(children.size() + 1);
    for (std::size_t index = 0; index < children.size(); ++index) {
        itemPrefix[index + 1] = itemPrefix[index] + itemSizes[index];
        spacingPrefix[index + 1] = spacingPrefix[index] + spacing[index];
    }
    const auto lineSize = [&](std::size_t begin, std::size_t end) {
        return itemPrefix[end] - itemPrefix[begin] + spacingPrefix[end] - spacingPrefix[begin + 1];
    };
    float total = lineSize(0, children.size());
    const float target = total / static_cast<float>(lines.size());
    struct State {
        float cost = std::numeric_limits<float>::infinity();
        std::size_t previous = 0;
    };
    // ponytail: bounded O(n²) partitioning; replace with a dedicated flex line-breaker only if large lists require it.
    const std::size_t lineCount = lines.size();
    std::vector<std::vector<State>> states(lineCount + 1, std::vector<State>(children.size() + 1));
    states[0][0].cost = 0.f;
    for (std::size_t line = 1; line <= lineCount; ++line) {
        for (std::size_t end = line; end + (lineCount - line) <= children.size(); ++end) {
            const std::size_t firstBegin = line - 1;
            const std::size_t lastBegin = end - 1;
            for (std::size_t begin = firstBegin; begin <= lastBegin; ++begin) {
                if (!std::isfinite(states[line - 1][begin].cost)) continue;
                const float width = lineSize(begin, end);
                const float overflow = std::max(0.f, width - availableMain);
                const float deviation = width - target;
                const float cost = states[line - 1][begin].cost + deviation * deviation + overflow * overflow * 1000.f;
                if (cost < states[line][end].cost) states[line][end] = {cost, begin};
            }
        }
    }
    if (!std::isfinite(states[lineCount].back().cost)) return lines;
    std::vector<std::pair<std::size_t, std::size_t>> balanced(lineCount);
    std::size_t end = children.size();
    for (std::size_t line = lineCount; line > 0; --line) {
        const std::size_t begin = states[line][end].previous;
        balanced[line - 1] = {begin, end};
        end = begin;
    }
    return balanced;
}

ContentDistributionResult justifyContentDistribution(ContentPosition alignment, ContentDistribution distribution, OverflowAlignment safety,
                                                     LayoutDirection direction, FlexDirection flexDirection, float freeSpace, std::size_t itemCount,
                                                     bool stretchNormal) {
    const float distributable = std::max(0.f, freeSpace);
    if (itemCount == 0) return {};
    if (stretchNormal && (alignment == ContentPosition::Normal || distribution == ContentDistribution::Stretch)) return {0.f, 0.f, true};
    const float positionSpace = safety == OverflowAlignment::Safe ? distributable : freeSpace;
    if (alignment == ContentPosition::Center) return {positionSpace * .5f, 0.f};
    if (alignment == ContentPosition::End) {
        const bool row = isRowFlexDirection(flexDirection);
        const bool logicalStartForward = row ? direction == LayoutDirection::LeftToRight : true;
        const bool flexStartForward =
            row ? (direction == LayoutDirection::LeftToRight) != isReverseFlexDirection(flexDirection) : !isReverseFlexDirection(flexDirection);
        return {logicalStartForward == flexStartForward ? positionSpace : 0.f, 0.f};
    }
    if (alignment == ContentPosition::FlexEnd) return {positionSpace, 0.f};
    if (alignment == ContentPosition::Start) {
        const bool row = isRowFlexDirection(flexDirection);
        const bool logicalStartForward = row ? direction == LayoutDirection::LeftToRight : true;
        const bool flexStartForward =
            row ? (direction == LayoutDirection::LeftToRight) != isReverseFlexDirection(flexDirection) : !isReverseFlexDirection(flexDirection);
        return {logicalStartForward == flexStartForward ? 0.f : positionSpace, 0.f};
    }
    if (alignment == ContentPosition::Left && isRowFlexDirection(flexDirection)) {
        const bool flexStartForward = (direction == LayoutDirection::LeftToRight) != isReverseFlexDirection(flexDirection);
        return {flexStartForward ? 0.f : positionSpace, 0.f};
    }
    if (alignment == ContentPosition::Right && isRowFlexDirection(flexDirection)) {
        const bool flexStartForward = (direction == LayoutDirection::LeftToRight) != isReverseFlexDirection(flexDirection);
        return {flexStartForward ? positionSpace : 0.f, 0.f};
    }
    if (distribution == ContentDistribution::SpaceBetween && itemCount > 1) return {0.f, distributable / static_cast<float>(itemCount - 1)};
    if (distribution == ContentDistribution::SpaceAround) {
        const float extra = distributable / static_cast<float>(itemCount);
        return {extra * .5f, extra};
    }
    if (distribution == ContentDistribution::SpaceEvenly) {
        const float extra = distributable / static_cast<float>(itemCount + 1);
        return {extra, extra};
    }
    return {};
}

ContentDistributionResult alignContentDistribution(ContentPosition alignment, ContentDistribution distribution, OverflowAlignment safety,
                                                   std::vector<float>& lineSizes, float availableCross, float baseGap) {
    if (lineSizes.empty() || availableCross < 0.f) return {};
    float used = baseGap * static_cast<float>(lineSizes.size() - 1);
    for (const float size : lineSizes) used += size;
    const float freeSpace = availableCross - used;
    const float distributable = std::max(0.f, freeSpace);
    if (alignment == ContentPosition::Normal || distribution == ContentDistribution::Stretch) {
        const float extra = distributable / static_cast<float>(lineSizes.size());
        for (float& size : lineSizes) size += extra;
        return {0.f, baseGap};
    }
    const float positionSpace = safety == OverflowAlignment::Safe ? distributable : freeSpace;
    if (alignment == ContentPosition::Center) return {positionSpace * .5f, baseGap};
    if (alignment == ContentPosition::End || alignment == ContentPosition::FlexEnd) return {positionSpace, baseGap};
    if (distribution == ContentDistribution::SpaceBetween && lineSizes.size() > 1)
        return {0.f, baseGap + distributable / static_cast<float>(lineSizes.size() - 1)};
    if (distribution == ContentDistribution::SpaceAround) {
        const float extra = distributable / static_cast<float>(lineSizes.size());
        return {extra * .5f, baseGap + extra};
    }
    if (distribution == ContentDistribution::SpaceEvenly) {
        const float extra = distributable / static_cast<float>(lineSizes.size() + 1);
        return {extra, baseGap + extra};
    }
    if (alignment == ContentPosition::Baseline) return {0.f, baseGap};
    if (alignment == ContentPosition::LastBaseline) return {positionSpace, baseGap};
    return {0.f, baseGap};
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

MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end,
                                    const ComputedStyle& parentStyle, FlexDirection flexDirection, float availableMain) {
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
            mainSize(child, flexDirection) + (isRowFlexDirection(flexDirection) ? child.style.margin.horizontal() : child.style.margin.vertical());
        autoMargins += isRowFlexDirection(flexDirection) ? child.style.margin.horizontalAutoCount() : child.style.margin.verticalAutoCount();
    }
    std::size_t gapCount = 0;
    float overlap = 0.f;
    for (std::size_t index = begin + 1; index < end; ++index) {
        const OrderedChildRef& previous = children[index - 1].node;
        const OrderedChildRef& current = children[index].node;
        if (!parentState.layoutValid()) return invalidAllocation();
        const std::optional<AdjacentLayout> adjacent = adjacentLayout(parentState, previous, current, parentStyle);
        if (!adjacent) return invalidAllocation();
        if (adjacent->hasGap) ++gapCount;
        overlap += adjacent->overlap;
    }
    MainAxisAllocation allocation;
    const GapValue& mainGap = isRowFlexDirection(flexDirection) ? parentStyle.columnGap : parentStyle.rowGap;
    allocation.gap = mainGap.fixedPixels();
    total += allocation.gap * static_cast<float>(gapCount) - overlap;
    distributeFlexSpace(children, begin, end, flexDirection, availableMain, total);
    allocation.freeSpace = availableMain - total;
    allocation.hasAutoMargins = autoMargins != 0;
    if (autoMargins) allocation.autoMargin = std::max(0.f, allocation.freeSpace) / static_cast<float>(autoMargins);
    return allocation;
}

void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain) {
    for (ChildLayout& child : children) {
        const bool horizontal = isRowFlexDirection(flexDirection);
        const Dimension& dimension = child.style.flexBasis.isAuto() ? (horizontal ? child.style.width : child.style.height) : child.style.flexBasis;
        const float fallback = dimension.isIntrinsic() || child.style.flexBasis.isAuto() ? mainSize(child, flexDirection) : 0.f;
        const float base = styledBoxDimension(child.style, horizontal, dimension, std::nullopt, std::nullopt, fallback, availableMain,
                                              horizontal ? child.minContent.x : child.minContent.y);
        flexBaseSize(child, flexDirection) = base;
        const float minimum = mainMinimum(child, flexDirection, availableMain, base);
        mainSize(child, flexDirection) =
            std::max(clampBoxDimension(child.style, horizontal, base, horizontal ? child.style.minWidth : child.style.minHeight,
                                       horizontal ? child.style.maxWidth : child.style.maxHeight, availableMain),
                     minimum);
    }
}
} // namespace radia::ui::layout_detail
