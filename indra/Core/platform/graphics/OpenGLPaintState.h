/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <optional>
#include <utility>
#include <vector>
#include "PaintTarget.h"
#include "llgl.h"
#include "llrendertarget.h"

namespace Core::detail {
struct PaintState {
    PaintTarget target;
    Layout::Vec2 origin;
};

class MatrixGuard final {
public:
    explicit MatrixGuard(const Layout::Rect& bounds, float scale = 1.f);
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
    GLfloat mColor[4] {};
};

class ClipStack final {
public:
    void beginFrame(const PaintTarget& target);
    void push(const Layout::Rect& rect, float scale, Layout::ClipAxes axes);
    void pop();
    void popAll();
    void pushTranslation(const Layout::Vec2& translation);
    void popTranslation();
    void popAllTranslations();
    void reapply();

    const Layout::Rect& bounds() const;
    PaintTargetKind targetKind() const { return mState.target.kind; }
    const Layout::Vec2& translation() const { return mTranslation; }
    float pixelScale() const { return mClips.empty() ? mState.target.scale : mClips.back().second; }
    Layout::Rect pixelRect() const;
    std::optional<Layout::Rect> coverageBounds() const;
    PaintState snapshot() const;
    PaintState beginCapture(const Layout::Rect& capture);
    void restoreCapture(PaintState previous);

private:
    Layout::Rect clipInTargetSpace(const Layout::Rect& clip) const;

    PaintState mState;
    std::unique_ptr<LLGLState> mScissorState;
    std::vector<std::pair<Layout::Rect, float>> mClips;
    std::vector<Layout::Vec2> mTranslations;
    Layout::Vec2 mTranslation;
    GLint mPreviousScissor[4] = {};
};

class EffectCaptureGuard final {
public:
    EffectCaptureGuard(ClipStack& clips, LLRenderTarget& target, const Layout::Rect& capture, float scale);
    ~EffectCaptureGuard();

    EffectCaptureGuard(const EffectCaptureGuard&) = delete;
    EffectCaptureGuard& operator=(const EffectCaptureGuard&) = delete;

private:
    ClipStack& mClips;
    PaintState mPreviousState {};
    RenderTargetGuard mTarget;
    std::optional<MatrixGuard> mMatrixGuard;
};
} // namespace Core::detail
