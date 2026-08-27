/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <vector>
#include "elements/element.h"
#include "elements/elementinternal.h"
#include "style/style.h"

namespace radia::ui::layout_detail {
class ElementLayoutAccess {
public:
    static bool hasGap(const Element& parent, const Element& first, const Element& second) { return parent.hasLayoutGapBetween(first, second); }
    static float overlap(const Element& parent, const Element& first, const Element& second, const Style& style) {
        return parent.layoutOverlapBetween(first, second, style);
    }
};

struct ChildLayout {
    detail::NodeRef node;
    Style style;
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
bool isInlineLevel(DisplayMode display);
const Style& emptyChildStyle();
ChildLayout invalidChildLayout();
void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children);
bool isDisplayed(const ChildLayout& child);
bool isWhitespaceOnlyText(const detail::NodeRef& node);
bool flowBreakBefore(const ChildLayout& child);
float& mainSize(ChildLayout& child, FlexDirection flexDirection);
float mainSize(const ChildLayout& child, FlexDirection flexDirection);
float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase);
void applyFlexBasis(ChildLayout& child, FlexDirection flexDirection, float availableMain);
void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain,
                         bool allowGrowth, float& total);
float verticalAlignmentOffset(VerticalAlign alignment, float freeSpace);
CrossAlignment crossAlignment(const Style& parent, const Style& child, FlexDirection flexDirection);
void applyCrossAxisSizing(Vec2& size, const Style& style, FlexDirection flexDirection, float availableCross, CrossAlignment alignment);
float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float freeSpace);
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
std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const detail::NodeRef& first, const detail::NodeRef& second,
                                             const Style& parentStyle);
std::vector<std::pair<std::size_t, std::size_t>> rowLines(const std::vector<ChildLayout>& children);
std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth);
MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, const Style& parentStyle,
                                    FlexDirection flexDirection, float availableMain);
void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain);
} // namespace radia::ui::layout_detail
