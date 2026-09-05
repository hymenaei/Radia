/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <optional>
#include <utility>
#include <vector>
#include "llgl.h"
#include "llrendertarget.h"
#include "render/painttarget.h"

namespace radia::ui::paint {
struct PaintState {
    PaintTarget target;
    Vec2 origin;
};

class MatrixGuard final {
public:
    explicit MatrixGuard(const Rect& bounds, float scale = 1.f);
    ~MatrixGuard();

    MatrixGuard(const MatrixGuard&) = delete;
    MatrixGuard& operator=(const MatrixGuard&) = delete;

private:
    LLRender::eMatrixMode mPreviousMode;
    float mScale = 1.f;
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
    void beginFrame(const PaintTarget& target);
    void push(const Rect& rect, float scale, ClipAxes axes);
    void pop();
    void popAll();
    void pushTranslation(const Vec2& translation);
    void popTranslation();
    void popAllTranslations();
    void reapply();

    const Rect& bounds() const;
    std::optional<Rect> coverageBounds() const;
    PaintState snapshot() const;
    PaintState beginCapture(const Rect& capture);
    void restoreCapture(PaintState previous);

private:
    PaintState mState;
    std::unique_ptr<LLGLState> mScissorState;
    std::vector<std::pair<Rect, float>> mClips;
    std::vector<Vec2> mTranslations;
    Vec2 mTranslation;
    GLint mPreviousScissor[4] = {};
};

class EffectCaptureGuard final {
public:
    EffectCaptureGuard(ClipStack& clips, LLRenderTarget& target, const Rect& capture, float scale);
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
