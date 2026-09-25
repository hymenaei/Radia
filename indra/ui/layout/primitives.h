/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <vector>
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "style/computedstyle.h"
#include "style/pseudoelement.h"

namespace radia::ui::layout_detail {
class ElementLayoutAccess {
public:
    static bool hasGap(const Element& parent, const Element& first, const Element& second) { return parent.hasLayoutGapBetween(first, second); }
    static float overlap(const Element& parent, const Element& first, const Element& second, const ComputedStyle& style) {
        return parent.layoutOverlapBetween(first, second, style);
    }
};

struct LayoutChildRef {
    detail::NodeRef node;
    PseudoElement* pseudoElement = nullptr;

    LayoutChildRef() = default;
    explicit LayoutChildRef(Node* child) : node(child) {}
    explicit LayoutChildRef(PseudoElement* child) : pseudoElement(child) {}

    bool isPseudoElement() const noexcept { return pseudoElement != nullptr; }
    bool attachedTo(const Element& parent) const noexcept {
        return pseudoElement ? pseudoElement->parentPseudoElement() == nullptr && &pseudoElement->originatingElement() == &parent
                             : node && node.get()->parentElement() == &parent;
    }
    bool attachedTo(const PseudoElement& parent) const noexcept { return pseudoElement && pseudoElement->parentPseudoElement() == &parent; }
    Node* get() const noexcept { return node.get(); }
    Element* element() const noexcept { return node.element(); }
    Text* text() const noexcept { return node.text(); }
    explicit operator bool() const noexcept { return pseudoElement || static_cast<bool>(node); }
};

struct ChildLayout {
    LayoutChildRef node;
    ComputedStyle style;
    Vec2 fitSize;
    Vec2 measured;
};

struct NormalLine {
    std::size_t begin = 0;
    std::size_t end = 0;
    float width = 0.f;
    float height = 0.f;
    bool block = false;
};

enum class CrossAlignment { Start, Center, End, Stretch };

struct AdjacentLayout {
    bool hasGap = false;
    float overlap = 0.f;
};

struct MainAxisAllocation {
    float gap = 0.f;
    float freeSpace = 0.f;
    float autoMargin = 0.f;
    bool hasAutoMargins = false;
    bool valid = true;
};

float styledDimension(const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference = 0.f);
float styledBoxDimension(const ComputedStyle& style, bool horizontal, const Dimension& value, const std::optional<Length>& minimum, float fallback,
                         float reference = 0.f);
float minimumBoxDimension(const ComputedStyle& style, bool horizontal, const std::optional<Length>& minimum, float reference);
float contentBoxDimension(const ComputedStyle& style, bool horizontal, float borderBoxSize);
bool isInlineLevel(DisplayMode display);
const ComputedStyle& emptyChildStyle();
ChildLayout invalidChildLayout();
void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children);
bool isDisplayed(const ChildLayout& child);
bool isWhitespaceOnlyText(const detail::NodeRef& node);
bool isWhitespaceOnlyText(const LayoutChildRef& node);
bool flowBreakBefore(const ChildLayout& child);
float& mainSize(ChildLayout& child, FlexDirection flexDirection);
float mainSize(const ChildLayout& child, FlexDirection flexDirection);
float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase);
void applyFlexBasis(ChildLayout& child, FlexDirection flexDirection, float availableMain);
void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain,
                         bool allowGrowth, float& total);
float verticalAlignmentOffset(VerticalAlign alignment, float freeSpace);
CrossAlignment crossAlignment(const ComputedStyle& parent, const ComputedStyle& child, FlexDirection flexDirection);
void applyCrossAxisSizing(Vec2& size, const ComputedStyle& style, FlexDirection flexDirection, float availableCross, CrossAlignment alignment);
float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float freeSpace);
float textAlignmentOffset(TextAlign alignment, LayoutDirection direction, float freeSpace);
float justifySelfOffset(JustifySelf alignment, LayoutDirection direction, float freeSpace);
struct GridTrackSizes {
    std::vector<float> columns;
    std::vector<float> rows;
};
GridTrackSizes gridTrackSizes(const std::vector<ChildLayout>& children, std::optional<float> availableWidth = std::nullopt,
                              std::optional<float> availableHeight = std::nullopt);
Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment);
Rect relativeRect(const ChildLayout& child, const Rect& rect, const Rect& containingBlock);
Rect translatedRect(const ChildLayout& child, const Rect& rect);
void setArrangedRect(Element& node, const Rect& rect);
std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const LayoutChildRef& first, const LayoutChildRef& second,
                                             const ComputedStyle& parentStyle);
std::vector<std::pair<std::size_t, std::size_t>> rowLines(const std::vector<ChildLayout>& children);
std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth);
MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end,
                                    const ComputedStyle& parentStyle, FlexDirection flexDirection, float availableMain);
void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain);
} // namespace radia::ui::layout_detail
