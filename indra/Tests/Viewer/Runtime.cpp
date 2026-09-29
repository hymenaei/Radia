/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/Element.h>
#include <Core/Event.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/HTMLPanelElement.h>
#include <Core/RecordingPaintContext.h>
#include <Viewer/ControllerRegistration.h>
#include <Viewer/DocumentController.h>
#include <Viewer/InputBridge.h>
#include <Viewer/NativeInput.h>
#include <Viewer/Runtime.h>
#include <chrono>
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>
#include "llcontrol.h"
#include "llglslshader.h"
#include "llsd.h"

namespace {
using Core::Document;
using Core::Element;
using Core::HTMLButtonElement;
using Core::HTMLFloaterElement;
using Core::KeybindingPresentation;
using Core::KeyEvent;
using Core::kKeyReturn;
using Core::kKeyTab;
using Core::kModifierControl;
using Core::kPointerMoveEvent;
using Core::PaintCommandKind;
using Core::PointerButton;
using Core::PointerEvent;
using Core::RecordingPaintContext;
using Core::System;
using Core::WheelEvent;
using Core::Layout::Rect;
using Core::Layout::Vec2;
using Core::Style::CursorStyle;
using ::testing::Test;
using Viewer::DocumentController;
using Viewer::NativePointerButton;
using Viewer::NativePointerInput;
using Viewer::Runtime;
using Viewer::RuntimeKeybindingState;
using Viewer::RuntimeState;
using Viewer::SkinSnapshotResult;
using Viewer::takePointerMoveForFrame;
using Viewer::translatePointerInput;

PointerEvent makePointerEvent(float x, float y, PointerButton button = PointerButton::NoButton, std::uint32_t modifiers = 0,
    std::uint8_t clickCount = 1, float deltaX = 0.f, float deltaY = 0.f) {
    return {{x, y}, button, modifiers, clickCount, {deltaX, deltaY}};
}

WheelEvent makeWheelEvent(int x, int y, float deltaX, float deltaY, std::uint32_t modifiers = 0) {
    return {{static_cast<float>(x), static_cast<float>(y)}, deltaX, deltaY, modifiers};
}

KeyEvent makeKeyEvent(int key, std::uint32_t modifiers = 0, bool repeated = false) { return {key, modifiers, repeated}; }

SkinSnapshotResult runtimeSkinSnapshot() {
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {strings: {runtime: Runtime}}}\n";
    constexpr char kSkin[] = "floater { display: flex; flex-direction: column; } floater > head { height: 30px; } "
                             "button { size: 128px 32px; cursor: pointer; }";
    constexpr char kView[] =
        "<floater resizable><head><title>runtime</title><minimize></minimize><close></close></head><body><button id=\"press\" "
        "onClick=\"press()\"></button></body></floater>";

    SkinSnapshotResult result;
    result.snapshot.add("localization.yaml", kLocalization);
    result.snapshot.add("skin.css", kSkin);
    result.snapshot.add("view.html", kView);
    return result;
}

enum class RuntimeLifecycleEvent {
    FrameClock,
    IdleKeybindingState,
    ControllerClose
};

struct RuntimeControllerState {
    int constructed = 0;
    int opened = 0;
    int closed = 0;
    int presses = 0;
    std::vector<RuntimeLifecycleEvent> lifecycleEvents;
};

Element* findElement(Element& root, std::string_view id) {
    if (root.id() == id)
        return &root;
    for (const auto& child : root.children())
        if (Element* found = findElement(*child, id))
            return found;
    return nullptr;
}

class RuntimeController final : public DocumentController {
public:
    RuntimeController(System& system, Document& document, RuntimeControllerState& state)
        : DocumentController(system, document)
        , mState(state) {
        handler("press", &RuntimeController::press);
        ++mState.constructed;
    }

    void onOpen() override { ++mState.opened; }
    void onClose() override {
        ++mState.closed;
        mState.lifecycleEvents.emplace_back(RuntimeLifecycleEvent::ControllerClose);
    }

private:
    void press() { ++mState.presses; }

    RuntimeControllerState& mState;
};

