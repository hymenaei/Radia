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

namespace rdui {
namespace {
using detail::TextLine;
using detail::TextRun;

Style inlineStyle(const StyleSheet* theme, const Widget& owner, const std::vector<std::string>& inline_ancestors, const Style& inherited) {
    Style style = theme ? theme->resolveInline(owner, "kbd", inline_ancestors) : Style{};
    inheritStyle(style, inherited);
    return style;
}

void appendNode(const InlineContentNode& node, const Style& inherited, const TextMetrics& metrics, const StyleSheet* theme, const Widget& owner,
                std::vector<TextLine>& lines) {
    Style style = inherited;
    switch (node.kind()) {
        case InlineContentKind::B: style.font_weight = 700; break;
        case InlineContentKind::I: style.font_italic = true; break;
        case InlineContentKind::S: style.font_strike = true; break;
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
        const Style key_style = inlineStyle(theme, owner, {"kbd"}, run.style);
        const float gap = run.style.gap.fixedPixels();
        float content_width = 0.f;
        float content_height = 0.f;
        for (const std::string& key : presentation.keys) {
            if (key.empty()) continue;
            TextRun::Key key_run;
            key_run.value = key;
            key_run.style = key_style;
            key_run.text_size = metrics.measureText(key, key_style);
            key_run.box_size = {key_run.text_size.x + key_style.padding.horizontal(), key_run.text_size.y + key_style.padding.vertical()};
            key_run.size = {key_run.box_size.x + key_style.margin.horizontal(), key_run.box_size.y + key_style.margin.vertical()};
            if (!run.keys.empty()) {
                content_width += gap;
                run.value += ' ';
            }
            content_width += key_run.size.x;
            content_height = std::max(content_height, key_run.size.y);
            run.value += key;
            run.keys.push_back(std::move(key_run));
        }
        if (run.keys.empty()) return;
        run.box_size = {content_width + run.style.padding.horizontal(), content_height + run.style.padding.vertical()};
        run.size = {run.box_size.x + run.style.margin.horizontal(), run.box_size.y + run.style.margin.vertical()};
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
        signature += node.metadata();
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
    mixStyleValue(hash, static_cast<std::size_t>(style.font_family));
    mixStyleValue(hash, style.font_size);
    mixStyleValue(hash, static_cast<std::size_t>(style.font_weight));
    mixStyleValue(hash, static_cast<std::size_t>(style.font_italic));
    mixStyleValue(hash, static_cast<std::size_t>(style.font_strike));
    mixStyleValue(hash, static_cast<std::size_t>(style.line_height.has_value()));
    if (style.line_height) mixLength(hash, *style.line_height);
    mixLength(hash, style.letter_spacing);
    mixLength(hash, style.word_spacing);
    mixColor(hash, style.text_color);
    return hash;
}

std::size_t textLayoutFingerprint(const Style& style, bool visual_order, bool apply_overflow) {
    std::size_t hash = 0;
    mixStyleValue(hash, static_cast<std::size_t>(style.text_wrap));
    if (apply_overflow) {
        mixStyleValue(hash, static_cast<std::size_t>(style.text_overflow));
        mixStyleValue(hash, static_cast<std::size_t>(style.overflow_x));
    }
    if (visual_order) mixStyleValue(hash, static_cast<std::size_t>(style.direction));
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
                       std::optional<float> resolved_width) const {
    std::optional<float> available_width;
    if (resolved_width) available_width = std::max(0.f, *resolved_width - style.padding.horizontal());
    else if (!style.width.isAuto() && !style.width.isPercentage()) available_width = std::max(0.f, style.width.pixels() - style.padding.horizontal());
    return cachedLayout(metrics, style, &theme, owner, available_width, false, false).size;
}

void TextHost::paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const {
    const detail::TextLayout& layout = cachedLayout(context, style, theme, owner, rect.w, true, true);
    float y = rect.top();
    for (const detail::LaidOutTextLine& line : layout.lines) {
        y -= line.size.y;
        float x = rect.x + alignedOffset(rect.w, line.size.x, style.text_align);
        for (std::size_t run_index = 0; run_index < line.runs.size(); ++run_index) {
            const TextRun& run = line.runs[run_index];
            if (run.keybinding()) {
                const float run_bottom = y + (line.size.y - run.size.y) * .5f;
                const Rect box{x + run.style.margin.left.fixedPixels(), run_bottom + run.style.margin.bottom.fixedPixels(), run.box_size.x,
                               run.box_size.y};
                context.paintBox(box, run.style);
                float key_x = box.x + run.style.padding.left;
                const float content_height = std::max(0.f, box.h - run.style.padding.vertical());
                for (std::size_t index = 0; index < run.keys.size(); ++index) {
                    const TextRun::Key& key = run.keys[index];
                    key_x += key.style.margin.left.fixedPixels();
                    const float key_y =
                        box.y + run.style.padding.bottom + (content_height - key.size.y) * .5f + key.style.margin.bottom.fixedPixels();
                    const Rect key_box{key_x, key_y, key.box_size.x, key.box_size.y};
                    context.paintBox(key_box, key.style);
                    Style text_style = key.style;
                    text_style.text_align = TextAlign::Left;
                    context.paintText(key.value,
                                      {key_box.x + key.style.padding.left, key_box.y + key.style.padding.bottom, key.text_size.x, key.text_size.y},
                                      text_style);
                    key_x += key.box_size.x + key.style.margin.right.fixedPixels();
                    if (index + 1 < run.keys.size()) key_x += run.style.gap.fixedPixels();
                }
                x += run.size.x;
                if (run_index + 1 < line.runs.size()) x += interRunSpacing(run, line.runs[run_index + 1], context);
                continue;
            }
            Style run_style = run.style;
            run_style.text_align = TextAlign::Left;
            context.paintText(run.value, {x, y, run.size.x, line.size.y}, run_style);
            x += run.size.x;
            if (run_index + 1 < line.runs.size()) x += interRunSpacing(run, line.runs[run_index + 1], context);
        }
    }
}

const std::vector<detail::TextLine>& TextHost::cachedLines(const TextMetrics& metrics, const Style& style, const StyleSheet* theme,
                                                           const Widget& owner) const {
    const std::size_t fingerprint = textStyleFingerprint(style);
    const std::uint64_t theme_generation = theme ? theme->generation() : 0;
    const std::uint64_t owner_style_revision = owner.styleContextRevision();
    const Widget* owner_parent = owner.parent();
    if (mCachedContentGeneration == mContentGeneration
        && mCachedMetrics == &metrics
        && mCachedMetricsGeneration == metrics.generation()
        && mCachedTheme == theme
        && mCachedThemeGeneration == theme_generation
        && mCachedOwner == &owner
        && mCachedOwnerParent == owner_parent
        && mCachedOwnerStyleRevision == owner_style_revision
        && mCachedStyleFingerprint == fingerprint)
        return mCachedLines;

    mCachedLines = layoutLines(mContent, style, metrics, theme, owner);
    mCachedLayoutValid = false;
    mCachedContentGeneration = mContentGeneration;
    mCachedMetrics = &metrics;
    mCachedMetricsGeneration = metrics.generation();
    mCachedTheme = theme;
    mCachedThemeGeneration = theme_generation;
    mCachedOwner = &owner;
    mCachedOwnerParent = owner_parent;
    mCachedOwnerStyleRevision = owner_style_revision;
    mCachedStyleFingerprint = fingerprint;
    return mCachedLines;
}

const detail::TextLayout& TextHost::cachedLayout(const TextMetrics& metrics, const Style& style, const StyleSheet* theme, const Widget& owner,
                                                 std::optional<float> available_width, bool visual_order, bool apply_overflow) const {
    const std::vector<detail::TextLine>& lines = cachedLines(metrics, style, theme, owner);
    const std::size_t layout_fingerprint = textLayoutFingerprint(style, visual_order, apply_overflow);
    const bool width_matches = mCachedLayoutWidthSet == available_width.has_value() && (!available_width || mCachedLayoutWidth == *available_width);
    if (!mCachedLayoutValid
        || !width_matches
        || mCachedLayoutVisualOrder != visual_order
        || mCachedLayoutOverflow != apply_overflow
        || mCachedLayoutStyleFingerprint != layout_fingerprint) {
        mCachedLayout = detail::layoutText(lines, style, metrics, available_width, visual_order, apply_overflow);
        mCachedLayoutWidthSet = available_width.has_value();
        mCachedLayoutWidth = available_width.value_or(0.f);
        mCachedLayoutVisualOrder = visual_order;
        mCachedLayoutOverflow = apply_overflow;
        mCachedLayoutStyleFingerprint = layout_fingerprint;
        mCachedLayoutValid = true;
    }
    return mCachedLayout;
}
} // namespace rdui
