/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "LayoutPrimitives.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <limits>
#include <variant>
#include "LayoutGeometry.h"

namespace Core::Layout::detail {
using Core::detail::ElementInternalAccess;

namespace {
float boxSizingExtra(const Style::ComputedStyle& style, bool horizontal) {
    if (style.boxSizing() == Style::BoxSizing::BorderBox)
        return 0.f;
    const RectEdges<float> borderInsets = borderWidths(style);
    return (horizontal ? paddingPixels(style).horizontal() : paddingPixels(style).vertical())
        + (horizontal ? borderInsets.horizontal() : borderInsets.vertical());
}
} // namespace

namespace {
float resolveBoxDimension(const Style::ComputedStyle& style, bool horizontal, const Style::Dimension& value, float fallback,
    float reference, float minContent) {
    const float extra = boxSizingExtra(style, horizontal);
    float contentSize = value.resolve(fallback, reference);
    if (value.isIntrinsic()) {
        switch (value.intrinsicKeyword()) {
        case Style::DimensionKeyword::Content:
            contentSize = fallback;
            break;
        case Style::DimensionKeyword::MaxContent:
            contentSize = fallback;
            break;
        case Style::DimensionKeyword::MinContent:
            contentSize = minContent;
            break;
        case Style::DimensionKeyword::FitContent:
            if (reference > 0.f)
                contentSize = std::min(fallback, std::max(minContent, reference - extra));
            break;
        }
    }
    const float resolved = contentSize + (value.isAuto() ? 0.f : extra);
    return resolved;
}

float resolveConstraint(const Style::ComputedStyle& style, bool horizontal, const Style::Dimension& value, float fallback, float reference,
    float minContent) {
    if (value.isAuto())
        return 0.f;
    return resolveBoxDimension(style, horizontal, value, fallback, reference, minContent);
}
} // namespace

float styledBoxDimension(const Style::ComputedStyle& style, bool horizontal, const Style::Dimension& value,
    const std::optional<Style::Dimension>& minimum, const std::optional<Style::Dimension>& maximum, float fallback, float reference,
    float minContent) {
    const float resolved = resolveBoxDimension(style, horizontal, value, fallback, reference, minContent);
    return clampBoxDimension(style, horizontal, resolved, minimum, maximum, reference, minContent);
}

float clampBoxDimension(const Style::ComputedStyle& style, bool horizontal, float size, const std::optional<Style::Dimension>& minimum,
    const std::optional<Style::Dimension>& maximum, float reference, float minContent) {
    const float minimumSize = minimum ? resolveConstraint(style, horizontal, *minimum, size, reference, minContent) : 0.f;
    const float maximumSize =
        maximum ? resolveConstraint(style, horizontal, *maximum, size, reference, minContent) : std::numeric_limits<float>::infinity();
    return std::min(std::max(size, minimumSize), std::max(maximumSize, minimumSize));
}

float minimumBoxDimension(const Style::ComputedStyle& style, bool horizontal, const std::optional<Style::Dimension>& minimum,
    float reference, float fallback, float minContent) {
    return minimum ? resolveConstraint(style, horizontal, *minimum, fallback, reference, minContent) : 0.f;
}

float maximumBoxDimension(const Style::ComputedStyle& style, bool horizontal, const std::optional<Style::Dimension>& maximum,
    float reference, float fallback, float minContent) {
    return maximum ? resolveConstraint(style, horizontal, *maximum, fallback, reference, minContent)
                   : std::numeric_limits<float>::infinity();
}

float contentBoxDimension(const Style::ComputedStyle& style, bool horizontal, float borderBoxSize) {
    const RectEdges<float> borderInsets = borderWidths(style);
    return std::max(0.f,
        borderBoxSize
            - (horizontal ? paddingPixels(style).horizontal() + borderInsets.horizontal()
                          : paddingPixels(style).vertical() + borderInsets.vertical()));
}

bool isInlineLevel(Style::Display display) {
    return display == Style::Display::Inline || display == Style::Display::InlineBlock || display == Style::Display::InlineFlex
        || display == Style::Display::InlineGrid;
}

const Style::ComputedStyle& emptyChildStyle() {
    static const Style::ComputedStyle sEmpty;
    return sEmpty;
}

ChildLayout invalidChildLayout() { return {OrderedChildRef(), emptyChildStyle(), {}, {}, {}}; }

bool isDisplayed(const ChildLayout& child) {
    if (!child.node)
        return false;
    if (child.node.pseudoElement)
        return child.style.display() != Style::Display::NoneValue;
    if (const Element* element = child.node.element())
        return element->isDisplayed(child.style);
    return child.style.display() != Style::Display::NoneValue;
}

bool isWhitespaceOnlyText(const Core::detail::NodeRef& node) {
    const Text* text = node.text();
    if (!text || text->data().empty())
        return false;
    return std::all_of(text->data().begin(), text->data().end(), [](unsigned char character) {
        return std::isspace(character) != 0;
    });
}

bool isWhitespaceOnlyText(const OrderedChildRef& node) { return !node.pseudoElement && isWhitespaceOnlyText(node.node); }

bool flowBreakBefore(const OrderedChildRef& child) {
    if (child.pseudoElement)
        return false;
    const Node* node = child.get();
    return node && Core::detail::NodeAccess::flowBreakBefore(*node);
}

bool flowBreakBefore(const ChildLayout& child) { return child.flowBreakBefore || flowBreakBefore(child.node); }

void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children) {
    if (std::all_of(children.begin(), children.end(), [&parent](const ChildLayout& child) {
            return child.node.attachedTo(parent) && isDisplayed(child);
        }))
        return;
    std::vector<ChildLayout> attached;
    attached.reserve(children.size());
    for (const ChildLayout& child : children)
        if (child.node.attachedTo(parent) && isDisplayed(child))
            attached.push_back(child);
    children.swap(attached);
}

