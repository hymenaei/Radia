/**
 * @file floaterresize.cpp
 * @brief Provides floater resize-edge detection, cursor mapping, and constrained geometry.
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
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "surface/floaterresize.h"
#include <algorithm>
#include <cmath>
#include <limits>

namespace radia::ui::detail {
ResizeEdges resizeEdgesAt(const Rect& bounds, const Vec2& point) {
    if (!bounds.contains(point)) return ResizeEdges::NoEdges;

    const bool corner_left = point.x <= bounds.left() + FLOATER_RESIZE_CORNER_SPAN;
    const bool corner_right = point.x >= bounds.right() - FLOATER_RESIZE_CORNER_SPAN;
    const bool corner_bottom = point.y <= bounds.bottom() + FLOATER_RESIZE_CORNER_SPAN;
    const bool corner_top = point.y >= bounds.top() - FLOATER_RESIZE_CORNER_SPAN;

    if (corner_left && corner_bottom) return ResizeEdges::Left | ResizeEdges::Bottom;
    if (corner_left && corner_top) return ResizeEdges::Left | ResizeEdges::Top;
    if (corner_right && corner_bottom) return ResizeEdges::Right | ResizeEdges::Bottom;
    if (corner_right && corner_top) return ResizeEdges::Right | ResizeEdges::Top;

    if (point.x <= bounds.left() + FLOATER_RESIZE_BORDER) return ResizeEdges::Left;
    if (point.x >= bounds.right() - FLOATER_RESIZE_BORDER) return ResizeEdges::Right;
    if (point.y <= bounds.bottom() + FLOATER_RESIZE_BORDER) return ResizeEdges::Bottom;
    if (point.y >= bounds.top() - FLOATER_RESIZE_BORDER) return ResizeEdges::Top;
    return ResizeEdges::NoEdges;
}

CursorStyle resizeCursor(ResizeEdges edges) {
    const bool left = hasResizeEdge(edges, ResizeEdges::Left);
    const bool right = hasResizeEdge(edges, ResizeEdges::Right);
    const bool bottom = hasResizeEdge(edges, ResizeEdges::Bottom);
    const bool top = hasResizeEdge(edges, ResizeEdges::Top);
    if ((left && bottom) || (right && top)) return CursorStyle::NortheastSouthwestResize;
    if ((left && top) || (right && bottom)) return CursorStyle::NorthwestSoutheastResize;
    if (left || right) return CursorStyle::EastWestResize;
    if (bottom || top) return CursorStyle::NorthSouthResize;
    return CursorStyle::Auto;
}

bool preserveUserResizeOnReload(bool current_resizable, bool replacement_resizable, const FloaterAuthoredGeometry& current,
                                const FloaterAuthoredGeometry& replacement) {
    constexpr float SIZE_EPSILON = .5f;
    const auto unchanged = [=](const Vec2& left, const Vec2& right) {
        return std::abs(left.x - right.x) < SIZE_EPSILON && std::abs(left.y - right.y) < SIZE_EPSILON;
    };
    return current_resizable
        && replacement_resizable
        && unchanged(current.outer, replacement.outer)
        && unchanged(current.content, replacement.content);
}

Rect resizedRect(const Rect& initial, const Vec2& initial_pointer, const Vec2& pointer, ResizeEdges edges,
                 const FloaterResizeConstraints& constraints) {
    const Vec2 delta = pointer - initial_pointer;
    const float minimum_width =
        constraints.bounds ? std::min(std::max(0.f, constraints.minimum.x), constraints.bounds->w) : std::max(0.f, constraints.minimum.x);
    const float minimum_height =
        constraints.bounds ? std::min(std::max(0.f, constraints.minimum.y), constraints.bounds->h) : std::max(0.f, constraints.minimum.y);

    float left = initial.left();
    float right = initial.right();
    float bottom = initial.bottom();
    float top = initial.top();

    if (hasResizeEdge(edges, ResizeEdges::Left)) {
        const float lower = constraints.bounds ? constraints.bounds->left() : -std::numeric_limits<float>::max();
        const float upper = std::max(lower, initial.right() - minimum_width);
        left = std::clamp(initial.left() + delta.x, lower, upper);
    } else if (hasResizeEdge(edges, ResizeEdges::Right)) {
        const float upper = constraints.bounds ? constraints.bounds->right() : std::numeric_limits<float>::max();
        const float lower = std::min(upper, initial.left() + minimum_width);
        right = std::clamp(initial.right() + delta.x, lower, upper);
    }

    if (hasResizeEdge(edges, ResizeEdges::Bottom)) {
        const float lower = constraints.bounds ? constraints.bounds->bottom() : -std::numeric_limits<float>::max();
        const float upper = std::max(lower, initial.top() - minimum_height);
        bottom = std::clamp(initial.bottom() + delta.y, lower, upper);
    } else if (hasResizeEdge(edges, ResizeEdges::Top)) {
        const float upper = constraints.bounds ? constraints.bounds->top() : std::numeric_limits<float>::max();
        const float lower = std::min(upper, initial.bottom() + minimum_height);
        top = std::clamp(initial.top() + delta.y, lower, upper);
    }

    return {left, bottom, right - left, top - bottom};
}
} // namespace radia::ui::detail
