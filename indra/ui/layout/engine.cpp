/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/engineinternal.h"
#include "layout/layoutcontext.h"
#include "surface/surface.h"
#include "text/metrics.h"

namespace radia::ui {
Style resolveElementStyle(const StyleSheet& styleSheet, const Element& node) {
    LayoutPass pass(styleSheet, fixedTextMetrics(), node.mSurface ? node.mSurface->layoutDirection() : LayoutDirection::LeftToRight);
    return pass.style(node);
}

Vec2 LayoutEngine::measure(Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics, std::optional<float> outerWidth,
                           std::optional<float> outerHeight) {
    LayoutPass pass(styleSheet, textMetrics, node.mSurface ? node.mSurface->layoutDirection() : LayoutDirection::LeftToRight);
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
    if (!state.valid()) return pass.statistics();
    if (current->isDisplayed(pass.style(*current))) arrangeNode(*current, pass);
    return pass.statistics();
}

Vec2 measureElement(const Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics) {
    return LayoutEngine::measure(const_cast<Element&>(node), styleSheet, textMetrics);
}

void measureTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics) {
    LayoutEngine::measure(root, styleSheet, textMetrics);
}

void arrangeTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                 ScrollLayoutOptions scrollOptions) {
    LayoutEngine::arrange(root, styleSheet, textMetrics, direction, scrollOptions);
}

LayoutStatistics layoutTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                            ScrollLayoutOptions scrollOptions) {
    return LayoutEngine::layout(root, styleSheet, textMetrics, direction, scrollOptions);
}

LayoutStatistics layoutTreeUsingStylePass(Element& root, StylePass& styles, ScrollLayoutOptions scrollOptions) {
    return LayoutEngine::layout(root, styles, scrollOptions);
}
} // namespace radia::ui
