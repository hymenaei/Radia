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
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <string_view>
#include <vector>
#include "componentcontroller.h"
#include "componentcontrollerregistration.h"
#include "llcontrol.h"
#include "llglslshader.h"
#include "llsd.h"
#include "render/recordingpaintcontext.h"
#include "runtime.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/panel.h"
#include "widgets/widget.h"

namespace {
using radia::ui::Button;
using radia::ui::Floater;
using radia::ui::KeybindingPresentation;
using radia::ui::KeyEvent;
using radia::ui::kKeyReturn;
using radia::ui::kKeyTab;
using radia::ui::kModifierControl;
using radia::ui::PaintCommandKind;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::RecordingPaintContext;
using radia::ui::Rect;
using radia::ui::ScrollEvent;
using radia::ui::System;
using radia::ui::Widget;
using radia::viewer::ui::ComponentController;
using radia::viewer::ui::Runtime;
using radia::viewer::ui::RuntimeKeybindingState;
using radia::viewer::ui::SkinSnapshotResult;
using ::testing::Test;

PointerEvent makePointerEvent(float x, float y, PointerButton button = PointerButton::NoButton, std::uint32_t modifiers = 0,
                              std::uint8_t clickCount = 1, float deltaX = 0.f, float deltaY = 0.f) {
    return {{x, y}, button, modifiers, clickCount, {deltaX, deltaY}};
}

ScrollEvent makeScrollEvent(int x, int y, float deltaX, float deltaY, std::uint32_t modifiers = 0) {
    return {{static_cast<float>(x), static_cast<float>(y)}, deltaX, deltaY, modifiers};
}

KeyEvent makeKeyEvent(int key, std::uint32_t modifiers = 0, bool repeated = false) {
    return {key, modifiers, repeated};
}

SkinSnapshotResult runtimeSkinSnapshot() {
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {name: English, strings: {runtime: Runtime}}}\n";
    constexpr char kSkin[] = "floater { display: flex; flex-direction: column; } floater::header { height: 30px; } button { size: 128px 32px; }";
    constexpr char kView[] = "<floater title=\"runtime\"><button id=\"press\" onClick=\"press()\"/></floater>";

    SkinSnapshotResult result;
    result.snapshot.add("localization.yaml", kLocalization);
    result.snapshot.add("skin.radia", kSkin);
    result.snapshot.add("view.xml", kView);
    return result;
}

enum class RuntimeLifecycleEvent { FrameClock, IdleKeybindingState, ControllerClose };

struct RuntimeControllerState {
    int postBuild = 0;
    int opened = 0;
    int closed = 0;
    int presses = 0;
    std::vector<RuntimeLifecycleEvent> lifecycleEvents;
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
    void onClose() override {
        ++mState.closed;
        mState.lifecycleEvents.emplace_back(RuntimeLifecycleEvent::ControllerClose);
    }

private:
    void press() { ++mState.presses; }

    RuntimeControllerState& mState;
};