class RuntimeFixture : public Test {
protected:
    RuntimeFixture()
        : runtime(savedSettings, accountSettings, uiShader, mainWindow,
              Runtime::IntegrationHooks {.resolveKeybinding =
                                             [](const std::string&) {
                                                 return KeybindingPresentation {};
                                             },
                  .keybindingState =
                      [this] {
                          controllerState.lifecycleEvents.emplace_back(RuntimeLifecycleEvent::IdleKeybindingState);
                          ++keybindingStateCalls;
                          return RuntimeKeybindingState {};
                      }},
              Runtime::TestOverrides {
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
                      },
                  .failTeardown =
                      [this] {
                          return failTeardown;
                      },
              }) {
        savedSettings.declareString("Locale", "", "test locale");
        savedSettings.declareLLSD("UILayout", LLSD::emptyMap(), "test layout", LLControlVariable::PERSIST_NO);
        savedSettings.declareBOOL("SkinAutoReload", false, "test skin reload");
        savedSettings.declareS32("SkinAutoReloadScanInterval", 250, "test scan interval");
        savedSettings.declareS32("SkinAutoReloadSettleInterval", 150, "test settle interval");
        accountSettings.declareLLSD("UIWorkspace", LLSD::emptyMap(), "test workspace", LLControlVariable::PERSIST_NO);
    }
    bool registerTestFloater() {
        return runtime.registerFloater("runtimeTest", "view.html", [this](System& system, Document& document) {
            return std::make_unique<RuntimeController>(system, document, controllerState);
        });
    }

    LLControlGroup savedSettings {"RuntimeSaved"};
    LLControlGroup accountSettings {"RuntimeAccount"};
    LLGLSLShader uiShader;
    LLWindow* mainWindow = nullptr;
    SkinSnapshotResult snapshot = runtimeSkinSnapshot();
    int captures = 0;
    int keybindingStateCalls = 0;
    int nowCalls = 0;
    std::chrono::steady_clock::time_point now;
    RuntimeControllerState controllerState;
    bool failTeardown = false;
    RecordingPaintContext* paintContext = nullptr;
    Runtime runtime;
};

TEST_F(RuntimeFixture, IgnoresEarlyInput) {
    EXPECT_FALSE(runtime.pointerMove(makePointerEvent(10.f, 20.f)).handled);
    EXPECT_FALSE(runtime.pointerDown(makePointerEvent(10.f, 20.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.pointerUp(makePointerEvent(10.f, 20.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.scroll(makeWheelEvent(10, 20, 0.f, 1.f)).handled);
    EXPECT_FALSE(runtime.keyDown(makeKeyEvent(0)).handled);
    EXPECT_FALSE(runtime.keyUp(makeKeyEvent(0)).handled);
    EXPECT_FALSE(runtime.character('a').handled);
    EXPECT_FALSE(runtime.hasPointerCapture());

    runtime.pointerLeave();
    runtime.focusLost();
    runtime.pointerCaptureLost();
}

TEST_F(RuntimeFixture, HandlesEarlyLifecycleCalls) {
    runtime.setVisibility(false);
    runtime.frame(800, 600);
    runtime.idle();
    runtime.restoreWorkspace();
    runtime.requestSkinReload();
    runtime.endAccountSession();

    EXPECT_EQ(keybindingStateCalls, 0);
    EXPECT_FALSE(runtime.hasPointerCapture());
}

TEST_F(RuntimeFixture, InitializationIsIdempotent) {
    ASSERT_TRUE(runtime.initialize());

    EXPECT_TRUE(runtime.initialize());
    EXPECT_EQ(captures, 1);
}

TEST_F(RuntimeFixture, ReportsItsLifecycleState) {
    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Running);
    ASSERT_TRUE(runtime.initialize());
    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Running);

    runtime.shutdown();

    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Stopped);
    runtime.shutdown();
    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Stopped);
}

TEST_F(RuntimeFixture, RetriesFailedShutdown) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);

    failTeardown = true;
    runtime.shutdown();

    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::TeardownFailed);
    EXPECT_FALSE(floater->closed());
    EXPECT_EQ(controllerState.closed, 0);
    EXPECT_EQ(runtime.openFloater("runtimeTest", "after-failure"), nullptr);

    failTeardown = false;
    runtime.shutdown();

    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Stopped);
    EXPECT_EQ(controllerState.closed, 1);
    runtime.shutdown();
    EXPECT_EQ(runtime.lifecycleState(), RuntimeState::Stopped);
}

TEST_F(RuntimeFixture, ShutdownPreventsReinitialization) {
    runtime.shutdown();

    EXPECT_FALSE(runtime.initialize());
    EXPECT_EQ(captures, 0);
}

