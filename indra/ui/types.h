/**
 * @file types.h
 * @brief Defines shared UI geometry, color, inset, and widget-state types.
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

#ifndef RD_TYPES_H
#define RD_TYPES_H

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

namespace radia::ui {
enum class LayoutDirection { LeftToRight, RightToLeft };

enum class Visibility : std::uint8_t { Visible, Hidden, Collapsed };

struct Vec2 {
    float x = 0.f;
    float y = 0.f;

    Vec2() = default;
    Vec2(float px, float py) : x(px), y(py) {}

    Vec2 operator+(const Vec2& rhs) const { return Vec2(x + rhs.x, y + rhs.y); }
    Vec2 operator-(const Vec2& rhs) const { return Vec2(x - rhs.x, y - rhs.y); }
    Vec2 operator*(float s) const { return Vec2(x * s, y * s); }
};

inline float dot(const Vec2& a, const Vec2& b) {
    return a.x * b.x + a.y * b.y;
}

inline float length(const Vec2& v) {
    return std::sqrt(dot(v, v));
}

inline Vec2 normalize(const Vec2& v) {
    const float len = length(v);
    return len > 0.00001f ? Vec2(v.x / len, v.y / len) : Vec2();
}

struct Rect {
    float x = 0.f;
    float y = 0.f;
    float w = 0.f;
    float h = 0.f;

    Rect() = default;
    Rect(float px, float py, float pw, float ph) : x(px), y(py), w(pw), h(ph) {}

    float left() const { return x; }
    float right() const { return x + w; }
    float bottom() const { return y; }
    float top() const { return y + h; }

    bool empty() const { return w <= 0.f || h <= 0.f; }

    bool contains(const Vec2& p) const { return p.x >= left() && p.x <= right() && p.y >= bottom() && p.y <= top(); }
};

inline Rect intersectRects(const Rect& lhs, const Rect& rhs) {
    const float left = std::max(lhs.left(), rhs.left());
    const float right = std::min(lhs.right(), rhs.right());
    const float bottom = std::max(lhs.bottom(), rhs.bottom());
    const float top = std::min(lhs.top(), rhs.top());
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

enum class ClipAxes : uint8_t { NoAxes = 0, X = 1 << 0, Y = 1 << 1, Both = (1 << 0) | (1 << 1) };

inline ClipAxes operator|(ClipAxes lhs, ClipAxes rhs) {
    return static_cast<ClipAxes>(static_cast<uint8_t>(lhs) | static_cast<uint8_t>(rhs));
}

inline bool clipsAxis(ClipAxes axes, ClipAxes axis) {
    return (static_cast<uint8_t>(axes) & static_cast<uint8_t>(axis)) != 0;
}

inline Rect clipToAxes(const Rect& inherited, const Rect& bounds, ClipAxes axes) {
    const Rect axisBounds{
        clipsAxis(axes, ClipAxes::X) ? bounds.x : inherited.x,
        clipsAxis(axes, ClipAxes::Y) ? bounds.y : inherited.y,
        clipsAxis(axes, ClipAxes::X) ? bounds.w : inherited.w,
        clipsAxis(axes, ClipAxes::Y) ? bounds.h : inherited.h,
    };
    return intersectRects(inherited, axisBounds);
}

struct Color {
    float r = 1.f;
    float g = 1.f;
    float b = 1.f;
    float a = 1.f;

    Color() = default;
    Color(float pr, float pg, float pb, float pa = 1.f) : r(pr), g(pg), b(pb), a(pa) {}

    Color withAlpha(float alpha) const { return Color(r, g, b, alpha); }
};

struct EdgeInsets {
    float top = 0.f;
    float right = 0.f;
    float bottom = 0.f;
    float left = 0.f;

    bool isUniform() const { return top == right && right == bottom && bottom == left; }

    float horizontal() const { return left + right; }
    float vertical() const { return top + bottom; }

    float maxValue() const { return std::max(std::max(top, right), std::max(bottom, left)); }

    bool any() const { return maxValue() > 0.f; }
};

inline Rect insetRect(const Rect& rect, const EdgeInsets& insets) {
    return {
        rect.x + insets.left,
        rect.y + insets.bottom,
        std::max(0.f, rect.w - insets.horizontal()),
        std::max(0.f, rect.h - insets.vertical()),
    };
}

enum class StrokeCap { Butt, Round, Square };

enum class WidgetState : uint8_t {
    Default = 0,
    Hovered = 1 << 0,
    Active = 1 << 1,
    Focused = 1 << 2,
    Disabled = 1 << 3,
    Checked = 1 << 4,
    FocusVisible = 1 << 5,
    Minimized = 1 << 6,
    Invalid = 1 << 7
};

inline uint8_t operator|(WidgetState lhs, WidgetState rhs) {
    return static_cast<uint8_t>(lhs) | static_cast<uint8_t>(rhs);
}

inline bool hasState(uint8_t states, WidgetState state) {
    return (states & static_cast<uint8_t>(state)) != 0;
}

inline void setState(uint8_t& states, WidgetState state, bool enabled) {
    const uint8_t bit = static_cast<uint8_t>(state);
    states = enabled ? static_cast<uint8_t>(states | bit) : static_cast<uint8_t>(states & ~bit);
}
} // namespace radia::ui
#endif // RD_TYPES_H