class RuntimeTest : public Test {
protected:
    RuntimeTest()
        : runtime(savedSettings, accountSettings, uiShader, mainWindow,
                  Runtime::IntegrationHooks{.resolveKeybinding = [](const std::string&) { return KeybindingPresentation{}; },
                                            .keybindingState =
                                                [this] {
                                                    controllerState.lifecycleEvents.emplace_back(RuntimeLifecycleEvent::IdleKeybindingState);
                                                    ++keybindingStateCalls;
                                                    return RuntimeKeybindingState{};
                                                },
                                            .captureSkin =
                                                [this] {
                                                    ++captures;
                                                    return snapshot;
                                                },
                                            .now =
                                                [this] {
                                                    controllerState.lifecycleEvents.emplace_back(RuntimeLifecycleEvent::FrameClock);
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
        savedSettings.declareLLSD("UILayout", LLSD::emptyMap(), "test layout", LLControlVariable::PERSIST_NO);
        savedSettings.declareBOOL("SkinAutoReload", false, "test skin reload");
        savedSettings.declareS32("LongClickDelay", 500, "test long click delay");
        savedSettings.declareS32("SkinAutoReloadScanInterval", 250, "test scan interval");
        savedSettings.declareS32("SkinAutoReloadSettleInterval", 150, "test settle interval");
        accountSettings.declareLLSD("UIWorkspace", LLSD::emptyMap(), "test workspace", LLControlVariable::PERSIST_NO);
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
    int keybindingStateCalls = 0;
    int nowCalls = 0;
    std::chrono::steady_clock::time_point now;
    RuntimeControllerState controllerState;
    RecordingPaintContext* paintContext = nullptr;
    Runtime runtime;
};

TEST_F(RuntimeTest, IgnoresInputBeforeInitialization) {
    EXPECT_FALSE(runtime.pointerMove(makePointerEvent(10.f, 20.f)).handled);
    EXPECT_FALSE(runtime.pointerDown(makePointerEvent(10.f, 20.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.pointerUp(makePointerEvent(10.f, 20.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.scroll(makeScrollEvent(10, 20, 0.f, 1.f)).handled);
    EXPECT_FALSE(runtime.keyDown(makeKeyEvent(0)).handled);
    EXPECT_FALSE(runtime.keyUp(makeKeyEvent(0)).handled);
    EXPECT_FALSE(runtime.character('a').handled);
    EXPECT_FALSE(runtime.hasPointerCapture());

    runtime.pointerLeave();
    runtime.focusLost();
    runtime.mouseCaptureLost();
}

TEST_F(RuntimeTest, LifecycleOperationsAreSafeBeforeInitialization) {
    runtime.setVisibility(false);
    runtime.frame(800, 600);
    runtime.idle();
    runtime.restoreWorkspace();
    runtime.requestSkinReload();
    runtime.endAccountSession();

    EXPECT_EQ(keybindingStateCalls, 0);
    EXPECT_FALSE(runtime.hasPointerCapture());
}

TEST_F(RuntimeTest, InitializationIsIdempotent) {
    ASSERT_TRUE(runtime.initialize());

    EXPECT_TRUE(runtime.initialize());
    EXPECT_EQ(captures, 1);
}

TEST_F(RuntimeTest, ShutdownPreventsReinitialization) {
    runtime.shutdown();

    EXPECT_FALSE(runtime.initialize());
    EXPECT_EQ(captures, 0);
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
    EXPECT_TRUE(runtime.pointerMove(makePointerEvent(x, y)).handled);
    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.pointerUp(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_EQ(controllerState.presses, 1);
    EXPECT_TRUE(runtime.keyDown(makeKeyEvent(kKeyTab)).handled);
    EXPECT_TRUE(runtime.keyUp(makeKeyEvent(kKeyTab)).handled);
    EXPECT_FALSE(runtime.keyUp(makeKeyEvent(kKeyTab, kModifierControl)).handled);
}

TEST_F(RuntimeTest, LeavesUnclaimedInputForViewerFallback) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);
    runtime.frame(800, 600);

    EXPECT_FALSE(runtime.pointerDown(makePointerEvent(-10.f, -10.f, PointerButton::Left)).handled);
}

TEST_F(RuntimeTest, HeaderDragRetainsCaptureUntilRelease) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    Button* press = dynamic_cast<Button*>(findWidget(*floater, "press"));
    ASSERT_NE(press, nullptr);
    ASSERT_NE(floater->header(), nullptr);
    floater->setCanClose(false);

    runtime.frame(800, 600);
    const Rect headerRect = floater->header()->rect();
    const Rect pressRect = press->rect();
    ASSERT_GT(headerRect.w, 0.f);
    ASSERT_GT(headerRect.h, 0.f);
    ASSERT_GT(pressRect.w, 0.f);
    ASSERT_GT(pressRect.h, 0.f);

    const F32 headerX = headerRect.x + headerRect.w * 0.2f;
    const F32 headerY = headerRect.y + headerRect.h * 0.5f;
    const F32 pressX = pressRect.x + pressRect.w * 0.5f;
    const F32 pressY = pressRect.y + pressRect.h * 0.5f;
    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(headerX, headerY, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    EXPECT_TRUE(runtime.pointerMove(makePointerEvent(pressX, pressY)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());
    EXPECT_TRUE(runtime.pointerUp(makePointerEvent(pressX, pressY, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
    floater->setCanClose(true);
}

TEST_F(RuntimeTest, CapturedPointerContinuesOutsideViewportUntilRelease) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->header(), nullptr);
    floater->setCanClose(false);
    runtime.frame(800, 600);
    const Rect headerRect = floater->header()->rect();
    ASSERT_GT(headerRect.w, 0.f);
    ASSERT_GT(headerRect.h, 0.f);
    const F32 x = headerRect.x + headerRect.w * 0.2f;
    const F32 y = headerRect.y + headerRect.h * 0.5f;

    ASSERT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    ASSERT_TRUE(runtime.hasPointerCapture());

    EXPECT_TRUE(runtime.pointerMove(makePointerEvent(-10.f, 900.f)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());
    EXPECT_TRUE(floater->dragging());

    EXPECT_TRUE(runtime.pointerUp(makePointerEvent(-10.f, 900.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
    floater->setCanClose(true);
}

TEST_F(RuntimeTest, FocusLossCancelsHeaderDrag) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->header(), nullptr);
    floater->setCanClose(false);

    runtime.frame(800, 600);
    const Rect headerRect = floater->header()->rect();
    ASSERT_GT(headerRect.w, 0.f);
    ASSERT_GT(headerRect.h, 0.f);
    const F32 x = headerRect.x + headerRect.w * 0.2f;
    const F32 y = headerRect.y + headerRect.h * 0.5f;

    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    runtime.focusLost();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
    floater->setCanClose(true);
}

TEST_F(RuntimeTest, MouseCaptureLossCancelsHeaderDrag) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    Floater* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->header(), nullptr);
    floater->setCanClose(false);

    runtime.frame(800, 600);
    const Rect headerRect = floater->header()->rect();
    ASSERT_GT(headerRect.w, 0.f);
    ASSERT_GT(headerRect.h, 0.f);
    const F32 x = headerRect.x + headerRect.w * 0.2f;
    const F32 y = headerRect.y + headerRect.h * 0.5f;

    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    runtime.mouseCaptureLost();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
    floater->setCanClose(true);
}

TEST_F(RuntimeTest, ShutdownIsIdempotentAndStopsFurtherInput) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);

    runtime.shutdown();
    runtime.shutdown();

    EXPECT_EQ(controllerState.closed, 1);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(runtime.pointerMove(makePointerEvent(10.f, 20.f)).handled);
    EXPECT_FALSE(runtime.keyDown(makeKeyEvent(kKeyReturn)).handled);
}

TEST_F(RuntimeTest, ShutdownStopsFrameAndIdleWork) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);

    controllerState.lifecycleEvents.clear();
    runtime.frame(800, 600);
    ASSERT_NE(paintContext, nullptr);
    const auto paintedFrames = paintContext->count(PaintCommandKind::BeginFrame);
    keybindingStateCalls = 0;
    runtime.idle();
    EXPECT_EQ(keybindingStateCalls, 1);
    ASSERT_GE(controllerState.lifecycleEvents.size(), 2u);
    EXPECT_EQ(controllerState.lifecycleEvents[0], RuntimeLifecycleEvent::FrameClock);
    EXPECT_EQ(controllerState.lifecycleEvents[1], RuntimeLifecycleEvent::IdleKeybindingState);

    runtime.shutdown();
    ASSERT_FALSE(controllerState.lifecycleEvents.empty());
    EXPECT_EQ(controllerState.lifecycleEvents.back(), RuntimeLifecycleEvent::ControllerClose);
    const auto lifecycleEventsAfterShutdown = controllerState.lifecycleEvents.size();
    runtime.frame(800, 600);
    runtime.idle();

    EXPECT_EQ(paintContext->count(PaintCommandKind::BeginFrame), paintedFrames);
    EXPECT_EQ(keybindingStateCalls, 1);
    EXPECT_EQ(controllerState.lifecycleEvents.size(), lifecycleEventsAfterShutdown);
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
    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);

    runtime.setVisibility(false);
    EXPECT_FALSE(runtime.pointerUp(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_EQ(controllerState.presses, 0);
    nowCalls = 0;
    runtime.frame(800, 600);
    EXPECT_EQ(nowCalls, 0);
    EXPECT_FALSE(runtime.pointerMove(makePointerEvent(10.f, 20.f)).handled);
    EXPECT_FALSE(runtime.keyDown(makeKeyEvent(kKeyReturn)).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());

    runtime.endAccountSession();
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_EQ(controllerState.closed, 1);
}

TEST_F(RuntimeTest, ConsumesUnmodifiedTabWhenVisible) {
    ASSERT_TRUE(runtime.initialize());
    runtime.setVisibility(true);

    EXPECT_TRUE(runtime.keyDown(makeKeyEvent(kKeyTab)).handled);
    EXPECT_TRUE(runtime.keyUp(makeKeyEvent(kKeyTab)).handled);
}
} // namespace