float& mainSize(ChildLayout& child, Style::FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.measured.x : child.measured.y;
}
float mainSize(const ChildLayout& child, Style::FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.measured.x : child.measured.y;
}

float& flexBaseSize(ChildLayout& child, Style::FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.flexBase.x : child.flexBase.y;
}

float flexBaseSize(const ChildLayout& child, Style::FlexDirection flexDirection) {
    return isRowFlexDirection(flexDirection) ? child.flexBase.x : child.flexBase.y;
}

float mainMinimum(const ChildLayout& child, Style::FlexDirection flexDirection, float availableMain, float flexBase) {
    const std::optional<Style::Dimension>& minimum = isRowFlexDirection(flexDirection) ? child.style.minWidth() : child.style.minHeight();
    if (minimum)
        return minimumBoxDimension(child.style, isRowFlexDirection(flexDirection), minimum, availableMain, flexBase,
            isRowFlexDirection(flexDirection) ? child.minContent.x : child.minContent.y);
    const float automatic = isRowFlexDirection(flexDirection) ? child.fitSize.x : child.fitSize.y;
    return std::min(automatic, flexBase);
}

void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, Style::FlexDirection flexDirection,
    float availableMain, float& total) {
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
        const float maximum =
            std::max(maximumBoxDimension(child.style, horizontal, horizontal ? child.style.maxWidth() : child.style.maxHeight(),
                         availableMain, base, horizontal ? child.minContent.x : child.minContent.y),
                minimum);
        items.push_back({index, base, mainSize(child, flexDirection), base, minimum, maximum, false});
        hypotheticalTotal += mainSize(child, flexDirection);
    }

    const float nonFlexTotal = total - hypotheticalTotal;
    for (FlexItem& item : items) {
        const ChildLayout& child = children[item.index];
        const bool hasFlexFactor = growing ? child.style.flexGrow().value > 0.f : child.style.flexShrink().value > 0.f;
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
            if (item.frozen)
                used += item.target;
            else {
                hasUnfrozenItems = true;
                used += item.base;
                const ChildLayout& child = children[item.index];
                factorTotal += growing ? child.style.flexGrow().value : child.style.flexShrink().value * item.base;
            }
        }
        if (!hasUnfrozenItems)
            break;

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
            if (item.frozen)
                continue;
            const ChildLayout& child = children[item.index];
            const float proposed = growing ? item.base + adjustedFreeSpace * child.style.flexGrow().value / factorTotal
                                           : item.base + adjustedFreeSpace * child.style.flexShrink().value * item.base / factorTotal;
            item.target = std::min(std::max(proposed, item.minimum), item.maximum);
            if (std::abs(item.target - proposed) > kFlexEpsilon) {
                item.frozen = true;
                frozeItem = true;
            }
        }
        if (!frozeItem)
            break;
    }

    total = nonFlexTotal;
    for (const FlexItem& item : items) {
        mainSize(children[item.index], flexDirection) = item.target;
        total += item.target;
    }
}

