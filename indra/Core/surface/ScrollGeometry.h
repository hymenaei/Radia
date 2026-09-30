/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include "LayoutGeometry.h"
#include "NativeAppearance.h"

namespace Core {
struct ScrollbarAxisInput {
    float scrollOffset = 0.f;
    float scrollExtent = 0.f;
    float viewportExtent = 0.f;
    bool visible = false;
};

struct ScrollGeometryInput {
    Layout::Rect scrollport;
    ScrollbarAxisInput horizontal;
    ScrollbarAxisInput vertical;
    ScrollbarMode mode = ScrollbarMode::Classic;
    Layout::Direction direction = Layout::Direction::LeftToRight;
    float thickness;
    float arrowLength;
    float minimumThumbLength;
    float thumbPadding;
};

struct ScrollbarAxisGeometry {
    ScrollbarAxis axis = ScrollbarAxis::NoneValue;
    Layout::Rect bounds;
    Layout::Rect track;
    Layout::Rect thumb;
    Layout::Rect startArrow;
    Layout::Rect endArrow;
    float maxScrollOffset = 0.f;
    float thumbTravel = 0.f;
    float thumbButtonGap = 0.f;
    bool visible = false;
    bool reversed = false;
};

struct ScrollGeometry {
    ScrollbarAxisGeometry horizontal;
    ScrollbarAxisGeometry vertical;
    Layout::Rect corner;
    bool hasCorner = false;
};

struct ScrollbarHit {
    ScrollbarAxis axis = ScrollbarAxis::NoneValue;
    ScrollbarPart part = ScrollbarPart::NoneValue;

    bool valid() const { return axis != ScrollbarAxis::NoneValue && part != ScrollbarPart::NoneValue; }
};

ScrollGeometry makeScrollGeometry(const ScrollGeometryInput& input);
ScrollbarHit hitTestScrollbar(const ScrollGeometry& geometry, const Layout::Vec2& point);
float scrollbarAxisPosition(ScrollbarAxis axis, const Layout::Vec2& point);
float scrollOffsetForThumbPosition(const ScrollbarAxisGeometry& geometry, float pointerPosition, float grabOffset);
Layout::Vec2 scrollContentTranslation(Layout::Direction direction, const Layout::Vec2& scrollOffset);
} // namespace Core
