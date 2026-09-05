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
void applyScissor(const Rect& rect, float scale, const Vec2& renderOrigin, const Vec2& pixelOrigin, bool coverage) {
    const S32 left = llfloor(pixelOrigin.x + (rect.left() - renderOrigin.x) * scale);
    const S32 right = llceil(pixelOrigin.x + (rect.right() - renderOrigin.x) * scale);
    const S32 bottom = llfloor(pixelOrigin.y + (rect.bottom() - renderOrigin.y) * scale);
    const S32 top = llceil(pixelOrigin.y + (rect.top() - renderOrigin.y) * scale);
    const S32 fringe = coverage ? 1 : 0;
    glScissor(left - fringe, bottom - fringe, llmax(0, right - left + fringe * 2), llmax(0, top - bottom + fringe * 2));
}
} // namespace

MatrixGuard::MatrixGuard(const Rect& bounds, float scale) : mPreviousMode(gGL.getMatrixMode()), mScale(std::max(scale, .0001f)) {
    gGL.matrixMode(LLRender::MM_PROJECTION);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.ortho(0.f, bounds.w * mScale, 0.f, bounds.h * mScale, -1.f, 1.f);
    gGL.matrixMode(LLRender::MM_MODELVIEW);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.pushUIMatrix();
    gGL.loadUIIdentity();
    gGL.scaleUI(mScale, mScale, 1.f);
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

void ClipStack::beginFrame(const PaintTarget& target) {
    popAllTranslations();
    if (!mClips.empty()) popAll();
    else if (mScissorState) {
        gGL.flush();
        glScissor(mPreviousScissor[0], mPreviousScissor[1], mPreviousScissor[2], mPreviousScissor[3]);
        mScissorState.reset();
    }
    GLint viewport[4]{};
    glGetIntegerv(GL_VIEWPORT, viewport);
    PaintTarget resolvedTarget = target;
    if (resolvedTarget.kind == PaintTargetKind::Direct)
        resolvedTarget.pixelOrigin = {static_cast<float>(viewport[0]) + target.pixelOrigin.x, static_cast<float>(viewport[1]) + target.pixelOrigin.y};
    mState = {resolvedTarget, {target.bounds.x, target.bounds.y}};
    mTranslation = {};
}

void ClipStack::push(const Rect& rect, float scale, ClipAxes axes) {
    const float resolvedScale = std::max(0.f, scale);
    const Rect inherited = mClips.empty() ? mState.target.bounds : mClips.back().first;
    const Rect translated = {rect.x + mTranslation.x, rect.y + mTranslation.y, rect.w, rect.h};
    const Rect clipped = clipToAxes(inherited, translated, axes);
    if (mClips.empty()) {
        gGL.flush();
        glGetIntegerv(GL_SCISSOR_BOX, mPreviousScissor);
        mScissorState = std::make_unique<LLGLState>(GL_SCISSOR_TEST, LLGLState::ENABLED_STATE);
    }
    mClips.emplace_back(clipped, resolvedScale);
    gGL.flush();
    applyScissor(intersectRects(clipped, mState.target.bounds), resolvedScale, mState.origin, mState.target.pixelOrigin,
                 mState.target.clipAA == AAIntent::Coverage);
}

void ClipStack::pop() {
    if (mClips.empty()) return;
    gGL.flush();
    mClips.pop_back();
    if (!mClips.empty()) {
        const Rect& clip = mClips.back().first;
        const float scale = mClips.back().second;
        applyScissor(intersectRects(clip, mState.target.bounds), scale, mState.origin, mState.target.pixelOrigin,
                     mState.target.clipAA == AAIntent::Coverage);
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
    const Rect clipped = intersectRects(mClips.back().first, mState.target.bounds);
    applyScissor(clipped, mClips.back().second, mState.origin, mState.target.pixelOrigin, mState.target.clipAA == AAIntent::Coverage);
}

const Rect& ClipStack::bounds() const {
    return mState.target.bounds;
}

std::optional<Rect> ClipStack::coverageBounds() const {
    if (mClips.empty() || mState.target.clipAA != AAIntent::Coverage) return std::nullopt;
    const Rect logical = intersectRects(mClips.back().first, mState.target.bounds);
    const float scale = mClips.back().second;
    return Rect{mState.target.pixelOrigin.x + (logical.left() - mState.origin.x) * scale,
                mState.target.pixelOrigin.y + (logical.bottom() - mState.origin.y) * scale, logical.w * scale, logical.h * scale};
}

PaintState ClipStack::snapshot() const {
    return mState;
}

PaintState ClipStack::beginCapture(const Rect& capture) {
    PaintState previous = mState;
    PaintTarget target = mState.target;
    target.bounds = capture;
    target.pixelOrigin = {};
    target.kind = PaintTargetKind::Offscreen;
    target.opaque = false;
    mState = {target, {capture.x, capture.y}};
    return previous;
}

void ClipStack::restoreCapture(PaintState previous) {
    mState = std::move(previous);
}

EffectCaptureGuard::EffectCaptureGuard(ClipStack& clips, LLRenderTarget& target, const Rect& capture, float scale) : mClips(clips), mTarget(target) {
    {
        LLGLDisable disableScissor(GL_SCISSOR_TEST);
        ClearColorGuard clearColor;
        glClearColor(0.f, 0.f, 0.f, 0.f);
        mTarget.clear(GL_COLOR_BUFFER_BIT);
    }
    mMatrixGuard.emplace(capture, scale);
    mPreviousState = mClips.beginCapture(capture);
}

EffectCaptureGuard::~EffectCaptureGuard() {
    mClips.restoreCapture(std::move(mPreviousState));
}
} // namespace radia::ui::paint
