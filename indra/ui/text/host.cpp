/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "text/host.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <functional>
#include <utility>
#include "elements/element.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "style/stylesheet.h"
#include "text/layout.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
using detail::TextLine;
using detail::TextRun;

std::vector<TextLine> layoutLines(const std::string& text, const Style& style, const TextMetrics& metrics) {
    std::vector<TextLine> lines(1);
    std::size_t start = 0;
    while (start <= text.size()) {
        const std::size_t end = text.find('\n', start);
        const std::string value = text.substr(start, end == std::string::npos ? std::string::npos : end - start);
        if (!value.empty()) lines.back().push_back({value, style, metrics.measureText(value, style)});
        if (end == std::string::npos) break;
        lines.emplace_back();
        start = end + 1;
    }
    return lines;
}

float alignedOffset(float available, float occupied, TextAlign alignment) {
    if (alignment == TextAlign::Center) return (available - occupied) * .5f;
    if (alignment == TextAlign::Right || alignment == TextAlign::End) return available - occupied;
    return 0.f;
}

void mixStyleValue(std::size_t& hash, std::size_t value) {
    hash ^= value + static_cast<std::size_t>(0x9e3779b9) + (hash << 6) + (hash >> 2);
}

void mixStyleValue(std::size_t& hash, float value) {
    mixStyleValue(hash, std::hash<float>{}(value));
}

void mixLength(std::size_t& hash, const Length& value) {
    mixStyleValue(hash, value.pixels);
    mixStyleValue(hash, value.percent);
}

void mixColor(std::size_t& hash, const Color& value) {
    mixStyleValue(hash, value.r);
    mixStyleValue(hash, value.g);
    mixStyleValue(hash, value.b);
    mixStyleValue(hash, value.a);
}

std::size_t textStyleFingerprint(const Style& style) {
    std::size_t hash = 0;
    mixStyleValue(hash, static_cast<std::size_t>(style.fontFamily));
    mixStyleValue(hash, style.fontSize);
    mixStyleValue(hash, static_cast<std::size_t>(style.fontWeight));
    mixStyleValue(hash, static_cast<std::size_t>(style.fontItalic));
    mixStyleValue(hash, static_cast<std::size_t>(style.textDecoration));
    mixStyleValue(hash, static_cast<std::size_t>(style.lineHeight.has_value()));
    if (style.lineHeight) mixLength(hash, *style.lineHeight);
    mixLength(hash, style.letterSpacing);
    mixLength(hash, style.wordSpacing);
    mixColor(hash, style.textColor);
    return hash;
}

std::size_t textLayoutFingerprint(const Style& style, bool visualOrder, bool applyOverflow) {
    std::size_t hash = 0;
    mixStyleValue(hash, static_cast<std::size_t>(style.textWrap));
    if (applyOverflow) {
        mixStyleValue(hash, static_cast<std::size_t>(style.textOverflow));
        mixStyleValue(hash, static_cast<std::size_t>(style.overflowX));
    }
    if (visualOrder) mixStyleValue(hash, static_cast<std::size_t>(style.direction));
    return hash;
}

} // namespace

void TextLayout::setText(std::string text) {
    mText = std::move(text);
    ++mContentGeneration;
    updatePlainText();
}

void TextLayout::updatePlainText() {
    mPlainText = mText;
}

Vec2 TextLayout::measure(const TextMetrics& metrics, const Style& style, const StyleSheet& styleSheet, const Element& owner,
                         std::optional<float> resolvedWidth) const {
    std::optional<float> availableWidth;
    if (resolvedWidth) availableWidth = std::max(0.f, *resolvedWidth - style.padding.horizontal());
    else if (!style.width.isAuto() && !style.width.isPercentage()) availableWidth = std::max(0.f, style.width.pixels() - style.padding.horizontal());
    return cachedLayout(metrics, style, &styleSheet, owner, availableWidth, false, false).size;
}

