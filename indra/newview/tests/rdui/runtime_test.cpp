/**
 * @file runtime_test.cpp
 * @brief Tests the viewer-facing UI Runtime lifecycle and input boundary.
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
#include <chrono>
#include "../test/lltut.h"
#include "auxiliarywindow.h"
#include "componentcontroller.h"
#include "componentcontrollerregistration.h"
#include "indra_constants.h"
#include "llcontrol.h"
#include "llglslshader.h"
#include "render/recordingpaintcontext.h"
#include "runtime.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/widget.h"

namespace tut {
namespace {
rdui::viewer::SkinSnapshotResult runtimeSkinSnapshot() {
    rdui::viewer::SkinSnapshotResult result;
    result.snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n");
    result.snapshot.add("skin.radia", "floater { flow: column; } button { size: 128px 32px; }");
    result.snapshot.add("view.xml", "<floater><button id=\"press\" onClick=\"press()\"/></floater>");
    return result;
}

struct RuntimeControllerState {
    int postBuild = 0;
    int opened = 0;
    int closed = 0;
    int presses = 0;
};

rdui::Widget* findWidget(rdui::Widget& root, const std::string& id) {
    if (root.id() == id) return &root;
    for (const auto& child : root.children())
        if (rdui::Widget* found = findWidget(*child, id)) return found;
    return nullptr;
}

class RuntimeController final : public rdui::viewer::ComponentController {
public:
    RuntimeController(rdui::System& system, RuntimeControllerState& state) : ComponentController(system), mState(state) {
        event("press", &RuntimeController::press);
    }

    void postBuild() override { ++mState.postBuild; }
    void onOpen() override { ++mState.opened; }
    void onClose() override { ++mState.closed; }

private:
    void press() { ++mState.presses; }

    RuntimeControllerState& mState;
};
} // namespace

struct runtimeData {
    struct AuxiliaryWindows final : AuxiliaryWindowFactory {
        std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect&, const std::string&, AuxiliaryWindowClient&) override { return {}; }

        bool placementVisible(const AuxiliaryWindowRect&) const override { return false; }
    } auxiliaryWindows;

    LLControlGroup savedSettings{"RuntimeSaved"};
    LLControlGroup accountSettings{"RuntimeAccount"};
    LLGLSLShader uiShader;
    LLWindow* mainWindow = nullptr;
    rdui::viewer::SkinSnapshotResult snapshot = runtimeSkinSnapshot();
    int captures = 0;
    int nowCalls = 0;
    std::chrono::steady_clock::time_point now;
    RuntimeControllerState controllerState;
    rdui::RecordingPaintContext* paintContext = nullptr;
    rdui::viewer::Runtime runtime;

    runtimeData()
        : runtime(savedSettings, accountSettings, uiShader,
                  rdui::viewer::Runtime::WindowEnvironment{
                      .mainWindow = mainWindow, .displayScale = [] { return rdui::Vec2{1.f, 1.f}; }, .auxiliaryWindowFactory = auxiliaryWindows},
                  rdui::viewer::Runtime::IntegrationHooks{.resolveKeybinding = [](const std::string&) { return rdui::KeybindingPresentation{}; },
                                                          .keybindingState = [] { return rdui::viewer::RuntimeKeybindingState{}; },
                                                          .captureSkin =
                                                              [this] {
                                                                  ++captures;
                                                                  return snapshot;
                                                              },
                                                          .now =
                                                              [this] {
                                                                  ++nowCalls;
                                                                  return now;
                                                              },
                                                          .paintContext =
                                                              [this](LLGLSLShader&, rdui::System&) {
                                                                  auto context = std::make_unique<rdui::RecordingPaintContext>();
                                                                  paintContext = context.get();
                                                                  return context;
                                                              }}) {
        savedSettings.declareString("Locale", "", "test locale");
        savedSettings.declareBOOL("SkinAutoReload", false, "test skin reload");
        savedSettings.declareS32("LongClickDelay", 500, "test long click delay");
        savedSettings.declareS32("SkinAutoReloadScanInterval", 250, "test scan interval");
        savedSettings.declareS32("SkinAutoReloadSettleInterval", 150, "test settle interval");
    }

    bool registerTestFloater() {
        return runtime.registerFloater("runtimeTest", "view.xml",
                                       [this](rdui::System& system) { return std::make_unique<RuntimeController>(system, controllerState); });
    }
};
using runtimeTest = test_group<runtimeData>;
using runtimeObject = runtimeTest::object;
runtimeTest runtimeTestCase("UIRuntime");

template<> template<> void runtimeObject::test<1>() {
    set_test_name("Runtime keeps named input operations inert before initialization");
    ensure("pointer move is not handled before initialization", !runtime.pointerMove(10.f, 20.f, 0).handled);
    ensure("pointer down is not handled before initialization", !runtime.pointerDown(10.f, 20.f, rdui::viewer::NativePointerButton::Left, 0).handled);
    ensure("pointer up is not handled before initialization", !runtime.pointerUp(10.f, 20.f, rdui::viewer::NativePointerButton::Left, 0).handled);
    ensure("scroll is not handled before initialization", !runtime.scroll(10, 20, 0.f, 1.f, 0).handled);
    ensure("key down is not handled before initialization", !runtime.keyDown(KEY_NONE, 0).handled);
    ensure("key up is not handled before initialization", !runtime.keyUp(KEY_NONE, 0).handled);
    ensure("character input is not handled before initialization", !runtime.character('a', 0).handled);
    ensure("pointer capture is clear before initialization", !runtime.hasPointerCapture());

    runtime.pointerLeave();
    runtime.focusLost();
    runtime.mouseCaptureLost();
}

template<> template<> void runtimeObject::test<2>() {
    set_test_name("Runtime lifecycle calls are safe while uninitialized");
    runtime.setVisibility(false, false);
    runtime.frame(800, 600);
    runtime.restoreWorkspace();
    runtime.requestSkinReload();
    runtime.endAccountSession();
    ensure("uninitialized Runtime remains without pointer capture", !runtime.hasPointerCapture());
}

template<> template<> void runtimeObject::test<3>() {
    set_test_name("Runtime initializes from an injected skin source and shuts down explicitly");
    ensure("injected skin source initializes Runtime", runtime.initialize());
    ensure("Runtime used the injected skin source", captures > 0);
    ensure("test component registers", registerTestFloater());
    ensure("test component opens", runtime.openFloater("runtimeTest") != nullptr);
    ensure_equals("controller post-build runs", controllerState.postBuild, 1);
    ensure_equals("controller open hook runs", controllerState.opened, 1);

    runtime.shutdown();
    ensure("shutdown releases pointer capture", !runtime.hasPointerCapture());
    ensure_equals("shutdown closes the component", controllerState.closed, 1);
}

template<> template<> void runtimeObject::test<4>() {
    set_test_name("Runtime routes visible attached input through a bound component");
    ensure("Runtime initializes", runtime.initialize());
    ensure("test component registers", registerTestFloater());
    rdui::Floater* floater = runtime.openFloater("runtimeTest");
    ensure("test component opens", floater != nullptr);
    if (!floater) return;

    auto* press = dynamic_cast<rdui::Button*>(findWidget(*floater, "press"));
    ensure("bound Button exists", press != nullptr);
    if (!press) return;

    nowCalls = 0;
    runtime.frame(800, 600);
    ensure("visible frame observes the injected clock", nowCalls > 0);
    ensure("visible frame paints through the Runtime seam",
           paintContext != nullptr
               && paintContext->count(rdui::PaintCommandKind::BeginFrame) == 1
               && paintContext->count(rdui::PaintCommandKind::EndFrame) == 1);
    const rdui::Rect pressRect = press->rect();
    const F32 x = pressRect.x + pressRect.w * 0.5f;
    const F32 y = pressRect.y + pressRect.h * 0.5f;
    ensure("bound Button has a hit-testable rectangle", pressRect.w > 0.f && pressRect.h > 0.f);
    ensure("pointer move reaches the visible component", runtime.pointerMove(x, y, 0).handled);
    ensure("pointer down reaches the visible component", runtime.pointerDown(x, y, rdui::viewer::NativePointerButton::Left, 0).handled);
    ensure("pointer up reaches the pressed component", runtime.pointerUp(x, y, rdui::viewer::NativePointerButton::Left, 0).handled);
    ensure_equals("pointer activation reaches the controller", controllerState.presses, 1);
}

template<> template<> void runtimeObject::test<5>() {
    set_test_name("Runtime clears attached interaction across visibility and account transitions");
    ensure("Runtime initializes", runtime.initialize());
    ensure("test component registers", registerTestFloater());
    rdui::Floater* floater = runtime.openFloater("runtimeTest");
    ensure("test component opens", floater != nullptr);
    if (!floater) return;
    auto* press = dynamic_cast<rdui::Button*>(findWidget(*floater, "press"));
    ensure("bound Button exists", press != nullptr);
    if (!press) return;

    runtime.frame(800, 600);
    const rdui::Rect pressRect = press->rect();
    const F32 x = pressRect.x + pressRect.w * 0.5f;
    const F32 y = pressRect.y + pressRect.h * 0.5f;
    ensure("pointer down starts a visible interaction", runtime.pointerDown(x, y, rdui::viewer::NativePointerButton::Left, 0).handled);

    runtime.setVisibility(false, true);
    ensure("hidden attached UI rejects pointer input", !runtime.pointerUp(x, y, rdui::viewer::NativePointerButton::Left, 0).handled);
    ensure_equals("hiding attached UI cancels the pending activation", controllerState.presses, 0);
    nowCalls = 0;
    runtime.frame(800, 600);
    ensure("hidden attached UI short-circuits the frame clock", nowCalls == 0);
    ensure("hidden attached UI rejects pointer movement", !runtime.pointerMove(10.f, 20.f, 0).handled);
    ensure("hidden attached UI rejects keyboard input", !runtime.keyDown(KEY_RETURN, 0).handled);
    ensure("hidden attached UI clears pointer capture", !runtime.hasPointerCapture());

    runtime.endAccountSession();
    ensure("account transition leaves pointer capture clear", !runtime.hasPointerCapture());
    ensure_equals("account transition closes the component", controllerState.closed, 1);
}
} // namespace tut
