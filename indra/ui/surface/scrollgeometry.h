/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include "types.h"

namespace radia::ui {
enum class ScrollbarAxis : std::uint8_t { NoneValue, Horizontal, Vertical };
enum class ScrollbarPart : std::uint8_t { NoneValue, Track, Thumb, StartArrow, EndArrow, Corner };

struct ScrollbarAxisInput {
    float scrollOffset = 0.f;
    float scrollExtent = 0.f;
    float viewportExtent = 0.f;
    bool visible = false;
};

struct ScrollGeometryInput {
    Rect scrollport;
    ScrollbarAxisInput horizontal;
    ScrollbarAxisInput vertical;
    ScrollbarMode mode = ScrollbarMode::Classic;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    float thickness;
    float arrowLength;
    float minimumThumbLength;
    float thumbPadding;
};

struct ScrollbarAxisGeometry {
    ScrollbarAxis axis = ScrollbarAxis::NoneValue;
    Rect bounds;
    Rect track;
    Rect thumb;
    Rect startArrow;
    Rect endArrow;
    float maxScrollOffset = 0.f;
    float thumbTravel = 0.f;
    float thumbButtonGap = 0.f;
    bool visible = false;
    bool reversed = false;
};

struct ScrollGeometry {
    ScrollbarAxisGeometry horizontal;
    ScrollbarAxisGeometry vertical;
    Rect corner;
    bool hasCorner = false;
};

struct ScrollbarHit {
    ScrollbarAxis axis = ScrollbarAxis::NoneValue;
    ScrollbarPart part = ScrollbarPart::NoneValue;

    bool valid() const { return axis != ScrollbarAxis::NoneValue && part != ScrollbarPart::NoneValue; }
};

ScrollGeometry makeScrollGeometry(const ScrollGeometryInput& input);
ScrollbarHit hitTestScrollbar(const ScrollGeometry& geometry, const Vec2& point);
float scrollbarAxisPosition(ScrollbarAxis axis, const Vec2& point);
float scrollOffsetForThumbPosition(const ScrollbarAxisGeometry& geometry, float pointerPosition, float grabOffset);
Vec2 scrollContentTranslation(LayoutDirection direction, const Vec2& scrollOffset);
} // namespace radia::ui
