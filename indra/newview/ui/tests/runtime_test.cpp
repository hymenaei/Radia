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
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <string_view>
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

namespace {
using radia::ui::Button;
using radia::ui::Floater;
using radia::ui::KeybindingPresentation;
using radia::ui::PaintCommandKind;
using radia::ui::RecordingPaintContext;
using radia::ui::Rect;
using radia::ui::System;
using radia::ui::Vec2;
using radia::ui::Widget;
using radia::viewer::ui::ComponentController;
using radia::viewer::ui::NativePointerButton;
using radia::viewer::ui::Runtime;
using radia::viewer::ui::RuntimeKeybindingState;
using radia::viewer::ui::SkinSnapshotResult;

SkinSnapshotResult runtimeSkinSnapshot() {
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {name: English, strings: {}}}\n";
    constexpr char kSkin[] = "floater { flow: column; } button { size: 128px 32px; }";
    constexpr char kView[] = "<floater><button id=\"press\" onClick=\"press()\"/></floater>";

    SkinSnapshotResult result;
    result.snapshot.add("localization.yaml", kLocalization);
    result.snapshot.add("skin.radia", kSkin);
    result.snapshot.add("view.xml", kView);
    return result;
}

struct RuntimeControllerState {
    int postBuild = 0;
    int opened = 0;
    int closed = 0;
    int presses = 0;
};

Widget* findWidget(Widget& root, std::string_view id) {
    if (root.id() == id) return &root;
    for (const auto& child : root.children())
        if (Widget* found = findWidget(*child, id)) return found;
    return nullptr;
}

class RuntimeController final : public ComponentController {
public:
    RuntimeController(System& system, RuntimeControllerState& state) : ComponentController(system), mState(state) {
        event("press", &RuntimeController::press);
    }

    void postBuild() override { ++mState.postBuild; }
    void onOpen() override { ++mState.opened; }
    void onClose() override { ++mState.closed; }

private:
    void press() { ++mState.presses; }

    RuntimeControllerState& mState;
};

class RuntimeTest : public ::testing::Test {
protected:
    struct AuxiliaryWindows final : AuxiliaryWindowFactory {
        std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect&, const std::string&, AuxiliaryWindowClient&) override { return {}; }

        bool placementVisible(const AuxiliaryWindowRect&) const override { return false; }
    } auxiliaryWindows;

    RuntimeTest()
        : runtime(savedSettings, accountSettings, uiShader,
                  Runtime::WindowEnvironment{
                      .mainWindow = mainWindow, .displayScale = [] { return Vec2{1.f, 1.f}; }, .auxiliaryWindowFactory = auxiliaryWindows},
                  Runtime::IntegrationHooks{.resolveKeybinding = [](const std::string&) { return KeybindingPresentation{}; },
                                            .keybindingState = [] { return RuntimeKeybindingState{}; },
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
                                                [this](LLGLSLShader&, System&) {
                                                    auto context = std::make_unique<RecordingPaintContext>();
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
                                       [this](System& system) { return std::make_unique<RuntimeController>(system, controllerState); });
    }

    LLControlGroup savedSettings{"RuntimeSaved"};
    LLControlGroup accountSettings{"RuntimeAccount"};
    LLGLSLShader uiShader;
    LLWindow* mainWindow = nullptr;
    SkinSnapshotResult snapshot = runtimeSkinSnapshot();
    int captures = 0;
    int nowCalls = 0;
    std::chrono::steady_clock::time_point now;
    RuntimeControllerState controllerState;
    RecordingPaintContext* paintContext = nullptr;
    Runtime runtime;
};

TEST_F(RuntimeTest, IgnoresInputBeforeInitialization) {
    EXPECT_FALSE(runtime.pointerMove(10.f, 20.f, 0).handled);
    EXPECT_FALSE(runtime.pointerDown(10.f, 20.f, NativePointerButton::Left, 0).handled);
    EXPECT_FALSE(runtime.pointerUp(10.f, 20.f, NativePointerButton::Left, 0).handled);
    EXPECT_FALSE(runtime.scroll(10, 20, 0.f, 1.f, 0).handled);
    EXPECT_FALSE(runtime.keyDown(KEY_NONE, 0).handled);
    EXPECT_FALSE(runtime.keyUp(KEY_NONE, 0).handled);
    EXPECT_FALSE(runtime.character('a', 0).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());

    runtime.pointerLeave();
    runtime.focusLost();
    runtime.mouseCaptureLost();
}