float verticalAlignmentOffset(const Style::ComputedStyle& style, float freeSpace) {
    const Style::VerticalAlignValue& alignment = style.verticalAlign();
    if (alignment.value == Style::VerticalAlign::Middle)
        return freeSpace * .5f;
    if (alignment.value == Style::VerticalAlign::Bottom)
        return freeSpace;
    if (alignment.value == Style::VerticalAlign::Sub)
        return -style.fontSize() * .2f;
    if (alignment.value == Style::VerticalAlign::Super)
        return style.fontSize() * .4f;
    if (alignment.value == Style::VerticalAlign::TextTop)
        return freeSpace;
    if (alignment.value == Style::VerticalAlign::TextBottom)
        return 0.f;
    if (alignment.value == Style::VerticalAlign::Length || alignment.value == Style::VerticalAlign::Percentage) {
        const auto* length = std::get_if<Style::LineHeight::Length>(&style.lineHeight().mValue);
        return alignment.offset.resolve(length ? length->pixels : freeSpace);
    }
    return 0.f;
}

Style::ItemPosition crossAlignment(const Style::ComputedStyle& parent, const Style::ComputedStyle& child) {
    const Style::SelfAlignmentData childAlignSelf = child.alignSelf();
    const Style::SelfAlignmentData parentAlignItems = parent.alignItems();
    if (childAlignSelf.position != Style::ItemPosition::Auto) {
        if (childAlignSelf.position == Style::ItemPosition::Start || childAlignSelf.position == Style::ItemPosition::SelfStart)
            return Style::ItemPosition::Start;
        if (childAlignSelf.position == Style::ItemPosition::FlexStart)
            return Style::ItemPosition::FlexStart;
        if (childAlignSelf.position == Style::ItemPosition::Center)
            return Style::ItemPosition::Center;
        if (childAlignSelf.position == Style::ItemPosition::End || childAlignSelf.position == Style::ItemPosition::SelfEnd)
            return Style::ItemPosition::End;
        if (childAlignSelf.position == Style::ItemPosition::FlexEnd)
            return Style::ItemPosition::FlexEnd;
        if (childAlignSelf.position == Style::ItemPosition::Baseline || childAlignSelf.position == Style::ItemPosition::LastBaseline)
            return childAlignSelf.position;
        if (childAlignSelf.position == Style::ItemPosition::AnchorCenter)
            return Style::ItemPosition::Center;
        return Style::ItemPosition::Stretch;
    }
    if (parentAlignItems.position == Style::ItemPosition::Start || parentAlignItems.position == Style::ItemPosition::SelfStart)
        return Style::ItemPosition::Start;
    if (parentAlignItems.position == Style::ItemPosition::FlexStart)
        return Style::ItemPosition::FlexStart;
    if (parentAlignItems.position == Style::ItemPosition::Center)
        return Style::ItemPosition::Center;
    if (parentAlignItems.position == Style::ItemPosition::End || parentAlignItems.position == Style::ItemPosition::SelfEnd)
        return Style::ItemPosition::End;
    if (parentAlignItems.position == Style::ItemPosition::FlexEnd)
        return Style::ItemPosition::FlexEnd;
    if (parentAlignItems.position == Style::ItemPosition::Baseline || parentAlignItems.position == Style::ItemPosition::LastBaseline)
        return parentAlignItems.position;
    if (parentAlignItems.position == Style::ItemPosition::AnchorCenter)
        return Style::ItemPosition::Center;
    if (parentAlignItems.position == Style::ItemPosition::Stretch)
        return Style::ItemPosition::Stretch;
    return Style::ItemPosition::Stretch;
}

Style::OverflowAlignment crossAlignmentSafety(const Style::ComputedStyle& parent, const Style::ComputedStyle& child) {
    const Style::SelfAlignmentData childAlignSelf = child.alignSelf();
    const Style::SelfAlignmentData parentAlignItems = parent.alignItems();
    return childAlignSelf.position == Style::ItemPosition::Auto ? parentAlignItems.overflow : childAlignSelf.overflow;
}

void applyCrossAxisSizing(Vec2& size, const Style::ComputedStyle& style, Style::FlexDirection flexDirection, float availableCross,
    Style::ItemPosition alignment) {
    if (alignment != Style::ItemPosition::Stretch)
        return;
    if (isRowFlexDirection(flexDirection)) {
        if (!style.height().isAuto() || verticalAutoMarginCount(style.margin()))
            return;
        const float height = std::max(0.f, availableCross - verticalMargin(style.margin()));
        size.y = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), height, availableCross);
        if (style.aspectRatio && style.width().isAuto() && *style.aspectRatio > 0.f)
            size.x = size.y * *style.aspectRatio;
    } else {
        if (!style.width().isAuto() || horizontalAutoMarginCount(style.margin()))
            return;
        const float width = std::max(0.f, availableCross - horizontalMargin(style.margin()));
        size.x = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), width, availableCross);
        if (style.aspectRatio && style.height().isAuto() && *style.aspectRatio > 0.f)
            size.y = size.x / *style.aspectRatio;
    }
}