TEST_F(RuntimeFixture, InitializesWithInjectedSkin) {
    ASSERT_TRUE(runtime.initialize());
    EXPECT_GT(captures, 0);
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);
    EXPECT_EQ(controllerState.constructed, 1);
    EXPECT_EQ(controllerState.opened, 1);

    runtime.shutdown();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_EQ(controllerState.closed, 1);
}

TEST_F(RuntimeFixture, ForwardsPaintScaleToSurface) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);

    runtime.frame(800, 600, 1.5f, -24.f, 18.f);

    ASSERT_NE(paintContext, nullptr);
    const auto* frame = paintContext->last(PaintCommandKind::BeginFrame);
    ASSERT_NE(frame, nullptr);
    EXPECT_FLOAT_EQ(frame->target.scale, 1.5f);
    EXPECT_FLOAT_EQ(frame->target.pixelOrigin.x, -24.f);
    EXPECT_FLOAT_EQ(frame->target.pixelOrigin.y, 18.f);
}

TEST_F(RuntimeFixture, RoutesAttachedInput) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    HTMLButtonElement* press = dynamic_cast<HTMLButtonElement*>(findElement(*floater, "press"));
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

TEST_F(RuntimeFixture, UpdatesCursorAcrossFramesAndClearsOnMiss) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    HTMLButtonElement* press = dynamic_cast<HTMLButtonElement*>(findElement(*floater, "press"));
    ASSERT_NE(press, nullptr);
    ASSERT_NE(floater->head(), nullptr);

    runtime.frame(800, 600);
    const Rect headRect = floater->head()->rect();
    const F32 headX = headRect.x + headRect.w * 0.5f;
    const F32 headY = headRect.y + headRect.h * 0.5f;
    ASSERT_TRUE(runtime.pointerMove(makePointerEvent(headX, headY)).handled);

    std::optional<CursorStyle> cursor = runtime.pointerCursor();
    ASSERT_TRUE(cursor.has_value());
    EXPECT_EQ(*cursor, Core::Style::CursorStyle::Default);

    runtime.frame(800, 600);
    cursor = runtime.pointerCursor();
    ASSERT_TRUE(cursor.has_value());
    EXPECT_EQ(*cursor, Core::Style::CursorStyle::Default);

    const Rect pressRect = press->rect();
    const F32 x = pressRect.x + pressRect.w * 0.5f;
    const F32 y = pressRect.y + pressRect.h * 0.5f;
    ASSERT_TRUE(runtime.pointerMove(makePointerEvent(x, y)).handled);

    cursor = runtime.pointerCursor();
    ASSERT_TRUE(cursor.has_value());
    EXPECT_EQ(*cursor, Core::Style::CursorStyle::Pointer);

    runtime.frame(800, 600);
    cursor = runtime.pointerCursor();
    ASSERT_TRUE(cursor.has_value());
    EXPECT_EQ(*cursor, Core::Style::CursorStyle::Pointer);

    EXPECT_FALSE(runtime.pointerMove(makePointerEvent(-10.f, -10.f)).handled);
    EXPECT_FALSE(runtime.pointerCursor().has_value());
}

TEST_F(RuntimeFixture, PreservesUnclaimedInput) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    ASSERT_NE(runtime.openFloater("runtimeTest"), nullptr);
    runtime.frame(800, 600);

    EXPECT_FALSE(runtime.pointerDown(makePointerEvent(-10.f, -10.f, PointerButton::Left)).handled);
}

