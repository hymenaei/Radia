/**
 * @file rduiopenglpaintcontext.h
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

#ifndef LL_RDUI_OPENGL_PAINT_CONTEXT_H
#define LL_RDUI_OPENGL_PAINT_CONTEXT_H

#include <memory>
#include <string>
#include "rduipaintcontext.h"

class LLGLSLShader;
class LLRenderTarget;

namespace rdui {
struct Mesh;
class System;

class OpenGLPaintContext final : public PaintContext {
public:
    OpenGLPaintContext(::LLGLSLShader& shape_program, const System& system);
    ~OpenGLPaintContext() override;

    void beginFrame() override;
    void endFrame() override;
    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;
    void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) override;
    void popClip() override;
    void beginEffects(const Rect& rect, const Style& style, float scale) override;
    void endEffects() override;
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt) override;
    void paintText(const std::string& text, const Rect& rect, const Style& style) override;
    void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) override;

private:
    struct Impl;
    void drawMesh(const Mesh& mesh);
    void drawRoundedShape(int mode, const Rect& rect, float radius, float border_width, const Color& color,
                          OutlineStyle outline_style = OutlineStyle::Solid, std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawRoundedGradient(const Rect& rect, float radius, const Gradient& gradient, const EdgeInsets* border_widths = nullptr,
                             std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawShadow(const Rect& rect, float radius, const BoxShadow& shadow);
    bool captureFramebuffer(const Rect& capture, float scale, ::LLRenderTarget& target);
    ::LLRenderTarget* applyBlur(::LLRenderTarget& source, ::LLRenderTarget& horizontal_target, ::LLRenderTarget& vertical_target, const Rect& capture,
                                const Rect& effect_rect, const Effect& effect, float scale);
    void compositeEffect(::LLRenderTarget& source, const Rect& capture, const Rect& destination, float radius, bool rounded_mask);
    void pushEffectMatrices(const Rect& bounds);
    void popEffectMatrices();
    void reapplyClip();
    void drawBorder(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawOutline(const Rect& rect, const Style& style);
    static void drawShapeQuad(const Rect& rect);
    void prepareVectorDraw();
    static void prepareTextDraw();

    ::LLGLSLShader& mShapeProgram;
    const System& mSystem;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui
#endif // LL_RDUI_OPENGL_PAINT_CONTEXT_H
