/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "BorderImageGrid.h"
#include <algorithm>
#include <cmath>
#include <limits>

namespace Core::Layout {
namespace {
float slice(Style::BorderImageSliceValue value, float extent) {
    const float amount = value.percentage ? value.value * extent / 100.f : value.value;
    return std::clamp(amount, 0.f, extent);
}

float scale(float destination, float source) {
    const float result = destination / source;
    return std::isfinite(result) && result > 0.f ? result : 0.f;
}

RectEdges<float> resolveOutsets(const Style::BorderImageOutset& outsetValues, const RectEdges<float>& borderWidths) {
    const auto resolve = [](Style::BorderImageOutsetValue value, float borderWidth) {
        return value.multiplier ? value.value * borderWidth : value.value;
    };
    return {resolve(outsetValues.edges.top, borderWidths.top), resolve(outsetValues.edges.right, borderWidths.right),
        resolve(outsetValues.edges.bottom, borderWidths.bottom), resolve(outsetValues.edges.left, borderWidths.left)};
}
} // namespace

std::optional<BorderImageTilePlan> borderImageTilePlan(Style::BorderImageRepeatMode mode, float start, float extent, float preferredSize) {
    if (!std::isfinite(start) || !std::isfinite(extent) || !std::isfinite(preferredSize) || extent <= 0.f)
        return std::nullopt;
    if (mode == Style::BorderImageRepeatMode::Stretch)
        return BorderImageTilePlan {start, extent, 0.f, 1};
    if (preferredSize <= 0.f)
        return std::nullopt;

    const double quotient = static_cast<double>(extent) / preferredSize;
    if (!std::isfinite(quotient))
        return std::nullopt;
    if (mode == Style::BorderImageRepeatMode::Repeat) {
        if (quotient > static_cast<double>(std::numeric_limits<long long>::max() - 4))
            return std::nullopt;
        const float centered = start + (extent - preferredSize) * .5f;
        const auto firstIndex = static_cast<long long>(std::floor((start - centered) / preferredSize));
        const auto endIndex = static_cast<long long>(std::ceil((start + extent - centered) / preferredSize));
        if (endIndex <= firstIndex)
            return std::nullopt;
        return BorderImageTilePlan {centered + static_cast<float>(firstIndex) * preferredSize, preferredSize, 0.f,
            static_cast<std::size_t>(endIndex - firstIndex)};
    }

    const double tileCount = mode == Style::BorderImageRepeatMode::Round ? std::max(1.0, std::round(quotient)) : std::floor(quotient);
    if (tileCount < 1.0 || tileCount >= static_cast<double>(std::numeric_limits<std::size_t>::max()))
        return std::nullopt;

    const auto count = static_cast<std::size_t>(tileCount);
    if (mode == Style::BorderImageRepeatMode::Round)
        return BorderImageTilePlan {start, extent / static_cast<float>(count), 0.f, count};

    const float gap = (extent - static_cast<float>(count) * preferredSize) / static_cast<float>(count + 1);
    return BorderImageTilePlan {start + gap, preferredSize, gap, count};
}

std::optional<Rect> resolveBorderImageArea(const Rect& borderBox, const Style::BorderImageOutset& outsetValues,
    const RectEdges<float>& computedBorderWidths) {
    if (borderBox.empty() || !std::isfinite(borderBox.x) || !std::isfinite(borderBox.y) || !std::isfinite(borderBox.w)
        || !std::isfinite(borderBox.h))
        return std::nullopt;

    const RectEdges<float> outsets = resolveOutsets(outsetValues, computedBorderWidths);
    if (!std::isfinite(outsets.top) || !std::isfinite(outsets.right) || !std::isfinite(outsets.bottom) || !std::isfinite(outsets.left))
        return std::nullopt;

    const Rect area {borderBox.x - outsets.left, borderBox.y - outsets.bottom, borderBox.w + outsets.left + outsets.right,
        borderBox.h + outsets.top + outsets.bottom};
    if (area.empty() || !std::isfinite(area.x) || !std::isfinite(area.y) || !std::isfinite(area.w) || !std::isfinite(area.h))
        return std::nullopt;
    return area;
}

std::optional<BorderImageGrid> resolveBorderImageGrid(const Style::BorderImageSlice& sliceValues,
    const Style::BorderImageWidth& widthValues, const Style::BorderImageOutset& outsetValues, const Style::BorderImageRepeat& repeat,
    const Rect& borderBox, float imageWidth, float imageHeight, const RectEdges<float>& computedBorderWidths) {
    if (borderBox.empty() || !std::isfinite(borderBox.x) || !std::isfinite(borderBox.y) || !std::isfinite(imageWidth)
        || !std::isfinite(imageHeight) || imageWidth <= 0.f || imageHeight <= 0.f)
        return std::nullopt;

    const RectEdges<float> slices {slice(sliceValues.edges.top, imageHeight), slice(sliceValues.edges.right, imageWidth),
        slice(sliceValues.edges.bottom, imageHeight), slice(sliceValues.edges.left, imageWidth)};
    const auto area = resolveBorderImageArea(borderBox, outsetValues, computedBorderWidths);
    if (!area)
        return std::nullopt;

    const auto width = [area](Style::BorderImageWidthValue value, bool horizontal, float borderWidth, float sliceSize) {
        const float extent = horizontal ? area->w : area->h;
        switch (value.unit) {
        case Style::BorderImageValueUnit::Number:
            return value.value * borderWidth;
        case Style::BorderImageValueUnit::Length:
            return value.value;
        case Style::BorderImageValueUnit::Percentage:
            return value.value * extent / 100.f;
        case Style::BorderImageValueUnit::Auto:
            return sliceSize > 0.f ? sliceSize : borderWidth;
        }
        return std::numeric_limits<float>::infinity();
    };
    RectEdges<float> widths {width(widthValues.edges.top, false, computedBorderWidths.top, slices.top),
        width(widthValues.edges.right, true, computedBorderWidths.right, slices.right),
        width(widthValues.edges.bottom, false, computedBorderWidths.bottom, slices.bottom),
        width(widthValues.edges.left, true, computedBorderWidths.left, slices.left)};
    if (!std::isfinite(widths.top) || !std::isfinite(widths.right) || !std::isfinite(widths.bottom) || !std::isfinite(widths.left))
        return std::nullopt;

    float factor = 1.f;
    if (widths.left + widths.right > area->w)
        factor = std::min(factor, area->w / (widths.left + widths.right));
    if (widths.top + widths.bottom > area->h)
        factor = std::min(factor, area->h / (widths.top + widths.bottom));
    widths.top *= factor;
    widths.right *= factor;
    widths.bottom *= factor;
    widths.left *= factor;

    const float sourceInnerLeft = slices.left;
    const float sourceInnerRight = imageWidth - slices.right;
    const float sourceInnerBottom = slices.bottom;
    const float sourceInnerTop = imageHeight - slices.top;
    const float destinationInnerLeft = area->left() + widths.left;
    const float destinationInnerRight = area->right() - widths.right;
    const float destinationInnerBottom = area->bottom() + widths.bottom;
    const float destinationInnerTop = area->top() - widths.top;
    const std::array<Rect, 3> sourceColumns {
        Rect {0.f, 0.f, slices.left, imageHeight},
        Rect {sourceInnerLeft, 0.f, std::max(0.f, sourceInnerRight - sourceInnerLeft), imageHeight},
        Rect {sourceInnerRight, 0.f, slices.right, imageHeight},
    };
    const std::array<Rect, 3> sourceRows {
        Rect {0.f, sourceInnerTop, imageWidth, slices.top},
        Rect {0.f, sourceInnerBottom, imageWidth, std::max(0.f, sourceInnerTop - sourceInnerBottom)},
        Rect {0.f, 0.f, imageWidth, slices.bottom},
    };
    const std::array<Rect, 3> destinationColumns {
        Rect {area->left(), area->bottom(), widths.left, area->h},
        Rect {destinationInnerLeft, area->bottom(), std::max(0.f, destinationInnerRight - destinationInnerLeft), area->h},
        Rect {destinationInnerRight, area->bottom(), widths.right, area->h},
    };
    const std::array<Rect, 3> destinationRows {
        Rect {area->left(), destinationInnerTop, area->w, widths.top},
        Rect {area->left(), destinationInnerBottom, area->w, std::max(0.f, destinationInnerTop - destinationInnerBottom)},
        Rect {area->left(), area->bottom(), area->w, widths.bottom},
    };

    const bool emptyHorizontalRegions = slices.left + slices.right >= imageWidth;
    const bool emptyVerticalRegions = slices.top + slices.bottom >= imageHeight;
    const auto middleScale = [](float first, float second) {
        return first > 0.f ? first : second > 0.f ? second : 1.f;
    };
    const float middleScaleX = middleScale(scale(widths.top, slices.top), scale(widths.bottom, slices.bottom));
    const float middleScaleY = middleScale(scale(widths.left, slices.left), scale(widths.right, slices.right));
    BorderImageGrid grid {};
    for (std::size_t piece = 0; piece < grid.pieces.size(); ++piece) {
        const std::size_t row = piece / 3;
        const std::size_t column = piece % 3;
        BorderImagePatch& patch = grid.pieces[piece];
        patch.source = {sourceColumns[column].x, sourceRows[row].y, sourceColumns[column].w, sourceRows[row].h};
        patch.destination = {destinationColumns[column].x, destinationRows[row].y, destinationColumns[column].w, destinationRows[row].h};
        patch.repeatX = column == 1 ? repeat.horizontal : Style::BorderImageRepeatMode::Stretch;
        patch.repeatY = row == 1 ? repeat.vertical : Style::BorderImageRepeatMode::Stretch;
        if ((row == 1 && emptyVerticalRegions) || (column == 1 && emptyHorizontalRegions))
            patch.source = {};
        if (patch.source.empty() || patch.destination.empty())
            continue;

        patch.tileSize = {patch.destination.w, patch.destination.h};
        if (row != 1)
            patch.tileSize.x = patch.source.w * scale(patch.destination.h, patch.source.h);
        if (column != 1)
            patch.tileSize.y = patch.source.h * scale(patch.destination.w, patch.source.w);
        if (row == 1 && column == 1)
            patch.tileSize = {patch.source.w * middleScaleX, patch.source.h * middleScaleY};
    }
    if (!sliceValues.fill)
        grid.pieces[static_cast<std::size_t>(BorderImageGridPiece::Center)].source = {};
    return grid;
}
} // namespace Core::Layout
