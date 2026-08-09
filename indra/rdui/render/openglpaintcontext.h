/**
 * @file openglpaintcontext.h
 * @brief Implements retained UI painting on the OpenGL renderer.
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

#ifndef RD_RENDER_OPENGLPAINTCONTEXT_H
#define RD_RENDER_OPENGLPAINTCONTEXT_H

#include <memory>
#include <string>
#include "render/paintcontext.h"

class LLGLSLShader;

namespace rdui {
class System;

class OpenGLPaintContext final : public PaintContext {
public:
    OpenGLPaintContext(::LLGLSLShader& shape_program, const System& system);
    ~OpenGLPaintContext() override;

    void beginFrame() override;
    void endFrame() override;
    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;
    std::uint64_t generation() const override;
    void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) override;
    void popClip() override;
    void beginEffects(const Rect& rect, const Style& style, float scale) override;
    void endEffects() override;
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt) override;
    void paintText(const std::string& text, const Rect& rect, const Style& style) override;
    void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) override;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui
#endif // RD_RENDER_OPENGLPAINTCONTEXT_H