float textAlignmentOffset(Style::TextAlign alignment, Direction direction, float freeSpace) {
    if (alignment == Style::TextAlign::Center)
        return freeSpace * .5f;
    if (alignment == Style::TextAlign::End)
        return freeSpace;
    if (alignment == Style::TextAlign::Left)
        return direction == Direction::RightToLeft ? freeSpace : 0.f;
    if (alignment == Style::TextAlign::Right)
        return direction == Direction::RightToLeft ? 0.f : freeSpace;
    if (alignment == Style::TextAlign::MatchParent)
        return direction == Direction::RightToLeft ? freeSpace : 0.f;
    return 0.f;
}

float justifySelfOffset(Style::ItemPosition alignment, Style::OverflowAlignment safety, Direction direction, float freeSpace) {
    const float distributable = safety == Style::OverflowAlignment::Safe ? std::max(0.f, freeSpace) : freeSpace;
    if (alignment == Style::ItemPosition::Center || alignment == Style::ItemPosition::AnchorCenter)
        return distributable * .5f;
    if (alignment == Style::ItemPosition::End || alignment == Style::ItemPosition::SelfEnd || alignment == Style::ItemPosition::FlexEnd)
        return direction == Direction::RightToLeft ? 0.f : distributable;
    if (alignment == Style::ItemPosition::Start || alignment == Style::ItemPosition::SelfStart
        || alignment == Style::ItemPosition::FlexStart)
        return direction == Direction::RightToLeft ? distributable : 0.f;
    if (alignment == Style::ItemPosition::Right)
        return direction == Direction::RightToLeft ? 0.f : distributable;
    if (alignment == Style::ItemPosition::Left)
        return direction == Direction::RightToLeft ? distributable : 0.f;
    if (alignment == Style::ItemPosition::Baseline) {
        const float safeSpace = std::max(0.f, freeSpace);
        return direction == Direction::RightToLeft ? safeSpace : 0.f;
    }
    if (alignment == Style::ItemPosition::LastBaseline)
        return direction == Direction::RightToLeft ? 0.f : std::max(0.f, freeSpace);
    return 0.f;
}

float alignSelfOffset(Style::ItemPosition alignment, Style::OverflowAlignment safety, float freeSpace) {
    const float distributable = safety == Style::OverflowAlignment::Safe ? std::max(0.f, freeSpace) : freeSpace;
    if (alignment == Style::ItemPosition::Center || alignment == Style::ItemPosition::AnchorCenter)
        return distributable * .5f;
    if (alignment == Style::ItemPosition::End || alignment == Style::ItemPosition::SelfEnd || alignment == Style::ItemPosition::FlexEnd)
        return distributable;
    if (alignment == Style::ItemPosition::LastBaseline)
        return std::max(0.f, freeSpace);
    return 0.f;
}

GridTrackSizes gridTrackSizes(const std::vector<ChildLayout>& children, std::optional<float> availableWidth,
    std::optional<float> availableHeight, float columnGap, float rowGap) {
    std::size_t columnCount = 1;
    std::size_t rowCount = 1;
    for (const ChildLayout& child : children) {
        if (!isDisplayed(child))
            continue;
        const Style::GridArea area = child.style.gridArea.value_or(Style::GridArea {});
        columnCount = std::max(columnCount, static_cast<std::size_t>(std::max(1, area.column)));
        rowCount = std::max(rowCount, static_cast<std::size_t>(std::max(1, area.row)));
    }

    GridTrackSizes result;
    result.columns.resize(columnCount);
    result.rows.resize(rowCount);
    for (const ChildLayout& child : children) {
        if (!isDisplayed(child))
            continue;
        const Style::GridArea area = child.style.gridArea.value_or(Style::GridArea {});
        const std::size_t column = static_cast<std::size_t>(std::max(1, area.column)) - 1;
        const std::size_t row = static_cast<std::size_t>(std::max(1, area.row)) - 1;
        result.columns[column] = std::max(result.columns[column], child.measured.x + horizontalMargin(child.style.margin()));
        result.rows[row] = std::max(result.rows[row], child.measured.y + verticalMargin(child.style.margin()));
    }

    const auto distributeFreeSpace = [](std::vector<float>& tracks, std::optional<float> available, float gap) {
        if (!available || tracks.empty())
            return;
        float used = 0.f;
        for (const float track : tracks)
            used += track;
        const float freeSpace = *available - used - gap * static_cast<float>(tracks.size() - 1);
        if (freeSpace <= 0.f)
            return;
        const float extra = freeSpace / static_cast<float>(tracks.size());
        for (float& track : tracks)
            track += extra;
    };
    distributeFreeSpace(result.columns, availableWidth, columnGap);
    distributeFreeSpace(result.rows, availableHeight, rowGap);
    return result;
}

