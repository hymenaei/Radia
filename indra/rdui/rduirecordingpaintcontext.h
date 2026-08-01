/**
 * @file rduirecordingpaintcontext.h
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

#ifndef LL_RDUI_RECORDING_PAINT_CONTEXT_H
#define LL_RDUI_RECORDING_PAINT_CONTEXT_H

#include <cstddef>
#include <string>
#include <vector>
#include "rduipaintcontext.h"

namespace rdui {
enum class PaintCommandKind { BeginFrame, EndFrame, PushClip, PopClip, BeginEffects, EndEffects, Box, Text, Icon };

struct PaintCommand {
    PaintCommandKind kind;
    Rect rect;
    Style style;
    std::string value;
    float scale = 1.f;
    ClipAxes clip_axes = ClipAxes::Both;
    std::optional<TopBorderGap> top_border_gap;
};

class RecordingPaintContext final : public PaintContext {
public:
    explicit RecordingPaintContext(const TextMetrics& text_metrics = fixedTextMetrics()) : mTextMetrics(text_metrics) {}

    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;
    void beginFrame() override;
    void endFrame() override;
    void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) override;
    void popClip() override;
    void beginEffects(const Rect& rect, const Style& style, float scale) override;
    void endEffects() override;
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt) override;
    void paintText(const std::string& text, const Rect& rect, const Style& style) override;
    void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) override;

    const std::vector<PaintCommand>& commands() const { return mCommands; }
    std::size_t count(PaintCommandKind kind) const;
    const PaintCommand* last(PaintCommandKind kind) const;
    int clipDepth() const { return mClipDepth; }
    int maxClipDepth() const { return mMaxClipDepth; }
    void clear();

private:
    const TextMetrics& mTextMetrics;
    std::vector<PaintCommand> mCommands;
    int mClipDepth = 0;
    int mMaxClipDepth = 0;
};
} // namespace rdui
#endif // LL_RDUI_RECORDING_PAINT_CONTEXT_H
