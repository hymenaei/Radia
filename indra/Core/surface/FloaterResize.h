/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <optional>
#include "ComputedStyle.h"
#include "LayoutGeometry.h"

namespace Core::detail {
inline constexpr float kFloaterResizeBorder = 6.f;
inline constexpr float kFloaterResizeCornerSpan = 9.f;

enum class ResizeEdges : std::uint8_t {
    NoEdges = 0,
    Left = 1,
    Right = 2,
    Bottom = 4,
    Top = 8
};

constexpr ResizeEdges operator|(ResizeEdges lhs, ResizeEdges rhs) {
    return static_cast<ResizeEdges>(static_cast<std::uint8_t>(lhs) | static_cast<std::uint8_t>(rhs));
}

constexpr bool hasResizeEdge(ResizeEdges edges, ResizeEdges edge) {
    return (static_cast<std::uint8_t>(edges) & static_cast<std::uint8_t>(edge)) != 0;
}

struct FloaterResizeConstraints {
    Layout::Vec2 minimum;
    std::optional<Layout::Rect> bounds;
};

struct FloaterAuthoredGeometry {
    Layout::Vec2 outer;
    Layout::Vec2 content;
};

ResizeEdges resizeEdgesAt(const Layout::Rect& bounds, const Layout::Vec2& point);
Style::CursorStyle resizeCursor(ResizeEdges edges);
bool preserveUserResizeOnReload(bool currentResizable, bool replacementResizable, const FloaterAuthoredGeometry& current,
    const FloaterAuthoredGeometry& replacement);
Layout::Rect resizedRect(const Layout::Rect& initial, const Layout::Vec2& initialPointer, const Layout::Vec2& pointer, ResizeEdges edges,
    const FloaterResizeConstraints& constraints);
} // namespace Core::detail
