#include "linden_common.h"
#include "rduifloaterresize.h"
#include <algorithm>
#include <limits>

namespace rdui::detail
{
    ResizeEdges resizeEdgesAt(const Rect& bounds, const Vec2& point)
    {
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

    CursorStyle resizeCursor(ResizeEdges edges)
    {
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

    Rect resizedRect(const Rect& initial, const Vec2& initial_pointer,
                     const Vec2& pointer, ResizeEdges edges,
                     const FloaterResizeConstraints& constraints)
    {
        const Vec2 delta = pointer - initial_pointer;
        const float minimum_width = constraints.bounds
            ? std::min(std::max(0.f, constraints.minimum.x), constraints.bounds->w)
            : std::max(0.f, constraints.minimum.x);
        const float minimum_height = constraints.bounds
            ? std::min(std::max(0.f, constraints.minimum.y), constraints.bounds->h)
            : std::max(0.f, constraints.minimum.y);

        float left = initial.left();
        float right = initial.right();
        float bottom = initial.bottom();
        float top = initial.top();

        if (hasResizeEdge(edges, ResizeEdges::Left))
        {
            const float lower = constraints.bounds ? constraints.bounds->left() : -std::numeric_limits<float>::max();
            const float upper = std::max(lower, initial.right() - minimum_width);
            left = std::clamp(initial.left() + delta.x, lower, upper);
        }
        else if (hasResizeEdge(edges, ResizeEdges::Right))
        {
            const float upper = constraints.bounds ? constraints.bounds->right() : std::numeric_limits<float>::max();
            const float lower = std::min(upper, initial.left() + minimum_width);
            right = std::clamp(initial.right() + delta.x, lower, upper);
        }

        if (hasResizeEdge(edges, ResizeEdges::Bottom))
        {
            const float lower = constraints.bounds ? constraints.bounds->bottom() : -std::numeric_limits<float>::max();
            const float upper = std::max(lower, initial.top() - minimum_height);
            bottom = std::clamp(initial.bottom() + delta.y, lower, upper);
        }
        else if (hasResizeEdge(edges, ResizeEdges::Top))
        {
            const float upper = constraints.bounds ? constraints.bounds->top() : std::numeric_limits<float>::max();
            const float lower = std::min(upper, initial.bottom() + minimum_height);
            top = std::clamp(initial.top() + delta.y, lower, upper);
        }

        return {left, bottom, right - left, top - bottom};
    }
}
