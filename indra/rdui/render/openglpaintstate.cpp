/**
 * @file openglpaintstate.cpp
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

#include "linden_common.h"
#include "render/openglpaintstate.h"
#include <algorithm>
#include "llgl.h"
#include "llrender.h"

namespace rdui::paint {
namespace {
void applyScissor(const Rect& rect, float scale, const Vec2& render_origin, const Vec2& pixel_origin) {
    const S32 left = llfloor(pixel_origin.x + (rect.left() - render_origin.x) * scale);
    const S32 right = llceil(pixel_origin.x + (rect.right() - render_origin.x) * scale);
    const S32 bottom = llfloor(pixel_origin.y + (rect.bottom() - render_origin.y) * scale);
    const S32 top = llceil(pixel_origin.y + (rect.top() - render_origin.y) * scale);
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
}

void ClipStack::push(const Rect& rect, float scale, ClipAxes axes) {
    const float resolved_scale = std::max(0.f, scale);
    const Rect inherited = mClips.empty() ? mState.bounds : mClips.back().first;
    const Rect clipped = clipToAxes(inherited, rect, axes);
    if (mClips.empty()) {
        gGL.flush();
        glGetIntegerv(GL_SCISSOR_BOX, mPreviousScissor);
        mScissorState = std::make_unique<LLGLState>(GL_SCISSOR_TEST, LLGLState::ENABLED_STATE);
    }
    mClips.emplace_back(clipped, resolved_scale);
    gGL.flush();
    applyScissor(intersectRects(clipped, mState.bounds), resolved_scale, mState.origin, mState.pixelOrigin);
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
        LLGLDisable disable_scissor(GL_SCISSOR_TEST);
        ClearColorGuard clear_color;
        glClearColor(0.f, 0.f, 0.f, 0.f);
        mTarget.clear(GL_COLOR_BUFFER_BIT);
    }
    mMatrixGuard.emplace(capture);
    mPreviousState = mClips.beginCapture(capture);
}

EffectCaptureGuard::~EffectCaptureGuard() {
    mClips.restoreCapture(std::move(mPreviousState));
}
} // namespace rdui::paint
