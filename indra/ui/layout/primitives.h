/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <utility>
#include <vector>
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "layout/treecache.h"
#include "style/computedstyle.h"

namespace radia::ui::layout_detail {
class ElementLayoutAccess {
public:
    static bool hasGap(const Element& parent, const Element& first, const Element& second) { return parent.hasLayoutGapBetween(first, second); }
    static float overlap(const Element& parent, const Element& first, const Element& second, const ComputedStyle& style) {
        return parent.layoutOverlapBetween(first, second, style);
    }
};

struct ChildLayout {
    OrderedChildRef node;
    ComputedStyle style;
    Vec2 fitSize;
    Vec2 measured;
    Vec2 flexBase;
    Vec2 minContent;
};

struct NormalLine {
    std::size_t begin = 0;
    std::size_t end = 0;
    float width = 0.f;
    float height = 0.f;
    bool block = false;
};

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

float styledBoxDimension(const ComputedStyle& style, bool horizontal, const Dimension& value, const std::optional<Dimension>& minimum,
                         const std::optional<Dimension>& maximum, float fallback, float reference = 0.f, float minContent = 0.f);
float clampBoxDimension(const ComputedStyle& style, bool horizontal, float size, const std::optional<Dimension>& minimum,
                        const std::optional<Dimension>& maximum, float reference, float minContent = 0.f);
float minimumBoxDimension(const ComputedStyle& style, bool horizontal, const std::optional<Dimension>& minimum, float reference, float fallback = 0.f,
                          float minContent = 0.f);
float maximumBoxDimension(const ComputedStyle& style, bool horizontal, const std::optional<Dimension>& maximum, float reference, float fallback = 0.f,
                          float minContent = 0.f);
float contentBoxDimension(const ComputedStyle& style, bool horizontal, float borderBoxSize);
bool isInlineLevel(Display display);
const ComputedStyle& emptyChildStyle();
ChildLayout invalidChildLayout();
void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children);
bool isDisplayed(const ChildLayout& child);
bool isWhitespaceOnlyText(const detail::NodeRef& node);
bool isWhitespaceOnlyText(const OrderedChildRef& node);
bool flowBreakBefore(const ChildLayout& child);
float& mainSize(ChildLayout& child, FlexDirection flexDirection);
float mainSize(const ChildLayout& child, FlexDirection flexDirection);
float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase);
void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain,
                         float& total);
float verticalAlignmentOffset(const ComputedStyle& style, float freeSpace);
ItemPosition crossAlignment(const ComputedStyle& parent, const ComputedStyle& child);
OverflowAlignment crossAlignmentSafety(const ComputedStyle& parent, const ComputedStyle& child);
void applyCrossAxisSizing(Vec2& size, const ComputedStyle& style, FlexDirection flexDirection, float availableCross, ItemPosition alignment);
float textAlignmentOffset(TextAlign alignment, LayoutDirection direction, float freeSpace);
float justifySelfOffset(ItemPosition alignment, OverflowAlignment safety, LayoutDirection direction, float freeSpace);
float alignSelfOffset(ItemPosition alignment, OverflowAlignment safety, float freeSpace);
struct GridTrackSizes {
    std::vector<float> columns;
    std::vector<float> rows;
};
GridTrackSizes gridTrackSizes(const std::vector<ChildLayout>& children, std::optional<float> availableWidth = std::nullopt,
                              std::optional<float> availableHeight = std::nullopt, float columnGap = 0.f, float rowGap = 0.f);
Rect positionedRect(const ChildLayout& child, const Rect& parent);
Rect outOfFlowRect(const ChildLayout& child, const Rect& containingBlock);
Rect relativeRect(const ChildLayout& child, const Rect& rect, const Rect& containingBlock);
Rect translatedRect(const ChildLayout& child, const Rect& rect);
void setArrangedRect(Element& node, const Rect& rect);
std::optional<AdjacentLayout> adjacentLayout(const ElementVisit& parentState, const OrderedChildRef& first, const OrderedChildRef& second,
                                             const ComputedStyle& parentStyle);
std::vector<std::pair<std::size_t, std::size_t>> flexLines(Element& parent, const std::vector<ChildLayout>& children,
                                                           const ComputedStyle& parentStyle, FlexDirection flexDirection, float availableMain);
struct ContentDistributionResult {
    float offset = 0.f;
    float extraGap = 0.f;
    bool stretch = false;
};
ContentDistributionResult justifyContentDistribution(ContentPosition alignment, ContentDistribution distribution, OverflowAlignment safety,
                                                     LayoutDirection direction, FlexDirection flexDirection, float freeSpace, std::size_t itemCount,
                                                     bool stretchNormal = false);
ContentDistributionResult alignContentDistribution(ContentPosition alignment, ContentDistribution distribution, OverflowAlignment safety,
                                                   std::vector<float>& lineSizes, float availableCross, float baseGap);
std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth);
MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end,
                                    const ComputedStyle& parentStyle, FlexDirection flexDirection, float availableMain);
void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain);
} // namespace radia::ui::layout_detail