Rect positionedRect(const ChildLayout& child, const Rect& parent) {
    const Element* node = child.node.element();
    const bool explicitRect = node && node->mRectExplicit;
    const float width = explicitRect && child.style.width().isAuto()
        ? clampBoxDimension(child.style, true, node->mRect.w, child.style.minWidth(), child.style.maxWidth(), parent.w)
        : styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
              child.measured.x > 0.f ? child.measured.x : parent.w, parent.w, child.minContent.x);
    const float height = explicitRect && child.style.height().isAuto()
        ? clampBoxDimension(child.style, false, node->mRect.h, child.style.minHeight(), child.style.maxHeight(), parent.h)
        : styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(), child.style.maxHeight(), child.measured.y,
              parent.h, child.minContent.y);
    const RectEdges<Style::MarginEdge>& margin = child.style.margin();
    const float horizontalSpace = std::max(0.f, parent.w - width - horizontalMargin(margin));
    float x = explicitRect     ? node->mRect.x
        : margin.left.isAuto() ? parent.left() + horizontalSpace
                               : parent.left() + margin.left.fixedPixels();
    if (margin.left.isAuto() && margin.right.isAuto())
        x = parent.left() + horizontalSpace * .5f;
    const float y = explicitRect ? node->mRect.y : parent.top() - margin.top.fixedPixels() - height;
    return {x, y, width, height};
}

Rect outOfFlowRect(const ChildLayout& child, const Rect& containingBlock) {
    const Style::ComputedStyle& style = child.style;
    const RectEdges<Style::MarginEdge>& margin = style.margin();
    const Style::InsetEdge left = style.left();
    const Style::InsetEdge right = style.right();
    const Style::InsetEdge top = style.top();
    const Style::InsetEdge bottom = style.bottom();
    const float leftInset = left ? left->resolve(containingBlock.w) : 0.f;
    const float rightInset = right ? right->resolve(containingBlock.w) : 0.f;
    const float topInset = top ? top->resolve(containingBlock.h) : 0.f;
    const float bottomInset = bottom ? bottom->resolve(containingBlock.h) : 0.f;
    float width = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), child.measured.x, containingBlock.w,
        child.minContent.x);
    float height = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), child.measured.y,
        containingBlock.h, child.minContent.y);
    if (style.width().isAuto() && left && right)
        width = std::max(0.f, containingBlock.w - leftInset - rightInset - horizontalMargin(margin));
    if (style.height().isAuto() && top && bottom)
        height = std::max(0.f, containingBlock.h - topInset - bottomInset - verticalMargin(margin));
    const float x = left ? containingBlock.left() + leftInset + margin.left.fixedPixels()
        : right          ? containingBlock.right() - rightInset - margin.right.fixedPixels() - width
                         : containingBlock.left() + margin.left.fixedPixels();
    const float y = top ? containingBlock.top() - topInset - margin.top.fixedPixels() - height
        : bottom        ? containingBlock.bottom() + bottomInset + margin.bottom.fixedPixels()
                        : containingBlock.top() - margin.top.fixedPixels() - height;
    return {x, y, width, height};
}

Rect relativeRect(const ChildLayout& child, const Rect& rect, const Rect& containingBlock) {
    if (child.style.position() != Style::Position::Relative)
        return rect;

    const Style::InsetEdge left = child.style.left();
    const Style::InsetEdge right = child.style.right();
    const Style::InsetEdge top = child.style.top();
    const Style::InsetEdge bottom = child.style.bottom();
    float x = rect.x;
    float y = rect.y;
    if (left)
        x += left->resolve(containingBlock.w);
    else if (right)
        x -= right->resolve(containingBlock.w);
    if (top)
        y -= top->resolve(containingBlock.h);
    else if (bottom)
        y += bottom->resolve(containingBlock.h);
    return {x, y, rect.w, rect.h};
}

