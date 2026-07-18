#ifndef LL_RDUI_FLOATER_RESIZE_H
#define LL_RDUI_FLOATER_RESIZE_H

#include "rduistyle.h"
#include "rduitypes.h"
#include <cstdint>
#include <optional>

namespace rdui::detail
{
    inline constexpr float FLOATER_RESIZE_BORDER = 6.f;
    inline constexpr float FLOATER_RESIZE_CORNER_SPAN = 9.f;

    enum class ResizeEdges : std::uint8_t
    {
        None = 0,
        Left = 1,
        Right = 2,
        Bottom = 4,
        Top = 8,
    };

    constexpr ResizeEdges operator|(ResizeEdges lhs, ResizeEdges rhs)
    {
        return static_cast<ResizeEdges>(static_cast<std::uint8_t>(lhs)
                                      | static_cast<std::uint8_t>(rhs));
    }

    constexpr bool hasResizeEdge(ResizeEdges edges, ResizeEdges edge)
    {
        return (static_cast<std::uint8_t>(edges) & static_cast<std::uint8_t>(edge)) != 0;
    }

    struct FloaterResizeConstraints
    {
        Vec2 minimum;
        std::optional<Rect> bounds;
    };

    ResizeEdges resizeEdgesAt(const Rect& bounds, const Vec2& point);
    CursorStyle resizeCursor(ResizeEdges edges);
    Rect resizedRect(const Rect& initial, const Vec2& initial_pointer,
                     const Vec2& pointer, ResizeEdges edges,
                     const FloaterResizeConstraints& constraints);
}

#endif // LL_RDUI_FLOATER_RESIZE_H
