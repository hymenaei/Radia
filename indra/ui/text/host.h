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
#include "text/layout.h"
#include "types.h"

namespace radia::ui {
class StyleSheet;
class Element;

class PaintContext;
struct ComputedStyle;
class TextMetrics;

class TextLayout {
public:
    TextLayout() = default;
    explicit TextLayout(std::string text) { setText(std::move(text)); }

    void setText(std::string text);
    const std::string& plainText() const { return mPlainText; }

    Vec2 measure(const TextMetrics& metrics, const ComputedStyle& style, const StyleSheet& styleSheet, const Element& owner,
                 std::optional<float> resolvedWidth = std::nullopt) const;
    void paint(PaintContext& context, const Rect& rect, const ComputedStyle& style, const StyleSheet* styleSheet, const Element& owner) const;

private:
    void updatePlainText();
    const std::vector<detail::TextLine>& cachedLines(const TextMetrics& metrics, const ComputedStyle& style, const StyleSheet* styleSheet,
                                                     const Element& owner) const;
    const detail::TextLayout& cachedLayout(const TextMetrics& metrics, const ComputedStyle& style, const StyleSheet* styleSheet, const Element& owner,
                                           std::optional<float> availableWidth, bool visualOrder, bool applyOverflow) const;

    std::string mText;
    std::string mPlainText;
    std::uint64_t mContentGeneration = 0;
    mutable std::uint64_t mCachedContentGeneration = 0;
    mutable const TextMetrics* mCachedMetrics = nullptr;
    mutable std::uint64_t mCachedMetricsGeneration = 0;
    mutable const StyleSheet* mCachedStyleSheet = nullptr;
    mutable std::uint64_t mCachedStyleSheetGeneration = 0;
    mutable const Element* mCachedOwner = nullptr;
    mutable const Element* mCachedOwnerParent = nullptr;
    mutable std::uint64_t mCachedOwnerStyleRevision = 0;
    mutable std::size_t mCachedStyleFingerprint = 0;
    mutable std::vector<detail::TextLine> mCachedLines;
    mutable bool mCachedLayoutValid = false;
    mutable bool mCachedLayoutWidthSet = false;
    mutable float mCachedLayoutWidth = 0.f;
    mutable bool mCachedLayoutVisualOrder = false;
    mutable bool mCachedLayoutOverflow = false;
    mutable std::size_t mCachedLayoutStyleFingerprint = 0;
    mutable detail::TextLayout mCachedLayout;
};
} // namespace radia::ui
