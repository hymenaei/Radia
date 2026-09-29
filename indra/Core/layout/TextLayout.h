/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include "Color.h"
#include "ComputedStyle.h"
#include "LayoutGeometry.h"
#include "llstring.h"

namespace Core {
class Element;
class PaintContext;
class TextMeasurer;
namespace CSS {
class StyleSheet;
}
} // namespace Core

namespace Core::Layout {
struct TextPaintStyle {
    Color color;
    Style::TextDecoration textDecoration = Style::TextDecoration::NoneValue;
    Style::TextAlign textAlign = Style::TextAlign::Left;
    Direction direction = Direction::LeftToRight;
};
} // namespace Core::Layout

namespace Core::Layout::detail {
std::vector<std::size_t> graphemeBoundaries(const LLWString& value);

struct TextRun {
    std::string value;
    Style::ComputedStyle style;
    Vec2 size;
};

using TextLine = std::vector<TextRun>;

float interRunSpacing(const TextRun& left, const TextRun& right, const TextMeasurer& metrics);

struct LaidOutTextLine {
    TextLine runs;
    Vec2 size;
};

struct LaidOutText {
    std::vector<LaidOutTextLine> lines;
    Vec2 size;
};

LaidOutText layoutText(const std::vector<TextLine>& hardLines, const Style::ComputedStyle& style, const TextMeasurer& metrics,
    std::optional<float> availableWidth, bool visualOrder, bool applyOverflow);
} // namespace Core::Layout::detail

namespace Core::Layout {
class TextLayout {
public:
    TextLayout() = default;
    explicit TextLayout(std::string text) { setText(std::move(text)); }

    void setText(std::string text);
    const std::string& plainText() const { return mText; }

    Vec2 measure(const TextMeasurer& metrics, const Style::ComputedStyle& style, const CSS::StyleSheet& styleSheet, const Element& owner,
        std::optional<float> resolvedWidth = std::nullopt) const;
    void preparePaint(const TextMeasurer& metrics, const Style::ComputedStyle& style, const CSS::StyleSheet& styleSheet,
        const Element& owner, float availableWidth) const;
    void paint(PaintContext& context, const Rect& rect, const Style::ComputedStyle& style, const CSS::StyleSheet* styleSheet,
        const Element& owner) const;
    void paintPrepared(PaintContext& context, const Rect& rect, const Style::ComputedStyle& layoutStyle, const TextPaintStyle& paintStyle,
        const CSS::StyleSheet* styleSheet, const Element& owner) const;

private:
    void paintLayout(PaintContext& context, const Rect& rect, const TextPaintStyle& style, const detail::LaidOutText& layout,
        const TextMeasurer& metrics) const;
    const std::vector<detail::TextLine>& cachedLines(const TextMeasurer& metrics, const Style::ComputedStyle& style,
        const CSS::StyleSheet* styleSheet, const Element& owner) const;
    const detail::LaidOutText& cachedLayout(const TextMeasurer& metrics, const Style::ComputedStyle& style,
        const CSS::StyleSheet* styleSheet, const Element& owner, std::optional<float> availableWidth, bool visualOrder,
        bool applyOverflow) const;

    std::string mText;
    std::uint64_t mContentGeneration = 0;
    mutable std::uint64_t mCachedContentGeneration = 0;
    mutable const TextMeasurer* mCachedMetrics = nullptr;
    mutable std::uint64_t mCachedMetricsGeneration = 0;
    mutable const CSS::StyleSheet* mCachedStyleSheet = nullptr;
    mutable std::uint64_t mCachedStyleSheetGeneration = 0;
    mutable const Element* mCachedOwner = nullptr;
    mutable std::size_t mCachedStyleFingerprint = 0;
    mutable std::vector<detail::TextLine> mCachedLines;
    mutable bool mCachedLayoutValid = false;
    mutable bool mCachedLayoutWidthSet = false;
    mutable float mCachedLayoutWidth = 0.f;
    mutable bool mCachedLayoutVisualOrder = false;
    mutable bool mCachedLayoutOverflow = false;
    mutable std::size_t mCachedLayoutStyleFingerprint = 0;
    mutable detail::LaidOutText mCachedLayout;
};
} // namespace Core::Layout
