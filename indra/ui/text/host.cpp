/**
 * @file host.cpp
 * @brief Measures and paints TextHost content with localization, inline styles, and keybindings.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "text/host.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <functional>
#include "render/paintcontext.h"
#include "style/style.h"
#include "style/stylesheet.h"
#include "text/layout.h"
#include "text/metrics.h"
#include "widgets/widget.h"

namespace radia::ui {
namespace {
using detail::TextLine;
using detail::TextRun;

Style inlineStyle(const StyleSheet* theme, const Widget& owner, const std::vector<std::string>& inlineAncestors, const Style& inherited) {
    Style style = theme ? theme->resolveInline(owner, "kbd", inlineAncestors) : Style{};
    inheritStyle(style, inherited);
    return style;
}

void appendNode(const InlineContentNode& node, const Style& inherited, const TextMetrics& metrics, const StyleSheet* theme, const Widget& owner,
                std::vector<TextLine>& lines) {
    Style style = inherited;
    switch (node.kind()) {
        case InlineContentKind::B: style.fontWeight = 700; break;
        case InlineContentKind::I: style.fontItalic = true; break;
        case InlineContentKind::S: style.fontStrike = true; break;
        case InlineContentKind::Br: lines.emplace_back(); return;
        default: break;
    }

    if (node.kind() == InlineContentKind::Text) {
        const std::string& value = node.value();
        if (!value.empty()) lines.back().push_back({value, style, metrics.measureText(value, style)});
        return;
    }
    if (node.kind() == InlineContentKind::Kbd) {
        const KeybindingPresentation& presentation = node.keybindingPresentation();
        if (presentation.keys.empty()) return;

        TextRun run;
        run.style = inlineStyle(theme, owner, {}, style);
        const Style keyStyle = inlineStyle(theme, owner, {"kbd"}, run.style);
        const float gap = run.style.gap.fixedPixels();
        float contentWidth = 0.f;
        float contentHeight = 0.f;
        for (const std::string& key : presentation.keys) {
            if (key.empty()) continue;
            TextRun::Key keyRun;
            keyRun.value = key;
            keyRun.style = keyStyle;
            keyRun.textSize = metrics.measureText(key, keyStyle);
            keyRun.boxSize = {keyRun.textSize.x + keyStyle.padding.horizontal(), keyRun.textSize.y + keyStyle.padding.vertical()};
            keyRun.size = {keyRun.boxSize.x + keyStyle.margin.horizontal(), keyRun.boxSize.y + keyStyle.margin.vertical()};
            if (!run.keys.empty()) {
                contentWidth += gap;
                run.value += ' ';
            }
            contentWidth += keyRun.size.x;
            contentHeight = std::max(contentHeight, keyRun.size.y);
            run.value += key;
            run.keys.push_back(std::move(keyRun));
        }
        if (run.keys.empty()) return;
        run.boxSize = {contentWidth + run.style.padding.horizontal(), contentHeight + run.style.padding.vertical()};
        run.size = {run.boxSize.x + run.style.margin.horizontal(), run.boxSize.y + run.style.margin.vertical()};
        lines.back().push_back(std::move(run));
        return;
    }
    for (const InlineContentNode& child : node.children()) appendNode(child, style, metrics, theme, owner, lines);
}

std::vector<TextLine> layoutLines(const InlineContent& content, const Style& style, const TextMetrics& metrics, const StyleSheet* theme,
                                  const Widget& owner) {
    std::vector<TextLine> lines(1);
    for (const InlineContentNode& node : content.nodes()) appendNode(node, style, metrics, theme, owner, lines);
    return lines;
}

float alignedOffset(float available, float occupied, TextAlign alignment) {
    if (alignment == TextAlign::Center) return (available - occupied) * .5f;
    if (alignment == TextAlign::Right || alignment == TextAlign::End) return available - occupied;
    return 0.f;
}

void appendPlainText(const InlineContentNode& node, std::string& text) {
    if (node.kind() == InlineContentKind::Text) {
        text += node.value();
        return;
    }
    if (node.kind() == InlineContentKind::Kbd) {
        bool first = true;
        for (const std::string& key : node.keybindingPresentation().keys) {
            if (!first) text += ' ';
            text += key;
            first = false;
        }
        return;
    }
    if (node.kind() == InlineContentKind::Br) {
        text += '\n';
        return;
    }
    for (const InlineContentNode& child : node.children()) appendPlainText(child, text);
}

bool hasKeybinding(const InlineContentNode& node) {
    if (node.kind() == InlineContentKind::Kbd) return true;
    return std::any_of(node.children().begin(), node.children().end(), [](const InlineContentNode& child) { return hasKeybinding(child); });
}

void appendKeybindingSignature(const InlineContentNode& node, std::string& signature) {
    if (node.kind() == InlineContentKind::Kbd) {
        signature += node.shortcutId();
        signature += '\x1d';
        for (const std::string& key : node.keybindingPresentation().keys) {
            signature += key;
            signature += '\x1e';
        }
        signature += '\x1f';
        return;
    }
    for (const InlineContentNode& child : node.children()) appendKeybindingSignature(child, signature);
}

std::string keybindingSignature(const InlineContent& content) {
    std::string signature;
    for (const InlineContentNode& node : content.nodes()) appendKeybindingSignature(node, signature);
    return signature;
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
    mixStyleValue(hash, static_cast<std::size_t>(style.fontStrike));
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

void TextHost::setContent(TextSource content) {
    mSource = std::move(content);
    mContent = mSource.materialize();
    mHasKeybindings =
        std::any_of(mContent.nodes().begin(), mContent.nodes().end(), [](const InlineContentNode& node) { return hasKeybinding(node); });
    ++mContentGeneration;
    updatePlainText();
}

void TextHost::resolveLocalized(const std::function<InlineContent(const LocalizationRequest&)>& resolve) {
    mContent = mSource.materialize(resolve);
    mHasKeybindings =
        std::any_of(mContent.nodes().begin(), mContent.nodes().end(), [](const InlineContentNode& node) { return hasKeybinding(node); });
    ++mContentGeneration;
    updatePlainText();
}

bool TextHost::resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve) {
    if (!mHasKeybindings) return false;
    const std::string previous = keybindingSignature(mContent);
    mContent = mContent.resolveKeybindings(resolve);
    updatePlainText();
    const bool changed = keybindingSignature(mContent) != previous;
    if (changed) ++mContentGeneration;
    return changed;
}

void TextHost::updatePlainText() {
    mPlainText.clear();
    for (const InlineContentNode& node : mContent.nodes()) appendPlainText(node, mPlainText);
}

Vec2 TextHost::measure(const TextMetrics& metrics, const Style& style, const StyleSheet& theme, const Widget& owner,
                       std::optional<float> resolvedWidth) const {
    std::optional<float> availableWidth;
    if (resolvedWidth) availableWidth = std::max(0.f, *resolvedWidth - style.padding.horizontal());
    else if (!style.width.isAuto() && !style.width.isPercentage()) availableWidth = std::max(0.f, style.width.pixels() - style.padding.horizontal());
    return cachedLayout(metrics, style, &theme, owner, availableWidth, false, false).size;
}

void TextHost::paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const {
    const detail::TextLayout& layout = cachedLayout(context, style, theme, owner, rect.w, true, true);
    float y = rect.top();
    for (const detail::LaidOutTextLine& line : layout.lines) {
        y -= line.size.y;
        float x = rect.x + alignedOffset(rect.w, line.size.x, style.textAlign);
        for (std::size_t runIndex = 0; runIndex < line.runs.size(); ++runIndex) {
            const TextRun& run = line.runs[runIndex];
            if (run.keybinding()) {
                const float runBottom = y + (line.size.y - run.size.y) * .5f;
                const Rect box{x + run.style.margin.left.fixedPixels(), runBottom + run.style.margin.bottom.fixedPixels(), run.boxSize.x,
                               run.boxSize.y};
                context.paintBox(box, run.style);
                float keyX = box.x + run.style.padding.left;
                const float contentHeight = std::max(0.f, box.h - run.style.padding.vertical());
                for (std::size_t index = 0; index < run.keys.size(); ++index) {
                    const TextRun::Key& key = run.keys[index];
                    keyX += key.style.margin.left.fixedPixels();
                    const float keyY =
                        box.y + run.style.padding.bottom + (contentHeight - key.size.y) * .5f + key.style.margin.bottom.fixedPixels();
                    const Rect keyBox{keyX, keyY, key.boxSize.x, key.boxSize.y};
                    context.paintBox(keyBox, key.style);
                    Style textStyle = key.style;
                    textStyle.textAlign = TextAlign::Left;
                    context.paintText(key.value,
                                      {keyBox.x + key.style.padding.left, keyBox.y + key.style.padding.bottom, key.textSize.x, key.textSize.y},
                                      textStyle);
                    keyX += key.boxSize.x + key.style.margin.right.fixedPixels();
                    if (index + 1 < run.keys.size()) keyX += run.style.gap.fixedPixels();
                }
                x += run.size.x;
                if (runIndex + 1 < line.runs.size()) x += interRunSpacing(run, line.runs[runIndex + 1], context);
                continue;
            }
            Style runStyle = run.style;
            runStyle.textAlign = TextAlign::Left;
            context.paintText(run.value, {x, y, run.size.x, line.size.y}, runStyle);
            x += run.size.x;
            if (runIndex + 1 < line.runs.size()) x += interRunSpacing(run, line.runs[runIndex + 1], context);
        }
    }
}

const std::vector<detail::TextLine>& TextHost::cachedLines(const TextMetrics& metrics, const Style& style, const StyleSheet* theme,
                                                           const Widget& owner) const {
    const std::size_t fingerprint = textStyleFingerprint(style);
    const std::uint64_t themeGeneration = theme ? theme->generation() : 0;
    const std::uint64_t ownerStyleRevision = owner.styleContextRevision();
    const Widget* ownerParent = owner.parent();
    if (mCachedContentGeneration == mContentGeneration
        && mCachedMetrics == &metrics
        && mCachedMetricsGeneration == metrics.generation()
        && mCachedTheme == theme
        && mCachedThemeGeneration == themeGeneration
        && mCachedOwner == &owner
        && mCachedOwnerParent == ownerParent
        && mCachedOwnerStyleRevision == ownerStyleRevision
        && mCachedStyleFingerprint == fingerprint)
        return mCachedLines;

    mCachedLines = layoutLines(mContent, style, metrics, theme, owner);
    mCachedLayoutValid = false;
    mCachedContentGeneration = mContentGeneration;
    mCachedMetrics = &metrics;
    mCachedMetricsGeneration = metrics.generation();
    mCachedTheme = theme;
    mCachedThemeGeneration = themeGeneration;
    mCachedOwner = &owner;
    mCachedOwnerParent = ownerParent;
    mCachedOwnerStyleRevision = ownerStyleRevision;
    mCachedStyleFingerprint = fingerprint;
    return mCachedLines;
}

const detail::TextLayout& TextHost::cachedLayout(const TextMetrics& metrics, const Style& style, const StyleSheet* theme, const Widget& owner,
                                                 std::optional<float> availableWidth, bool visualOrder, bool applyOverflow) const {
    const std::vector<detail::TextLine>& lines = cachedLines(metrics, style, theme, owner);
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
