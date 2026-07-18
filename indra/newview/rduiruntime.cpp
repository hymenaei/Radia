#include "llviewerprecompiledheaders.h"
#include "rduiruntime.h"
#include "llcoord.h"
#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llwindow.h"
#include "llviewercontrol.h"
#include "llviewerwindow.h"
#include "llviewershadermgr.h"
#include "rdfloater.h"
#include "rdbutton.h"
#include "rduidetachedfloatermanager.h"
#include "rduifloaterdocumentmanager.h"
#include "rduifloaterplacementstore.h"
#include "rduiinputbridge.h"
#include "rduinativewindow.h"
#include "rduiskincompiler.h"
#include "rduiskinreloadcoordinator.h"
#include "rduiskinresources.h"
#include "rdpanel.h"
#include "rduisurface.h"
#include "rduiopenglpaintcontext.h"
#include "rduisystem.h"
#include <algorithm>
#include <chrono>
#include <iterator>
#include <memory>
#include <optional>
#include <unordered_set>
#include <utility>
#include <vector>

namespace
{
    constexpr float MARGIN = 24.f;

    enum class InitializationState { Uninitialized, Ready, Failed };
    const rdui::viewer::RduiInputBridge INPUT_BRIDGE;

    rdui::SkinGenerationPrepareResult prepareSkin(rdui::viewer::SkinSnapshotResult captured)
    {
        rdui::SkinGenerationPrepareResult result;
        result.warnings.insert(result.warnings.end(),
                               std::make_move_iterator(captured.warnings.begin()),
                               std::make_move_iterator(captured.warnings.end()));
        result.errors.insert(result.errors.end(),
                             std::make_move_iterator(captured.errors.begin()),
                             std::make_move_iterator(captured.errors.end()));
        if (result.hasErrors()) return result;

        rdui::SkinGenerationPrepareResult compiled =
            rdui::SkinCompiler().prepare(std::move(captured.snapshot));
        result.warnings.insert(result.warnings.end(),
                               std::make_move_iterator(compiled.warnings.begin()),
                               std::make_move_iterator(compiled.warnings.end()));
        result.errors.insert(result.errors.end(),
                             std::make_move_iterator(compiled.errors.begin()),
                             std::make_move_iterator(compiled.errors.end()));
        result.generation = std::move(compiled.generation);
        return result;
    }

    class SavedSettingsFloaterPlacementPersistence final
        : public rdui::viewer::FloaterPlacementStore::Persistence
    {
        public:
            LLSD read() const override
            {
                return gSavedSettings.getLLSD("RduiFloaterPlacements");
            }

            void write(const LLSD& placements) override
            {
                gSavedSettings.setLLSD("RduiFloaterPlacements", placements);
            }
    };

    std::vector<rdui::viewer::FloaterDocumentId> savedOpenFloaters()
    {
        std::vector<rdui::viewer::FloaterDocumentId> result;
        const LLSD saved = gSavedSettings.getLLSD("RduiOpenFloaters");
        if (!saved.isArray())
        {
            LL_WARNS("rdui") << "RduiOpenFloaters is not an array; ignoring it." << LL_ENDL;
            return result;
        }

        for (LLSD::array_const_iterator entry = saved.beginArray(); entry != saved.endArray(); ++entry)
        {
            if (!entry->isMap()) continue;
            const std::string definition_id = (*entry)["definition"].asString();
            if (definition_id.empty()) continue;
            result.push_back({definition_id, (*entry)["instance"].asString()});
        }
        return result;
    }

    void saveOpenFloaters(const std::vector<rdui::viewer::FloaterDocumentId>& documents)
    {
        LLSD saved = LLSD::emptyArray();
        for (const rdui::viewer::FloaterDocumentId& document : documents)
        {
            LLSD entry = LLSD::emptyMap();
            entry["definition"] = document.definitionId;
            entry["instance"] = document.instanceKey;
            saved.append(std::move(entry));
        }
        gSavedSettings.setLLSD("RduiOpenFloaters", saved);
    }

