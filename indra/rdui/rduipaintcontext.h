/**
 * @file rduipaintcontext.h
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

#ifndef LL_RDUI_PAINT_CONTEXT_H
#define LL_RDUI_PAINT_CONTEXT_H

#include <optional>
#include <string>
#include "rduistyle.h"
#include "rduitextmetrics.h"

namespace rdui {
struct TopBorderGap {
    float left = 0.f;
    float right = 0.f;

    bool empty() const { return right <= left; }
};

class PaintContext : public TextMetrics {
public:
    virtual ~PaintContext() = default;

    virtual void beginFrame() {}
    virtual void endFrame() {}
    virtual void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) = 0;
    virtual void popClip() = 0;
    virtual void beginEffects(const Rect& rect, const Style& style, float scale) = 0;
    virtual void endEffects() = 0;
    virtual void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt) = 0;
    virtual void paintText(const std::string& text, const Rect& rect, const Style& style) = 0;
    virtual void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) = 0;
};
} // namespace rdui
#endif // LL_RDUI_PAINT_CONTEXT_H
