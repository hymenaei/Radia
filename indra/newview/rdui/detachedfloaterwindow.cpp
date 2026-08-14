/**
 * @file detachedfloaterwindow.cpp
 * @brief Presents a detached Floater in a native window and bridges its lifecycle and input.
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

#include "llviewerprecompiledheaders.h"
#include "detachedfloaterwindow.h"
#include <algorithm>
#include <chrono>
#include <utility>
#include "auxiliarywindow.h"
#include "inputbridge.h"
#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "render/openglpaintcontext.h"
#include "surface/surface.h"
#include "system.h"
#include "widgets/floater.h"
#include "widgets/panel.h"

namespace rdui::viewer {
class DetachedFloaterWindow::Impl final : public DetachedFloaterPresentation, public AuxiliaryWindowClient, private SurfaceFloaterDelegate {
public:
    using Clock = DetachedFloaterWindow::Clock;
    using TimePoint = std::chrono::steady_clock::time_point;

    Impl(AuxiliaryWindowFactory& auxiliaryWindowFactory, LLGLSLShader& uiShader, System& system, DetachedFloaterManager& manager,
         std::unique_ptr<Floater> floater, Clock now)
        : mAuxiliaryWindowFactory(auxiliaryWindowFactory), mManager(manager), mNow(std::move(now)), mPaintContext(uiShader, system),
          mSurface(system.createSurface(mPaintContext)), mFloater(floater.get()) {
        mSurface->setFloaterDelegate(this);
        mSurface->mountFloater(std::move(floater));
    }

    std::optional<DetachedFloaterPresentationUpdate> open(const DetachedFloaterPresentationOpenRequest& request) override {
        mWindow = mAuxiliaryWindowFactory.create(request.rect, mFloater->title(), *this);
        if (!mWindow) return std::nullopt;
        mWindow->setScaleMultiplier(request.scaleMultiplier);
        Vec2 size = request.logicalSize.value_or(Vec2{mFloater->rect().w, mFloater->rect().h});
        if (mFloater->canResize()) {
            const Vec2 minimum = mSurface->minimumFloaterSize(*mFloater);
            size.x = std::max(size.x, minimum.x);
            size.y = std::max(size.y, minimum.y);
        }
        mLogicalSize = size;
        mWindow->setLogicalSize(size.x, size.y);
        const AuxiliaryWindowRect actualRect = mWindow->rect();
        const float scale = mWindow->scale();
        const float width = static_cast<float>(actualRect.width) / scale;
        const float height = static_cast<float>(actualRect.height) / scale;
        mSurface->setViewport(width, height);
        mSurface->placeFloater(*mFloater, {0.f, 0.f, width, height});
        mSurface->updateLayout();
        mNativeTitle = mFloater->title();
        mWindow->show(false);
        mWindow->render();
        if (request.dragOffset) mWindow->beginDrag(request.dragOffset->x, request.dragOffset->y, request.dragCursor);
        return makeUpdate(false);
    }

    bool beginResize() override {
        if (!mWindow) return false;
        mWindow->beginResize();
        return true;
    }

    void applyResize(const Rect& logicalRect) override {
        if (!mWindow) return;
        mLogicalSize = {logicalRect.w, logicalRect.h};
        mWindow->setLogicalRect({logicalRect.x, logicalRect.y, logicalRect.w, logicalRect.h});
    }

    DetachedFloaterPresentationUpdate update() override {
        if (!mWindow) return {};
        const auto now = currentTime();
        mWindow->pump();
        if (mFloater && mFloater->title() != mNativeTitle) {
            mNativeTitle = mFloater->title();
            mWindow->setTitle(mNativeTitle);
        }
        if (mLastTick != TimePoint()) mSurface->update(std::chrono::duration_cast<std::chrono::milliseconds>(now - mLastTick));
        mLastTick = now;
        if (!mCloseRequested && !mMinimizeRequested && (mSurface->needsPaint() || nativePresentationChanged())) mWindow->render();

        return makeUpdate(true);
    }

    void setVisible(bool visible) override {
        if (mWindow) mWindow->setVisible(visible);
        if (!visible) mSurface->clearInteractionState();
    }

    std::optional<Rect> prepareReplacement(Floater& replacement) override { return mSurface->prepareFloater(replacement); }

    std::unique_ptr<Floater> releaseFloater() override {
        if (!mFloater) return nullptr;
        std::unique_ptr<Floater> floater = mSurface->unmountFloater(*mFloater);
        mFloater.set(nullptr);
        return floater;
    }

    std::unique_ptr<Floater> replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize) override {
        if (!mFloater || !replacement) return nullptr;
        Floater* installed = replacement.get();
        std::unique_ptr<Floater> retired;
        retired = mSurface->replaceFloater(*mFloater, std::move(replacement));
        if (!retired) return nullptr;
        mFloater.set(installed);
        if (logicalSize && mWindow) {
            Vec2 size = *logicalSize;
            if (installed->canResize()) {
                const Vec2 minimum = mSurface->minimumFloaterSize(*installed);
                size.x = std::max(size.x, minimum.x);
                size.y = std::max(size.y, minimum.y);
            }
            mLogicalSize = size;
            mWindow->setLogicalSize(size.x, size.y);
            const AuxiliaryWindowRect native = mWindow->rect();
            const float scale = mWindow->scale();
            mSurface->setViewport(static_cast<float>(native.width) / scale, static_cast<float>(native.height) / scale);
        }
        installed->setRect({0.f, 0.f, mSurface->width(), mSurface->height()});
        Floater& mounted = *installed;
        mSurface->placeFloater(mounted, {0.f, 0.f, mSurface->width(), mSurface->height()});
        mSurface->updateLayout();
        mNativeTitle.clear();
        return retired;
    }

    AuxiliaryInputResult pointerMove(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) override {
        const bool handled = mSurface->pointerMove(translatePointerInput({x, y, translateButton(button), modifiers, clickCount, deltaX, deltaY}));
        return {handled, handled ? std::optional<ECursorType>(translateCursor(mSurface->cursor())) : std::nullopt};
    }

    AuxiliaryInputResult pointerDown(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) override {
        const PointerEvent event = translatePointerInput({x, y, translateButton(button), modifiers, clickCount, deltaX, deltaY});
        const bool handled = mSurface->pointerDown(event);
        if (event.button == PointerButton::Left && mFloater->dragging()) {
            mSurface->clearInteractionState();
            if (mWindow) mWindow->beginDrag(event.position.x, event.position.y);
            return {true, UI_CURSOR_ARROW};
        }
        return {handled, handled ? std::optional<ECursorType>(translateCursor(mSurface->cursor())) : std::nullopt};
    }

    AuxiliaryInputResult pointerUp(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) override {
        return {mSurface->pointerUp(translatePointerInput({x, y, translateButton(button), modifiers, clickCount, deltaX, deltaY})), std::nullopt};
    }

    void pointerLeave() override { mSurface->pointerLeave(); }

    AuxiliaryInputResult scroll(S32 x, S32 y, F32 horizontal, F32 vertical, MASK modifiers) override {
        return {mSurface->scroll(translateScrollInput({x, y, horizontal, vertical, modifiers})), std::nullopt};
    }

    AuxiliaryInputResult keyDown(KEY key, MASK modifiers, bool repeated) override {
        return {mSurface->keyDown(translateKeyInput({key, modifiers, true, repeated})), std::nullopt};
    }

    AuxiliaryInputResult keyUp(KEY key, MASK modifiers) override {
        return {mSurface->keyUp(translateKeyInput({key, modifiers, false, false})), std::nullopt};
    }

    AuxiliaryInputResult character(U32 codepoint, MASK) override { return {mSurface->charInput(codepoint), std::nullopt}; }

    void interactionLost(AuxiliaryInteractionLoss) override { mSurface->clearInteractionState(); }

    void paint(S32 pixelWidth, S32 pixelHeight, F32 scale) override {
        if (!mFloater || pixelWidth <= 0 || pixelHeight <= 0) return;
        mLastPaintWidth = pixelWidth;
        mLastPaintHeight = pixelHeight;
        mLastPaintScale = scale;
        const float width = static_cast<float>(pixelWidth) / scale;
        const float height = static_cast<float>(pixelHeight) / scale;
        if (mSurface->width() != width || mSurface->height() != height) {
            mSurface->setViewport(width, height);
            mSurface->placeFloater(*mFloater, {0.f, 0.f, width, height});
            mSurface->updateLayout();
        }

        GLint framebuffer = 0;
        GLint viewport[4]{};
        GLfloat clearColor[4]{};
        GLboolean colorMask[4]{};
        glGetIntegerv(GL_FRAMEBUFFER_BINDING, &framebuffer);
        glGetIntegerv(GL_VIEWPORT, viewport);
        glGetFloatv(GL_COLOR_CLEAR_VALUE, clearColor);
        glGetBooleanv(GL_COLOR_WRITEMASK, colorMask);
        const GLboolean scissorEnabled = glIsEnabled(GL_SCISSOR_TEST);
        glViewport(0, 0, pixelWidth, pixelHeight);
        glDisable(GL_SCISSOR_TEST);
        glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
        glClearColor(0.f, 0.f, 0.f, 0.f);
        glClear(GL_COLOR_BUFFER_BIT);
        if (scissorEnabled) glEnable(GL_SCISSOR_TEST);

        const LLRender::eMatrixMode previousMode = gGL.getMatrixMode();
        gGL.matrixMode(LLRender::MM_PROJECTION);
        gGL.pushMatrix();
        gGL.loadIdentity();
        gGL.ortho(0.f, width, 0.f, height, -1.f, 1.f);
        gGL.matrixMode(LLRender::MM_MODELVIEW);
        gGL.pushMatrix();
        gGL.loadIdentity();
        gGL.pushUIMatrix();
        gGL.loadUIIdentity();
        gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
        mSurface->refreshHover();
        mSurface->paint(mPaintContext, scale);
        gGL.setSceneBlendType(LLRender::BT_ALPHA);
        gGL.popUIMatrix();
        gGL.popMatrix();
        gGL.matrixMode(LLRender::MM_PROJECTION);
        gGL.popMatrix();
        gGL.matrixMode(previousMode);
        gGL.flush();

        glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
        glViewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        glColorMask(colorMask[0], colorMask[1], colorMask[2], colorMask[3]);
        glClearColor(clearColor[0], clearColor[1], clearColor[2], clearColor[3]);
    }

    void closeRequested() override {
        if (mFloater) mFloater->close();
    }

    void dragEnded() override { mDragEnded = true; }

    void resizeEnded(F32 logicalWidth, F32 logicalHeight) override {
        mLogicalSize = {logicalWidth, logicalHeight};
        mResizeEnded = true;
    }

    DetachedFloaterPresentationUpdate makeUpdate(bool consumeEvents) {
        DetachedFloaterPresentationUpdate result;
        result.closeRequested = mCloseRequested;
        result.minimizeRequested = mMinimizeRequested;
        result.dragEnded = consumeEvents ? std::exchange(mDragEnded, false) : false;
        result.resizeEnded = consumeEvents ? std::exchange(mResizeEnded, false) : false;
        result.nativeRect = nativeRect();
        result.logicalSize = mLogicalSize;
        result.headerCenterScreen = headerCenterScreen();
        return result;
    }

    AuxiliaryWindowRect nativeRect() const { return mWindow ? mWindow->rect() : AuxiliaryWindowRect{}; }

    Vec2 headerCenterScreen() const {
        if (!mWindow || !mFloater || !mFloater->header()) return {};
        const AuxiliaryWindowRect windowRect = mWindow->rect();
        const Rect header = mFloater->header()->rect();
        return {windowRect.x + (header.x + header.w * 0.5f) * mWindow->scale(),
                windowRect.y + (mSurface->height() - header.y - header.h * 0.5f) * mWindow->scale()};
    }

private:
    TimePoint currentTime() const { return mNow ? mNow() : std::chrono::steady_clock::now(); }

    void floaterClosed(Surface&, Floater&) override { mCloseRequested = true; }

    void floaterMinimizedChanged(Surface&, Floater&, bool minimized) override {
        if (minimized) mMinimizeRequested = true;
    }

    bool beginNativeFloaterResize(Surface&, Floater&) override { return mFloater && mManager.beginResize(*mFloater); }

    void floaterResized(Surface&, Floater& floater, bool) override { mManager.applyResize(floater, floater.rect()); }

    bool nativePresentationChanged() const {
        if (!mWindow) return false;
        const AuxiliaryWindowRect native = mWindow->rect();
        return native.width != mLastPaintWidth || native.height != mLastPaintHeight || mWindow->scale() != mLastPaintScale;
    }

    static NativePointerButton translateButton(AuxiliaryPointerButton button) {
        switch (button) {
            case AuxiliaryPointerButton::Left: return NativePointerButton::Left;
            case AuxiliaryPointerButton::Right: return NativePointerButton::Right;
            case AuxiliaryPointerButton::Middle: return NativePointerButton::Middle;
            case AuxiliaryPointerButton::Auxiliary1: return NativePointerButton::Auxiliary1;
            case AuxiliaryPointerButton::Auxiliary2: return NativePointerButton::Auxiliary2;
            default: return NativePointerButton::NoButton;
        }
    }

    AuxiliaryWindowFactory& mAuxiliaryWindowFactory;
    DetachedFloaterManager& mManager;
    Clock mNow;
    OpenGLPaintContext mPaintContext;
    std::unique_ptr<Surface> mSurface;
    WidgetRef<Floater> mFloater;
    std::unique_ptr<AuxiliaryWindow> mWindow;
    bool mCloseRequested = false;
    bool mMinimizeRequested = false;
    bool mDragEnded = false;
    bool mResizeEnded = false;
    S32 mLastPaintWidth = 0;
    S32 mLastPaintHeight = 0;
    F32 mLastPaintScale = 0.f;
    std::string mNativeTitle;
    Vec2 mLogicalSize;
    TimePoint mLastTick;
};

DetachedFloaterWindow::DetachedFloaterWindow(AuxiliaryWindowFactory& auxiliaryWindowFactory, LLGLSLShader& uiShader, System& system,
                                             DetachedFloaterManager& manager, std::unique_ptr<Floater> floater, Clock now)
    : mImpl(std::make_unique<Impl>(auxiliaryWindowFactory, uiShader, system, manager, std::move(floater), std::move(now))) {}

DetachedFloaterWindow::~DetachedFloaterWindow() = default;

std::optional<DetachedFloaterPresentationUpdate> DetachedFloaterWindow::open(const DetachedFloaterPresentationOpenRequest& request) {
    return mImpl->open(request);
}

bool DetachedFloaterWindow::beginResize() {
    return mImpl->beginResize();
}
void DetachedFloaterWindow::applyResize(const Rect& logicalRect) {
    mImpl->applyResize(logicalRect);
}
DetachedFloaterPresentationUpdate DetachedFloaterWindow::update() {
    return mImpl->update();
}
void DetachedFloaterWindow::setVisible(bool visible) {
    mImpl->setVisible(visible);
}
std::optional<Rect> DetachedFloaterWindow::prepareReplacement(Floater& replacement) {
    return mImpl->prepareReplacement(replacement);
}
std::unique_ptr<Floater> DetachedFloaterWindow::releaseFloater() {
    return mImpl->releaseFloater();
}
std::unique_ptr<Floater> DetachedFloaterWindow::replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize) {
    return mImpl->replaceFloater(std::move(replacement), logicalSize);
}
} // namespace rdui::viewer
