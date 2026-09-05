/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "runtime.h"
#include <algorithm>
#include <chrono>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <utility>
#include <vector>
#include "componentmanager.h"
#include "floaterhost.h"
#include "html/button.h"
#include "html/floater.h"
#include "llgl.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llviewercontrol.h"
#include "llviewershadermgr.h"
#include "llwindow.h"
#include "reloadcoordinator.h"
#include "render/openglpaintcontext.h"
#include "render/paintcontext.h"
#include "resources.h"
#include "settingsadapter.h"
#include "skinpreparation.h"
#include "surface/surface.h"
#include "system.h"
#include "workspacepersistence.h"

namespace {
using radia::ui::OpenGLPaintContext;
using radia::ui::PaintContext;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::System;
using radia::viewer::ui::Runtime;
using radia::viewer::ui::SkinSnapshotResult;
using radia::viewer::ui::SkinSnapshotSource;

enum class InitializationState { Uninitialized, Ready, Failed };

constexpr char kSkinReloadScanIntervalSetting[] = "SkinAutoReloadScanInterval";
constexpr char kSkinReloadSettleIntervalSetting[] = "SkinAutoReloadSettleInterval";

struct SurfaceState final {
    SurfaceState(LLGLSLShader& uiShader, System& system, Runtime::PaintContextFactory paintContextFactory)
        : paintContext(paintContextFactory ? paintContextFactory(uiShader, system) : std::make_unique<OpenGLPaintContext>(uiShader, system)),
          surface(system.createSurface(*paintContext)) {}

    std::unique_ptr<PaintContext> paintContext;
    std::unique_ptr<Surface> surface;
    int width = 0;
    int height = 0;
    bool visible = true;
    bool dragCursorClipping = false;
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
} // namespace

namespace radia::viewer::ui {
using radia::ui::CursorStyle;
using radia::ui::Diagnostic;
using radia::ui::DiagnosticResult;
using radia::ui::HTMLFloaterElement;
using radia::ui::KeybindingPresentation;
using radia::ui::KeyEvent;
using radia::ui::kKeyTab;
using radia::ui::kModifierShift;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::Rect;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::SurfaceFloaterDelegate;
using radia::ui::System;
using radia::ui::WheelEvent;

struct Runtime::Impl final : private SurfaceFloaterDelegate {
public:
    using KeybindingResolver = Runtime::KeybindingResolver;
    using KeybindingStateProvider = Runtime::KeybindingStateProvider;
    using Clock = Runtime::Clock;
    using TimePoint = std::chrono::steady_clock::time_point;

    Impl(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, LLWindow* mainWindow,
         Runtime::IntegrationHooks integrationHooks, Runtime::TestOverrides testOverrides)
        : mSavedSettings(savedSettings), mResolveKeybinding(std::move(integrationHooks.resolveKeybinding)),
          mKeybindingState(std::move(integrationHooks.keybindingState)), mNow(std::move(testOverrides.now)),
          mFailTeardown(std::move(testOverrides.failTeardown)), mWorkspacePersistence(savedSettings, perAccountSettings), mMainWindow(mainWindow),
          mResources(), mSnapshotSource(mResources, std::move(testOverrides.captureSkin)), mSystem(), mReloadCoordinator(mSystem, mSnapshotSource),
          mSurfaceState(uiShader, mSystem, std::move(testOverrides.paintContext)), mFloaterHost(*mSurfaceState.surface),
          mSettingsAdapter(savedSettings), mComponents(mSystem, mFloaterHost, mSettingsAdapter) {
        mSurfaceState.surface->setFloaterDelegate(this);
        mSystem.setKeybindingResolver([this](const std::string& authoredId) {
            if (!mResolveKeybinding) return KeybindingPresentation{};
            std::string viewerCommand = authoredId;
            std::replace(viewerCommand.begin(), viewerCommand.end(), '-', '_');
            return mResolveKeybinding(viewerCommand);
        });
    }

    ~Impl() {
        shutdown();
        if (mState != RuntimeState::Stopped) {
            mFailTeardown = {};
            if (!mComponents.clearInstances()) LL_ERRS("UI") << "Runtime could not clear component owners before destruction." << LL_ENDL;
        }
        mSurfaceState.surface->setFloaterDelegate(nullptr);
        mSystem.setLocaleChangedHandler({});
        mSystem.setKeybindingResolver({});
    }

