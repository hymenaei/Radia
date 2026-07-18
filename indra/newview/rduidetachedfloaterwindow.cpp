#include "llviewerprecompiledheaders.h"
#include "rduidetachedfloaterwindow.h"

#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llviewershadermgr.h"
#include "rdfloater.h"
#include "rdpanel.h"
#include "rduiinputbridge.h"
#include "rduinativewindow.h"
#include "rduiopenglpaintcontext.h"
#include "rduisurface.h"
#include "rduisystem.h"

#include <algorithm>
#include <chrono>
#include <utility>

namespace rdui::viewer
{
    namespace
    {
        const RduiInputBridge INPUT_BRIDGE;
    }

    class DetachedFloaterWindow::Impl final : public DetachedFloaterPresentation,
                                               public NativeWindowClient,
                                               private SurfaceFloaterDelegate
    {
        public:
            Impl(NativeWindowFactory& native_windows, System& system,
                 DetachedFloaterManager& manager, std::unique_ptr<Floater> floater)
                : mNativeWindows(native_windows), mManager(manager),
                  mPaintContext(gRduiProgram, system),
                  mSurface(system.createSurface(mPaintContext)), mFloater(floater.get())
            {
                mSurface->setFloaterDelegate(this);
                mSurface->mountFloater(std::move(floater));
            }

            bool open(const NativeRect& rect, float scale_multiplier,
                      const std::optional<Vec2>& drag_offset,
                      const std::optional<Vec2>& logical_size,
                      const std::optional<NativePoint>& drag_cursor) override
            {
                mWindow = mNativeWindows.create(rect, mFloater->title(), *this);
                if (!mWindow) return false;
                mWindow->setScaleMultiplier(scale_multiplier);
                Vec2 size = logical_size.value_or(Vec2{mFloater->rect().w, mFloater->rect().h});
                if (mFloater->canResize())
                {
                    const Vec2 minimum = mSurface->minimumFloaterSize(*mFloater);
                    size.x = std::max(size.x, minimum.x);
                    size.y = std::max(size.y, minimum.y);
                }
                mLogicalSize = size;
                mWindow->setLogicalSize(size.x, size.y);
                const NativeRect actual_rect = mWindow->rect();
                const float scale = mWindow->scale();
                const float width = static_cast<float>(actual_rect.width) / scale;
                const float height = static_cast<float>(actual_rect.height) / scale;
                mSurface->setViewport(width, height);
                mSurface->placeFloater(*mFloater, {0.f, 0.f, width, height});
                mSurface->updateLayout();
                mNativeTitle = mFloater->title();
                mWindow->show(false);
                mWindow->render();
                if (drag_offset) mWindow->beginDrag(drag_offset->x, drag_offset->y, drag_cursor);
                return true;
            }

            bool beginResize() override
            {
                if (!mWindow) return false;
                mWindow->beginResize();
                return true;
            }

            void applyResize(const Rect& logical_rect) override
            {
                if (!mWindow) return;
                mLogicalSize = {logical_rect.w, logical_rect.h};
                mWindow->setLogicalRect(logical_rect);
            }

            void tick() override
            {
                if (!mWindow) return;
                const auto now = std::chrono::steady_clock::now();
                mWindow->pump();
                if (mFloater && mFloater->title() != mNativeTitle)
                {
                    mNativeTitle = mFloater->title();
                    mWindow->setTitle(mNativeTitle);
                }
                if (mLastTick != std::chrono::steady_clock::time_point())
                    mSurface->update(std::chrono::duration_cast<std::chrono::milliseconds>(now - mLastTick));
                mLastTick = now;
                if (!mCloseRequested && !mMinimizeRequested
                    && (mSurface->needsPaint() || nativePresentationChanged()))
                    mWindow->render();
            }

            void setVisible(bool visible) override
            {
                if (mWindow) mWindow->setVisible(visible);
                if (!visible) mSurface->clearInteractionState();
            }

            std::unique_ptr<Floater> releaseFloater() override
            {
                if (!mFloater) return nullptr;
                std::unique_ptr<Floater> floater = mSurface->unmountFloater(*mFloater);
                mFloater.set(nullptr);
                return floater;
            }

            Floater& replaceFloater(std::unique_ptr<Floater> replacement,
                                    const std::optional<Vec2>& logical_size) override
            {
                if (mFloater) mSurface->unmountFloater(*mFloater);
                if (logical_size && mWindow)
                {
                    Vec2 size = *logical_size;
                    if (replacement->canResize())
                    {
                        const Vec2 minimum = mSurface->minimumFloaterSize(*replacement);
                        size.x = std::max(size.x, minimum.x);
                        size.y = std::max(size.y, minimum.y);
                    }
                    mLogicalSize = size;
                    mWindow->setLogicalSize(size.x, size.y);
                    const NativeRect native = mWindow->rect();
                    const float scale = mWindow->scale();
                    mSurface->setViewport(static_cast<float>(native.width) / scale,
                                          static_cast<float>(native.height) / scale);
                }
                replacement->setRect({0.f, 0.f, mSurface->width(), mSurface->height()});
                Floater& mounted = mSurface->mountFloater(std::move(replacement));
                mSurface->placeFloater(mounted, {0.f, 0.f, mSurface->width(), mSurface->height()});
                mSurface->updateLayout();
                mFloater.set(&mounted);
                mNativeTitle.clear();
                return mounted;
            }

            NativeInputDispatchResult dispatchNative(const NativeInputEvent& event) override
            {
                const SurfaceInputEvent translated = INPUT_BRIDGE.translate(event);
                if (const auto* pointer = std::get_if<SurfacePointerInput>(&translated))
                {
                    if (pointer->phase == NativePointerPhase::Leave)
                    {
                        mSurface->pointerLeave();
                        return {};
                    }
                    const bool handled = pointer->phase == NativePointerPhase::Move
                        ? mSurface->pointerMove(pointer->event)
                        : pointer->phase == NativePointerPhase::Down
                            ? mSurface->pointerDown(pointer->event)
                            : mSurface->pointerUp(pointer->event);
                    if (pointer->phase == NativePointerPhase::Down
                        && pointer->event.button == PointerButton::Left
                        && mFloater->dragging())
                    {
                        mSurface->clearInteractionState();
                        if (mWindow) mWindow->beginDrag(pointer->event.position.x, pointer->event.position.y);
                        return {true, UI_CURSOR_ARROW};
                    }
                    return {handled, handled
                        ? std::optional<ECursorType>(INPUT_BRIDGE.translateCursor(mSurface->cursor()))
                        : std::nullopt};
                }
                if (const auto* scroll = std::get_if<ScrollEvent>(&translated))
                    return {mSurface->scroll(*scroll), std::nullopt};
                if (const auto* key = std::get_if<SurfaceKeyInput>(&translated))
                    return {key->down ? mSurface->keyDown(key->event) : mSurface->keyUp(key->event), std::nullopt};
                if (const auto* character = std::get_if<SurfaceCharacterInput>(&translated))
                    return {mSurface->charInput(character->codepoint), std::nullopt};
                if (std::holds_alternative<NativeInteractionLoss>(translated))
                    mSurface->clearInteractionState();
                return {};
            }

            void paintNative(S32 pixel_width, S32 pixel_height, F32 scale) override
            {
                if (!mFloater || pixel_width <= 0 || pixel_height <= 0) return;
                mLastPaintWidth = pixel_width;
                mLastPaintHeight = pixel_height;
                mLastPaintScale = scale;
                const float width = static_cast<float>(pixel_width) / scale;
                const float height = static_cast<float>(pixel_height) / scale;
                if (mSurface->width() != width || mSurface->height() != height)
                {
                    mSurface->setViewport(width, height);
                    mSurface->placeFloater(*mFloater, {0.f, 0.f, width, height});
                    mSurface->updateLayout();
                }

                GLint framebuffer = 0;
                GLint viewport[4]{};
                GLfloat clear_color[4]{};
                GLboolean color_mask[4]{};
                glGetIntegerv(GL_FRAMEBUFFER_BINDING, &framebuffer);
                glGetIntegerv(GL_VIEWPORT, viewport);
                glGetFloatv(GL_COLOR_CLEAR_VALUE, clear_color);
                glGetBooleanv(GL_COLOR_WRITEMASK, color_mask);
                const GLboolean scissor_enabled = glIsEnabled(GL_SCISSOR_TEST);
                glViewport(0, 0, pixel_width, pixel_height);
                glDisable(GL_SCISSOR_TEST);
                glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
                glClearColor(0.f, 0.f, 0.f, 0.f);
                glClear(GL_COLOR_BUFFER_BIT);
                if (scissor_enabled) glEnable(GL_SCISSOR_TEST);

                const LLRender::eMatrixMode previous_mode = gGL.getMatrixMode();
                gGL.matrixMode(LLRender::MM_PROJECTION);
                gGL.pushMatrix();
                gGL.loadIdentity();
                gGL.ortho(0.f, width, 0.f, height, -1.f, 1.f);
                gGL.matrixMode(LLRender::MM_MODELVIEW);
                gGL.pushMatrix();
                gGL.loadIdentity();
                gGL.pushUIMatrix();
                gGL.loadUIIdentity();
                gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA,
                              LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
                mSurface->refreshHover();
                mSurface->paint(mPaintContext, scale);
                gGL.setSceneBlendType(LLRender::BT_ALPHA);
                gGL.popUIMatrix();
                gGL.popMatrix();
                gGL.matrixMode(LLRender::MM_PROJECTION);
                gGL.popMatrix();
                gGL.matrixMode(previous_mode);
                gGL.flush();

                glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
                glViewport(viewport[0], viewport[1], viewport[2], viewport[3]);
                glColorMask(color_mask[0], color_mask[1], color_mask[2], color_mask[3]);
                glClearColor(clear_color[0], clear_color[1], clear_color[2], clear_color[3]);
            }

            void closeNative() override
            {
                if (mFloater) mFloater->close();
            }

            void nativeDragEnded() override { mDragEnded = true; }

            void nativeResizeEnded(F32 logical_width, F32 logical_height) override
            {
                mLogicalSize = {logical_width, logical_height};
                mResizeEnded = true;
            }

            bool closeRequested() const override { return mCloseRequested; }
            bool minimizeRequested() const override { return mMinimizeRequested; }
            bool takeDragEnded() override { return std::exchange(mDragEnded, false); }
            bool takeResizeEnded() override { return std::exchange(mResizeEnded, false); }
            Floater* floater() const override { return mFloater.get(); }
            NativeRect nativeRect() const override { return mWindow ? mWindow->rect() : NativeRect{}; }
            std::string monitorId() const override { return mWindow ? mWindow->monitorId() : std::string(); }
            Vec2 logicalSize() const override { return mLogicalSize; }

            Vec2 headerCenterScreen() const override
            {
                if (!mWindow || !mFloater || !mFloater->header()) return {};
                const NativeRect window_rect = mWindow->rect();
                const Rect header = mFloater->header()->rect();
                return {window_rect.x + (header.x + header.w * 0.5f) * mWindow->scale(),
                        window_rect.y + (mSurface->height() - header.y - header.h * 0.5f) * mWindow->scale()};
            }

        private:
            void floaterClosed(Surface&, Floater&) override { mCloseRequested = true; }

            void floaterMinimizedChanged(Surface&, Floater&, bool minimized) override
            {
                if (minimized) mMinimizeRequested = true;
            }

            bool beginNativeFloaterResize(Surface&, Floater&) override
            {
                return mFloater && mManager.beginResize(*mFloater);
            }

            void floaterResized(Surface&, Floater& floater, bool) override
            {
                mManager.applyResize(floater, floater.rect());
            }

            bool nativePresentationChanged() const
            {
                if (!mWindow) return false;
                const NativeRect native = mWindow->rect();
                return native.width != mLastPaintWidth || native.height != mLastPaintHeight
                    || mWindow->scale() != mLastPaintScale;
            }

            NativeWindowFactory& mNativeWindows;
            DetachedFloaterManager& mManager;
            OpenGLPaintContext mPaintContext;
            std::unique_ptr<Surface> mSurface;
            WidgetRef<Floater> mFloater;
            std::unique_ptr<NativeWindow> mWindow;
            bool mCloseRequested = false;
            bool mMinimizeRequested = false;
            bool mDragEnded = false;
            bool mResizeEnded = false;
            S32 mLastPaintWidth = 0;
            S32 mLastPaintHeight = 0;
            F32 mLastPaintScale = 0.f;
            std::string mNativeTitle;
            Vec2 mLogicalSize;
            std::chrono::steady_clock::time_point mLastTick;
    };