Rect translatedRect(const ChildLayout& child, const Rect& rect) {
    const Style::Translate& translation = child.style.translate();
    if (translation.isNone)
        return rect;
    return {rect.x + translation.x.resolve(rect.w), rect.y - translation.y.resolve(rect.h), rect.w, rect.h};
}

void setArrangedRect(Element& node, const Rect& rect) {
    const bool changed = node.mRect.x != rect.x || node.mRect.y != rect.y || node.mRect.w != rect.w || node.mRect.h != rect.h;
    if (!changed)
        return;
    node.mRect = rect;
    ElementInternalAccess::layoutCache(node).arrangeValid = false;
    node.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    node.invalidatePaint();
}

std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const OrderedChildRef& firstRef,
    const OrderedChildRef& secondRef, const Style::ComputedStyle& parentStyle) {
    Element* parent = parentState.get();
    if (!parentState.layoutValid() || !firstRef.attachedTo(*parent) || !secondRef.attachedTo(*parent))
        return std::nullopt;
    if (firstRef.isPseudoElement() || secondRef.isPseudoElement())
        return AdjacentLayout {true, 0.f};
    Node* firstNode = firstRef.get();
    Node* secondNode = secondRef.get();
    if (!firstNode || !secondNode)
        return std::nullopt;
    Element* first = firstRef.element();
    Element* second = secondRef.element();
    if (!first || !second)
        return AdjacentLayout {true, 0.f};
    const ElementVisit firstState(*first);
    const ElementVisit secondState(*second);
    const bool hasGap = ElementLayoutAccess::hasGap(*parent, *first, *second);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.layoutValid() || !parent || !firstState.layoutValid() || !firstState.attachedTo(*parent) || !secondState.layoutValid()
        || !secondState.attachedTo(*parent))
        return std::nullopt;
    const float overlap = ElementLayoutAccess::overlap(*parent, *first, *second, parentStyle);
    parent = parentState.get();
    first = firstState.get();
    second = secondState.get();
    if (!parentState.layoutValid() || !parent || !firstState.layoutValid() || !firstState.attachedTo(*parent) || !secondState.layoutValid()
        || !secondState.attachedTo(*parent))
        return std::nullopt;
    return AdjacentLayout {hasGap, overlap};
}

std::vector<std::pair<std::size_t, std::size_t>> flexLines(Element& parent, const std::vector<ChildLayout>& children,
    const Style::ComputedStyle& parentStyle, Style::FlexDirection flexDirection, float availableMain) {
    std::vector<std::pair<std::size_t, std::size_t>> lines;
    if (children.empty())
        return lines;
    if (!isFlexWrapMultiLine(parentStyle.flexWrap()) || availableMain < 0.f) {
        lines.emplace_back(0, children.size());
        return lines;
    }

    const ElementVisit parentState(parent);
    const float gap = isRowFlexDirection(flexDirection) ? parentStyle.columnGap().fixedPixels() : parentStyle.rowGap().fixedPixels();
    const auto outerSize = [&](std::size_t index) {
        return std::max(0.f,
            mainSize(children[index], flexDirection)
                + (isRowFlexDirection(flexDirection) ? horizontalMargin(children[index].style.margin())
                                                     : verticalMargin(children[index].style.margin())));
    };
    std::vector<float> itemSizes(children.size());
    std::vector<float> spacing(children.size());
    for (std::size_t index = 0; index < children.size(); ++index)
        itemSizes[index] = outerSize(index);
    for (std::size_t index = 1; index < children.size(); ++index) {
        const std::optional<AdjacentLayout> adjacent =
            adjacentLayout(parentState, children[index - 1].node, children[index].node, parentStyle);
        if (!adjacent)
            return {{0, children.size()}};
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
            } else
                lineSize += spacing[index] + itemSizes[index];
        result.emplace_back(lineStart, children.size());
        return result;
    };
    lines = greedyLines();
    if (!parentStyle.flexWrap().balance || lines.size() < 2)
        return lines;

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
                if (!std::isfinite(states[line - 1][begin].cost))
                    continue;
                const float width = lineSize(begin, end);
                const float overflow = std::max(0.f, width - availableMain);
                const float deviation = width - target;
                const float cost = states[line - 1][begin].cost + deviation * deviation + overflow * overflow * 1000.f;
                if (cost < states[line][end].cost)
                    states[line][end] = {cost, begin};
            }
        }
    }
    if (!std::isfinite(states[lineCount].back().cost))
        return lines;
    std::vector<std::pair<std::size_t, std::size_t>> balanced(lineCount);
    std::size_t end = children.size();
    for (std::size_t line = lineCount; line > 0; --line) {
        const std::size_t begin = states[line][end].previous;
        balanced[line - 1] = {begin, end};
        end = begin;
    }
    return balanced;
}

