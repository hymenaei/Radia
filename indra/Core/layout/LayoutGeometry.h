/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>

namespace Core::Style {
struct ComputedStyle;
} // namespace Core::Style

namespace Core::Layout {
enum class Direction {
    LeftToRight,
    RightToLeft
};

struct Vec2 {
    float x = 0.f;
    float y = 0.f;

    Vec2() = default;
    Vec2(float px, float py)
        : x(px)
        , y(py) {}

    Vec2 operator+(const Vec2& rhs) const { return {x + rhs.x, y + rhs.y}; }
    Vec2 operator-(const Vec2& rhs) const { return {x - rhs.x, y - rhs.y}; }
    Vec2 operator*(float scale) const { return {x * scale, y * scale}; }

    friend constexpr bool operator==(const Vec2&, const Vec2&) = default;
};

inline float dot(const Vec2& left, const Vec2& right) { return left.x * right.x + left.y * right.y; }

inline float length(const Vec2& value) { return std::sqrt(dot(value, value)); }

inline Vec2 normalize(const Vec2& value) {
    const float magnitude = length(value);
    return magnitude > 0.00001f ? Vec2 {value.x / magnitude, value.y / magnitude} : Vec2 {};
}

struct Rect {
    float x = 0.f;
    float y = 0.f;
    float w = 0.f;
    float h = 0.f;

    Rect() = default;
    Rect(float px, float py, float width, float height)
        : x(px)
        , y(py)
        , w(width)
        , h(height) {}

    float left() const { return x; }
    float right() const { return x + w; }
    float bottom() const { return y; }
    float top() const { return y + h; }

    bool empty() const { return w <= 0.f || h <= 0.f; }

    bool contains(const Vec2& point) const { return point.x >= left() && point.x <= right() && point.y >= bottom() && point.y <= top(); }
};

inline Rect intersectRects(const Rect& left, const Rect& right) {
    const float leftEdge = std::max(left.left(), right.left());
    const float rightEdge = std::min(left.right(), right.right());
    const float bottomEdge = std::max(left.bottom(), right.bottom());
    const float topEdge = std::min(left.top(), right.top());
    return {leftEdge, bottomEdge, std::max(0.f, rightEdge - leftEdge), std::max(0.f, topEdge - bottomEdge)};
}

enum class ClipAxes : std::uint8_t {
    NoAxes = 0,
    X = 1 << 0,
    Y = 1 << 1,
    Both = (1 << 0) | (1 << 1)
};

inline ClipAxes operator|(ClipAxes left, ClipAxes right) {
    return static_cast<ClipAxes>(static_cast<std::uint8_t>(left) | static_cast<std::uint8_t>(right));
}

inline bool clipsAxis(ClipAxes axes, ClipAxes axis) { return (static_cast<std::uint8_t>(axes) & static_cast<std::uint8_t>(axis)) != 0; }

inline Rect clipToAxes(const Rect& inherited, const Rect& bounds, ClipAxes axes) {
    const Rect axisBounds {
        clipsAxis(axes, ClipAxes::X) ? bounds.x : inherited.x,
        clipsAxis(axes, ClipAxes::Y) ? bounds.y : inherited.y,
        clipsAxis(axes, ClipAxes::X) ? bounds.w : inherited.w,
        clipsAxis(axes, ClipAxes::Y) ? bounds.h : inherited.h,
    };
    return intersectRects(inherited, axisBounds);
}

template<typename T> struct RectEdges {
    T top {};
    T right {};
    T bottom {};
    T left {};

    auto horizontal() const { return left + right; }
    auto vertical() const { return top + bottom; }
    bool isUniform() const { return top == right && right == bottom && bottom == left; }
    bool any() const { return top != T {} || right != T {} || bottom != T {} || left != T {}; }

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

RectEdges<float> paddingPixels(const Style::ComputedStyle& style);
RectEdges<float> borderWidths(const Style::ComputedStyle& style);

template<typename MarginEdges> float horizontalMargin(const MarginEdges& margin) {
    return margin.left.fixedPixels() + margin.right.fixedPixels();
}

template<typename MarginEdges> float verticalMargin(const MarginEdges& margin) {
    return margin.top.fixedPixels() + margin.bottom.fixedPixels();
}

template<typename MarginEdges> int horizontalAutoMarginCount(const MarginEdges& margin) {
    return static_cast<int>(margin.left.isAuto()) + static_cast<int>(margin.right.isAuto());
}

template<typename MarginEdges> int verticalAutoMarginCount(const MarginEdges& margin) {
    return static_cast<int>(margin.top.isAuto()) + static_cast<int>(margin.bottom.isAuto());
}
} // namespace Core::Layout
