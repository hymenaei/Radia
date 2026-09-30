/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstddef>
#include <optional>
#include "ComputedStyle.h"
#include "LayoutGeometry.h"

namespace Core::Layout {
enum class BorderImageGridPiece : std::size_t {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight
};

struct BorderImagePatch {
    Rect source;
    Rect destination;
    Vec2 tileSize;
    Style::BorderImageRepeatMode repeatX = Style::BorderImageRepeatMode::Stretch;
    Style::BorderImageRepeatMode repeatY = Style::BorderImageRepeatMode::Stretch;
};

struct BorderImageGrid {
    std::array<BorderImagePatch, 9> pieces;
};

struct BorderImageTilePlan {
    float first = 0.f;
    float size = 0.f;
    float gap = 0.f;
    std::size_t count = 0;

    float position(std::size_t index) const { return first + static_cast<float>(index) * (size + gap); }
};

std::optional<BorderImageTilePlan> borderImageTilePlan(Style::BorderImageRepeatMode mode, float start, float extent, float preferredSize);
std::optional<Rect> resolveBorderImageArea(const Rect& borderBox, const Style::BorderImageOutset& outset,
    const RectEdges<float>& computedBorderWidths);
std::optional<BorderImageGrid> resolveBorderImageGrid(const Style::BorderImageSlice& slice, const Style::BorderImageWidth& width,
    const Style::BorderImageOutset& outset, const Style::BorderImageRepeat& repeat, const Rect& borderBox, float imageWidth,
    float imageHeight, const RectEdges<float>& computedBorderWidths);
} // namespace Core::Layout