ContentDistributionResult justifyContentDistribution(Style::ContentPosition alignment, Style::ContentDistribution distribution,
    Style::OverflowAlignment safety, Direction direction, Style::FlexDirection flexDirection, float freeSpace, std::size_t itemCount,
    bool stretchNormal) {
    const float distributable = std::max(0.f, freeSpace);
    if (itemCount == 0)
        return {};
    if (stretchNormal && (alignment == Style::ContentPosition::Normal || distribution == Style::ContentDistribution::Stretch))
        return {0.f, 0.f, true};
    const float positionSpace = safety == Style::OverflowAlignment::Safe ? distributable : freeSpace;
    if (alignment == Style::ContentPosition::Center)
        return {positionSpace * .5f, 0.f};
    if (alignment == Style::ContentPosition::End) {
        const bool row = isRowFlexDirection(flexDirection);
        const bool logicalStartForward = row ? direction == Direction::LeftToRight : true;
        const bool flexStartForward =
            row ? (direction == Direction::LeftToRight) != isReverseFlexDirection(flexDirection) : !isReverseFlexDirection(flexDirection);
        return {logicalStartForward == flexStartForward ? positionSpace : 0.f, 0.f};
    }
    if (alignment == Style::ContentPosition::FlexEnd)
        return {positionSpace, 0.f};
    if (alignment == Style::ContentPosition::Start) {
        const bool row = isRowFlexDirection(flexDirection);
        const bool logicalStartForward = row ? direction == Direction::LeftToRight : true;
        const bool flexStartForward =
            row ? (direction == Direction::LeftToRight) != isReverseFlexDirection(flexDirection) : !isReverseFlexDirection(flexDirection);
        return {logicalStartForward == flexStartForward ? 0.f : positionSpace, 0.f};
    }
    if (alignment == Style::ContentPosition::Left && isRowFlexDirection(flexDirection)) {
        const bool flexStartForward = (direction == Direction::LeftToRight) != isReverseFlexDirection(flexDirection);
        return {flexStartForward ? 0.f : positionSpace, 0.f};
    }
    if (alignment == Style::ContentPosition::Right && isRowFlexDirection(flexDirection)) {
        const bool flexStartForward = (direction == Direction::LeftToRight) != isReverseFlexDirection(flexDirection);
        return {flexStartForward ? positionSpace : 0.f, 0.f};
    }
    if (distribution == Style::ContentDistribution::SpaceBetween && itemCount > 1)
        return {0.f, distributable / static_cast<float>(itemCount - 1)};
    if (distribution == Style::ContentDistribution::SpaceAround) {
        const float extra = distributable / static_cast<float>(itemCount);
        return {extra * .5f, extra};
    }
    if (distribution == Style::ContentDistribution::SpaceEvenly) {
        const float extra = distributable / static_cast<float>(itemCount + 1);
        return {extra, extra};
    }
    return {};
}

ContentDistributionResult alignContentDistribution(Style::ContentPosition alignment, Style::ContentDistribution distribution,
    Style::OverflowAlignment safety, std::vector<float>& lineSizes, float availableCross, float baseGap) {
    if (lineSizes.empty() || availableCross < 0.f)
        return {};
    float used = baseGap * static_cast<float>(lineSizes.size() - 1);
    for (const float size : lineSizes)
        used += size;
    const float freeSpace = availableCross - used;
    const float distributable = std::max(0.f, freeSpace);
    if (alignment == Style::ContentPosition::Normal || distribution == Style::ContentDistribution::Stretch) {
        const float extra = distributable / static_cast<float>(lineSizes.size());
        for (float& size : lineSizes)
            size += extra;
        return {0.f, baseGap};
    }
    const float positionSpace = safety == Style::OverflowAlignment::Safe ? distributable : freeSpace;
    if (alignment == Style::ContentPosition::Center)
        return {positionSpace * .5f, baseGap};
    if (alignment == Style::ContentPosition::End || alignment == Style::ContentPosition::FlexEnd)
        return {positionSpace, baseGap};
    if (distribution == Style::ContentDistribution::SpaceBetween && lineSizes.size() > 1)
        return {0.f, baseGap + distributable / static_cast<float>(lineSizes.size() - 1)};
    if (distribution == Style::ContentDistribution::SpaceAround) {
        const float extra = distributable / static_cast<float>(lineSizes.size());
        return {extra * .5f, baseGap + extra};
    }
    if (distribution == Style::ContentDistribution::SpaceEvenly) {
        const float extra = distributable / static_cast<float>(lineSizes.size() + 1);
        return {extra, baseGap + extra};
    }
    if (alignment == Style::ContentPosition::Baseline)
        return {0.f, baseGap};
    if (alignment == Style::ContentPosition::LastBaseline)
        return {positionSpace, baseGap};
    return {0.f, baseGap};
}