    void focusLost() { clearInteraction(); }

    void pointerCaptureLost() { clearInteraction(); }

    bool initialize() {
        if (mState != RuntimeState::Running) return false;
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
        if (!mSystem.publish(std::move(skinGenerationResult.generation))) return false;
        applySettings();

        const std::string savedLocale = mSavedSettings.getString("Locale");
        if (!savedLocale.empty()) mSystem.setLocale(savedLocale);
        mSavedSettings.setString("Locale", mSystem.activeLocale());
        mSystem.setLocaleChangedHandler([this](const std::string& locale) { mSavedSettings.setString("Locale", locale); });

        mInitialization = InitializationState::Ready;
        return true;
    }

    void shutdown() {
        if (mState == RuntimeState::Stopped || mState == RuntimeState::ShuttingDown) return;
        mState = RuntimeState::ShuttingDown;
        clearInteraction();
        persistWorkspace();
        if ((mFailTeardown && mFailTeardown()) || !mComponents.clearInstances()) {
            LL_WARNS("UI") << "Some Radia controllers could not be cleared during runtime shutdown; shutdown can be retried." << LL_ENDL;
            mState = RuntimeState::TeardownFailed;
            return;
        }
        surface().setFloaterDelegate(nullptr);
        mSystem.setLocaleChangedHandler({});
        mSystem.setKeybindingResolver({});
        clearDragCursorState();
        mLayoutInitialized.clear();
        mWorkspaceRestored = false;
        mPersistenceDirty = false;
        mInitialization = InitializationState::Failed;
        mState = RuntimeState::Stopped;
    }

    RuntimeState lifecycleState() const { return mState; }

    bool registerFloater(std::string definitionId, std::string resource, Runtime::ControllerFactory factory) {
        if (mState != RuntimeState::Running) return false;
        return mComponents.registerDefinition(std::move(definitionId), std::move(resource), std::move(factory));
    }

    HTMLFloaterElement* openFloater(const std::string& definitionId, const std::string& instanceKey) {
        if (mState != RuntimeState::Running || mInitialization != InitializationState::Ready) return nullptr;
        ComponentOpenResult result = mComponents.open(definitionId, instanceKey);
        logDiagnostics(result);
        if (!result.ok()) return nullptr;
        if (mSurfaceState.width > 0 && mSurfaceState.height > 0) layout(mSurfaceState.width, mSurfaceState.height);
        mUnrestoredWorkspace.erase({definitionId, instanceKey});
        mPersistenceDirty = true;
        persistWorkspace();
        return result.floater;
    }

    void restoreWorkspace() {
        if (mState != RuntimeState::Running || mInitialization != InitializationState::Ready || mWorkspaceRestored) return;
        mRestoringWorkspace = true;
        mUnrestoredWorkspace.clear();
        for (const ComponentInstanceKey& component : mWorkspacePersistence.openComponentKeys())
            if (!openFloater(component.definitionId, component.instanceKey)) mUnrestoredWorkspace.insert(component);
        mRestoringWorkspace = false;
        mWorkspaceRestored = true;
        mPersistenceDirty = true;
        persistWorkspace();
    }

    void endAccountSession() {
        if (mState != RuntimeState::Running) return;
        clearInteraction();
        if (mWorkspaceRestored) persistWorkspace();
        if (!mComponents.clearInstances()) {
            LL_WARNS("UI") << "Some Radia controllers could not be cleared during account transition; retaining the current UI state." << LL_ENDL;
            return;
        }
        mWorkspaceRestored = false;
        mRestoringWorkspace = false;
        mPersistenceDirty = false;
        mUnrestoredWorkspace.clear();
        mLayoutInitialized.clear();
    }

    void requestSkinReload() {
        if (mState == RuntimeState::Running) mReloadCoordinator.request();
    }

    void setVisibility(bool visible) {
        if (mSurfaceState.visible != visible) {
            mSurfaceState.visible = visible;
            if (!mSurfaceState.visible) clearInteraction();
        }
    }

