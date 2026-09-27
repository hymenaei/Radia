/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include "platform/graphics/Color.h"

namespace radia::ui {
enum class LayoutDirection { LeftToRight, RightToLeft };

enum class Visibility : std::uint8_t { Visible, Hidden, Collapse };

enum class ScrollbarMode : std::uint8_t { Classic, Overlay };

enum class ScrollbarAxis : std::uint8_t { NoneValue, Horizontal, Vertical };
enum class ScrollbarPart : std::uint8_t { NoneValue, Track, Thumb, StartArrow, EndArrow, Corner };

struct Vec2 {
    float x = 0.f;
    float y = 0.f;

    Vec2() = default;
    Vec2(float px, float py) : x(px), y(py) {}

    Vec2 operator+(const Vec2& rhs) const { return Vec2(x + rhs.x, y + rhs.y); }
    Vec2 operator-(const Vec2& rhs) const { return Vec2(x - rhs.x, y - rhs.y); }
    Vec2 operator*(float s) const { return Vec2(x * s, y * s); }

    friend constexpr bool operator==(const Vec2&, const Vec2&) = default;
};

struct NativeScrollbarMetrics {
    float thickness = 0.f;
    float minimumThumbLength = 0.f;
    float arrowLength = 0.f;
    float thumbPadding = 0.f;

    friend constexpr bool operator==(const NativeScrollbarMetrics&, const NativeScrollbarMetrics&) = default;
};

enum class NativeInputControl : std::uint8_t { Checkbox, Radio, Switch };

struct NativeInputMetrics {
    Vec2 intrinsicSize;

    friend constexpr bool operator==(const NativeInputMetrics& left, const NativeInputMetrics& right) {
        return left.intrinsicSize.x == right.intrinsicSize.x && left.intrinsicSize.y == right.intrinsicSize.y;
    }
};

struct NativeLayoutMetrics {
    NativeScrollbarMetrics classicScrollbar;
    NativeScrollbarMetrics overlayScrollbar;
    NativeInputMetrics checkbox;
    NativeInputMetrics radio;
    NativeInputMetrics switchControl;
    std::uint64_t revision = 1;

    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const { return mode == ScrollbarMode::Classic ? classicScrollbar : overlayScrollbar; }

    NativeInputMetrics inputMetrics(NativeInputControl control) const {
        switch (control) {
            case NativeInputControl::Checkbox: return checkbox;
            case NativeInputControl::Radio: return radio;
            case NativeInputControl::Switch: return switchControl;
        }
        return {};
    }

    friend constexpr bool operator==(const NativeLayoutMetrics&, const NativeLayoutMetrics&) = default;
};

NativeLayoutMetrics defaultNativeLayoutMetrics() noexcept;

struct ScrollLayoutOptions {
    ScrollbarMode scrollbarMode = ScrollbarMode::Classic;
    NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics();
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

template<typename T> struct RectEdges {
    T top{};
    T right{};
    T bottom{};
    T left{};

    auto horizontal() const { return left + right; }
    auto vertical() const { return top + bottom; }
    bool isUniform() const { return top == right && right == bottom && bottom == left; }
    bool any() const { return top != T{} || right != T{} || bottom != T{} || left != T{}; }

    friend bool operator==(const RectEdges&, const RectEdges&) = default;
};

inline Rect insetRect(const Rect& rect, const RectEdges<float>& insets) {
    return {
        rect.x + insets.left,
        rect.y + insets.bottom,
        std::max(0.f, rect.w - insets.horizontal()),
        std::max(0.f, rect.h - insets.vertical()),
    };
}

enum class StrokeCap { Butt, Round, Square };

} // namespace radia::ui