    class DetachedFloater final : public rdui::viewer::DetachedFloaterPresentation,
                                  public RduiNativeWindowClient,
                                  private rdui::SurfaceFloaterDelegate
    {
        public:
            DetachedFloater(RduiNativeWindowFactory& native_windows,
                            rdui::System& system, rdui::viewer::DetachedFloaterManager& manager,
                            std::unique_ptr<rdui::Floater> floater)
                          : mNativeWindows(native_windows), mManager(manager), mPaintContext(gRduiProgram, system),
                            mSurface(system.createSurface(mPaintContext)), mFloater(floater.get())
            {
                mSurface->setFloaterDelegate(this);
                mSurface->mountFloater(std::move(floater));
            }

            bool open(const RduiNativeRect& rect, float scale_multiplier,
                      const std::optional<rdui::Vec2>& drag_offset,
                      const std::optional<rdui::Vec2>& logical_size = std::nullopt,
                      const std::optional<RduiNativePoint>& drag_cursor = std::nullopt) override
            {
                mWindow = mNativeWindows.create(rect, mFloater->title(), *this);
                if (!mWindow) return false;
                mWindow->setScaleMultiplier(scale_multiplier);
                rdui::Vec2 size = logical_size.value_or(rdui::Vec2{mFloater->rect().w, mFloater->rect().h});
                if (mFloater->canResize())
                {
                    const rdui::Vec2 minimum = mSurface->minimumFloaterSize(*mFloater);
                    size.x = std::max(size.x, minimum.x);
                    size.y = std::max(size.y, minimum.y);
                }
                mLogicalSize = size;
                mWindow->setLogicalSize(size.x, size.y);
                const RduiNativeRect actual_rect = mWindow->rect();
                const float scale = mWindow->scale();
                const float width = static_cast<float>(actual_rect.width) / scale;
                const float height = static_cast<float>(actual_rect.height) / scale;
                mSurface->setViewport(width, height);
                mSurface->placeFloater(*mFloater, {0.f, 0.f, width, height});
                mSurface->updateLayout();
                mNativeTitle = mFloater->title();
                // Taking focus here makes the main viewer process focus loss in
                // the middle of the pointer handoff. Capture is sufficient for
                // the continuing drag; later clicks activate the window normally.
                mWindow->show(false);
                // Fill the companion window before returning to the viewer's
                // input loop so breakaway does not expose an empty native frame.
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

            void applyResize(const rdui::Rect& logical_rect) override
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

            std::unique_ptr<rdui::Floater> releaseFloater() override
            {
                if (!mFloater) return nullptr;
                std::unique_ptr<rdui::Floater> floater = mSurface->unmountFloater(*mFloater);
                mFloater.set(nullptr);
                return floater;
            }

            rdui::Floater& replaceFloater(std::unique_ptr<rdui::Floater> replacement,
                                          const std::optional<rdui::Vec2>& logical_size) override
            {
                if (mFloater) mSurface->unmountFloater(*mFloater);
                if (logical_size && mWindow)
                {
                    rdui::Vec2 size = *logical_size;
                    if (replacement->canResize())
                    {
                        const rdui::Vec2 minimum = mSurface->minimumFloaterSize(*replacement);
                        size.x = std::max(size.x, minimum.x);
                        size.y = std::max(size.y, minimum.y);
                    }
                    mLogicalSize = size;
                    mWindow->setLogicalSize(size.x, size.y);
                    const RduiNativeRect native = mWindow->rect();
                    const float scale = mWindow->scale();
                    mSurface->setViewport(static_cast<float>(native.width) / scale,
                                          static_cast<float>(native.height) / scale);
                }
                replacement->setRect({0.f, 0.f, mSurface->width(), mSurface->height()});
                rdui::Floater& mounted = mSurface->mountFloater(std::move(replacement));
                mSurface->placeFloater(mounted, {0.f, 0.f, mSurface->width(), mSurface->height()});
                mSurface->updateLayout();
                mFloater.set(&mounted);
                mNativeTitle.clear();
                return mounted;
            }

            rdui::viewer::NativeInputDispatchResult dispatchNative(
                const rdui::viewer::NativeInputEvent& event) override
            {
                const rdui::viewer::SurfaceInputEvent translated = INPUT_BRIDGE.translate(event);
                if (const auto* pointer = std::get_if<rdui::viewer::SurfacePointerInput>(&translated))
                {
                    if (pointer->phase == rdui::viewer::NativePointerPhase::Leave)
                    {
                        mSurface->pointerLeave();
                        return {};
                    }
                    const bool handled = pointer->phase == rdui::viewer::NativePointerPhase::Move
                        ? mSurface->pointerMove(pointer->event)
                        : pointer->phase == rdui::viewer::NativePointerPhase::Down
                            ? mSurface->pointerDown(pointer->event)
                            : mSurface->pointerUp(pointer->event);
                    if (pointer->phase == rdui::viewer::NativePointerPhase::Down
                        && pointer->event.button == rdui::PointerButton::Left
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
                if (const auto* scroll = std::get_if<rdui::ScrollEvent>(&translated))
                    return {mSurface->scroll(*scroll), std::nullopt};
                if (const auto* key = std::get_if<rdui::viewer::SurfaceKeyInput>(&translated))
                {
                    return {key->down ? mSurface->keyDown(key->event) : mSurface->keyUp(key->event), std::nullopt};
                }
                if (const auto* character = std::get_if<rdui::viewer::SurfaceCharacterInput>(&translated))
                    return {mSurface->charInput(character->codepoint), std::nullopt};
                if (std::holds_alternative<rdui::viewer::NativeInteractionLoss>(translated))
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
                gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
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
            rdui::Floater* floater() const override { return mFloater.get(); }
            RduiNativeRect nativeRect() const override { return mWindow ? mWindow->rect() : RduiNativeRect{}; }
            std::string monitorId() const override { return mWindow ? mWindow->monitorId() : std::string(); }
            float scale() const { return mWindow ? mWindow->scale() : 1.f; }
            rdui::Vec2 logicalSize() const override { return mLogicalSize; }

            rdui::Vec2 headerCenterScreen() const override
            {
                if (!mWindow || !mFloater || !mFloater->header()) return {};
                const RduiNativeRect window_rect = mWindow->rect();
                const rdui::Rect header = mFloater->header()->rect();
                return {window_rect.x + (header.x + header.w * 0.5f) * mWindow->scale(),
                        window_rect.y + (mSurface->height() - header.y - header.h * 0.5f) * mWindow->scale()};
            }

        private:
            void floaterClosed(rdui::Surface&, rdui::Floater&) override { mCloseRequested = true; }

            void floaterMinimizedChanged(rdui::Surface&, rdui::Floater&, bool minimized) override
            {
                if (minimized) mMinimizeRequested = true;
            }

            bool beginNativeFloaterResize(rdui::Surface&, rdui::Floater&) override
            {
                return mFloater && mManager.beginResize(*mFloater);
            }

            void floaterResized(rdui::Surface&, rdui::Floater& floater, bool) override
            {
                mManager.applyResize(floater, floater.rect());
            }

            bool nativePresentationChanged() const
            {
                if (!mWindow) return false;
                const RduiNativeRect native = mWindow->rect();
                return native.width != mLastPaintWidth || native.height != mLastPaintHeight
                    || mWindow->scale() != mLastPaintScale;
            }

            RduiNativeWindowFactory& mNativeWindows;
            rdui::viewer::DetachedFloaterManager& mManager;
            rdui::OpenGLPaintContext mPaintContext;
            std::unique_ptr<rdui::Surface> mSurface;
            rdui::WidgetRef<rdui::Floater> mFloater;
            std::unique_ptr<RduiNativeWindow> mWindow;
            bool mCloseRequested = false;
            bool mMinimizeRequested = false;
            bool mDragEnded = false;
            bool mResizeEnded = false;
            S32 mLastPaintWidth = 0;
            S32 mLastPaintHeight = 0;
            F32 mLastPaintScale = 0.f;
            std::string mNativeTitle;
            rdui::Vec2 mLogicalSize;
            std::chrono::steady_clock::time_point mLastTick;
    };

}

namespace rdui::viewer
{
    struct Runtime::Impl final : private rdui::SurfaceFloaterDelegate,
                                 private DetachedFloaterManager::Environment,
                                 private FloaterDocumentManager::Host
    {
        public:
            explicit Impl(RduiNativeWindowFactory& native_windows = defaultRduiNativeWindowFactory())
                         : mPlacementStore(mPlacementPersistence),
                           mNativeWindows(native_windows),
                           mReloadCoordinator(mSystem, mResources),
                           mPaintContext(gRduiProgram, mSystem), mSurface(mSystem.createSurface(mPaintContext)),
                           mDetachedManager(*mSurface, mPlacementStore,
                               [this](std::unique_ptr<rdui::Floater>& floater)
                               {
                                   return std::make_unique<DetachedFloater>(
                                       mNativeWindows, mSystem, mDetachedManager, std::move(floater));
                               }, *this),
                           mDocuments(mSystem, *this)
            {
                mSurface->setFloaterDelegate(this);
            }

            ~Impl()
            {
                persistOpenFloaters();
                clearDragCursorState();
            }

            bool initialize()
            {
                if (mInitialization != InitializationState::Uninitialized)
                    return mInitialization == InitializationState::Ready;
                mInitialization = InitializationState::Failed;

                rdui::SkinGenerationPrepareResult system_result = prepareSkin(mResources.capture());
                if (!system_result.ok() && !mResources.selectedIsBundledDefault())
                {
                    for (const rdui::Diagnostic& warning : system_result.warnings) LL_WARNS("rdui") << warning.formatted() << LL_ENDL;
                    for (const rdui::Diagnostic& error : system_result.errors) LL_WARNS("rdui") << error.formatted() << LL_ENDL;
                    LL_WARNS("rdui") << "Selected Radia Skin rejected; attempting bundled default Skin." << LL_ENDL;
                    system_result = prepareSkin(mResources.captureBundledDefault());
                }
                for (const rdui::Diagnostic& warning : system_result.warnings) LL_WARNS("rdui") << warning.formatted() << LL_ENDL;
                for (const rdui::Diagnostic& error : system_result.errors) LL_WARNS("rdui") << error.formatted() << LL_ENDL;
                if (!system_result.ok()) return false;
                mSystem.publish(std::move(system_result.generation));

                const std::string saved_locale = gSavedSettings.getString("RduiLanguage");
                if (!saved_locale.empty()) mSystem.setLocale(saved_locale);
                gSavedSettings.setString("RduiLanguage", mSystem.activeLocale());
                mSystem.setLocaleChangedHandler([](const std::string& locale)
                {
                    gSavedSettings.setString("RduiLanguage", locale);
                });

                mInitialization = InitializationState::Ready;
                return true;
            }

            bool registerFloater(std::string definition_id, Runtime::ControllerFactory factory)
            {
                return mDocuments.registerDefinition(std::move(definition_id), std::move(factory));
            }

            rdui::Floater* openFloater(const std::string& definition_id,
                                       const std::string& instance_key)
            {
                if (mInitialization != InitializationState::Ready) return nullptr;
                FloaterDocumentOpenResult result = mDocuments.open(definition_id, instance_key);
                logDiagnostics(result);
                if (!result.ok()) return nullptr;
                if (mWidth > 0 && mHeight > 0) layout(mWidth, mHeight);
                persistOpenFloaters();
                return result.floater;
            }

            void restoreOpenFloaters()
            {
                if (mInitialization != InitializationState::Ready || mSessionRestored) return;
                for (const FloaterDocumentId& document : savedOpenFloaters())
                    openFloater(document.definitionId, document.instanceKey);
                mSessionRestored = true;
                persistOpenFloaters();
            }

            void requestReload() { mReloadCoordinator.request(); }

            void setVisibility(bool attached_visible, bool detached_visible)
            {
                if (mAttachedVisible != attached_visible)
                {
                    mAttachedVisible = attached_visible;
                    mLastFrameTime = {};
                    if (!mAttachedVisible) clearInteraction();
                }
                if (mDetachedVisible != detached_visible)
                {
                    mDetachedVisible = detached_visible;
                    mDetachedManager.setVisible(detached_visible);
                }
            }

            void draw(int width, int height)
            {
                if (!isInteractive() || width <= 0 || height <= 0) return;
                const auto now = std::chrono::steady_clock::now();
                if (width != mWidth || height != mHeight) layout(width, height);
                mSurface->refreshHover();
                if (mLastFrameTime != std::chrono::steady_clock::time_point())
                    mSurface->update(std::chrono::duration_cast<std::chrono::milliseconds>(now - mLastFrameTime));
                mLastFrameTime = now;
                mSurface->paint(mPaintContext);
            }

            bool pointerMove(const rdui::PointerEvent& event)
            {
                if (!isInteractive()) return false;
                rdui::PointerEvent routed = event;
                if (cursorMagnetActive())
                {
                    // Native client coordinates address the last pixel at
                    // extent - 1, while RDUI's movement bounds end at extent.
                    const float cursor_right = std::max(0.f, static_cast<float>(mWidth) - 1.f);
                    const float cursor_top = std::max(0.f, static_cast<float>(mHeight) - 1.f);
                    routed.position.x = magnetizedAxis(event.position.x, event.delta.x, 0.f,
                                                       cursor_right, mMagnetX, mVirtualPointer.x);
                    routed.position.y = magnetizedAxis(event.position.y, event.delta.y, 0.f,
                                                       cursor_top, mMagnetY, mVirtualPointer.y);
                }
                else
                {
                    resetCursorMagnet();
                }

                const bool handled = mSurface->pointerMove(routed);
                mDetachedManager.processPendingDetach();
                setDragCursorClipping(dragCursorClippingRequired());
                return handled;
            }

            void pointerLeave()
            {
                if (mInitialization == InitializationState::Ready) mSurface->pointerLeave();
            }

            bool dispatchPointerButton(const rdui::PointerEvent& event, bool down)
            {
                if (!isInteractive()) return false;
                if (down && event.button == rdui::PointerButton::Left)
                    mLastFrameTime = std::chrono::steady_clock::now();
                bool handled = false;
                if (!down && event.button == rdui::PointerButton::Left && mSurface->hasPointerCapture())
                {
                    handled = pointerMove(event);
                }
                handled = (down ? mSurface->pointerDown(event) : mSurface->pointerUp(event)) || handled;
                if (down && !handled) mSurface->clearFocus();
                if (down && draggingFloater())
                {
                    mVirtualPointer = event.position;
                    setDragCursorClipping(dragCursorClippingRequired());
                }
                if (!down || !draggingFloater()) clearDragCursorState();
                return handled;
            }

            bool scroll(const rdui::ScrollEvent& event)
            {
                return isInteractive() && mSurface->scroll(event);
            }

            bool keyDown(const rdui::KeyEvent& event)
            {
                return isInteractive() && mSurface->keyDown(event);
            }

            bool keyUp(const rdui::KeyEvent& event)
            {
                return isInteractive() && mSurface->keyUp(event);
            }

            bool charInput(U32 codepoint)
            {
                return isInteractive() && mSurface->charInput(codepoint);
            }

            rdui::CursorStyle cursor() const { return mSurface->cursor(); }
            bool hasPointerCapture() const { return mSurface->hasPointerCapture(); }

            void clearInteraction()
            {
                if (mInitialization != InitializationState::Uninitialized) mSurface->clearInteractionState();
                clearDragCursorState();
            }

            void idle()
            {
                mDocuments.idle();
                processReload();
                mDetachedManager.update();
                persistOpenFloaters();
            }

        private:
            static void logDiagnostics(const rdui::DiagnosticResult& result)
            {
                for (const rdui::Diagnostic& warning : result.warnings)
                    LL_WARNS("rdui") << warning.code << ": " << warning.formatted() << LL_ENDL;
                for (const rdui::Diagnostic& error : result.errors)
                    LL_WARNS("rdui") << error.code << ": " << error.formatted() << LL_ENDL;
            }

            void rejectReload(const rdui::DiagnosticResult& result)
            {
                logDiagnostics(result);
                LL_WARNS("rdui") << "Candidate Skin Generation rejected; generation "
                                  << mSystem.generation() << " remains live." << LL_ENDL;
                mDocuments.reportReloadFailed(result);
            }

            void persistOpenFloaters()
            {
                if (!mSessionRestored) return;
                const std::vector<FloaterDocumentId> documents = mDocuments.openDocuments();
                if (mPersistedOpenDocuments && *mPersistedOpenDocuments == documents) return;
                saveOpenFloaters(documents);
                mPersistedOpenDocuments = documents;
            }

            void processReload()
            {
                mReloadCoordinator.setAuthoringEnabled(gSavedSettings.getBOOL("RduiAuthoringMode"));
                if (mInitialization != InitializationState::Ready) return;

                std::optional<rdui::viewer::SkinReloadResult> result = mReloadCoordinator.update(
                    std::chrono::steady_clock::now(), mDocuments.reloadTargets());
                if (!result) return;
                if (!result->ok())
                {
                    rejectReload(*result);
                    return;
                }

                logDiagnostics(*result);
                mDocuments.reportReloadSucceeded();
                LL_INFOS("rdui") << "Committed Skin Generation " << result->generation << "." << LL_ENDL;
            }

            bool isInteractive() const
            {
                if (mInitialization != InitializationState::Ready || !mAttachedVisible) return false;
                for (const rdui::Floater* floater : mDocuments.floaters())
                    if (floater && !isDetached(*floater) && !floater->closed()
                        && floater->visibility() == rdui::Visibility::Visible) return true;
                return false;
            }

            static float magnetizedAxis(float position, float delta, float minimum, float maximum,
                                        int& direction, float& virtual_position)
            {
                if (direction == 0)
                {
                    if (position <= minimum && delta < 0.f)
                    {
                        direction = -1;
                        virtual_position += delta;
                        return virtual_position;
                    }
                    else if (position >= maximum && delta > 0.f)
                    {
                        direction = 1;
                        virtual_position += delta;
                        return virtual_position;
                    }
                    virtual_position = position;
                    return position;
                }

                if ((direction < 0 && position > minimum) || (direction > 0 && position < maximum))
                {
                    direction = 0;
                    virtual_position = position;
                    return position;
                }

                virtual_position += delta;
                return virtual_position;
            }

            bool cursorMagnetActive() const
            {
                const rdui::Floater* floater = draggingFloater();
                return floater && floater->canDetach() && !floater->minimized();
            }

            bool dragCursorClippingRequired() const
            {
                const rdui::Floater* floater = draggingFloater();
                return floater && (floater->canDetach() || floater->minimized());
            }

            rdui::Floater* draggingFloater() const
            {
                for (rdui::Floater* floater : mDocuments.floaters())
                    if (floater && !isDetached(*floater) && floater->dragging()) return floater;
                return nullptr;
            }

            void setDragCursorClipping(bool enabled)
            {
                if (mDragCursorClipping == enabled) return;
                mDragCursorClipping = enabled;
                if (gWindowp) gWindowp->setMouseClipping(enabled);
            }

            void resetCursorMagnet()
            {
                mMagnetX = 0;
                mMagnetY = 0;
            }

            void clearDragCursorState()
            {
                resetCursorMagnet();
                setDragCursorClipping(false);
            }

            bool isDetached(const rdui::Floater& floater) const
            {
                return mDetachedManager.contains(floater);
            }

            bool canDetachFloater(const rdui::Surface& surface, const rdui::Floater&) const override
            {
                return &surface == mSurface.get();
            }

            void floaterClosed(rdui::Surface& surface, rdui::Floater&) override
            {
                if (&surface == mSurface.get()) clearInteraction();
            }

            void floaterMoved(rdui::Surface& surface, rdui::Floater& floater) override
            {
                if (&surface == mSurface.get() && !floater.minimized())
                    saveAttachedPlacement(floater);
            }

            void floaterResized(rdui::Surface& surface, rdui::Floater& floater, bool complete) override
            {
                if (complete && &surface == mSurface.get()) saveAttachedPlacement(floater);
            }

            void floaterDetachRequested(rdui::Surface& surface, rdui::Floater& floater,
                                        const rdui::Vec2& desired, const rdui::Vec2& drag_offset) override
            {
                if (&surface == mSurface.get())
                    if (const FloaterInstanceId* identity = mDocuments.identity(floater))
                        mDetachedManager.requestDetach(*identity, floater, desired, drag_offset);
            }

            RduiNativeRect mainRectToNative(const rdui::Rect& rect) const override
            {
                if (!gWindowp) return {};
                LLCoordScreen top_left;
                LLCoordScreen bottom_right;
                gWindowp->convertCoords(LLCoordGL(ll_round(rect.left()), ll_round(rect.top())), &top_left);
                gWindowp->convertCoords(LLCoordGL(ll_round(rect.right()), ll_round(rect.bottom())), &bottom_right);
                return {std::min(top_left.mX, bottom_right.mX), std::min(top_left.mY, bottom_right.mY),
                        std::abs(bottom_right.mX - top_left.mX), std::abs(bottom_right.mY - top_left.mY)};
            }

            RduiNativePoint mainPointToNative(const rdui::Vec2& point) const
            {
                if (!gWindowp) return {};
                const LLVector2 scale = gViewerWindow ? gViewerWindow->getDisplayScale() : LLVector2(1.f, 1.f);
                LLCoordScreen screen;
                gWindowp->convertCoords(LLCoordGL(ll_round(point.x * scale.mV[VX]),
                                                  ll_round(point.y * scale.mV[VY])), &screen);
                return {screen.mX, screen.mY};
            }

            float nativeScaleMultiplier() const override
            {
                if (!gWindowp) return 1.f;
                const RduiNativeRect sample = mainRectToNative({0.f, 0.f, 100.f, 100.f});
                const float effective_scale = sample.width > 0 ? static_cast<float>(sample.width) / 100.f : 1.f;
                return effective_scale / std::max(0.25f, gWindowp->getSystemUISize());
            }

            rdui::Vec2 nativeBottomLeftInMain(const RduiNativeRect& rect) const override
            {
                if (!gWindowp) return {};
                LLCoordGL bottom_left;
                gWindowp->convertCoords(LLCoordScreen(rect.x, rect.y + rect.height), &bottom_left);
                return {static_cast<float>(bottom_left.mX), static_cast<float>(bottom_left.mY)};
            }

            bool nativePointInsideMain(const rdui::Vec2& point) const override
            {
                if (!gWindowp || mWidth <= 0 || mHeight <= 0) return false;
                LLCoordScreen first;
                LLCoordScreen second;
                gWindowp->convertCoords(LLCoordGL(0, 0), &first);
                gWindowp->convertCoords(LLCoordGL(mWidth, mHeight), &second);
                return point.x >= std::min(first.mX, second.mX) && point.x <= std::max(first.mX, second.mX)
                    && point.y >= std::min(first.mY, second.mY) && point.y <= std::max(first.mY, second.mY);
            }

            bool placementVisible(const RduiNativeRect& rect, const std::string& monitor_id) const override
            {
                return mNativeWindows.placementVisible(rect, monitor_id);
            }

            std::optional<RduiNativePoint> releasePointerForDetach(
                const rdui::Vec2& main_position) override
            {
                const std::optional<RduiNativePoint> cursor = gWindowp
                    ? std::optional<RduiNativePoint>(mainPointToNative(main_position))
                    : std::nullopt;
                clearDragCursorState();
                if (gWindowp) gWindowp->releaseMouse();
                return cursor;
            }

            rdui::Floater* mount(const FloaterInstanceId& identity,
                                 std::unique_ptr<rdui::Floater> floater) override
            {
                if (!floater) return nullptr;
                mPositionInitialized.erase(identity.value());
                return &mSurface->mountFloater(std::move(floater));
            }

            rdui::Floater* replace(const FloaterInstanceId& identity,
                                   rdui::Floater& current,
                                   std::unique_ptr<rdui::Floater> replacement) override
            {
                if (!replacement) return nullptr;
                const bool detached = mDetachedManager.contains(current);
                const bool minimized = current.minimized();
                const rdui::Rect authored_rect = mSurface->prepareFloater(*replacement, MARGIN);
                rdui::Rect replacement_rect = minimized ? current.expandedRect() : current.rect();
                const bool preserve_size = current.canResize() && replacement->canResize();
                std::optional<rdui::Vec2> replacement_size;
                if (!preserve_size)
                {
                    replacement_size = {authored_rect.w, authored_rect.h};
                    replacement_rect.w = replacement_size->x;
                    replacement_rect.h = replacement_size->y;
                }

                if (detached)
                {
                    if (!replacement_size) replacement_size = mDetachedManager.logicalSize(current);
                    return mDetachedManager.replace(current, std::move(replacement), replacement_size);
                }

                std::unique_ptr<rdui::Floater> retired = mSurface->unmountFloater(current);
                replacement->setRect(replacement_rect);
                rdui::Floater& mounted = mSurface->mountFloater(std::move(replacement));
                mSurface->placeFloater(mounted, replacement_rect);
                if (minimized && mounted.canMinimize()) mounted.setMinimized(true);
                mSurface->updateLayout();
                saveAttachedPlacement(identity, mounted);
                return &mounted;
            }

            void show(rdui::Floater& floater) override
            {
                floater.open();
                if (!mDetachedManager.contains(floater)) mSurface->raise(floater);
            }

            void saveAttachedPlacement(const rdui::Floater& floater)
            {
                const FloaterInstanceId* identity = mDocuments.identity(floater);
                if (!identity) return;
                saveAttachedPlacement(*identity, floater);
            }

            void saveAttachedPlacement(const FloaterInstanceId& identity,
                                       const rdui::Floater& floater)
            {
                std::optional<rdui::viewer::FloaterPlacementSize> size;
                if (floater.canResize())
                    size = rdui::viewer::FloaterPlacementSize{floater.rect().w, floater.rect().h};
                mPlacementStore.save(identity, rdui::viewer::AttachedFloaterPlacement{
                    floater.rect().x, floater.rect().y, size});
            }

            void restorePlacement(const FloaterInstanceId& identity, rdui::Floater& floater)
            {
                const std::optional<rdui::viewer::FloaterPlacement> placement =
                    mPlacementStore.restore(identity);
                if (!placement) return;
                if (const auto* detached_placement =
                        std::get_if<rdui::viewer::DetachedFloaterPlacement>(&*placement))
                {
                    if (!floater.canDetach())
                    {
                        saveAttachedPlacement(identity, floater);
                        return;
                    }
                    if (mDetachedManager.restore(identity, floater, *detached_placement)) return;
                    saveAttachedPlacement(identity, floater);
                    return;
                }
                const auto& attached = std::get<rdui::viewer::AttachedFloaterPlacement>(*placement);
                rdui::Rect saved{attached.x, attached.y,
                                 floater.rect().w, floater.rect().h};
                if (floater.canResize() && attached.size)
                {
                    saved.w = attached.size->width;
                    saved.h = attached.size->height;
                }
                mSurface->placeFloater(floater, saved);
                mSurface->updateLayout();
                saveAttachedPlacement(identity, floater);
            }

            void layout(int width, int height)
            {
                mWidth = width;
                mHeight = height;
                mSurface->setViewport(static_cast<float>(width), static_cast<float>(height));
                for (rdui::Floater* floater : mDocuments.floaters())
                {
                    if (!floater || isDetached(*floater)) continue;
                    const FloaterInstanceId* identity = mDocuments.identity(*floater);
                    if (identity && mPositionInitialized.insert(identity->value()).second)
                        mSurface->placeFloater(*floater, mSurface->prepareFloater(*floater, MARGIN));
                }
                mSurface->updateLayout();
                for (rdui::Floater* floater : mDocuments.floaters())
                {
                    if (!floater) continue;
                    if (const FloaterInstanceId* identity = mDocuments.identity(*floater))
                        restorePlacement(*identity, *floater);
                }
                mSurface->refreshHover();
            }

            SavedSettingsFloaterPlacementPersistence mPlacementPersistence;
            rdui::viewer::FloaterPlacementStore mPlacementStore;
            RduiNativeWindowFactory& mNativeWindows;
            rdui::viewer::SkinResources mResources;
            rdui::System mSystem;
            rdui::viewer::SkinReloadCoordinator mReloadCoordinator;
            rdui::OpenGLPaintContext mPaintContext;
            std::unique_ptr<rdui::Surface> mSurface;
            rdui::viewer::DetachedFloaterManager mDetachedManager;
            rdui::viewer::FloaterDocumentManager mDocuments;
            int mWidth = 0;
            int mHeight = 0;
            InitializationState mInitialization = InitializationState::Uninitialized;
            bool mAttachedVisible = true;
            bool mDetachedVisible = true;
            bool mSessionRestored = false;
            std::optional<std::vector<FloaterDocumentId>> mPersistedOpenDocuments;
            std::unordered_set<std::string> mPositionInitialized;
            int mMagnetX = 0;
            int mMagnetY = 0;
            rdui::Vec2 mVirtualPointer;
            bool mDragCursorClipping = false;
            std::chrono::steady_clock::time_point mLastFrameTime;
    };

}

namespace rdui::viewer
{
    Runtime::Runtime() : mImpl(std::make_unique<Impl>()) {}
    Runtime::~Runtime() = default;

    bool Runtime::initialize()
    {
        return mImpl->initialize();
    }

    bool Runtime::registerFloater(std::string definition_id, ControllerFactory factory)
    {
        return mImpl->registerFloater(std::move(definition_id), std::move(factory));
    }

    rdui::Floater* Runtime::openFloater(const std::string& definition_id,
                                       const std::string& instance_key)
    {
        return mImpl->openFloater(definition_id, instance_key);
    }

    void Runtime::restoreOpenFloaters()
    {
        mImpl->restoreOpenFloaters();
    }

    void Runtime::requestReload()
    {
        mImpl->requestReload();
    }

    void Runtime::setVisibility(bool attached_visible, bool detached_visible)
    {
        mImpl->setVisibility(attached_visible, detached_visible);
    }

    void Runtime::frame(S32 width, S32 height)
    {
        mImpl->draw(width, height);
    }

    void Runtime::idle()
    {
        mImpl->idle();
    }

    bool Runtime::hasPointerCapture() const
    {
        return mImpl->hasPointerCapture();
    }

    NativeInputDispatchResult Runtime::dispatch(const NativeInputEvent& event)
    {
        const SurfaceInputEvent translated = INPUT_BRIDGE.translate(event);
        if (const auto* pointer = std::get_if<SurfacePointerInput>(&translated))
        {
            if (pointer->phase == NativePointerPhase::Leave)
            {
                mImpl->pointerLeave();
                return {};
            }
            const bool handled = pointer->phase == NativePointerPhase::Move
                ? mImpl->pointerMove(pointer->event)
                : mImpl->dispatchPointerButton(
                    pointer->event, pointer->phase == NativePointerPhase::Down);
            return {handled, handled && pointer->phase == NativePointerPhase::Move
                ? std::optional<ECursorType>(INPUT_BRIDGE.translateCursor(mImpl->cursor()))
                : std::nullopt};
        }
        if (const auto* scroll = std::get_if<rdui::ScrollEvent>(&translated))
        {
            const bool handled = mImpl->scroll(*scroll);
            return {handled, std::nullopt};
        }
        if (const auto* key = std::get_if<SurfaceKeyInput>(&translated))
        {
            const bool handled = key->down ? mImpl->keyDown(key->event) : mImpl->keyUp(key->event);
            return {handled, std::nullopt};
        }
        if (const auto* character = std::get_if<SurfaceCharacterInput>(&translated))
        {
            const bool handled = mImpl->charInput(character->codepoint);
            return {handled, std::nullopt};
        }
        if (std::holds_alternative<NativeInteractionLoss>(translated)) mImpl->clearInteraction();
        return {};
    }
}