std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth) {
    std::vector<NormalLine> lines;
    std::optional<NormalLine> current;
    const auto finish = [&] {
        if (current)
            lines.push_back(*current);
        current.reset();
    };

    for (std::size_t index = 0; index < children.size(); ++index) {
        const ChildLayout& child = children[index];
        if (!child.node)
            continue;
        const float width = child.measured.x + horizontalMargin(child.style.margin());
        const float height = child.measured.y + verticalMargin(child.style.margin());
        const bool block = !isInlineLevel(child.style.display());
        if (block) {
            finish();
            lines.push_back({index, index + 1, width, height, true});
            continue;
        }
        if (flowBreakBefore(child) && current)
            finish();
        if (availableWidth && current && current->width > 0.f && current->width + width > *availableWidth)
            finish();
        if (!current)
            current = NormalLine {index, index + 1, width, height, false};
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
    const Style::ComputedStyle& parentStyle, Style::FlexDirection flexDirection, float availableMain) {
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
        total += mainSize(child, flexDirection)
            + (isRowFlexDirection(flexDirection) ? horizontalMargin(child.style.margin()) : verticalMargin(child.style.margin()));
        autoMargins += isRowFlexDirection(flexDirection) ? horizontalAutoMarginCount(child.style.margin())
                                                         : verticalAutoMarginCount(child.style.margin());
    }
    std::size_t gapCount = 0;
    float overlap = 0.f;
    for (std::size_t index = begin + 1; index < end; ++index) {
        const OrderedChildRef& previous = children[index - 1].node;
        const OrderedChildRef& current = children[index].node;
        if (!parentState.layoutValid())
            return invalidAllocation();
        const std::optional<AdjacentLayout> adjacent = adjacentLayout(parentState, previous, current, parentStyle);
        if (!adjacent)
            return invalidAllocation();
        if (adjacent->hasGap)
            ++gapCount;
        overlap += adjacent->overlap;
    }
    MainAxisAllocation allocation;
    const Style::GapGutter& mainGap = isRowFlexDirection(flexDirection) ? parentStyle.columnGap() : parentStyle.rowGap();
    allocation.gap = mainGap.fixedPixels();
    total += allocation.gap * static_cast<float>(gapCount) - overlap;
    distributeFlexSpace(children, begin, end, flexDirection, availableMain, total);
    allocation.freeSpace = availableMain - total;
    allocation.hasAutoMargins = autoMargins != 0;
    if (autoMargins)
        allocation.autoMargin = std::max(0.f, allocation.freeSpace) / static_cast<float>(autoMargins);
    return allocation;
}

void prepareMainAxis(std::vector<ChildLayout>& children, Style::FlexDirection flexDirection, float availableMain) {
    for (ChildLayout& child : children) {
        const bool horizontal = isRowFlexDirection(flexDirection);
        const Style::Dimension& dimension =
            child.style.flexBasis().isAuto() ? (horizontal ? child.style.width() : child.style.height()) : child.style.flexBasis();
        const float fallback = dimension.isIntrinsic() || child.style.flexBasis().isAuto() ? mainSize(child, flexDirection) : 0.f;
        const float base = styledBoxDimension(child.style, horizontal, dimension, std::nullopt, std::nullopt, fallback, availableMain,
            horizontal ? child.minContent.x : child.minContent.y);
        flexBaseSize(child, flexDirection) = base;
        const float minimum = mainMinimum(child, flexDirection, availableMain, base);
        mainSize(child, flexDirection) =
            std::max(clampBoxDimension(child.style, horizontal, base, horizontal ? child.style.minWidth() : child.style.minHeight(),
                         horizontal ? child.style.maxWidth() : child.style.maxHeight(), availableMain),
                minimum);
    }
}
} // namespace Core::Layout::detail
