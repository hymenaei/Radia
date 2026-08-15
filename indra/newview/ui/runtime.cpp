/**
 * @file runtime.cpp
 * @brief Owns the viewer-side UI runtime, Floater lifecycle, reloads, and input dispatch.
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
#include "runtime.h"
#include <algorithm>
#include <chrono>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <utility>
#include <vector>
#include "auxiliarywindow.h"
#include "componentmanager.h"
#include "componentpersistence.h"
#include "detachedfloatermanager.h"
#include "detachedfloaterwindow.h"
#include "floaterhost.h"
#include "inputbridge.h"
#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llviewercontrol.h"
#include "llviewershadermgr.h"
#include "llwindow.h"
#include "render/openglpaintcontext.h"
#include "render/paintcontext.h"
#include "runtimewindowadapter.h"
#include "settingsadapter.h"
#include "skin/compiler.h"
#include "skin/reloadcoordinator.h"
#include "skin/resources.h"
#include "surface/surface.h"
#include "system.h"
#include "widgets/button.h"
#include "widgets/floater.h"

namespace {
using radia::ui::OpenGLPaintContext;
using radia::ui::PaintContext;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGeneration;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::Vec2;
using radia::viewer::ui::Runtime;
using radia::viewer::ui::SkinSnapshotResult;
using radia::viewer::ui::SkinSnapshotSource;

enum class InitializationState { Uninitialized, Ready, Failed };

constexpr char kLongClickDelaySetting[] = "LongClickDelay";
constexpr char kSkinReloadScanIntervalSetting[] = "SkinAutoReloadScanInterval";
constexpr char kSkinReloadSettleIntervalSetting[] = "SkinAutoReloadSettleInterval";

struct AttachedSurfaceState final {
    using TimePoint = std::chrono::steady_clock::time_point;

    AttachedSurfaceState(LLGLSLShader& uiShader, System& system, Runtime::PaintContextFactory paintContextFactory)
        : paintContext(paintContextFactory ? paintContextFactory(uiShader, system) : std::make_unique<OpenGLPaintContext>(uiShader, system)),
          surface(system.createSurface(*paintContext)) {}

    std::unique_ptr<PaintContext> paintContext;
    std::unique_ptr<Surface> surface;
    int width = 0;
    int height = 0;
    bool visible = true;
    int magnetX = 0;
    int magnetY = 0;
    Vec2 virtualPointer;
    bool dragCursorClipping = false;
    TimePoint lastFrameTime;
};

class RuntimeSkinSource final : public SkinSnapshotSource {
public:
    RuntimeSkinSource(const radia::viewer::ui::SkinResources& fallback, Runtime::SkinSnapshotProvider capture)
        : mFallback(fallback), mCapture(std::move(capture)) {}

    SkinSnapshotResult capture() const override { return mCapture ? mCapture() : mFallback.capture(); }

private:
    const radia::viewer::ui::SkinResources& mFallback;
    Runtime::SkinSnapshotProvider mCapture;
};

SkinGenerationPrepareResult prepareSkinGeneration(SkinSnapshotResult captured) {
    SkinGenerationPrepareResult result;
    ResourceSnapshot snapshot = std::move(captured.snapshot);
    result.append(std::move(captured));
    if (result.hasErrors()) return result;

    SkinGenerationPrepareResult compiled = SkinCompiler().prepare(std::move(snapshot));
    std::shared_ptr<const SkinGeneration> generation = std::move(compiled.generation);
    result.append(std::move(compiled));
    result.generation = std::move(generation);
    return result;
}
} // namespace

namespace radia::viewer::ui {
using radia::ui::Diagnostic;
using radia::ui::DiagnosticResult;
using radia::ui::Floater;
using radia::ui::KeybindingPresentation;
using radia::ui::KeyEvent;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::Rect;
using radia::ui::ScrollEvent;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::SurfaceFloaterDelegate;
using radia::ui::System;
using radia::ui::Vec2;
using radia::ui::Visibility;
using radia::ui::WidgetRef;

struct Runtime::Impl final : private SurfaceFloaterDelegate {
public:
    using KeybindingResolver = Runtime::KeybindingResolver;
    using KeybindingStateProvider = Runtime::KeybindingStateProvider;
    using Clock = Runtime::Clock;
    using TimePoint = AttachedSurfaceState::TimePoint;

    Impl(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, Runtime::WindowEnvironment window,
         Runtime::IntegrationHooks hooks)
        : mSavedSettings(savedSettings), mResolveKeybinding(std::move(hooks.resolveKeybinding)), mKeybindingState(std::move(hooks.keybindingState)),
          mNow(std::move(hooks.now)), mComponentPersistence(savedSettings, perAccountSettings),
          mAuxiliaryWindowFactory(window.auxiliaryWindowFactory), mUiShader(uiShader), mResources(),
          mSnapshotSource(mResources, std::move(hooks.captureSkin)), mSystem(), mReloadCoordinator(mSystem, mSnapshotSource),
          mAttachedSurface(uiShader, mSystem, std::move(hooks.paintContext)),
          mWindowAdapter(
              window.mainWindow, mAuxiliaryWindowFactory, std::move(window.displayScale),
              [this] { return std::pair{mAttachedSurface.width, mAttachedSurface.height}; }, [this] { clearDragCursorState(); }),
          mDetachedManager(
              *mAttachedSurface.surface,
              [this](std::unique_ptr<Floater> floater) {
                  return DetachedFloaterPresentationResult::success(std::make_unique<DetachedFloaterWindow>(
                      mAuxiliaryWindowFactory, mUiShader, mSystem, mDetachedManager, std::move(floater), [this] { return currentTime(); }));
              },
              mWindowAdapter,
              [this](const ComponentKey& componentKey, FloaterPlacement placement, ComponentOpenState state) {
                  mComponentPersistence.savePlacement(componentKey, std::move(placement), state);
              }),
          mFloaterHost(*mAttachedSurface.surface, mDetachedManager), mSettingsAdapter(savedSettings),
          mComponents(mSystem, mFloaterHost, mSettingsAdapter) {
        mAttachedSurface.surface->setFloaterDelegate(this);
        mSystem.setKeybindingResolver([this](const std::string& authoredId) {
            if (!mResolveKeybinding) return KeybindingPresentation{};
            std::string viewerCommand = authoredId;
            std::replace(viewerCommand.begin(), viewerCommand.end(), '-', '_');
            return mResolveKeybinding(viewerCommand);
        });
    }

    ~Impl() { shutdown(); }

    NativeInputDispatchResult pointerMove(F32 x, F32 y, U32 modifiers, F32 deltaX, F32 deltaY) {
        return pointerMove(translatePointerInput({x, y, NativePointerButton::NoButton, modifiers, 1, deltaX, deltaY}));
    }

    NativeInputDispatchResult pointerDown(F32 x, F32 y, NativePointerButton button, U32 modifiers, U8 clickCount, F32 deltaX, F32 deltaY) {
        return pointerDown(translatePointerInput({x, y, button, modifiers, clickCount, deltaX, deltaY}));
    }

    NativeInputDispatchResult pointerUp(F32 x, F32 y, NativePointerButton button, U32 modifiers, U8 clickCount, F32 deltaX, F32 deltaY) {
        return pointerUp(translatePointerInput({x, y, button, modifiers, clickCount, deltaX, deltaY}));
    }

    NativeInputDispatchResult scroll(S32 x, S32 y, F32 horizontal, F32 vertical, U32 modifiers) {
        return scroll(translateScrollInput({x, y, horizontal, vertical, modifiers}));
    }

    NativeInputDispatchResult keyDown(KEY key, U32 modifiers, bool repeated) { return keyDown(translateKeyInput({key, modifiers, true, repeated})); }

    NativeInputDispatchResult keyUp(KEY key, U32 modifiers) { return keyUp(translateKeyInput({key, modifiers, false, false})); }

    NativeInputDispatchResult character(U32 codepoint, U32) { return character(codepoint); }

    void focusLost() { clearInteraction(); }

    void mouseCaptureLost() { clearInteraction(); }

    bool initialize() {
        if (mInitialization != InitializationState::Uninitialized) return mInitialization == InitializationState::Ready;
        mInitialization = InitializationState::Failed;

        SkinGenerationPrepareResult skinGenerationResult = prepareSkinGeneration(mSnapshotSource.capture());
        if (!skinGenerationResult.ok() && !mResources.selectedIsBundledDefault()) {
            for (const Diagnostic& warning : skinGenerationResult.warnings) LL_WARNS("UI") << warning.formatted() << LL_ENDL;
            for (const Diagnostic& error : skinGenerationResult.errors) LL_WARNS("UI") << error.formatted() << LL_ENDL;
            LL_WARNS("UI") << "Selected Radia Skin rejected; attempting bundled default Skin." << LL_ENDL;
            skinGenerationResult = prepareSkinGeneration(mResources.captureBundledDefault());
        }
        for (const Diagnostic& warning : skinGenerationResult.warnings) LL_WARNS("UI") << warning.formatted() << LL_ENDL;
        for (const Diagnostic& error : skinGenerationResult.errors) LL_WARNS("UI") << error.formatted() << LL_ENDL;
        if (!skinGenerationResult.ok()) return false;
        mSystem.publish(std::move(skinGenerationResult.generation));
        applySettings();

        const std::string savedLocale = mSavedSettings.getString("Locale");
        if (!savedLocale.empty()) mSystem.setLocale(savedLocale);
        mSavedSettings.setString("Locale", mSystem.activeLocale());
        mSystem.setLocaleChangedHandler([this](const std::string& locale) { mSavedSettings.setString("Locale", locale); });

        mInitialization = InitializationState::Ready;
        return true;
    }

    void shutdown() {
        if (mShuttingDown) return;
        mShuttingDown = true;
        clearInteraction();
        attachedSurface().setFloaterDelegate(nullptr);
        persistWorkspace();
        mDetachedManager.reattachAll(DetachedFloaterManager::ReattachMode::PreservePlacement);
        if (!mComponents.clearInstances()) LL_WARNS("UI") << "Some Radia components could not be cleared during runtime shutdown." << LL_ENDL;
        mSystem.setLocaleChangedHandler({});
        mSystem.setKeybindingResolver({});
        clearDragCursorState();
        mLayoutInitialized.clear();
        mWorkspaceRestored = false;
        mPersistenceDirty = false;
        mInitialization = InitializationState::Failed;
    }

    bool registerFloater(std::string definitionId, std::string resourceId, Runtime::ControllerFactory factory) {
        return mComponents.registerDefinition(std::move(definitionId), std::move(resourceId), std::move(factory));
    }

    Floater* openFloater(const std::string& definitionId, const std::string& instanceKey) {
        if (mInitialization != InitializationState::Ready) return nullptr;
        ComponentOpenResult result = mComponents.open(definitionId, instanceKey);
        logDiagnostics(result);
        if (!result.ok()) return nullptr;
        if (mAttachedSurface.width > 0 && mAttachedSurface.height > 0) layout(mAttachedSurface.width, mAttachedSurface.height);
        mUnrestoredWorkspace.erase({definitionId, instanceKey});
        mPersistenceDirty = true;
        persistWorkspace();
        return result.floater;
    }

    void restoreWorkspace() {
        if (mInitialization != InitializationState::Ready || mWorkspaceRestored) return;
        mRestoringWorkspace = true;
        mUnrestoredWorkspace.clear();
        for (const ComponentKey& component : mComponentPersistence.openComponentKeys())
            if (!openFloater(component.definitionId, component.instanceKey)) mUnrestoredWorkspace.insert(component);
        mRestoringWorkspace = false;
        mWorkspaceRestored = true;
        mPersistenceDirty = true;
        persistWorkspace();
    }

    void endAccountSession() {
        clearInteraction();
        mAttachedSurface.lastFrameTime = {};
        if (mWorkspaceRestored) persistWorkspace();
        mDetachedManager.reattachAll(DetachedFloaterManager::ReattachMode::PreservePlacement);
        if (!mComponents.clearInstances()) {
            LL_WARNS("UI") << "Some Radia components could not be cleared during account transition; retaining the current UI state." << LL_ENDL;
            return;
        }
        mWorkspaceRestored = false;
        mRestoringWorkspace = false;
        mPersistenceDirty = false;
        mUnrestoredWorkspace.clear();
        mLayoutInitialized.clear();
    }

    void requestSkinReload() { mReloadCoordinator.request(); }

    void setVisibility(bool attachedVisible, bool detachedVisible) {
        if (mAttachedSurface.visible != attachedVisible) {
            mAttachedSurface.visible = attachedVisible;
            mAttachedSurface.lastFrameTime = {};
            if (!mAttachedSurface.visible) clearInteraction();
        }
        if (mDetachedVisible != detachedVisible) {
            mDetachedVisible = detachedVisible;
            mDetachedManager.setVisible(detachedVisible);
        }
    }

    void frame(int width, int height) {
        if (!isInteractive() || width <= 0 || height <= 0) return;
        const auto now = currentTime();
        if (width != mAttachedSurface.width || height != mAttachedSurface.height) layout(width, height);
        attachedSurface().refreshHover();
        if (mAttachedSurface.lastFrameTime != TimePoint())
            attachedSurface().update(std::chrono::duration_cast<std::chrono::milliseconds>(now - mAttachedSurface.lastFrameTime));
        mAttachedSurface.lastFrameTime = now;
        attachedSurface().paint(*mAttachedSurface.paintContext);
    }

    NativeInputDispatchResult pointerMove(const PointerEvent& event) {
        if (!isInteractive()) return {};
        PointerEvent routed = event;
        if (cursorMagnetActive()) {
            const float cursorRight = std::max(0.f, static_cast<float>(mAttachedSurface.width) - 1.f);
            const float cursorTop = std::max(0.f, static_cast<float>(mAttachedSurface.height) - 1.f);
            routed.position.x =
                magnetizedAxis(event.position.x, event.delta.x, 0.f, cursorRight, mAttachedSurface.magnetX, mAttachedSurface.virtualPointer.x);
            routed.position.y =
                magnetizedAxis(event.position.y, event.delta.y, 0.f, cursorTop, mAttachedSurface.magnetY, mAttachedSurface.virtualPointer.y);
        } else {
            resetCursorMagnet();
        }

        const bool handled = attachedSurface().pointerMove(routed);
        mDetachedManager.processPendingDetachment();
        setDragCursorClipping(dragCursorClippingRequired());
        return {handled, handled ? std::optional<ECursorType>(translateCursor(attachedSurface().cursor())) : std::nullopt};
    }

    void pointerLeave() {
        if (mInitialization == InitializationState::Ready) attachedSurface().pointerLeave();
    }

    bool dispatchPointerButton(const PointerEvent& event, bool down) {
        if (!isInteractive()) return false;
        if (down && event.button == PointerButton::Left) mAttachedSurface.lastFrameTime = currentTime();
        bool handled = false;
        if (!down && event.button == PointerButton::Left && attachedSurface().hasPointerCapture()) handled = pointerMove(event).handled;
        handled = (down ? attachedSurface().pointerDown(event) : attachedSurface().pointerUp(event)) || handled;
        if (down && !handled) attachedSurface().clearFocus();
        if (down && draggingFloater()) {
            mAttachedSurface.virtualPointer = event.position;
            setDragCursorClipping(dragCursorClippingRequired());
        }
        if (!down || !draggingFloater()) clearDragCursorState();
        return handled;
    }

    NativeInputDispatchResult pointerDown(const PointerEvent& event) { return {dispatchPointerButton(event, true), std::nullopt}; }

    NativeInputDispatchResult pointerUp(const PointerEvent& event) { return {dispatchPointerButton(event, false), std::nullopt}; }

    NativeInputDispatchResult scroll(const ScrollEvent& event) { return {isInteractive() && attachedSurface().scroll(event), std::nullopt}; }

    NativeInputDispatchResult keyDown(const KeyEvent& event) { return {isInteractive() && attachedSurface().keyDown(event), std::nullopt}; }

    NativeInputDispatchResult keyUp(const KeyEvent& event) { return {isInteractive() && attachedSurface().keyUp(event), std::nullopt}; }

    NativeInputDispatchResult character(U32 codepoint) { return {isInteractive() && attachedSurface().charInput(codepoint), std::nullopt}; }

    bool hasPointerCapture() const { return attachedSurface().hasPointerCapture(); }

    void clearInteraction() {
        if (mInitialization != InitializationState::Uninitialized) attachedSurface().clearInteractionState();
        clearDragCursorState();
    }

    void idle() {
        const RuntimeKeybindingState binding = mKeybindingState ? mKeybindingState() : RuntimeKeybindingState{};
        if (mObservedBindingState != binding) {
            mObservedBindingState = binding;
            mSystem.refreshKeybindings();
        }
        mDetachedManager.update();
        mComponents.idle();
        processReload();
        persistWorkspace();
    }

private:
    Surface& attachedSurface() { return *mAttachedSurface.surface; }
    const Surface& attachedSurface() const { return *mAttachedSurface.surface; }

    TimePoint currentTime() const { return mNow ? mNow() : std::chrono::steady_clock::now(); }

    S32 settingOrDefault(const char* name, S32 fallback) const { return mSavedSettings.getControl(name) ? mSavedSettings.getS32(name) : fallback; }

    void applySettings() {
        const S32 longClickDelay =
            std::max<S32>(1, settingOrDefault(kLongClickDelaySetting, static_cast<S32>(System::defaultLongClickDelay().count())));
        mSystem.setLongClickDelay(std::chrono::milliseconds{longClickDelay});

        radia::viewer::ui::SkinReloadTiming timing;
        timing.scanInterval = std::chrono::milliseconds{
            std::max<S32>(1, settingOrDefault(kSkinReloadScanIntervalSetting, static_cast<S32>(timing.scanInterval.count())))};
        timing.settleInterval = std::chrono::milliseconds{
            std::max<S32>(1, settingOrDefault(kSkinReloadSettleIntervalSetting, static_cast<S32>(timing.settleInterval.count())))};
        mReloadCoordinator.setAutoReloadTiming(timing);
    }

    static void logDiagnostics(const DiagnosticResult& result) {
        for (const Diagnostic& warning : result.warnings) LL_WARNS("UI") << warning.code << ": " << warning.formatted() << LL_ENDL;
        for (const Diagnostic& error : result.errors) LL_WARNS("UI") << error.code << ": " << error.formatted() << LL_ENDL;
    }

    void rejectReload(const DiagnosticResult& result) {
        logDiagnostics(result);
        LL_WARNS("UI") << "Candidate Skin Generation rejected; generation " << mSystem.generation() << " remains live." << LL_ENDL;
        mComponents.reportReloadFailed(result);
    }

    void persistWorkspace() {
        if (!mWorkspaceRestored || !mPersistenceDirty || mRestoringWorkspace) return;
        std::vector<ComponentInstanceState> states;
        mComponents.forEachOpen(
            [&](const ComponentKey& componentKey, Floater& floater) { states.push_back({componentKey, floater.minimized(), isDetached(floater)}); });

        const std::vector<ComponentKey> preserved(mUnrestoredWorkspace.begin(), mUnrestoredWorkspace.end());
        mComponentPersistence.saveWorkspace(states, preserved);
        mPersistenceDirty = false;
    }

    void processReload() {
        applySettings();
        mReloadCoordinator.setSkinAutoReload(mSavedSettings.getBOOL("SkinAutoReload"));
        if (mInitialization != InitializationState::Ready) return;

        std::optional<radia::viewer::ui::SkinReloadResult> result = mReloadCoordinator.update(currentTime(), mComponents);
        if (!result) return;
        if (!result->ok()) {
            rejectReload(*result);
            return;
        }

        logDiagnostics(*result);
        mComponents.reportReloadSucceeded();
        saveAttachedPlacements();
        LL_INFOS("UI") << "Committed Skin Generation " << result->generationNumber << "." << LL_ENDL;
    }

    bool isInteractive() const {
        if (mInitialization != InitializationState::Ready || !mAttachedSurface.visible) return false;
        bool interactive = false;
        mComponents.forEachOpen([&](const ComponentKey&, Floater& floater) {
            if (!isDetached(floater) && floater.visibility() == Visibility::Visible) interactive = true;
        });
        return interactive;
    }

    static float magnetizedAxis(float position, float delta, float minimum, float maximum, int& direction, float& virtualPosition) {
        if (direction == 0) {
            if (position <= minimum && delta < 0.f) {
                direction = -1;
                virtualPosition += delta;
                return virtualPosition;
            } else if (position >= maximum && delta > 0.f) {
                direction = 1;
                virtualPosition += delta;
                return virtualPosition;
            }
            virtualPosition = position;
            return position;
        }

        if ((direction < 0 && position > minimum) || (direction > 0 && position < maximum)) {
            direction = 0;
            virtualPosition = position;
            return position;
        }

        virtualPosition += delta;
        return virtualPosition;
    }

    bool cursorMagnetActive() const {
        const Floater* floater = draggingFloater();
        return floater && floater->canDetach() && !floater->minimized();
    }

    bool dragCursorClippingRequired() const {
        const Floater* floater = draggingFloater();
        return floater && (floater->canDetach() || floater->minimized());
    }

    Floater* draggingFloater() const {
        Floater* result = nullptr;
        mComponents.forEachOpen([&](const ComponentKey&, Floater& floater) {
            if (!result && !isDetached(floater) && floater.dragging()) result = &floater;
        });
        return result;
    }

    void setDragCursorClipping(bool enabled) {
        if (mAttachedSurface.dragCursorClipping == enabled) return;
        mAttachedSurface.dragCursorClipping = enabled;
        mWindowAdapter.setMouseClipping(enabled);
    }

    void resetCursorMagnet() {
        mAttachedSurface.magnetX = 0;
        mAttachedSurface.magnetY = 0;
    }

    void clearDragCursorState() {
        resetCursorMagnet();
        setDragCursorClipping(false);
    }

    bool isDetached(const Floater& floater) const { return mDetachedManager.isDetached(floater); }

    bool canDetachFloater(const Surface& surface, const Floater&) const override { return &surface == &attachedSurface(); }

    void floaterClosed(Surface& surface, Floater&) override {
        if (&surface == &attachedSurface()) {
            clearInteraction();
            mPersistenceDirty = true;
        }
    }

    void floaterMinimizedChanged(Surface& surface, Floater& floater, bool) override {
        if (&surface == &attachedSurface()) {
            saveAttachedPlacement(floater);
            mPersistenceDirty = true;
        }
    }

    void floaterMoveEnded(Surface& surface, Floater& floater) override {
        if (&surface == &attachedSurface()) {
            if (!floater.minimized()) saveAttachedPlacement(floater);
            mPersistenceDirty = true;
        }
    }

    void floaterResized(Surface& surface, Floater& floater, bool complete) override {
        if (complete && &surface == &attachedSurface()) {
            saveAttachedPlacement(floater);
            mPersistenceDirty = true;
        }
    }

    void floaterDetachRequested(Surface& surface, Floater& floater, const Vec2& desired, const Vec2& dragOffset) override {
        if (&surface == &attachedSurface())
            if (const std::optional<ComponentKey> componentKey = mComponents.componentKeyFor(floater))
                mDetachedManager.requestDetach(*componentKey, floater, desired, dragOffset);
    }

    void saveAttachedPlacement(const Floater& floater) {
        if (const std::optional<ComponentKey> componentKey = mComponents.componentKeyFor(floater)) saveAttachedPlacement(*componentKey, floater);
    }

    void saveAttachedPlacement(const ComponentKey& componentKey, const Floater& floater) {
        mComponentPersistence.saveAttachedPlacement(componentKey, floater);
    }

    void saveAttachedPlacements() {
        mComponents.forEachOpen([this](const ComponentKey& componentKey, Floater& floater) {
            if (!isDetached(floater)) saveAttachedPlacement(componentKey, floater);
        });
    }

    void restorePlacement(const ComponentKey& componentKey, Floater& floater) {
        const std::optional<radia::viewer::ui::FloaterPlacement> placement = mComponentPersistence.restorePlacement(componentKey);
        if (!placement) return;
        if (const auto* detachedPlacement = std::get_if<radia::viewer::ui::DetachedFloaterPlacement>(&*placement)) {
            if (!floater.canDetach()) {
                saveAttachedPlacement(componentKey, floater);
                return;
            }
            if (mDetachedManager.restoreDetachedPlacement(componentKey, floater, *detachedPlacement)) return;
            saveAttachedPlacement(componentKey, floater);
            return;
        }
        const auto& attached = std::get<radia::viewer::ui::AttachedFloaterPlacement>(*placement);
        Rect saved{attached.x, attached.y, floater.rect().w, floater.rect().h};
        if (floater.canResize() && attached.size) {
            saved.w = attached.size->width;
            saved.h = attached.size->height;
        }
        attachedSurface().placeFloater(floater, saved);
        attachedSurface().updateLayout();
        if (attached.minimized && floater.canMinimize()) floater.setMinimized(true);
        saveAttachedPlacement(componentKey, floater);
    }

    void layout(int width, int height) {
        mAttachedSurface.width = width;
        mAttachedSurface.height = height;
        attachedSurface().setViewport(static_cast<float>(width), static_cast<float>(height));
        mComponents.forEachOpen([&](const ComponentKey& componentKey, Floater& floater) {
            auto found = mLayoutInitialized.find(componentKey);
            if (found != mLayoutInitialized.end() && found->second.get() == &floater) return;
            mLayoutInitialized[componentKey].set(&floater);
            if (isDetached(floater)) return;
            if (const std::optional<Rect> prepared = attachedSurface().prepareFloater(floater)) attachedSurface().placeFloater(floater, *prepared);
            restorePlacement(componentKey, floater);
        });
        attachedSurface().updateLayout();
        attachedSurface().refreshHover();
    }

    LLControlGroup& mSavedSettings;
    KeybindingResolver mResolveKeybinding;
    KeybindingStateProvider mKeybindingState;
    Clock mNow;
    ComponentPersistence mComponentPersistence;
    AuxiliaryWindowFactory& mAuxiliaryWindowFactory;
    LLGLSLShader& mUiShader;
    radia::viewer::ui::SkinResources mResources;
    RuntimeSkinSource mSnapshotSource;
    System mSystem;
    radia::viewer::ui::SkinReloadCoordinator mReloadCoordinator;
    AttachedSurfaceState mAttachedSurface;
    RuntimeWindowAdapter mWindowAdapter;
    radia::viewer::ui::DetachedFloaterManager mDetachedManager;
    radia::viewer::ui::FloaterHost mFloaterHost;
    radia::viewer::ui::SettingsAdapter mSettingsAdapter;
    radia::viewer::ui::ComponentManager mComponents;
    InitializationState mInitialization = InitializationState::Uninitialized;
    bool mDetachedVisible = true;
    bool mWorkspaceRestored = false;
    bool mRestoringWorkspace = false;
    bool mPersistenceDirty = false;
    std::set<ComponentKey> mUnrestoredWorkspace;
    std::optional<RuntimeKeybindingState> mObservedBindingState;
    std::map<ComponentKey, WidgetRef<Floater>> mLayoutInitialized;
    bool mShuttingDown = false;
};

Runtime::Runtime(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, WindowEnvironment window,
                 IntegrationHooks hooks)
    : mImpl(std::make_unique<Impl>(savedSettings, perAccountSettings, uiShader, std::move(window), std::move(hooks))) {}
Runtime::~Runtime() = default;

bool Runtime::initialize() {
    return mImpl->initialize();
}

void Runtime::shutdown() {
    mImpl->shutdown();
}

bool Runtime::registerFloater(std::string definitionId, std::string resourceId, ControllerFactory factory) {
    return mImpl->registerFloater(std::move(definitionId), std::move(resourceId), std::move(factory));
}

Floater* Runtime::openFloater(const std::string& definitionId, const std::string& instanceKey) {
    return mImpl->openFloater(definitionId, instanceKey);
}

void Runtime::restoreWorkspace() {
    mImpl->restoreWorkspace();
}

void Runtime::endAccountSession() {
    mImpl->endAccountSession();
}

void Runtime::requestSkinReload() {
    mImpl->requestSkinReload();
}

void Runtime::setVisibility(bool attachedVisible, bool detachedVisible) {
    mImpl->setVisibility(attachedVisible, detachedVisible);
}

void Runtime::frame(S32 width, S32 height) {
    mImpl->frame(width, height);
}

void Runtime::idle() {
    mImpl->idle();
}

bool Runtime::hasPointerCapture() const {
    return mImpl->hasPointerCapture();
}

NativeInputDispatchResult Runtime::pointerMove(F32 x, F32 y, U32 modifiers, F32 deltaX, F32 deltaY) {
    return mImpl->pointerMove(x, y, modifiers, deltaX, deltaY);
}

NativeInputDispatchResult Runtime::pointerDown(F32 x, F32 y, NativePointerButton button, U32 modifiers, U8 clickCount, F32 deltaX, F32 deltaY) {
    return mImpl->pointerDown(x, y, button, modifiers, clickCount, deltaX, deltaY);
}

NativeInputDispatchResult Runtime::pointerUp(F32 x, F32 y, NativePointerButton button, U32 modifiers, U8 clickCount, F32 deltaX, F32 deltaY) {
    return mImpl->pointerUp(x, y, button, modifiers, clickCount, deltaX, deltaY);
}

void Runtime::pointerLeave() {
    mImpl->pointerLeave();
}

NativeInputDispatchResult Runtime::scroll(S32 x, S32 y, F32 horizontal, F32 vertical, U32 modifiers) {
    return mImpl->scroll(x, y, horizontal, vertical, modifiers);
}

NativeInputDispatchResult Runtime::keyDown(KEY key, U32 modifiers, bool repeated) {
    return mImpl->keyDown(key, modifiers, repeated);
}

NativeInputDispatchResult Runtime::keyUp(KEY key, U32 modifiers) {
    return mImpl->keyUp(key, modifiers);
}

NativeInputDispatchResult Runtime::character(U32 codepoint, U32 modifiers) {
    return mImpl->character(codepoint, modifiers);
}

void Runtime::focusLost() {
    mImpl->focusLost();
}

void Runtime::mouseCaptureLost() {
    mImpl->mouseCaptureLost();
}
} // namespace radia::viewer::ui
