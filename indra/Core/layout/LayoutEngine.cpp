/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "LayoutEngine.h"
#include "LayoutPass.h"
#include "Surface.h"

namespace Core::Layout {
Vec2 Engine::measure(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, std::optional<float> outerWidth,
    std::optional<float> outerHeight) {
    const Surface* surface = node.surface();
    const ScrollLayoutOptions scrollOptions = surface ? surface->scrollLayoutOptions() : ScrollLayoutOptions {};
    Pass pass(styleSheet, textMetrics, surface ? surface->layoutDirection() : Direction::LeftToRight, scrollOptions);
    return measure(node, pass, outerWidth, outerHeight);
}

Statistics Engine::arrange(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Direction direction,
    ScrollLayoutOptions scrollOptions) {
    return run(node, styleSheet, textMetrics, direction, scrollOptions);
}

Statistics Engine::layout(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Direction direction,
    ScrollLayoutOptions scrollOptions) {
    return run(node, styleSheet, textMetrics, direction, scrollOptions);
}

Statistics Engine::layout(Element& node, Style::Pass& styles, ScrollLayoutOptions scrollOptions) {
    return run(node, styles, scrollOptions);
}

Statistics Engine::run(Element& node, const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Direction direction,
    ScrollLayoutOptions scrollOptions) {
    Pass pass(styleSheet, textMetrics, direction, scrollOptions);
    return runWithPass(node, pass);
}

Statistics Engine::run(Element& node, Style::Pass& styles, ScrollLayoutOptions scrollOptions) {
    Pass pass(styles, scrollOptions);
    return runWithPass(node, pass);
}

void Engine::prepareTextPaint(Element& node, Pass& pass) {
    for (const auto& childNode : node.mChildren)
        if (Text* text = childNode->asText())
            text->preparePaint(pass.textMetrics(), pass.styleSheet());
        else if (Element* child = childNode->asElement())
            prepareTextPaint(*child, pass);
}

Statistics Engine::runWithPass(Element& node, Pass& pass) {
    const NodeSnapshot state(node);
    const Surface* surface = node.surface();
    pass.setViewport(surface ? Rect {0.f, 0.f, surface->width(), surface->height()} : node.rect());
    measure(node, pass);
    Element* current = state.get();
    if (!state.layoutValid())
        return pass.statistics();
    if (current->isDisplayed(pass.style(*current))) {
        arrangeNode(*current, pass);
        if (state.layoutValid())
            prepareTextPaint(*current, pass);
    }
    return pass.statistics();
}
} // namespace Core::Layout