TEST_F(RuntimeTest, LifecycleOperationsAreSafeBeforeInitialization) {
    runtime.setVisibility(false, false);
    runtime.frame(800, 600);
    runtime.restoreWorkspace();
    runtime.requestSkinReload();
    runtime.endAccountSession();

    EXPECT_FALSE(runtime.hasPointerCapture());
}

TEST_F(RuntimeTest, InitializesFromInjectedSkinAndShutsDownCleanly) {
    ASSERT_TRUE(runtime.initialize());
    EXPECT_GT(captures, 0);
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);
    EXPECT_EQ(controllerState.postBuild, 1);
    EXPECT_EQ(controllerState.opened, 1);

    runtime.shutdown();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_EQ(controllerState.closed, 1);
}

TEST_F(RuntimeTest, RoutesAttachedInputToBoundComponent) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    Button* press = dynamic_cast<Button*>(findWidget(*floater, "press"));
    ASSERT_NE(press, nullptr);

    nowCalls = 0;
    runtime.frame(800, 600);
    EXPECT_GT(nowCalls, 0);
    ASSERT_NE(paintContext, nullptr);
    EXPECT_EQ(paintContext->count(PaintCommandKind::BeginFrame), 1);
    EXPECT_EQ(paintContext->count(PaintCommandKind::EndFrame), 1);

    const Rect pressRect = press->rect();
    ASSERT_GT(pressRect.w, 0.f);
    ASSERT_GT(pressRect.h, 0.f);
    const F32 x = pressRect.x + pressRect.w * 0.5f;
    const F32 y = pressRect.y + pressRect.h * 0.5f;
    EXPECT_TRUE(runtime.pointerMove(x, y, 0).handled);
    EXPECT_TRUE(runtime.pointerDown(x, y, NativePointerButton::Left, 0).handled);
    EXPECT_TRUE(runtime.pointerUp(x, y, NativePointerButton::Left, 0).handled);
    EXPECT_EQ(controllerState.presses, 1);
    EXPECT_TRUE(runtime.keyDown(KEY_TAB, 0).handled);
    EXPECT_TRUE(runtime.keyUp(KEY_TAB, 0).handled);
    EXPECT_FALSE(runtime.keyUp(KEY_TAB, MASK_CONTROL).handled);
}

TEST_F(RuntimeTest, VisibilityAndAccountTransitionsClearInteraction) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    Button* press = dynamic_cast<Button*>(findWidget(*floater, "press"));
    ASSERT_NE(press, nullptr);

    runtime.frame(800, 600);
    const Rect pressRect = press->rect();
    const F32 x = pressRect.x + pressRect.w * 0.5f;
    const F32 y = pressRect.y + pressRect.h * 0.5f;
    EXPECT_TRUE(runtime.pointerDown(x, y, NativePointerButton::Left, 0).handled);

    runtime.setVisibility(false, true);
    EXPECT_FALSE(runtime.pointerUp(x, y, NativePointerButton::Left, 0).handled);
    EXPECT_EQ(controllerState.presses, 0);
    nowCalls = 0;
    runtime.frame(800, 600);
    EXPECT_EQ(nowCalls, 0);
    EXPECT_FALSE(runtime.pointerMove(10.f, 20.f, 0).handled);
    EXPECT_FALSE(runtime.keyDown(KEY_RETURN, 0).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());

    runtime.endAccountSession();
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_EQ(controllerState.closed, 1);
}

TEST_F(RuntimeTest, ConsumesUnmodifiedTabWhenVisible) {
    ASSERT_TRUE(runtime.initialize());
    runtime.setVisibility(true, true);

    EXPECT_TRUE(runtime.keyDown(KEY_TAB, 0).handled);
    EXPECT_TRUE(runtime.keyUp(KEY_TAB, 0).handled);
}
} // namespace
