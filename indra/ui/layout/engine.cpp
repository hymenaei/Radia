/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/layoutcontext.h"
#include "surface/surface.h"

namespace radia::ui {
Vec2 LayoutEngine::measure(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, std::optional<float> outerWidth,
                           std::optional<float> outerHeight) {
    const ScrollLayoutOptions scrollOptions = node.mSurface ? node.mSurface->scrollLayoutOptions() : ScrollLayoutOptions{};
    LayoutPass pass(styleSheet, textMetrics, node.mSurface ? node.mSurface->layoutDirection() : LayoutDirection::LeftToRight, scrollOptions);
    return measure(node, pass, outerWidth, outerHeight);
}

LayoutStatistics LayoutEngine::arrange(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                       ScrollLayoutOptions scrollOptions) {
    return run(node, styleSheet, textMetrics, direction, scrollOptions);
}

LayoutStatistics LayoutEngine::layout(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                      ScrollLayoutOptions scrollOptions) {
    return run(node, styleSheet, textMetrics, direction, scrollOptions);
}

LayoutStatistics LayoutEngine::layout(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions) {
    return run(node, styles, scrollOptions);
}

LayoutStatistics LayoutEngine::run(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                                   ScrollLayoutOptions scrollOptions) {
    LayoutPass pass(styleSheet, textMetrics, direction, scrollOptions);
    return runWithPass(node, pass);
}

LayoutStatistics LayoutEngine::run(Element& node, StylePass& styles, ScrollLayoutOptions scrollOptions) {
    LayoutPass pass(styles, scrollOptions);
    return runWithPass(node, pass);
}

LayoutStatistics LayoutEngine::runWithPass(Element& node, LayoutPass& pass) {
    const NodeSnapshot state(node);
    measure(node, pass);
    Element* current = state.get();
    if (!state.layoutValid()) return pass.statistics();
    if (current->isDisplayed(pass.style(*current))) arrangeNode(*current, pass);
    return pass.statistics();
}
} // namespace radia::ui
