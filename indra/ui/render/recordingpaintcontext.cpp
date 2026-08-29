/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "render/recordingpaintcontext.h"
#include <algorithm>

namespace radia::ui {
Vec2 RecordingPaintContext::measureText(const std::string& text, const Style& style) const {
    return mTextMetrics.measureText(text, style);
}

float RecordingPaintContext::usedLetterSpacing(const Style& style) const {
    return mTextMetrics.usedLetterSpacing(style);
}

void RecordingPaintContext::beginFrame(const PaintTarget& target) {
    PaintCommand command{PaintCommandKind::BeginFrame};
    command.target = target;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::endFrame() {
    mCommands.push_back({PaintCommandKind::EndFrame});
}

void RecordingPaintContext::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    PaintCommand command{PaintCommandKind::PushClip, rect, {}, {}, scale};
    command.clipAxes = axes;
    mCommands.push_back(std::move(command));
    ++mClipDepth;
    mMaxClipDepth = std::max(mMaxClipDepth, mClipDepth);
}

void RecordingPaintContext::popClip() {
    mCommands.push_back({PaintCommandKind::PopClip});
    --mClipDepth;
}

void RecordingPaintContext::pushTranslation(const Vec2& translation) {
    PaintCommand command{PaintCommandKind::PushTranslation};
    command.translation = translation;
    mCommands.push_back(std::move(command));
    ++mTranslationDepth;
    mMaxTranslationDepth = std::max(mMaxTranslationDepth, mTranslationDepth);
}

void RecordingPaintContext::popTranslation() {
    mCommands.push_back({PaintCommandKind::PopTranslation});
    --mTranslationDepth;
}

void RecordingPaintContext::beginEffects(const Rect& rect, const Style& style, float scale) {
    mCommands.push_back({PaintCommandKind::BeginEffects, rect, style, {}, scale});
}

void RecordingPaintContext::endEffects() {
    mCommands.push_back({PaintCommandKind::EndEffects});
}

void RecordingPaintContext::paintNativeScrollbar(const NativeScrollbarPaintRequest& request) {
    PaintCommand command{PaintCommandKind::Scrollbar};
    command.scrollbar = request;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::paintNativeInput(const NativeInputPaintRequest& request) {
    PaintCommand command{PaintCommandKind::NativeInput};
    command.nativeInput = request;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::paintNativeInputMark(const NativeInputMarkPaintRequest& request) {
    PaintCommand command{PaintCommandKind::NativeInputMark};
    command.nativeInputMark = request;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::paintNativeButton(const NativeButtonPaintRequest& request) {
    PaintCommand command{PaintCommandKind::NativeButton};
    command.nativeButton = request;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> topBorderGap) {
    PaintCommand command{PaintCommandKind::Box, rect, style};
    command.topBorderGap = topBorderGap;
    mCommands.push_back(std::move(command));
}

void RecordingPaintContext::paintText(const std::string& text, const Rect& rect, const Style& style) {
    mCommands.push_back({PaintCommandKind::Text, rect, style, text});
}

void RecordingPaintContext::paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) {
    mCommands.push_back({PaintCommandKind::Icon, rect, style, name, scale});
}

std::size_t RecordingPaintContext::count(PaintCommandKind kind) const {
    return static_cast<std::size_t>(
        std::count_if(mCommands.begin(), mCommands.end(), [kind](const PaintCommand& command) { return command.kind == kind; }));
}

const PaintCommand* RecordingPaintContext::last(PaintCommandKind kind) const {
    const auto found = std::find_if(mCommands.rbegin(), mCommands.rend(), [kind](const PaintCommand& command) { return command.kind == kind; });
    return found == mCommands.rend() ? nullptr : &*found;
}

void RecordingPaintContext::clear() {
    mCommands.clear();
    mClipDepth = 0;
    mMaxClipDepth = 0;
    mTranslationDepth = 0;
    mMaxTranslationDepth = 0;
}
} // namespace radia::ui
