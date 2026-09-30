/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <optional>
#include <utility>
#include <vector>
#include "ComputedStyle.h"
#include "Element.h"
#include "ElementInternal.h"
#include "OrderedChild.h"

namespace Core::Layout::detail {
class ElementLayoutAccess {
public:
    static bool hasGap(const Element& parent, const Element& first, const Element& second) {
        return parent.hasLayoutGapBetween(first, second);
    }
    static float overlap(const Element& parent, const Element& first, const Element& second, const Style::ComputedStyle& style) {
        return parent.layoutOverlapBetween(first, second, style);
    }
};

struct ChildLayout {
    OrderedChildRef node;
    Style::ComputedStyle style;
    Vec2 fitSize;
    Vec2 measured;
    Vec2 flexBase;
    Vec2 minContent;
    bool flowBreakBefore = false;
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

float styledBoxDimension(const Style::ComputedStyle& style, bool horizontal, const Style::Dimension& value,
    const std::optional<Style::Dimension>& minimum, const std::optional<Style::Dimension>& maximum, float fallback, float reference = 0.f,
    float minContent = 0.f);
float clampBoxDimension(const Style::ComputedStyle& style, bool horizontal, float size, const std::optional<Style::Dimension>& minimum,
    const std::optional<Style::Dimension>& maximum, float reference, float minContent = 0.f);
float minimumBoxDimension(const Style::ComputedStyle& style, bool horizontal, const std::optional<Style::Dimension>& minimum,
    float reference, float fallback = 0.f, float minContent = 0.f);
float maximumBoxDimension(const Style::ComputedStyle& style, bool horizontal, const std::optional<Style::Dimension>& maximum,
    float reference, float fallback = 0.f, float minContent = 0.f);
float contentBoxDimension(const Style::ComputedStyle& style, bool horizontal, float borderBoxSize);
bool isInlineLevel(Style::Display display);
const Style::ComputedStyle& emptyChildStyle();
ChildLayout invalidChildLayout();
void removeChildrenExcludedFromLayout(Element& parent, std::vector<ChildLayout>& children);
bool isDisplayed(const ChildLayout& child);
bool isWhitespaceOnlyText(const Core::detail::NodeRef& node);
bool isWhitespaceOnlyText(const OrderedChildRef& node);
bool flowBreakBefore(const OrderedChildRef& child);
bool flowBreakBefore(const ChildLayout& child);
float& mainSize(ChildLayout& child, Style::FlexDirection flexDirection);
float mainSize(const ChildLayout& child, Style::FlexDirection flexDirection);
float mainMinimum(const ChildLayout& child, Style::FlexDirection flexDirection, float availableMain, float flexBase);
void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, Style::FlexDirection flexDirection,
    float availableMain, float& total);
float verticalAlignmentOffset(const Style::ComputedStyle& style, float freeSpace);
Style::ItemPosition crossAlignment(const Style::ComputedStyle& parent, const Style::ComputedStyle& child);
Style::OverflowAlignment crossAlignmentSafety(const Style::ComputedStyle& parent, const Style::ComputedStyle& child);
void applyCrossAxisSizing(Vec2& size, const Style::ComputedStyle& style, Style::FlexDirection flexDirection, float availableCross,
    Style::ItemPosition alignment);
float textAlignmentOffset(Style::TextAlign alignment, Direction direction, float freeSpace);
float justifySelfOffset(Style::ItemPosition alignment, Style::OverflowAlignment safety, Direction direction, float freeSpace);
float alignSelfOffset(Style::ItemPosition alignment, Style::OverflowAlignment safety, float freeSpace);
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
    const Style::ComputedStyle& parentStyle);
std::vector<std::pair<std::size_t, std::size_t>> flexLines(Element& parent, const std::vector<ChildLayout>& children,
    const Style::ComputedStyle& parentStyle, Style::FlexDirection flexDirection, float availableMain);
struct ContentDistributionResult {
    float offset = 0.f;
    float extraGap = 0.f;
    bool stretch = false;
};
ContentDistributionResult justifyContentDistribution(Style::ContentPosition alignment, Style::ContentDistribution distribution,
    Style::OverflowAlignment safety, Direction direction, Style::FlexDirection flexDirection, float freeSpace, std::size_t itemCount,
    bool stretchNormal = false);
ContentDistributionResult alignContentDistribution(Style::ContentPosition alignment, Style::ContentDistribution distribution,
    Style::OverflowAlignment safety, std::vector<float>& lineSizes, float availableCross, float baseGap);
std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth);
MainAxisAllocation allocateMainAxis(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end,
    const Style::ComputedStyle& parentStyle, Style::FlexDirection flexDirection, float availableMain);
void prepareMainAxis(std::vector<ChildLayout>& children, Style::FlexDirection flexDirection, float availableMain);
} // namespace Core::Layout::detail