    void frame(int width, int height, float paintScale, float paintOriginX, float paintOriginY) {
        if (!isInteractive() || width <= 0 || height <= 0) return;
        const TimePoint frameTime = currentTime();
        const float deltaSeconds = mPreviousFrameTime ? std::max(0.f, std::chrono::duration<float>(frameTime - *mPreviousFrameTime).count()) : 0.f;
        mPreviousFrameTime = frameTime;
        surface().advanceScrollbarInteraction(deltaSeconds);
        if (width != mSurfaceState.width || height != mSurfaceState.height) layout(width, height);
        surface().refreshHover();
        surface().paint(*mSurfaceState.paintContext, std::max(paintScale, .0001f), {paintOriginX, paintOriginY});
    }

    InputDispatchResult pointerMove(const PointerEvent& event) {
        if (!isInteractive()) return {};
        const bool handled = surface().pointerMove(event);
        setDragCursorClipping(dragCursorClippingRequired());
        return {handled, handled ? std::optional<CursorStyle>(surface().cursor()) : std::nullopt};
    }

    void pointerLeave() {
        if (mState == RuntimeState::Running && mInitialization == InitializationState::Ready) surface().pointerLeave();
    }

    bool dispatchPointerButton(const PointerEvent& event, bool down) {
        if (!isInteractive()) return false;
        if (down) mPreviousFrameTime.reset();
        bool handled = false;
        if (!down && event.button == PointerButton::Left && surface().hasPointerCapture()) handled = pointerMove(event).handled;
        handled = (down ? surface().pointerDown(event) : surface().pointerUp(event)) || handled;
        if (!down) mPreviousFrameTime.reset();
        if (down && !handled) surface().clearFocus();
        if (down && draggingFloater()) setDragCursorClipping(dragCursorClippingRequired());
        if (!down || !draggingFloater()) clearDragCursorState();
        return handled;
    }

    InputDispatchResult pointerDown(const PointerEvent& event) { return {dispatchPointerButton(event, true), std::nullopt}; }

    InputDispatchResult pointerUp(const PointerEvent& event) { return {dispatchPointerButton(event, false), std::nullopt}; }

    InputDispatchResult scroll(const WheelEvent& event) { return {isInteractive() && surface().scroll(event), std::nullopt}; }

    InputDispatchResult keyDown(const KeyEvent& event) {
        const bool ownsTab =
            mState == RuntimeState::Running && mInitialization == InitializationState::Ready && mSurfaceState.visible && isSurfaceTab(event);
        const bool handled = (isInteractive() || ownsTab) && surface().keyDown(event);
        if (isSurfaceTab(event)) mTabKeyOwned = ownsTab;
        return {handled || ownsTab, std::nullopt};
    }

    InputDispatchResult keyUp(const KeyEvent& event) {
        const bool tabKey = event.key == kKeyTab;
        const bool owned = tabKey && mTabKeyOwned;
        if (tabKey) mTabKeyOwned = false;
        const bool handled = (isInteractive() || owned) && surface().keyUp(event);
        return {owned || handled, std::nullopt};
    }

    InputDispatchResult character(std::uint32_t codepoint) { return {isInteractive() && surface().charInput(codepoint), std::nullopt}; }

    bool hasPointerCapture() const { return surface().hasPointerCapture(); }

    void clearInteraction() {
        if (mInitialization != InitializationState::Uninitialized) surface().clearInteractionState();
        mPreviousFrameTime.reset();
        mTabKeyOwned = false;
        clearDragCursorState();
    }

    void idle() {
        if (mState != RuntimeState::Running || mInitialization != InitializationState::Ready) return;
        const RuntimeKeybindingState binding = mKeybindingState ? mKeybindingState() : RuntimeKeybindingState{};
        if (mObservedBindingState != binding) {
            mObservedBindingState = binding;
            mSystem.refreshKeybindings();
        }
        mComponents.idle();
        processReload();
        persistWorkspace();
    }

private:
    Surface& surface() { return *mSurfaceState.surface; }
    const Surface& surface() const { return *mSurfaceState.surface; }

    TimePoint currentTime() const { return mNow ? mNow() : std::chrono::steady_clock::now(); }

    S32 settingOrDefault(const char* name, S32 fallback) const { return mSavedSettings.getControl(name) ? mSavedSettings.getS32(name) : fallback; }

    void applySettings() {
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
            [&](const ComponentInstanceKey& componentKey, HTMLFloaterElement& floater) { states.push_back({componentKey, floater.minimized()}); });

