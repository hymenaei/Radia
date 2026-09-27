/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "style/computedstyle.h"
#include "types.h"

namespace radia::ui {
inline RectEdges<float> paddingPixels(const ComputedStyle& style) {
    const RectEdges<PaddingEdge> padding = style.padding();
    return {padding.top.pixels, padding.right.pixels, padding.bottom.pixels, padding.left.pixels};
}

inline RectEdges<float> borderWidths(const ComputedStyle& style) {
    const RectEdges<LineWidth> widths = style.borderWidth();
    const RectEdges<BorderStyle> styles = style.borderStyle();
    return {
        styles.top == BorderStyle::NoneValue ? 0.f : widths.top.pixels,
        styles.right == BorderStyle::NoneValue ? 0.f : widths.right.pixels,
        styles.bottom == BorderStyle::NoneValue ? 0.f : widths.bottom.pixels,
        styles.left == BorderStyle::NoneValue ? 0.f : widths.left.pixels,
    };
}

inline float horizontalMargin(const RectEdges<MarginEdge>& margin) {
    return margin.left.fixedPixels() + margin.right.fixedPixels();
}

inline float verticalMargin(const RectEdges<MarginEdge>& margin) {
    return margin.top.fixedPixels() + margin.bottom.fixedPixels();
}

inline int horizontalAutoMarginCount(const RectEdges<MarginEdge>& margin) {
    return static_cast<int>(margin.left.isAuto()) + static_cast<int>(margin.right.isAuto());
}

inline int verticalAutoMarginCount(const RectEdges<MarginEdge>& margin) {
    return static_cast<int>(margin.top.isAuto()) + static_cast<int>(margin.bottom.isAuto());
}
} // namespace radia::ui
