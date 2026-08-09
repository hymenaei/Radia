/**
 * @file recordingpaintcontext.cpp
 * @brief Records UI paint commands for inspection and tests.
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
#include "render/recordingpaintcontext.h"
#include <algorithm>

namespace rdui {
Vec2 RecordingPaintContext::measureText(const std::string& text, const Style& style) const {
    return mTextMetrics.measureText(text, style);
}

float RecordingPaintContext::usedLetterSpacing(const Style& style) const {
    return mTextMetrics.usedLetterSpacing(style);
}

void RecordingPaintContext::beginFrame() {
    mCommands.push_back({PaintCommandKind::BeginFrame});
}

void RecordingPaintContext::endFrame() {
    mCommands.push_back({PaintCommandKind::EndFrame});
}

void RecordingPaintContext::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    PaintCommand command{PaintCommandKind::PushClip, rect, {}, {}, scale};
    command.clip_axes = axes;
    mCommands.push_back(std::move(command));
    ++mClipDepth;
    mMaxClipDepth = std::max(mMaxClipDepth, mClipDepth);
}

void RecordingPaintContext::popClip() {
    mCommands.push_back({PaintCommandKind::PopClip});
    --mClipDepth;
}

void RecordingPaintContext::beginEffects(const Rect& rect, const Style& style, float scale) {
    mCommands.push_back({PaintCommandKind::BeginEffects, rect, style, {}, scale});
}

void RecordingPaintContext::endEffects() {
    mCommands.push_back({PaintCommandKind::EndEffects});
}

void RecordingPaintContext::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    PaintCommand command{PaintCommandKind::Box, rect, style};
    command.top_border_gap = top_border_gap;
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
}
} // namespace rdui