        const std::vector<ComponentInstanceKey> preserved(mUnrestoredWorkspace.begin(), mUnrestoredWorkspace.end());
        mWorkspacePersistence.saveWorkspace(states, preserved);
        mPersistenceDirty = false;
    }

    void processReload() {
        applySettings();
        mReloadCoordinator.setSkinAutoReload(mSavedSettings.getBOOL("SkinAutoReload"));
        if (mState != RuntimeState::Running || mInitialization != InitializationState::Ready) return;

        std::optional<radia::viewer::ui::SkinReloadResult> result = mReloadCoordinator.update(currentTime(), mComponents);
        if (!result) return;
        if (!result->ok()) {
            rejectReload(*result);
            return;
        }

        logDiagnostics(*result);
        mComponents.reportReloadSucceeded();
        saveFloaterPlacements();
        LL_INFOS("UI") << "Committed Skin Generation " << result->generationNumber << "." << LL_ENDL;
    }

    bool isInteractive() const {
        return mState == RuntimeState::Running
            && mInitialization == InitializationState::Ready
            && mSurfaceState.visible
            && mSurfaceState.surface->hasVisibleFloater();
    }

    static bool isSurfaceTab(const KeyEvent& event) { return event.key == kKeyTab && (event.modifiers & ~kModifierShift) == 0; }

    bool dragCursorClippingRequired() const {
        const HTMLFloaterElement* floater = draggingFloater();
        return floater && floater->minimized();
    }

    HTMLFloaterElement* draggingFloater() const {
        HTMLFloaterElement* result = nullptr;
        mComponents.forEachOpen([&](const ComponentInstanceKey&, HTMLFloaterElement& floater) {
            if (!result && floater.dragging()) result = &floater;
        });
        return result;
    }

    void setDragCursorClipping(bool enabled) {
        if (mSurfaceState.dragCursorClipping == enabled) return;
        mSurfaceState.dragCursorClipping = enabled;
        if (mMainWindow) mMainWindow->setMouseClipping(enabled);
    }

    void clearDragCursorState() { setDragCursorClipping(false); }

    void floaterClosed(Surface&, HTMLFloaterElement&) override {
        clearInteraction();
        mPersistenceDirty = true;
    }

    void floaterMinimizedChanged(Surface&, HTMLFloaterElement& floater) override {
        saveFloaterPlacement(floater);
        mPersistenceDirty = true;
    }

    void floaterMoveEnded(Surface&, HTMLFloaterElement& floater) override {
        saveFloaterPlacement(floater);
        mPersistenceDirty = true;
    }

    void floaterResizeEnded(Surface&, HTMLFloaterElement& floater) override {
        saveFloaterPlacement(floater);
        mPersistenceDirty = true;
    }

    void saveFloaterPlacement(const HTMLFloaterElement& floater) {
        if (const std::optional<ComponentInstanceKey> componentKey = mComponents.componentKeyFor(floater))
            saveFloaterPlacement(*componentKey, floater);
    }

    void saveFloaterPlacement(const ComponentInstanceKey& componentKey, const HTMLFloaterElement& floater) {
        mWorkspacePersistence.saveFloaterPlacement(componentKey, floater);
    }

    void saveFloaterPlacements() {
        mComponents.forEachOpen(
            [this](const ComponentInstanceKey& componentKey, HTMLFloaterElement& floater) { saveFloaterPlacement(componentKey, floater); });
    }

    void restorePlacement(const ComponentInstanceKey& componentKey, HTMLFloaterElement& floater) {
        const std::optional<radia::viewer::ui::FloaterPlacement> placement = mWorkspacePersistence.restorePlacement(componentKey);
        if (!placement) return;
        const auto& restored = *placement;
        Rect saved{restored.x, restored.y, floater.rect().w, floater.rect().h};
        if (floater.resizeable() && restored.size) {
            saved.w = restored.size->width;
            saved.h = restored.size->height;
        }
        surface().placeFloater(floater, saved);
        surface().updateLayout();
        if (restored.minimized && floater.minimizable()) floater.setMinimized(true);
        saveFloaterPlacement(componentKey, floater);
    }