void TextLayout::paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* styleSheet, const Element& owner) const {
    const detail::TextLayout& layout = cachedLayout(context, style, styleSheet, owner, rect.w, true, true);
    float y = rect.top();
    for (const detail::LaidOutTextLine& line : layout.lines) {
        y -= line.size.y;
        float x = rect.x + alignedOffset(rect.w, line.size.x, style.textAlign);
        for (std::size_t runIndex = 0; runIndex < line.runs.size(); ++runIndex) {
            const TextRun& run = line.runs[runIndex];
            Style runStyle = run.style;
            runStyle.textAlign = TextAlign::Left;
            context.paintText(run.value, {x, y, run.size.x, line.size.y}, runStyle);
            x += run.size.x;
            if (runIndex + 1 < line.runs.size()) x += interRunSpacing(run, line.runs[runIndex + 1], context);
        }
    }
}

const std::vector<detail::TextLine>& TextLayout::cachedLines(const TextMetrics& metrics, const Style& style, const StyleSheet* styleSheet,
                                                             const Element& owner) const {
    const std::size_t fingerprint = textStyleFingerprint(style);
    const std::uint64_t styleSheetGeneration = styleSheet ? styleSheet->generation() : 0;
    const std::uint64_t ownerStyleRevision = owner.styleContextRevision();
    const Element* ownerParent = owner.parentElement();
    if (mCachedContentGeneration == mContentGeneration
        && mCachedMetrics == &metrics
        && mCachedMetricsGeneration == metrics.generation()
        && mCachedStyleSheet == styleSheet
        && mCachedStyleSheetGeneration == styleSheetGeneration
        && mCachedOwner == &owner
        && mCachedOwnerParent == ownerParent
        && mCachedOwnerStyleRevision == ownerStyleRevision
        && mCachedStyleFingerprint == fingerprint)
        return mCachedLines;

    mCachedLines = layoutLines(mText, style, metrics);
    mCachedLayoutValid = false;
    mCachedContentGeneration = mContentGeneration;
    mCachedMetrics = &metrics;
    mCachedMetricsGeneration = metrics.generation();
    mCachedStyleSheet = styleSheet;
    mCachedStyleSheetGeneration = styleSheetGeneration;
    mCachedOwner = &owner;
    mCachedOwnerParent = ownerParent;
    mCachedOwnerStyleRevision = ownerStyleRevision;
    mCachedStyleFingerprint = fingerprint;
    return mCachedLines;
}

const detail::TextLayout& TextLayout::cachedLayout(const TextMetrics& metrics, const Style& style, const StyleSheet* styleSheet, const Element& owner,
                                                   std::optional<float> availableWidth, bool visualOrder, bool applyOverflow) const {
    const std::vector<detail::TextLine>& lines = cachedLines(metrics, style, styleSheet, owner);
    const std::size_t layoutFingerprint = textLayoutFingerprint(style, visualOrder, applyOverflow);
    const bool widthMatches = mCachedLayoutWidthSet == availableWidth.has_value() && (!availableWidth || mCachedLayoutWidth == *availableWidth);
    if (!mCachedLayoutValid
        || !widthMatches
        || mCachedLayoutVisualOrder != visualOrder
        || mCachedLayoutOverflow != applyOverflow
        || mCachedLayoutStyleFingerprint != layoutFingerprint) {
        mCachedLayout = detail::layoutText(lines, style, metrics, availableWidth, visualOrder, applyOverflow);
        mCachedLayoutWidthSet = availableWidth.has_value();
        mCachedLayoutWidth = availableWidth.value_or(0.f);
        mCachedLayoutVisualOrder = visualOrder;
        mCachedLayoutOverflow = applyOverflow;
        mCachedLayoutStyleFingerprint = layoutFingerprint;
        mCachedLayoutValid = true;
    }
    return mCachedLayout;
}
} // namespace radia::ui
