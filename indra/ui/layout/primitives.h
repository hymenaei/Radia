/**
 * @file primitives.h
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

#ifndef RD_LAYOUT_PRIMITIVES_H
#define RD_LAYOUT_PRIMITIVES_H

#include <cstddef>
#include <optional>
#include <vector>
#include "style/style.h"
#include "widgets/widget.h"

namespace radia::ui::layout_detail {
class WidgetLayoutAccess {
public:
    static bool hasGap(const Widget& parent, const Widget& first, const Widget& second) { return parent.hasLayoutGapBetween(first, second); }
    static float overlap(const Widget& parent, const Widget& first, const Widget& second, const Style& style) {
        return parent.layoutOverlapBetween(first, second, style);
    }
};

struct ChildLayout {
    WidgetRef<Widget> node;
    const Style& style;
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
void warnIgnoredPosition(const Widget& child, const Style& style, FlexDirection flexDirection);
const Style& emptyChildStyle();
ChildLayout invalidChildLayout();
void removeChildrenExcludedFromLayout(Widget& parent, std::vector<ChildLayout>& children);
float& mainSize(ChildLayout& child, FlexDirection flexDirection);
float mainSize(const ChildLayout& child, FlexDirection flexDirection);
float mainMinimum(const ChildLayout& child, FlexDirection flexDirection, float availableMain, float flexBase);
void applyFlexBasis(ChildLayout& child, FlexDirection flexDirection, float availableMain);
void distributeFlexSpace(std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, FlexDirection flexDirection, float availableMain, bool allowGrowth,
                         float& total);
float verticalAlignmentOffset(VerticalAlign alignment, float freeSpace);
CrossAlignment crossAlignment(const Style& parent, const Style& child, FlexDirection flexDirection);
void applyCrossAxisSizing(Vec2& size, const Style& style, FlexDirection flexDirection, float availableCross, CrossAlignment alignment);
float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float freeSpace);
Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment);
void setArrangedRect(Widget& node, const Rect& rect);
std::optional<AdjacentLayout> adjacentLayout(const WidgetVisit& parentState, Widget* first, Widget* second, const Style& parentStyle);
std::vector<std::pair<std::size_t, std::size_t>> rowLines(const std::vector<ChildLayout>& children);
std::vector<NormalLine> normalLines(const std::vector<ChildLayout>& children, std::optional<float> availableWidth);
MainAxisAllocation allocateMainAxis(Widget& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, const Style& parentStyle,
                                    FlexDirection flexDirection, float availableMain);
void prepareMainAxis(std::vector<ChildLayout>& children, FlexDirection flexDirection, float availableMain);
} // namespace radia::ui::layout_detail
#endif // RD_LAYOUT_PRIMITIVES_H