    void layout(int width, int height) {
        mSurfaceState.width = width;
        mSurfaceState.height = height;
        surface().setViewport(static_cast<float>(width), static_cast<float>(height));
        mComponents.forEachOpen([&](const ComponentInstanceKey& componentKey, HTMLFloaterElement& floater) {
            auto found = mLayoutInitialized.find(componentKey);
            if (found != mLayoutInitialized.end() && found->second == &floater) return;
            mLayoutInitialized[componentKey] = &floater;
            if (const std::optional<Rect> prepared = surface().prepareFloater(floater)) surface().placeFloater(floater, *prepared);
            restorePlacement(componentKey, floater);
        });
        surface().updateLayout();
        surface().refreshHover();
    }

    LLControlGroup& mSavedSettings;
    KeybindingResolver mResolveKeybinding;
    KeybindingStateProvider mKeybindingState;
    Clock mNow;
    std::function<bool()> mFailTeardown;
    std::optional<TimePoint> mPreviousFrameTime;
    WorkspacePersistence mWorkspacePersistence;
    LLWindow* mMainWindow;
    radia::viewer::ui::SkinResources mResources;
    RuntimeSkinSource mSnapshotSource;
    System mSystem;
    radia::viewer::ui::SkinReloadCoordinator mReloadCoordinator;
    SurfaceState mSurfaceState;
    radia::viewer::ui::FloaterHost mFloaterHost;
    radia::viewer::ui::SettingsAdapter mSettingsAdapter;
    radia::viewer::ui::ComponentManager mComponents;
    InitializationState mInitialization = InitializationState::Uninitialized;
    bool mWorkspaceRestored = false;
    bool mRestoringWorkspace = false;
    bool mPersistenceDirty = false;
    std::set<ComponentInstanceKey> mUnrestoredWorkspace;
    std::optional<RuntimeKeybindingState> mObservedBindingState;
    std::map<ComponentInstanceKey, HTMLFloaterElement*> mLayoutInitialized;
    RuntimeState mState = RuntimeState::Running;
    bool mTabKeyOwned = false;
};

Runtime::Runtime(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, LLWindow* mainWindow,
                 IntegrationHooks integrationHooks, TestOverrides testOverrides)
    : mImpl(std::make_unique<Impl>(savedSettings, perAccountSettings, uiShader, mainWindow, std::move(integrationHooks), std::move(testOverrides))) {}
Runtime::~Runtime() = default;

bool Runtime::initialize() {
    return mImpl->initialize();
}

void Runtime::shutdown() {
    mImpl->shutdown();
}

RuntimeState Runtime::lifecycleState() const {
    return mImpl->lifecycleState();
}

bool Runtime::registerFloater(std::string definitionId, std::string resource, ControllerFactory factory) {
    return mImpl->registerFloater(std::move(definitionId), std::move(resource), std::move(factory));
}

HTMLFloaterElement* Runtime::openFloater(const std::string& definitionId, const std::string& instanceKey) {
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

void Runtime::setVisibility(bool visible) {
    mImpl->setVisibility(visible);
}

void Runtime::frame(S32 width, S32 height, F32 paintScale, F32 paintOriginX, F32 paintOriginY) {
    mImpl->frame(width, height, paintScale, paintOriginX, paintOriginY);
}

void Runtime::idle() {
    mImpl->idle();
}

bool Runtime::hasPointerCapture() const {
    return mImpl->hasPointerCapture();
}

InputDispatchResult Runtime::pointerMove(const PointerEvent& event) {
    return mImpl->pointerMove(event);
}

InputDispatchResult Runtime::pointerDown(const PointerEvent& event) {
    return mImpl->pointerDown(event);
}

InputDispatchResult Runtime::pointerUp(const PointerEvent& event) {
    return mImpl->pointerUp(event);
}

void Runtime::pointerLeave() {
    mImpl->pointerLeave();
}

InputDispatchResult Runtime::scroll(const WheelEvent& event) {
    return mImpl->scroll(event);
}

InputDispatchResult Runtime::keyDown(const KeyEvent& event) {
    return mImpl->keyDown(event);
}

InputDispatchResult Runtime::keyUp(const KeyEvent& event) {
    return mImpl->keyUp(event);
}

InputDispatchResult Runtime::character(std::uint32_t codepoint) {
    return mImpl->character(codepoint);
}

void Runtime::focusLost() {
    mImpl->focusLost();
}

void Runtime::pointerCaptureLost() {
    mImpl->pointerCaptureLost();
}
} // namespace radia::viewer::ui
