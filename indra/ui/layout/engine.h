/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include "types.h"

namespace radia::ui {
class Element;
struct Style;
class StyleSheet;
class TextMetrics;

struct LayoutStatistics {
    std::size_t measuredNodes = 0;
    std::size_t constrainedRemeasures = 0;
    std::size_t arrangedNodes = 0;
    std::size_t skippedNodes = 0;
};

Style resolveElementStyle(const StyleSheet& styleSheet, const Element& node);
Vec2 measureElement(const Element& node, const StyleSheet& styleSheet, const TextMetrics& textMetrics);
void measureTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics);
void arrangeTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                 LayoutDirection direction = LayoutDirection::LeftToRight, ScrollLayoutOptions scrollOptions = {});
LayoutStatistics layoutTree(Element& root, const StyleSheet& styleSheet, const TextMetrics& textMetrics,
                            LayoutDirection direction = LayoutDirection::LeftToRight, ScrollLayoutOptions scrollOptions = {});
} // namespace radia::ui
