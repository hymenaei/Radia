#ifndef LL_RDUI_TYPES_H
#define LL_RDUI_TYPES_H

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

namespace rdui
{
    enum class LayoutDirection { LeftToRight, RightToLeft };

    struct Vec2
    {
        float x = 0.f;
        float y = 0.f;

        Vec2() = default;
        Vec2(float px, float py) : x(px), y(py) {}

        Vec2 operator+(const Vec2& rhs) const { return Vec2(x + rhs.x, y + rhs.y); }
        Vec2 operator-(const Vec2& rhs) const { return Vec2(x - rhs.x, y - rhs.y); }
        Vec2 operator*(float s) const { return Vec2(x * s, y * s); }
    };

    inline float dot(const Vec2& a, const Vec2& b)
    {
        return a.x * b.x + a.y * b.y;
    }

    inline float length(const Vec2& v)
    {
        return std::sqrt(dot(v, v));
    }

    inline Vec2 normalize(const Vec2& v)
    {
        const float len = length(v);
        return len > 0.00001f ? Vec2(v.x / len, v.y / len) : Vec2();
    }

    struct Rect
    {
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

        bool contains(const Vec2& p) const
        {
            return p.x >= left() && p.x <= right() && p.y >= bottom() && p.y <= top();
        }

    };

    struct Color
    {
        float r = 1.f;
        float g = 1.f;
        float b = 1.f;
        float a = 1.f;

        Color() = default;
        Color(float pr, float pg, float pb, float pa = 1.f) : r(pr), g(pg), b(pb), a(pa) {}

        Color withAlpha(float alpha) const { return Color(r, g, b, alpha); }
    };

    struct EdgeInsets
    {
        float top = 0.f;
        float right = 0.f;
        float bottom = 0.f;
        float left = 0.f;

        bool is_uniform() const
        {
            return top == right && right == bottom && bottom == left;
        }

        float horizontal() const { return left + right; }
        float vertical() const { return top + bottom; }

        float max_value() const
        {
            return std::max(std::max(top, right), std::max(bottom, left));
        }

        bool any() const { return max_value() > 0.f; }
    };

    enum class StrokeCap { Butt, Round, Square };

    enum class WidgetState : uint8_t
    {
        Default = 0,
        Hovered = 1 << 0,
        Active = 1 << 1,
        Focused = 1 << 2,
        Disabled = 1 << 3,
        Checked = 1 << 4,
        FocusVisible = 1 << 5,
        Minimized = 1 << 6,
    };

    inline uint8_t operator|(WidgetState lhs, WidgetState rhs)
    {
        return static_cast<uint8_t>(lhs) | static_cast<uint8_t>(rhs);
    }

    inline bool has_state(uint8_t states, WidgetState state)
    {
        return (states & static_cast<uint8_t>(state)) != 0;
    }

    inline void set_state(uint8_t& states, WidgetState state, bool enabled)
    {
        const uint8_t bit = static_cast<uint8_t>(state);
        states = enabled ? static_cast<uint8_t>(states | bit) : static_cast<uint8_t>(states & ~bit);
    }
}

#endif // LL_RDUI_TYPES_H
