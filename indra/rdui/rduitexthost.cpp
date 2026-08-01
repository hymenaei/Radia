/**
 * @file rduitexthost.cpp
 * @brief
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
#include "rduitexthost.h"
#include <algorithm>
#include <cmath>
#include "rduipaintcontext.h"
#include "rduistyle.h"
#include "rduistylesheet.h"
#include "rduitextlayout.h"
#include "rduitextmetrics.h"
#include "rduiwidget.h"

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
} // namespace

void TextHost::setContent(TextSource content) {
    mSource = std::move(content);
    mContent = mSource.materialize();
    mHasKeybindings =
        std::any_of(mContent.nodes().begin(), mContent.nodes().end(), [](const InlineContentNode& node) { return hasKeybinding(node); });
    updatePlainText();
}

void TextHost::resolveLocalized(const std::function<InlineContent(const LocalizationRequest&)>& resolve) {
    mContent = mSource.materialize(resolve);
    mHasKeybindings =
        std::any_of(mContent.nodes().begin(), mContent.nodes().end(), [](const InlineContentNode& node) { return hasKeybinding(node); });
    updatePlainText();
}

bool TextHost::resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve) {
    if (!mHasKeybindings) return false;
    const std::string previous = keybindingSignature(mContent);
    mContent = mContent.resolveKeybindings(resolve);
    updatePlainText();
    return keybindingSignature(mContent) != previous;
}

void TextHost::updatePlainText() {
    mPlainText.clear();
    for (const InlineContentNode& node : mContent.nodes()) appendPlainText(node, mPlainText);
}

Vec2 TextHost::measure(const TextMetrics& metrics, const Style& style, const StyleSheet& theme, const Widget& owner) const {
    std::optional<float> available_width;
    if (!style.width.isAuto() && !style.width.isPercentage()) available_width = std::max(0.f, style.width.pixels() - style.padding.horizontal());
    return detail::layoutText(layoutLines(mContent, style, metrics, &theme, owner), style, metrics, available_width, false, false).size;
}

void TextHost::paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const {
    const detail::TextLayout layout = detail::layoutText(layoutLines(mContent, style, context, theme, owner), style, context, rect.w, true, true);
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
} // namespace rdui