TEST_F(RuntimeFixture, HeadDragRetainsCaptureUntilRelease) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    HTMLButtonElement* press = dynamic_cast<HTMLButtonElement*>(findElement(*floater, "press"));
    ASSERT_NE(press, nullptr);
    ASSERT_NE(floater->head(), nullptr);

    runtime.frame(800, 600);
    const Rect headRect = floater->head()->rect();
    const Rect pressRect = press->rect();
    ASSERT_GT(headRect.w, 0.f);
    ASSERT_GT(headRect.h, 0.f);
    ASSERT_GT(pressRect.w, 0.f);
    ASSERT_GT(pressRect.h, 0.f);

    const F32 headX = headRect.x + headRect.w * 0.2f;
    const F32 headY = headRect.y + headRect.h * 0.5f;
    const F32 pressX = pressRect.x + pressRect.w * 0.5f;
    const F32 pressY = pressRect.y + pressRect.h * 0.5f;
    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(headX, headY, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    EXPECT_TRUE(runtime.pointerMove(makePointerEvent(pressX, pressY)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());
    EXPECT_TRUE(runtime.pointerUp(makePointerEvent(pressX, pressY, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
}

TEST_F(RuntimeFixture, PersistsMinimizedFloaterMove) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->head(), nullptr);

    runtime.frame(800, 600);
    floater->setMinimized(true);
    runtime.frame(800, 600);

    const Rect initialExpandedRect = floater->expandedRect();
    const LLSD initialPlacement = savedSettings.getLLSD("UILayout")["runtimeTest"];
    ASSERT_TRUE(initialPlacement.isMap());
    ASSERT_TRUE(initialPlacement["position"].isArray());
    ASSERT_TRUE(initialPlacement["size"].isArray());
    EXPECT_FLOAT_EQ(static_cast<float>(initialPlacement["position"][0].asReal()), initialExpandedRect.x);
    EXPECT_FLOAT_EQ(static_cast<float>(initialPlacement["position"][1].asReal()), initialExpandedRect.y);
    EXPECT_FLOAT_EQ(static_cast<float>(initialPlacement["size"][0].asReal()), initialExpandedRect.w);
    EXPECT_FLOAT_EQ(static_cast<float>(initialPlacement["size"][1].asReal()), initialExpandedRect.h);

    const Rect headRect = floater->head()->rect();
    const float startX = headRect.x + headRect.w * 0.2f;
    const float startY = headRect.y + headRect.h * 0.5f;
    ASSERT_TRUE(runtime.pointerDown(makePointerEvent(startX, startY, PointerButton::Left)).handled);
    ASSERT_TRUE(runtime.pointerMove(makePointerEvent(startX + 40.f, startY)).handled);
    ASSERT_GT(floater->expandedRect().x, initialExpandedRect.x);
    ASSERT_TRUE(runtime.pointerUp(makePointerEvent(startX + 40.f, startY, PointerButton::Left)).handled);

    const LLSD savedPlacement = savedSettings.getLLSD("UILayout")["runtimeTest"];
    ASSERT_TRUE(savedPlacement.isMap());
    EXPECT_FLOAT_EQ(static_cast<float>(savedPlacement["position"][0].asReal()), floater->expandedRect().x);
    EXPECT_FLOAT_EQ(static_cast<float>(savedPlacement["position"][1].asReal()), floater->expandedRect().y);
    EXPECT_FLOAT_EQ(static_cast<float>(savedPlacement["size"][0].asReal()), floater->expandedRect().w);
    EXPECT_FLOAT_EQ(static_cast<float>(savedPlacement["size"][1].asReal()), floater->expandedRect().h);
}

TEST_F(RuntimeFixture, CapturesPointerOutsideViewport) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->head(), nullptr);
    runtime.frame(800, 600);
    const Rect headRect = floater->head()->rect();
    ASSERT_GT(headRect.w, 0.f);
    ASSERT_GT(headRect.h, 0.f);
    const F32 x = headRect.x + headRect.w * 0.2f;
    const F32 y = headRect.y + headRect.h * 0.5f;

    ASSERT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    ASSERT_TRUE(runtime.hasPointerCapture());

    EXPECT_TRUE(runtime.pointerMove(makePointerEvent(-10.f, 900.f)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());
    EXPECT_TRUE(floater->dragging());

    EXPECT_TRUE(runtime.pointerUp(makePointerEvent(-10.f, 900.f, PointerButton::Left)).handled);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
}

TEST_F(RuntimeFixture, DispatchesOneMovePerSample) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->head(), nullptr);
    runtime.frame(800, 600);

    const Rect headRect = floater->head()->rect();
    const float startX = headRect.x + headRect.w * .2f;
    const float startY = headRect.y + headRect.h * .5f;
    ASSERT_TRUE(runtime.pointerDown(makePointerEvent(startX, startY, PointerButton::Left)).handled);
    ASSERT_TRUE(runtime.hasPointerCapture());

    int moveEvents = 0;
    Vec2 lastPosition;
    Vec2 lastDelta;
    floater->addEventListener(kPointerMoveEvent, [&](Core::Event& event) {
        ++moveEvents;
        const PointerEvent* pointer = event.pointer();
        ASSERT_NE(pointer, nullptr);
        if (!pointer)
            return;
        lastPosition = pointer->position;
        lastDelta = pointer->delta;
        if (moveEvents == 2)
            runtime.pointerCaptureLost();
    });

    std::optional<NativePointerInput> pending;
    pending = NativePointerInput {startX + 10.f, startY + 5.f, NativePointerButton::NoButton, 0, 1, 1.f, 2.f};
    pending = NativePointerInput {-10.f, 900.f, NativePointerButton::NoButton, 0, 1, 4.f, -3.f};
    std::optional<NativePointerInput> sample = takePointerMoveForFrame(pending, true, false, true, 0, 4.f, -3.f);
    ASSERT_TRUE(sample.has_value());
    EXPECT_TRUE(runtime.pointerMove(translatePointerInput(*sample)).handled);
    EXPECT_EQ(moveEvents, 1);
    EXPECT_FLOAT_EQ(lastPosition.x, -10.f);
    EXPECT_FLOAT_EQ(lastPosition.y, 900.f);
    EXPECT_FLOAT_EQ(lastDelta.x, 4.f);
    EXPECT_FLOAT_EQ(lastDelta.y, -3.f);
    EXPECT_TRUE(runtime.hasPointerCapture());
    EXPECT_TRUE(floater->dragging());
    EXPECT_FALSE(pending.has_value());

    pending = NativePointerInput {-20.f, 950.f, NativePointerButton::NoButton, 0, 1, 6.f, -5.f};
    sample = takePointerMoveForFrame(pending, true, false, true, 0, 6.f, -5.f);
    ASSERT_TRUE(sample.has_value());
    (void)runtime.pointerMove(translatePointerInput(*sample));
    EXPECT_EQ(moveEvents, 2);
    EXPECT_FLOAT_EQ(lastPosition.x, -20.f);
    EXPECT_FLOAT_EQ(lastPosition.y, 950.f);
    EXPECT_FLOAT_EQ(lastDelta.x, 6.f);
    EXPECT_FLOAT_EQ(lastDelta.y, -5.f);
    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
}

