/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstddef>
#include <optional>
#include "style/computedstyle.h"
#include "types.h"

namespace radia::ui {
enum class BorderImageGridPiece : std::size_t { TopLeft, Top, TopRight, Left, Center, Right, BottomLeft, Bottom, BottomRight };

struct BorderImagePatch {
    Rect source;
    Rect destination;
    Vec2 tileSize;
    BorderImageRepeatMode repeatX = BorderImageRepeatMode::Stretch;
    BorderImageRepeatMode repeatY = BorderImageRepeatMode::Stretch;
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

std::optional<BorderImageTilePlan> borderImageTilePlan(BorderImageRepeatMode mode, float start, float extent, float preferredSize);
std::optional<Rect> resolveBorderImageArea(const Rect& borderBox, const BorderImageOutset& outset, const RectEdges<float>& computedBorderWidths);
std::optional<BorderImageGrid> resolveBorderImageGrid(const BorderImageSlice& slice, const BorderImageWidth& width, const BorderImageOutset& outset,
                                                      const BorderImageRepeat& repeat, const Rect& borderBox, float imageWidth, float imageHeight,
                                                      const RectEdges<float>& computedBorderWidths);
} // namespace radia::ui
