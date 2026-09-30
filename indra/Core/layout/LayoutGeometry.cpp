/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "LayoutGeometry.h"
#include "ComputedStyle.h"

namespace Core::Layout {
RectEdges<float> paddingPixels(const Style::ComputedStyle& style) {
    const RectEdges<Style::PaddingEdge> padding = style.padding();
    return {padding.top.pixels, padding.right.pixels, padding.bottom.pixels, padding.left.pixels};
}

RectEdges<float> borderWidths(const Style::ComputedStyle& style) {
    const RectEdges<Style::LineWidth> widths = style.borderWidth();
    const RectEdges<Style::BorderStyle> styles = style.borderStyle();
    return {
        styles.top == Style::BorderStyle::NoneValue ? 0.f : widths.top.pixels,
        styles.right == Style::BorderStyle::NoneValue ? 0.f : widths.right.pixels,
        styles.bottom == Style::BorderStyle::NoneValue ? 0.f : widths.bottom.pixels,
        styles.left == Style::BorderStyle::NoneValue ? 0.f : widths.left.pixels,
    };
}
} // namespace Core::Layout
