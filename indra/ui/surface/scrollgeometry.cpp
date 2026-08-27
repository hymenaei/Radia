/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "surface/scrollgeometry.h"
#include <algorithm>

namespace radia::ui {
namespace {
constexpr float kScrollGeometryEpsilon = 1.0e-6f;

float axisLength(const Rect& rect, ScrollbarAxis axis) {
    return axis == ScrollbarAxis::Horizontal ? rect.w : rect.h;
}

float axisStart(const Rect& rect, ScrollbarAxis axis) {
    return axis == ScrollbarAxis::Horizontal ? rect.left() : rect.bottom();
}

float axisEnd(const Rect& rect, ScrollbarAxis axis) {
    return axis == ScrollbarAxis::Horizontal ? rect.right() : rect.top();
}

Rect axisRect(const Rect& bounds, ScrollbarAxis axis, float start, float length) {
    if (axis == ScrollbarAxis::Horizontal) return {start, bounds.y, length, bounds.h};
    return {bounds.x, start, bounds.w, length};
}

ScrollbarAxisGeometry makeAxisGeometry(const Rect& bounds, const ScrollbarAxisInput& input, ScrollbarAxis axis, bool reversed, bool showArrows,
                                       float arrowLength, float minimumThumbLength, float thumbPadding) {
    ScrollbarAxisGeometry result;
    if (!input.visible || bounds.empty()) return result;

    result.axis = axis;
    result.visible = true;
    result.bounds = bounds;
    result.reversed = reversed;
    result.maxScrollOffset = std::max(0.f, input.scrollExtent - input.viewportExtent);

    const float length = axisLength(bounds, axis);
    const float clampedArrowLength = showArrows ? std::min(std::max(0.f, arrowLength), length * .5f) : 0.f;
    const float trackLength = std::max(0.f, length - clampedArrowLength * 2.f);
    const float extent = std::max(input.viewportExtent, input.scrollExtent);
    const float viewportFraction = extent > 0.f ? std::clamp(input.viewportExtent / extent, 0.f, 1.f) : 1.f;
    const float crossAxisLength = axis == ScrollbarAxis::Horizontal ? bounds.h : bounds.w;
    const float clampedThumbPadding = std::min(std::max(0.f, thumbPadding), crossAxisLength * .5f);
    const float clampedThumbButtonGap = showArrows ? std::min(std::max(0.f, thumbPadding), trackLength * .5f) : 0.f;
    const float availableThumbTrack = std::max(0.f, trackLength - clampedThumbButtonGap * 2.f);
    const float thumbLength = std::clamp(std::max(minimumThumbLength, trackLength * viewportFraction), 0.f, availableThumbTrack);
    const float travel = std::max(0.f, availableThumbTrack - thumbLength);
    const float normalizedOffset = result.maxScrollOffset > 0.f ? std::clamp(input.scrollOffset / result.maxScrollOffset, 0.f, 1.f) : 0.f;
    const float physicalFraction = reversed ? 1.f - normalizedOffset : normalizedOffset;
    const float physicalOffset = travel * physicalFraction;

    result.thumbTravel = travel;
    result.thumbButtonGap = clampedThumbButtonGap;
    if (axis == ScrollbarAxis::Horizontal) {
        result.track = {bounds.x + clampedArrowLength, bounds.y, trackLength, bounds.h};
        result.thumb = {result.track.x + clampedThumbButtonGap + physicalOffset, bounds.y + clampedThumbPadding, thumbLength,
                        std::max(0.f, bounds.h - clampedThumbPadding * 2.f)};
        if (showArrows) {
            if (reversed) {
                result.startArrow = {bounds.right() - clampedArrowLength, bounds.y, clampedArrowLength, bounds.h};
                result.endArrow = {bounds.x, bounds.y, clampedArrowLength, bounds.h};
            } else {
                result.startArrow = {bounds.x, bounds.y, clampedArrowLength, bounds.h};
                result.endArrow = {bounds.right() - clampedArrowLength, bounds.y, clampedArrowLength, bounds.h};
            }
        }
    } else {
        result.track = {bounds.x, bounds.y + clampedArrowLength, bounds.w, trackLength};
        result.thumb = {bounds.x + clampedThumbPadding, result.track.top() - clampedThumbButtonGap - physicalOffset - thumbLength,
                        std::max(0.f, bounds.w - clampedThumbPadding * 2.f), thumbLength};
        if (showArrows) {
            result.startArrow = {bounds.x, bounds.top() - clampedArrowLength, bounds.w, clampedArrowLength};
            result.endArrow = {bounds.x, bounds.y, bounds.w, clampedArrowLength};
        }
    }
    return result;
}

ScrollbarPart hitTestAxis(const ScrollbarAxisGeometry& geometry, const Vec2& point) {
    if (!geometry.visible || !geometry.bounds.contains(point)) return ScrollbarPart::NoneValue;
    if (!geometry.thumb.empty() && geometry.thumb.contains(point)) return ScrollbarPart::Thumb;
    if (!geometry.startArrow.empty() && geometry.startArrow.contains(point)) return ScrollbarPart::StartArrow;
    if (!geometry.endArrow.empty() && geometry.endArrow.contains(point)) return ScrollbarPart::EndArrow;
    return geometry.track.contains(point) ? ScrollbarPart::Track : ScrollbarPart::NoneValue;
}
} // namespace

ScrollGeometry makeScrollGeometry(const ScrollGeometryInput& input) {
    ScrollGeometry result;
    const float thickness = std::max(0.f, input.thickness);
    if (thickness <= 0.f || input.scrollport.empty()) return result;

    const bool classic = input.mode == ScrollbarMode::Classic;
    const bool showArrows = classic;
    const float arrowLength = std::max(0.f, input.arrowLength);

    if (input.vertical.visible) {
        const float barWidth = classic ? thickness : std::min(thickness, input.scrollport.w);
        const float x = classic ? (input.direction == LayoutDirection::RightToLeft ? input.scrollport.left() - barWidth : input.scrollport.right())
                                : (input.direction == LayoutDirection::RightToLeft ? input.scrollport.left() : input.scrollport.right() - barWidth);
        result.vertical = makeAxisGeometry({x, input.scrollport.y, barWidth, input.scrollport.h}, input.vertical, ScrollbarAxis::Vertical, false,
                                           showArrows, arrowLength, input.minimumThumbLength, input.thumbPadding);
    }
    if (input.horizontal.visible) {
        const float barHeight = classic ? thickness : std::min(thickness, input.scrollport.h);
        const float y = classic ? input.scrollport.bottom() - barHeight : input.scrollport.y;
        result.horizontal =
            makeAxisGeometry({input.scrollport.x, y, input.scrollport.w, barHeight}, input.horizontal, ScrollbarAxis::Horizontal,
                             input.direction == LayoutDirection::RightToLeft, showArrows, arrowLength, input.minimumThumbLength, input.thumbPadding);
    }

    if (result.horizontal.visible && result.vertical.visible) {
        if (classic) {
            const float verticalWidth = result.vertical.bounds.w;
            const float horizontalHeight = result.horizontal.bounds.h;
            const float x = input.direction == LayoutDirection::RightToLeft ? input.scrollport.left() - verticalWidth : input.scrollport.right();
            result.corner = {x, input.scrollport.bottom() - horizontalHeight, verticalWidth, horizontalHeight};
        } else {
            result.corner = intersectRects(result.horizontal.bounds, result.vertical.bounds);
        }
        result.hasCorner = !result.corner.empty();
    }
    return result;
}

ScrollbarHit hitTestScrollbar(const ScrollGeometry& geometry, const Vec2& point) {
    if (geometry.hasCorner && geometry.corner.contains(point)) return {ScrollbarAxis::NoneValue, ScrollbarPart::Corner};
    const ScrollbarPart vertical = hitTestAxis(geometry.vertical, point);
    if (vertical != ScrollbarPart::NoneValue) return {ScrollbarAxis::Vertical, vertical};
    const ScrollbarPart horizontal = hitTestAxis(geometry.horizontal, point);
    if (horizontal != ScrollbarPart::NoneValue) return {ScrollbarAxis::Horizontal, horizontal};
    return {};
}

float scrollbarAxisPosition(ScrollbarAxis axis, const Vec2& point) {
    return axis == ScrollbarAxis::Horizontal ? point.x : point.y;
}

float scrollOffsetForThumbPosition(const ScrollbarAxisGeometry& geometry, float pointerPosition, float grabOffset) {
    if (!geometry.visible || geometry.thumb.empty() || geometry.thumbTravel <= kScrollGeometryEpsilon || geometry.maxScrollOffset <= 0.f) return 0.f;
    const ScrollbarAxis axis = geometry.axis;
    if (axis == ScrollbarAxis::NoneValue) return 0.f;
    const float trackStart = axisStart(geometry.track, axis);
    const float trackEnd = axisEnd(geometry.track, axis);
    const float thumbLength = axisLength(geometry.thumb, axis);
    const float thumbStart = trackStart + geometry.thumbButtonGap;
    const float thumbEnd = trackEnd - geometry.thumbButtonGap - thumbLength;
    const float desiredStart = std::clamp(pointerPosition - grabOffset, thumbStart, thumbEnd);
    const float physicalFraction = std::clamp((desiredStart - thumbStart) / geometry.thumbTravel, 0.f, 1.f);
    const bool reversePhysicalAxis = axis == ScrollbarAxis::Vertical || geometry.reversed;
    const float logicalFraction = reversePhysicalAxis ? 1.f - physicalFraction : physicalFraction;
    return geometry.maxScrollOffset * logicalFraction;
}
} // namespace radia::ui
