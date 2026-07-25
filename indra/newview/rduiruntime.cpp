#include "llviewerprecompiledheaders.h"
#include "rduiruntime.h"
#include "llcoord.h"
#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llwindow.h"
#include "llviewercontrol.h"
#include "llviewerinput.h"
#include "llviewerwindow.h"
#include "llviewershadermgr.h"
#include "rdfloater.h"
#include "rdbutton.h"
#include "rduidetachedfloatermanager.h"
#include "rduidetachedfloaterwindow.h"
#include "rduifloaterdocumentmanager.h"
#include "rduifloaterplacementstore.h"
#include "rduifloaterresize.h"
#include "rduifloaterstatestore.h"
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
#include <memory>
#include <optional>
#include <unordered_set>
#include <utility>
#include <vector>

namespace
{
    enum class InitializationState { Uninitialized, Ready, Failed };
    const rdui::viewer::RduiInputBridge INPUT_BRIDGE;

    rdui::SkinGenerationPrepareResult prepareSkin(rdui::viewer::SkinSnapshotResult captured)
    {
        rdui::SkinGenerationPrepareResult result;
        result.append(std::move(captured));
        if (result.hasErrors()) return result;

        rdui::SkinGenerationPrepareResult compiled =
            rdui::SkinCompiler().prepare(std::move(captured.snapshot));
        result.append(std::move(compiled));
        result.generation = std::move(compiled.generation);
        return result;
    }


}

namespace rdui::viewer
{
    struct Runtime::Impl final : private rdui::SurfaceFloaterDelegate,
                                 private DetachedFloaterManager::Environment,
                                 private FloaterDocumentManager::Host
    {
        public:
            explicit Impl(NativeWindowFactory& native_windows = defaultNativeWindowFactory())
                         : mPlacementStore(mFloaterStateStore.placementPersistence()),
                           mNativeWindows(native_windows),
                           mReloadCoordinator(mSystem, mResources),
                           mPaintContext(gRduiProgram, mSystem), mSurface(mSystem.createSurface(mPaintContext)),
                           mDetachedManager(*mSurface, mPlacementStore,
                               [this](std::unique_ptr<rdui::Floater>& floater)
                               {
                                   return std::make_unique<DetachedFloaterWindow>(
                                       mNativeWindows, mSystem, mDetachedManager, std::move(floater));
                               }, *this),
                           mDocuments(mSystem, *this)
            {
                mSurface->setFloaterDelegate(this);
                mSystem.setKeybindingResolver([](const std::string& authored_id)
                {
                    std::string viewer_command = authored_id;
                    std::replace(viewer_command.begin(), viewer_command.end(), '-', '_');
                    return KeybindingPresentation{
                        gViewerInput.getPrimaryKeyBinding({}, viewer_command)};
                });
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
                for (const FloaterDocumentId& document : mFloaterStateStore.openDocuments())
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
                const U64 binding_generation = gViewerInput.getBindingGeneration();
                const EKeyboardMode binding_mode = gViewerInput.getMode();
                if (mObservedBindingGeneration != binding_generation || mObservedBindingMode != binding_mode)
                {
                    mObservedBindingGeneration = binding_generation;
                    mObservedBindingMode = binding_mode;
                    mSystem.refreshKeybindings();
                }
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
                mFloaterStateStore.saveOpenDocuments(documents);
                mPersistedOpenDocuments = documents;
            }

            void processReload()
            {
                mReloadCoordinator.setAuthoringEnabled(gSavedSettings.getBOOL("RduiAuthoringMode"));
                if (mInitialization != InitializationState::Ready) return;

                std::optional<rdui::viewer::SkinReloadResult> result = mReloadCoordinator.update(
                    std::chrono::steady_clock::now(), mDocuments);
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

            NativeRect mainRectToNative(const rdui::Rect& rect) const override
            {
                if (!gWindowp) return {};
                LLCoordScreen top_left;
                LLCoordScreen bottom_right;
                gWindowp->convertCoords(LLCoordGL(ll_round(rect.left()), ll_round(rect.top())), &top_left);
                gWindowp->convertCoords(LLCoordGL(ll_round(rect.right()), ll_round(rect.bottom())), &bottom_right);
                return {std::min(top_left.mX, bottom_right.mX), std::min(top_left.mY, bottom_right.mY),
                        std::abs(bottom_right.mX - top_left.mX), std::abs(bottom_right.mY - top_left.mY)};
            }

            NativePoint mainPointToNative(const rdui::Vec2& point) const
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
                const NativeRect sample = mainRectToNative({0.f, 0.f, 100.f, 100.f});
                const float effective_scale = sample.width > 0 ? static_cast<float>(sample.width) / 100.f : 1.f;
                return effective_scale / std::max(0.25f, gWindowp->getSystemUISize());
            }

            rdui::Vec2 nativeBottomLeftInMain(const NativeRect& rect) const override
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

            bool placementVisible(const NativeRect& rect, const std::string& monitor_id) const override
            {
                return mNativeWindows.placementVisible(rect, monitor_id);
            }

            std::optional<NativePoint> releasePointerForDetach(
                const rdui::Vec2& main_position) override
            {
                const std::optional<NativePoint> cursor = gWindowp
                    ? std::optional<NativePoint>(mainPointToNative(main_position))
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
                const rdui::Rect authored_rect = mSurface->prepareFloater(*replacement);
                rdui::Rect replacement_rect = minimized ? current.expandedRect() : current.rect();
                const bool preserve_size = rdui::detail::preserveUserResizeOnReload(
                    current.canResize(), replacement->canResize(),
                    {current.authoredSize(), current.authoredContentSize()},
                    {{authored_rect.w, authored_rect.h}, replacement->authoredContentSize()});
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
                        mSurface->placeFloater(*floater, mSurface->prepareFloater(*floater));
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

            FloaterStateStore mFloaterStateStore;
            rdui::viewer::FloaterPlacementStore mPlacementStore;
            NativeWindowFactory& mNativeWindows;
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
            std::optional<U64> mObservedBindingGeneration;
            std::optional<EKeyboardMode> mObservedBindingMode;
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