TEST_F(RuntimeFixture, FocusLossCancelsHeadDrag) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->head(), nullptr);

    runtime.frame(800, 600);
    const Rect headRect = floater->head()->rect();
    ASSERT_GT(headRect.w, 0.f);
    ASSERT_GT(headRect.h, 0.f);
    const F32 x = headRect.x + headRect.w * 0.2f;
    const F32 y = headRect.y + headRect.h * 0.5f;

    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    runtime.focusLost();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
}

TEST_F(RuntimeFixture, PointerCaptureLossCancelsHeadDrag) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->head(), nullptr);

    runtime.frame(800, 600);
    const Rect headRect = floater->head()->rect();
    ASSERT_GT(headRect.w, 0.f);
    ASSERT_GT(headRect.h, 0.f);
    const F32 x = headRect.x + headRect.w * 0.2f;
    const F32 y = headRect.y + headRect.h * 0.5f;

    EXPECT_TRUE(runtime.pointerDown(makePointerEvent(x, y, PointerButton::Left)).handled);
    EXPECT_TRUE(runtime.hasPointerCapture());

    runtime.pointerCaptureLost();

    EXPECT_FALSE(runtime.hasPointerCapture());
    EXPECT_FALSE(floater->dragging());
}

TEST_F(RuntimeFixture, ShutdownStopsInput) {
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

TEST_F(RuntimeFixture, ShutdownStopsBackgroundWork) {
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

TEST_F(RuntimeFixture, ClearsInteractionOnContextChange) {
    ASSERT_TRUE(runtime.initialize());
    ASSERT_TRUE(registerTestFloater());
    HTMLFloaterElement* floater = runtime.openFloater("runtimeTest");
    ASSERT_NE(floater, nullptr);
    HTMLButtonElement* press = dynamic_cast<HTMLButtonElement*>(findElement(*floater, "press"));
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

TEST_F(RuntimeFixture, ConsumesUnmodifiedTabWhenVisible) {
    ASSERT_TRUE(runtime.initialize());
    runtime.setVisibility(true);

    EXPECT_TRUE(runtime.keyDown(makeKeyEvent(kKeyTab)).handled);
    EXPECT_TRUE(runtime.keyUp(makeKeyEvent(kKeyTab)).handled);
}
} // namespace