    DetachedFloaterWindow::DetachedFloaterWindow(
        NativeWindowFactory& native_windows, System& system,
        DetachedFloaterManager& manager, std::unique_ptr<Floater> floater)
        : mImpl(std::make_unique<Impl>(native_windows, system, manager, std::move(floater))) {}

    DetachedFloaterWindow::~DetachedFloaterWindow() = default;

    bool DetachedFloaterWindow::open(const NativeRect& rect, float scale_multiplier,
                                     const std::optional<Vec2>& drag_offset,
                                     const std::optional<Vec2>& logical_size,
                                     const std::optional<NativePoint>& drag_cursor)
    {
        return mImpl->open(rect, scale_multiplier, drag_offset, logical_size, drag_cursor);
    }

    bool DetachedFloaterWindow::beginResize() { return mImpl->beginResize(); }
    void DetachedFloaterWindow::applyResize(const Rect& logical_rect) { mImpl->applyResize(logical_rect); }
    void DetachedFloaterWindow::tick() { mImpl->tick(); }
    void DetachedFloaterWindow::setVisible(bool visible) { mImpl->setVisible(visible); }
    std::unique_ptr<Floater> DetachedFloaterWindow::releaseFloater() { return mImpl->releaseFloater(); }
    Floater& DetachedFloaterWindow::replaceFloater(
        std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logical_size)
    {
        return mImpl->replaceFloater(std::move(replacement), logical_size);
    }
    bool DetachedFloaterWindow::closeRequested() const { return mImpl->closeRequested(); }
    bool DetachedFloaterWindow::minimizeRequested() const { return mImpl->minimizeRequested(); }
    bool DetachedFloaterWindow::takeDragEnded() { return mImpl->takeDragEnded(); }
    bool DetachedFloaterWindow::takeResizeEnded() { return mImpl->takeResizeEnded(); }
    Floater* DetachedFloaterWindow::floater() const { return mImpl->floater(); }
    NativeRect DetachedFloaterWindow::nativeRect() const { return mImpl->nativeRect(); }
    std::string DetachedFloaterWindow::monitorId() const { return mImpl->monitorId(); }
    Vec2 DetachedFloaterWindow::logicalSize() const { return mImpl->logicalSize(); }
    Vec2 DetachedFloaterWindow::headerCenterScreen() const { return mImpl->headerCenterScreen(); }
}
