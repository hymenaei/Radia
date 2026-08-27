/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "render/openglpaintstate.h"
#include <algorithm>
#include "llgl.h"
#include "llrender.h"

namespace radia::ui::paint {
namespace {
void applyScissor(const Rect& rect, float scale, const Vec2& renderOrigin, const Vec2& pixelOrigin) {
    const S32 left = llfloor(pixelOrigin.x + (rect.left() - renderOrigin.x) * scale);
    const S32 right = llceil(pixelOrigin.x + (rect.right() - renderOrigin.x) * scale);
    const S32 bottom = llfloor(pixelOrigin.y + (rect.bottom() - renderOrigin.y) * scale);
    const S32 top = llceil(pixelOrigin.y + (rect.top() - renderOrigin.y) * scale);
    glScissor(left, bottom, llmax(0, right - left), llmax(0, top - bottom));
}
} // namespace

MatrixGuard::MatrixGuard(const Rect& bounds) : mPreviousMode(gGL.getMatrixMode()) {
    gGL.matrixMode(LLRender::MM_PROJECTION);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.ortho(0.f, bounds.w, 0.f, bounds.h, -1.f, 1.f);
    gGL.matrixMode(LLRender::MM_MODELVIEW);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.pushUIMatrix();
    gGL.loadUIIdentity();
    gGL.translateUI(-bounds.x, -bounds.y, 0.f);
}

MatrixGuard::~MatrixGuard() {
    gGL.popUIMatrix();
    gGL.matrixMode(LLRender::MM_MODELVIEW);
    gGL.popMatrix();
    gGL.matrixMode(LLRender::MM_PROJECTION);
    gGL.popMatrix();
    gGL.matrixMode(mPreviousMode);
}

RenderTargetGuard::RenderTargetGuard(LLRenderTarget& target) : mTarget(target) {
    mTarget.bindTarget();
}

RenderTargetGuard::~RenderTargetGuard() {
    mTarget.flush();
}

void RenderTargetGuard::clear(U32 mask) {
    mTarget.clear(mask);
}

ClearColorGuard::ClearColorGuard() {
    glGetFloatv(GL_COLOR_CLEAR_VALUE, mColor);
}

ClearColorGuard::~ClearColorGuard() {
    glClearColor(mColor[0], mColor[1], mColor[2], mColor[3]);
}

void ClipStack::beginFrame() {
    popAllTranslations();
    if (!mClips.empty()) popAll();
    else if (mScissorState) {
        gGL.flush();
        glScissor(mPreviousScissor[0], mPreviousScissor[1], mPreviousScissor[2], mPreviousScissor[3]);
        mScissorState.reset();
    }
    GLint viewport[4]{};
    glGetIntegerv(GL_VIEWPORT, viewport);
    mState = {{0.f, 0.f},
              {static_cast<float>(viewport[0]), static_cast<float>(viewport[1])},
              {0.f, 0.f, static_cast<float>(viewport[2]), static_cast<float>(viewport[3])}};
    mTranslation = {};
}

void ClipStack::push(const Rect& rect, float scale, ClipAxes axes) {
    const float resolvedScale = std::max(0.f, scale);
    const Rect inherited = mClips.empty() ? mState.bounds : mClips.back().first;
    const Rect translated = {rect.x + mTranslation.x, rect.y + mTranslation.y, rect.w, rect.h};
    const Rect clipped = clipToAxes(inherited, translated, axes);
    if (mClips.empty()) {
        gGL.flush();
        glGetIntegerv(GL_SCISSOR_BOX, mPreviousScissor);
        mScissorState = std::make_unique<LLGLState>(GL_SCISSOR_TEST, LLGLState::ENABLED_STATE);
    }
    mClips.emplace_back(clipped, resolvedScale);
    gGL.flush();
    applyScissor(intersectRects(clipped, mState.bounds), resolvedScale, mState.origin, mState.pixelOrigin);
}

void ClipStack::pop() {
    if (mClips.empty()) return;
    gGL.flush();
    mClips.pop_back();
    if (!mClips.empty()) {
        const Rect& clip = mClips.back().first;
        const float scale = mClips.back().second;
        applyScissor(intersectRects(clip, mState.bounds), scale, mState.origin, mState.pixelOrigin);
        return;
    }
    glScissor(mPreviousScissor[0], mPreviousScissor[1], mPreviousScissor[2], mPreviousScissor[3]);
    mScissorState.reset();
}

void ClipStack::popAll() {
    while (!mClips.empty()) pop();
}

void ClipStack::pushTranslation(const Vec2& translation) {
    gGL.flush();
    gGL.pushUIMatrix();
    gGL.translateUI(translation.x, translation.y, 0.f);
    mTranslations.push_back(translation);
    mTranslation = mTranslation + translation;
}

void ClipStack::popTranslation() {
    if (mTranslations.empty()) return;
    gGL.flush();
    gGL.popUIMatrix();
    mTranslation = mTranslation - mTranslations.back();
    mTranslations.pop_back();
}

void ClipStack::popAllTranslations() {
    while (!mTranslations.empty()) popTranslation();
}

void ClipStack::reapply() {
    if (mClips.empty()) return;
    const Rect clipped = intersectRects(mClips.back().first, mState.bounds);
    applyScissor(clipped, mClips.back().second, mState.origin, mState.pixelOrigin);
}

const Rect& ClipStack::bounds() const {
    return mState.bounds;
}

PaintState ClipStack::snapshot() const {
    return mState;
}

PaintState ClipStack::beginCapture(const Rect& capture) {
    PaintState previous = mState;
    mState = {{capture.x, capture.y}, {0.f, 0.f}, capture};
    return previous;
}

void ClipStack::restoreCapture(PaintState previous) {
    mState = std::move(previous);
}

EffectCaptureGuard::EffectCaptureGuard(ClipStack& clips, LLRenderTarget& target, const Rect& capture) : mClips(clips), mTarget(target) {
    {
        LLGLDisable disableScissor(GL_SCISSOR_TEST);
        ClearColorGuard clearColor;
        glClearColor(0.f, 0.f, 0.f, 0.f);
        mTarget.clear(GL_COLOR_BUFFER_BIT);
    }
    mMatrixGuard.emplace(capture);
    mPreviousState = mClips.beginCapture(capture);
}

EffectCaptureGuard::~EffectCaptureGuard() {
    mClips.restoreCapture(std::move(mPreviousState));
}
} // namespace radia::ui::paint
