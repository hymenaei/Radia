/**
 * @file openglpaintstate.h
 * @brief Private OpenGL paint-state guards and clip/capture state.
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

#ifndef RD_RENDER_OPENGLPAINTSTATE_H
#define RD_RENDER_OPENGLPAINTSTATE_H

#include <memory>
#include <optional>
#include <utility>
#include <vector>
#include "llgl.h"
#include "llrendertarget.h"
#include "types.h"

namespace radia::ui::paint {
struct PaintState {
    Vec2 origin;
    Vec2 pixelOrigin;
    Rect bounds;
};

class MatrixGuard final {
public:
    explicit MatrixGuard(const Rect& bounds);
    ~MatrixGuard();

    MatrixGuard(const MatrixGuard&) = delete;
    MatrixGuard& operator=(const MatrixGuard&) = delete;

private:
    LLRender::eMatrixMode mPreviousMode;
};

class RenderTargetGuard final {
public:
    explicit RenderTargetGuard(LLRenderTarget& target);
    ~RenderTargetGuard();

    void clear(U32 mask);

    RenderTargetGuard(const RenderTargetGuard&) = delete;
    RenderTargetGuard& operator=(const RenderTargetGuard&) = delete;

private:
    LLRenderTarget& mTarget;
};

class ClearColorGuard final {
public:
    ClearColorGuard();
    ~ClearColorGuard();

    ClearColorGuard(const ClearColorGuard&) = delete;
    ClearColorGuard& operator=(const ClearColorGuard&) = delete;

private:
    GLfloat mColor[4]{};
};

class ClipStack final {
public:
    void beginFrame();
    void push(const Rect& rect, float scale, ClipAxes axes);
    void pop();
    void popAll();
    void reapply();

    const Rect& bounds() const;
    PaintState snapshot() const;
    PaintState beginCapture(const Rect& capture);
    void restoreCapture(PaintState previous);

private:
    PaintState mState;
    std::unique_ptr<LLGLState> mScissorState;
    std::vector<std::pair<Rect, float>> mClips;
    GLint mPreviousScissor[4] = {};
};

class EffectCaptureGuard final {
public:
    EffectCaptureGuard(ClipStack& clips, LLRenderTarget& target, const Rect& capture);
    ~EffectCaptureGuard();

    EffectCaptureGuard(const EffectCaptureGuard&) = delete;
    EffectCaptureGuard& operator=(const EffectCaptureGuard&) = delete;

private:
    ClipStack& mClips;
    PaintState mPreviousState{};
    RenderTargetGuard mTarget;
    std::optional<MatrixGuard> mMatrixGuard;
};
} // namespace radia::ui::paint
#endif // RD_RENDER_OPENGLPAINTSTATE_H
