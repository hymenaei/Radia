/**
 * @file surface_test.cpp
 * @brief Tests Surface layout, input routing, focus, and paint invalidation.
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
#include <algorithm>
#include <chrono>
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include "binding/binder.h"
#include "render/recordingpaintcontext.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"

namespace {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::Button;
using radia::ui::ClipAxes;
using radia::ui::clipsAxis;
using radia::ui::CursorStyle;
using radia::ui::EventCall;
using radia::ui::EventKind;
using radia::ui::EventPhase;
using radia::ui::fixedTextMetrics;
using radia::ui::Floater;
using radia::ui::kKeyReturn;
using radia::ui::kKeySpace;
using radia::ui::kKeyTab;
using radia::ui::kModifierShift;
using radia::ui::Label;
using radia::ui::LongClickEvent;
using radia::ui::MouseWidgetEvent;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PaintContext;
using radia::ui::Panel;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::PreparedBindingResult;
using radia::ui::RecordingPaintContext;
using radia::ui::Rect;
using radia::ui::RoutedEvent;
using radia::ui::ScrollEvent;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::SurfaceLayer;
using radia::ui::Switch;
using radia::ui::System;
using radia::ui::Visibility;
using radia::ui::Widget;
using radia::ui::WidgetEvent;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetState;
using radia::ui::detail::makeEventRegistration;
using ::testing::Message;

constexpr char kFloaterInteractionLayout[] = "floater { flow: column; } "
                                             "floater::header { height: 30px; } "
                                             "floater::content { flex-grow: 1; } "
                                             "label { height: 20px; }";

const char* noEventArguments(const EventCall& call, WidgetEventKind) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

template<typename Callback> void bindAction(Binder& binder, std::string name, Callback callback) {
    binder.event(makeEventRegistration(
        std::move(name), std::nullopt, [callback = std::move(callback)](const WidgetEvent&, const EventCall&) mutable { callback(); },
        noEventArguments));
}

template<typename Event, typename Callback>
void bindSemanticEvent(Binder& binder, std::string name, std::optional<WidgetEventKind> kind, Callback callback) {
    binder.event(makeEventRegistration(
        std::move(name), kind,
        [callback = std::move(callback)](const WidgetEvent& event, const EventCall&) mutable { callback(static_cast<const Event&>(event)); },
        noEventArguments));
}
} // namespace

namespace {
class InputProbe final : public Widget {
public:
    InputProbe() : Widget("input_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    bool beginPointerInteraction(const PointerEvent& event) override {
        lastClickCount = event.clickCount;
        return false;
    }
    bool defaultCharacterInput(unsigned int codepoint) override {
        lastCodepoint = codepoint;
        return true;
    }
    bool defaultScroll(const ScrollEvent& event) override {
        lastScrollX = event.dx;
        lastScrollY = event.dy;
        return true;
    }

    uint8_t lastClickCount = 0;
    unsigned int lastCodepoint = 0;
    float lastScrollX = 0.f;
    float lastScrollY = 0.f;
};

class CaptureProbe final : public Widget {
public:
    CaptureProbe() : Widget("capture_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const PointerEvent&) override { return true; }
    bool endPointerInteraction(const PointerEvent&) override {
        ++ends;
        return true;
    }

    int ends = 0;
};

class PaintProbe final : public Widget {
public:
    PaintProbe() : Widget("paint_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(PaintContext&, const Style&, float) const override { ++paints; }

    mutable int paints = 0;
};

class OrderedPaintProbe final : public Widget {
public:
    OrderedPaintProbe(std::string name, std::vector<std::string>& paintOrder)
        : Widget("ordered_probe"), mName(std::move(name)), mPaintOrder(paintOrder) {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(PaintContext&, const Style&, float) const override { mPaintOrder.push_back(mName); }

private:
    std::string mName;
    std::vector<std::string>& mPaintOrder;
};

class RoutedProbe final : public Widget {
public:
    RoutedProbe(std::string name, std::vector<std::string>& log) : Widget("routed_probe"), mName(std::move(name)), mLog(log) {}

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const PointerEvent&) override {
        ++begins;
        return true;
    }

    bool preventDefault = false;
    int begins = 0;

protected:
    void onEvent(RoutedEvent& event) override {
        if (event.kind() != EventKind::PointerDown) return;
        const char* phase = event.phase() == EventPhase::Capture ? "capture" : event.phase() == EventPhase::Target ? "target" : "bubble";
        mLog.push_back(mName + ":" + phase);
        if (preventDefault && event.phase() == EventPhase::Target) event.preventDefault();
    }

private:
    std::string mName;
    std::vector<std::string>& mLog;
};

TEST(SurfaceTest, HandlesPointerHoverPressAndRelease) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true).setOnActivate([&](Widget&) { ++activations; });
    context.root().addChild(std::move(button));
    EXPECT_TRUE(context.pointerMove({{15.f, 15.f}}));
    EXPECT_TRUE(target->hasState(WidgetState::Hovered));
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    context.pointerMove({{50.f, 50.f}});
    EXPECT_FALSE(target->hasState(WidgetState::Active));
    context.pointerMove({{15.f, 15.f}});
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(activations, 1);
    EXPECT_TRUE(context.hasFocus());
    EXPECT_FALSE(target->hasState(WidgetState::FocusVisible));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerLeave();
    EXPECT_FALSE(target->hasState(WidgetState::Active));
    context.pointerUp({{50.f, 50.f}, PointerButton::Left});
    EXPECT_EQ(activations, 1);
}

TEST(SurfaceTest, ActivatesSwitchWithMouseAndKeyboard) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto control = std::make_unique<Switch>();
    Switch* target = control.get();
    int changes = 0;
    control->setOnCheckedChanged([&](bool) { ++changes; });
    control->setRect({10.f, 10.f, 40.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(control));
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->checked());
    EXPECT_FALSE(context.keyUp({kKeySpace}));
    EXPECT_TRUE(target->checked());
    context.keyDown({kKeySpace});
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    EXPECT_FALSE(context.keyUp({kKeyReturn}));
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    context.keyUp({kKeySpace});
    EXPECT_FALSE(target->checked());
    EXPECT_FALSE(context.keyUp({kKeySpace}));
    EXPECT_EQ(changes, 2);
}

TEST(SurfaceTest, ClearsInteractionAfterTreeMutation) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(button));
    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(context.hasFocus());
    context.root().clearChildren();
    EXPECT_FALSE(context.hasFocus());
    EXPECT_FALSE(context.pointerMove({{15.f, 15.f}}));
}

TEST(SurfaceTest, BlocksDisabledControlsWithoutFocusingThem) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    button->setDisabled(true).setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(button));
    EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_FALSE(context.hasFocus());
}

TEST(SurfaceTest, DragsMinimizesAndRestoresFloaters) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterInteractionLayout).ok());
    Surface context(styleSheet);
    context.setViewport(200.f, 200.f);

    auto floater = std::make_unique<Floater>();
    Floater* floaterPtr = floater.get();
    floater->setTitle("title").setCanClose(false).setCanMinimize(true);
    auto content = std::make_unique<Label>("content");
    Label* contentNode = content.get();
    floater->addChild(std::move(content));
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    context.mountFloater(std::move(floater));
    context.updateLayout();

    EXPECT_TRUE(context.pointerDown({{30.f, 110.f}, PointerButton::Left}));
    EXPECT_TRUE(context.pointerMove({{50.f, 120.f}, PointerButton::Left}));
    context.pointerUp({{50.f, 120.f}, PointerButton::Left});
    EXPECT_EQ(floaterPtr->rect().x, 40.f);
    EXPECT_EQ(floaterPtr->rect().y, 30.f);

    const float expandedTop = floaterPtr->rect().top();
    const float expandedWidth = floaterPtr->rect().w;
    floaterPtr->setMinimized(true);
    EXPECT_TRUE(floaterPtr->hasState(WidgetState::Minimized));
    EXPECT_EQ(floaterPtr->content()->visibility(), Visibility::Collapsed);
    EXPECT_EQ(contentNode->visibility(), Visibility::Visible);
    EXPECT_EQ(floaterPtr->rect().top(), expandedTop);
    EXPECT_EQ(floaterPtr->rect().h, 30.f);
    EXPECT_TRUE(floaterPtr->rect().w < expandedWidth);
    floaterPtr->setMinimized(false);
    EXPECT_FALSE(floaterPtr->hasState(WidgetState::Minimized));
    EXPECT_EQ(floaterPtr->content()->visibility(), Visibility::Visible);
    EXPECT_EQ(contentNode->visibility(), Visibility::Visible);
    EXPECT_EQ(floaterPtr->rect().h, 100.f);
    EXPECT_EQ(floaterPtr->rect().w, expandedWidth);

    EXPECT_TRUE(context.pointerDown({{50.f, 120.f}, PointerButton::Left, 0, 2}));
    EXPECT_TRUE(floaterPtr->minimized());
    context.pointerUp({{50.f, 120.f}, PointerButton::Left});
    EXPECT_TRUE(context.pointerDown({{50.f, 120.f}, PointerButton::Left, 0, 2}));
    EXPECT_FALSE(floaterPtr->minimized());
    context.pointerUp({{50.f, 120.f}, PointerButton::Left});
}

TEST(SurfaceTest, IgnoresNonPrimaryPointerButtons) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setOnActivate([&](Widget&) { ++activations; });
    context.root().addChild(std::move(button));

    for (PointerButton pointerButton : {PointerButton::Right, PointerButton::Middle, PointerButton::Auxiliary1, PointerButton::Auxiliary2}) {
        SCOPED_TRACE(Message() << "pointer button: " << static_cast<int>(pointerButton));
        EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, pointerButton}));
        EXPECT_TRUE(context.pointerUp({{15.f, 15.f}, pointerButton}));
    }
    EXPECT_EQ(activations, 0);
    EXPECT_FALSE(context.hasFocus());
    EXPECT_FALSE(target->hasState(WidgetState::Active));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(activations, 1);
}

TEST(SurfaceTest, RoutesDoubleClickTextAndScrollInput) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto probe = std::make_unique<InputProbe>();
    InputProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(probe));

    EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, PointerButton::Left, 0, 2}));
    EXPECT_EQ(target->lastClickCount, static_cast<uint8_t>(2));
    EXPECT_TRUE(context.pointerUp({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_FALSE(target->hasState(WidgetState::Active));
    EXPECT_TRUE(context.hasFocus());
    EXPECT_TRUE(context.charInput(0x03A9));
    EXPECT_EQ(target->lastCodepoint, 0x03A9u);

    EXPECT_TRUE(context.scroll({{15.f, 15.f}, 0.f, 3.f}));
    EXPECT_EQ(target->lastScrollY, 3.f);
    EXPECT_TRUE(context.scroll({{15.f, 15.f}, -2.f, 0.f}));
    EXPECT_EQ(target->lastScrollX, -2.f);

    context.clearInteractionState();
    EXPECT_FALSE(context.hasFocus());
    EXPECT_FALSE(context.charInput('x'));

    EXPECT_TRUE(context.pointerMove({{15.f, 15.f}}));
    context.pointerLeave();
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));
}

TEST(SurfaceTest, ReleasesPointerCaptureWhenInteractionStateClears) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterInteractionLayout).ok());
    Surface context(styleSheet);
    context.setViewport(200.f, 200.f);
    auto floater = std::make_unique<Floater>();
    floater->setTitle("title").setCanClose(false).setCanMinimize(false);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    context.mountFloater(std::move(floater));
    context.updateLayout();

    EXPECT_TRUE(context.pointerDown({{30.f, 110.f}, PointerButton::Left}));
    EXPECT_TRUE(context.hasPointerCapture());
    EXPECT_TRUE(context.pointerMove({{-50.f, -50.f}, PointerButton::Left}));
    EXPECT_TRUE(context.pointerUp({{-50.f, -50.f}, PointerButton::Left}));
    EXPECT_FALSE(context.hasPointerCapture());

    EXPECT_TRUE(context.pointerDown({{10.f, 90.f}, PointerButton::Left}));
    context.clearInteractionState();
    EXPECT_FALSE(context.hasPointerCapture());
}

TEST(SurfaceTest, TraversesFocusableControlsAndSkipsUnavailableNodes) {
    Surface context;
    context.setViewport(200.f, 200.f);

    auto first = std::make_unique<Button>();
    Button* firstTarget = first.get();
    first->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(first));

    auto hidden = std::make_unique<Button>();
    Button* hiddenTarget = hidden.get();
    hidden->setVisibility(Visibility::Hidden).setRect({40.f, 10.f, 20.f, 20.f});
    context.root().addChild(std::move(hidden));

    auto disabled = std::make_unique<Button>();
    Button* disabledTarget = disabled.get();
    disabled->setDisabled(true).setRect({70.f, 10.f, 20.f, 20.f});
    context.root().addChild(std::move(disabled));

    auto last = std::make_unique<Switch>();
    Switch* lastTarget = last.get();
    last->setRect({100.f, 10.f, 40.f, 20.f});
    context.root().addChild(std::move(last));

    EXPECT_TRUE(context.keyDown({kKeyTab}));
    EXPECT_TRUE(firstTarget->hasState(WidgetState::Focused));
    EXPECT_TRUE(firstTarget->hasState(WidgetState::FocusVisible));
    EXPECT_TRUE(context.keyUp({kKeyTab}));

    context.keyDown({kKeyTab});
    EXPECT_TRUE(lastTarget->hasState(WidgetState::Focused));
    EXPECT_FALSE(hiddenTarget->hasState(WidgetState::Focused));
    EXPECT_FALSE(disabledTarget->hasState(WidgetState::Focused));

    context.keyDown({kKeyTab});
    EXPECT_TRUE(firstTarget->hasState(WidgetState::Focused));
    context.keyDown({kKeyTab, kModifierShift});
    EXPECT_TRUE(lastTarget->hasState(WidgetState::Focused));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(firstTarget->hasState(WidgetState::Focused));
    EXPECT_FALSE(firstTarget->hasState(WidgetState::FocusVisible));
    firstTarget->setVisibility(Visibility::Hidden);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
    firstTarget->setVisibility(Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    firstTarget->setDisabled(true);
    EXPECT_FALSE(context.charInput('x'));
    EXPECT_FALSE(context.hasFocus());
    firstTarget->setDisabled(false);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.clearInteractionState();
    EXPECT_FALSE(firstTarget->hasState(WidgetState::Focused));
    EXPECT_FALSE(firstTarget->hasState(WidgetState::FocusVisible));
}

TEST(SurfaceTest, ClearsInteractionWhenDescendantsBecomeUnavailable) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto panel = std::make_unique<Panel>();
    Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    panel->addChild(std::move(button));
    context.root().addChild(std::move(panel));

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    context.keyDown({kKeySpace});
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    context.clearInteractionState();
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));
    EXPECT_FALSE(target->hasState(WidgetState::Active));
    EXPECT_FALSE(context.hasFocus());

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->hasState(WidgetState::Active));
    context.clearInteractionState();
    EXPECT_FALSE(target->hasState(WidgetState::Active));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    parent->setVisibility(Visibility::Hidden);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
    parent->setVisibility(Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    parent->setDisabled(true);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
}

TEST(SurfaceTest, DispatchesMouseBindingsInExpectedOrder) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    button->setEventCall(WidgetEventKind::MouseDown, EventCall("press"));
    button->setEventCall(WidgetEventKind::MouseUp, EventCall("release"));
    button->setEventCall(WidgetEventKind::Click, EventCall("click"));
    button->setEventCall(WidgetEventKind::DoubleClick, EventCall("doubleClick"));
    button->setEventCall(WidgetEventKind::ContextMenu, EventCall("contextMenu"));
    context.root().addChild(std::move(button));

    std::vector<std::string> events;
    Binder binder(context.root());
    bindSemanticEvent<MouseWidgetEvent>(binder, "press", std::nullopt, [&](const MouseWidgetEvent& event) {
        EXPECT_NE(event.mouse.button, PointerButton::NoButton);
        events.push_back("down");
    });
    bindAction(binder, "release", [&] { events.push_back("up"); });
    bindAction(binder, "click", [&] { events.push_back("click"); });
    bindSemanticEvent<MouseWidgetEvent>(binder, "doubleClick", std::nullopt, [&](const MouseWidgetEvent& event) {
        EXPECT_EQ(event.mouse.clickCount, 2);
        events.push_back("double");
    });
    bindSemanticEvent<MouseWidgetEvent>(binder, "contextMenu", std::nullopt, [&](const MouseWidgetEvent& event) {
        EXPECT_EQ(event.mouse.button, PointerButton::Right);
        events.push_back("context");
    });
    PreparedBindingResult prepared = binder.prepare();
    const bool bindingPrepared = prepared.ok();
    Binding binding = bindingPrepared ? prepared.binding.commit() : Binding{};
    ASSERT_TRUE(bindingPrepared && binding);

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    ASSERT_EQ(events.size(), 3U);
    EXPECT_EQ(events[0], "down");
    EXPECT_EQ(events[1], "up");
    EXPECT_EQ(events[2], "click");

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{50.f, 50.f}, PointerButton::Left});
    ASSERT_EQ(events.size(), 5U);
    EXPECT_EQ(events.back(), "up");

    context.pointerDown({{15.f, 15.f}, PointerButton::Right});
    context.pointerUp({{15.f, 15.f}, PointerButton::Right});
    ASSERT_EQ(events.size(), 8U);
    EXPECT_EQ(events.back(), "context");

    context.pointerDown({{15.f, 15.f}, PointerButton::Left, 0, 2});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    ASSERT_EQ(events.size(), 12U);
    EXPECT_EQ(events[10], "click");
    EXPECT_EQ(events[11], "double");
}

TEST(SurfaceTest, UnmountsRootWidgetsSafely) {
    Surface context;
    context.setViewport(100.f, 80.f);
    auto panel = std::make_unique<Panel>();
    Panel* mounted = panel.get();
    context.mount(std::move(panel));
    ASSERT_TRUE(context.unmount(*mounted));

    EXPECT_EQ(context.root().rect().w, 100.f);
    EXPECT_EQ(context.root().rect().h, 80.f);
    EXPECT_FALSE(context.pointerDown({{10.f, 10.f}, PointerButton::Left}));
}

TEST(SurfaceTest, ClearsPointerCaptureWhenWidgetBecomesDisabled) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto probe = std::make_unique<CaptureProbe>();
    CaptureProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    surface.root().addChild(std::move(probe));

    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    target->setDisabled(true);
    EXPECT_FALSE(surface.hasPointerCapture());
    EXPECT_EQ(target->ends, 1);
}

TEST(SurfaceTest, AppliesGlobalAndWidgetLongClickDelays) {
    System system;
    EXPECT_TRUE(system.setLongClickDelay(std::chrono::milliseconds(600)));
    std::unique_ptr<Surface> ownedSurface = system.createSurface(fixedTextMetrics());
    ASSERT_TRUE(ownedSurface);
    Surface& surface = *ownedSurface;
    surface.setViewport(100.f, 100.f);

    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    button->setEventCall(WidgetEventKind::LongClick, EventCall("hold"));
    button->setEventCall(WidgetEventKind::Click, EventCall("tap"));
    button->setEventCall(WidgetEventKind::MouseUp, EventCall("release"));
    surface.root().addChild(std::move(button));

    int holds = 0;
    int taps = 0;
    int releases = 0;
    std::chrono::milliseconds heldFor{0};
    Binder binder(surface.root());
    bindSemanticEvent<LongClickEvent>(binder, "hold", WidgetEventKind::LongClick, [&](const LongClickEvent& event) {
        heldFor = event.heldFor;
        ++holds;
    });
    bindAction(binder, "tap", [&] { ++taps; });
    bindAction(binder, "release", [&] { ++releases; });
    PreparedBindingResult prepared = binder.prepare();
    const bool bindingPrepared = prepared.ok();
    Binding binding = bindingPrepared ? prepared.binding.commit() : Binding{};
    ASSERT_TRUE(bindingPrepared && binding);

    surface.pointerDown({{15.f, 15.f}, PointerButton::Left});
    surface.update(std::chrono::milliseconds(599));
    EXPECT_EQ(holds, 0);
    surface.update(std::chrono::milliseconds(1));
    EXPECT_EQ(holds, 1);
    EXPECT_EQ(heldFor.count(), 600LL);
    surface.update(std::chrono::milliseconds(500));
    EXPECT_EQ(holds, 1);
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(releases, 1);
    EXPECT_EQ(taps, 0);

    target->setLongClickDelay(std::chrono::milliseconds(200));
    surface.pointerDown({{15.f, 15.f}, PointerButton::Left});
    surface.update(std::chrono::milliseconds(200));
    EXPECT_EQ(holds, 2);
    EXPECT_EQ(heldFor.count(), 200LL);
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(releases, 2);
    EXPECT_EQ(taps, 0);
}

TEST(SurfaceTest, AppliesPointerPolicyStylesWithoutLayout) {
    StyleSheet styleSheet;
    constexpr char kPointerPolicyStyles[] = "button { pointer-events: none; } "
                                            "panel { pointer-events: auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPointerPolicyStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto button = std::make_unique<Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(button));
    auto panel = std::make_unique<Panel>();
    panel->setRect({40.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(panel));

    EXPECT_FALSE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
}

TEST(SurfaceTest, RemeasuresAfterIntrinsicContentChanges) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { flow: row; } "
                                  "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = std::make_unique<Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = std::make_unique<Label>("a");
    Label* text = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));

    surface.updateLayout();
    const float shortWidth = text->rect().w;
    text->setText("a much longer label");
    surface.updateLayout();
    EXPECT_TRUE(text->rect().w > shortWidth);
}

TEST(SurfaceTest, InvalidatesLayoutAfterStylesheetGenerationChanges) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kExpandedLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = std::make_unique<Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = std::make_unique<Label>("text");
    Label* text = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));
    surface.updateLayout();
    EXPECT_EQ(text->rect().w, 10.f);

    ASSERT_TRUE(styleSheet.loadRadia(kExpandedLabelLayout).ok());
    surface.updateLayout();
    EXPECT_EQ(text->rect().w, 30.f);
}

TEST(SurfaceTest, RoutesPointerEventsThroughCaptureTargetAndBubble) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    std::vector<std::string> log;

    auto parent = std::make_unique<RoutedProbe>("parent", log);
    parent->setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<RoutedProbe>("child", log);
    child->setRect({10.f, 10.f, 20.f, 20.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    ASSERT_EQ(log.size(), std::size_t(3));
    EXPECT_EQ(log[0], std::string("parent:capture"));
    EXPECT_EQ(log[1], std::string("child:target"));
    EXPECT_EQ(log[2], std::string("parent:bubble"));
}

TEST(SurfaceTest, HonorsPreventDefaultDuringPointerRouting) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    std::vector<std::string> log;
    auto probe = std::make_unique<RoutedProbe>("target", log);
    RoutedProbe* target = probe.get();
    target->preventDefault = true;
    target->setRect({10.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(probe));

    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_EQ(target->begins, 0);
    EXPECT_FALSE(surface.hasPointerCapture());
}

TEST(SurfaceTest, InheritsAndOverridesCursorStyles) {
    StyleSheet styleSheet;
    constexpr char kInheritedCursorLayout[] = "#parent { pointer-events: auto; cursor: grab; } "
                                              "#child { pointer-events: auto; }";
    constexpr char kDefaultChildCursorLayout[] = "#parent { pointer-events: auto; cursor: grab; } "
                                                 "#child { pointer-events: auto; cursor: auto; }";
    constexpr char kTextChildCursorLayout[] = "#parent { pointer-events: auto; cursor: grab; } "
                                              "#child { pointer-events: auto; cursor: text; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInheritedCursorLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto parent = std::make_unique<Panel>();
    parent->setId("parent").setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<Panel>();
    child->setId("child").setRect({10.f, 10.f, 20.f, 20.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    EXPECT_TRUE(surface.pointerMove({{15.f, 15.f}}));
    EXPECT_EQ(static_cast<int>(surface.cursor()), static_cast<int>(CursorStyle::Grab));

    ASSERT_TRUE(styleSheet.loadRadia(kDefaultChildCursorLayout).ok());
    EXPECT_EQ(static_cast<int>(surface.cursor()), static_cast<int>(CursorStyle::Default));

    ASSERT_TRUE(styleSheet.loadRadia(kTextChildCursorLayout).ok());
    EXPECT_EQ(static_cast<int>(surface.cursor()), static_cast<int>(CursorStyle::Text));
}

TEST(SurfaceTest, RoutesInputBySurfaceLayerPriority) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    int contentActivations = 0;
    int floaterActivations = 0;
    int popupActivations = 0;
    int tooltipActivations = 0;
    int dragActivations = 0;
    int modalActivations = 0;

    auto mountButton = [&](SurfaceLayer layer, int& activations, const Rect& rect) {
        auto button = std::make_unique<Button>();
        button->setRect(rect).setOnActivate([&activations](Widget&) { ++activations; });
        surface.mount(std::move(button), layer);
    };
    mountButton(SurfaceLayer::Content, contentActivations, {0.f, 0.f, 100.f, 100.f});
    mountButton(SurfaceLayer::Floater, floaterActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(SurfaceLayer::Popup, popupActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(SurfaceLayer::Tooltip, tooltipActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(SurfaceLayer::Drag, dragActivations, {10.f, 10.f, 30.f, 30.f});

    surface.pointerDown({{15.f, 15.f}, PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(popupActivations, 1);
    EXPECT_EQ(tooltipActivations, 0);
    EXPECT_EQ(dragActivations, 0);

    mountButton(SurfaceLayer::Modal, modalActivations, {10.f, 10.f, 30.f, 30.f});
    surface.pointerDown({{15.f, 15.f}, PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(modalActivations, 1);

    EXPECT_TRUE(surface.pointerDown({{80.f, 80.f}, PointerButton::Left}));
    surface.pointerUp({{80.f, 80.f}, PointerButton::Left});
    EXPECT_EQ(contentActivations, 0);

    surface.clearLayer(SurfaceLayer::Modal);
    surface.pointerDown({{15.f, 15.f}, PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(popupActivations, 2);
}

TEST(SurfaceTest, RaisesContainingFloaterOnPress) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    int firstActivations = 0;
    int secondActivations = 0;

    auto first = std::make_unique<Panel>();
    first->setRect({0.f, 0.f, 50.f, 50.f});
    auto firstButton = std::make_unique<Button>();
    firstButton->setRect({0.f, 0.f, 50.f, 50.f}).setOnActivate([&firstActivations](Widget&) { ++firstActivations; });
    first->addChild(std::move(firstButton));
    surface.mount(std::move(first), SurfaceLayer::Floater);

    auto second = std::make_unique<Panel>();
    second->setRect({25.f, 0.f, 50.f, 50.f});
    auto secondButton = std::make_unique<Button>();
    secondButton->setRect({25.f, 0.f, 50.f, 50.f}).setOnActivate([&secondActivations](Widget&) { ++secondActivations; });
    second->addChild(std::move(secondButton));
    surface.mount(std::move(second), SurfaceLayer::Floater);

    surface.pointerDown({{10.f, 10.f}, PointerButton::Left});
    surface.pointerUp({{10.f, 10.f}, PointerButton::Left});
    surface.pointerDown({{30.f, 10.f}, PointerButton::Left});
    surface.pointerUp({{30.f, 10.f}, PointerButton::Left});
    EXPECT_EQ(firstActivations, 2);
    EXPECT_EQ(secondActivations, 0);
}

TEST(SurfaceTest, AppliesOverflowVisibilityToHitTestingAndPainting) {
    StyleSheet stylesheet;
    constexpr char kOverflowVisibleLayout[] = "#parent { overflow: visible; pointer-events: none; } "
                                              "#child { pointer-events: auto; }";
    ASSERT_TRUE(stylesheet.loadRadia(kOverflowVisibleLayout).ok());
    Surface surface(stylesheet);
    surface.setViewport(100.f, 100.f);
    auto parent = std::make_unique<Panel>();
    parent->setId("parent").setRect({10.f, 10.f, 20.f, 20.f});
    auto child = std::make_unique<Panel>();
    child->setId("child").setRect({40.f, 10.f, 10.f, 10.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    EXPECT_TRUE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, PointerButton::Left});

    constexpr char kVerticalOverflow[] = "#parent { overflow-x: visible; overflow-y: hidden; pointer-events: none; } "
                                         "#child { pointer-events: auto; }";
    ASSERT_TRUE(stylesheet.loadRadia(kVerticalOverflow).ok());
    EXPECT_TRUE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, PointerButton::Left});

    RecordingPaintContext verticalRecording;
    surface.paint(verticalRecording);
    const PaintCommand* verticalClip = verticalRecording.last(PaintCommandKind::PushClip);
    ASSERT_NE(verticalClip, nullptr);
    EXPECT_FALSE(clipsAxis(verticalClip->clipAxes, ClipAxes::X));
    EXPECT_TRUE(clipsAxis(verticalClip->clipAxes, ClipAxes::Y));

    constexpr char kHorizontalOverflow[] = "#parent { overflow-x: hidden; overflow-y: visible; pointer-events: none; } "
                                           "#child { pointer-events: auto; }";
    ASSERT_TRUE(stylesheet.loadRadia(kHorizontalOverflow).ok());
    EXPECT_FALSE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));

    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_EQ(recording.clipDepth(), 0);
    EXPECT_EQ(recording.maxClipDepth(), 2);
    const PaintCommand* overflowClip = recording.last(PaintCommandKind::PushClip);
    ASSERT_NE(overflowClip, nullptr);
    EXPECT_EQ(overflowClip->rect.w, 20.f);
    EXPECT_TRUE(clipsAxis(overflowClip->clipAxes, ClipAxes::X));
    EXPECT_FALSE(clipsAxis(overflowClip->clipAxes, ClipAxes::Y));
}

TEST(SurfaceTest, TransfersMountedWidgetsBetweenSurfaces) {
    Surface first;
    Surface second;
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto button = std::make_unique<Button>();
    Button* transferred = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    first.mount(std::move(button), SurfaceLayer::Floater);
    first.pointerDown({{15.f, 15.f}, PointerButton::Left});

    std::unique_ptr<Widget> detached = first.unmount(*transferred);
    ASSERT_TRUE(detached);
    EXPECT_EQ(detached.get(), transferred);
    EXPECT_FALSE(first.hasPointerCapture());
    EXPECT_EQ(transferred->parent(), nullptr);
    second.mount(std::move(detached), SurfaceLayer::Floater);
    ASSERT_NE(transferred->parent(), nullptr);
    EXPECT_FALSE(second.unmount(*transferred->parent()));
}

TEST(SurfaceTest, HonorsVisibilityForPaintingHitTestingAndFocus) {
    Surface surface;
    surface.setViewport(100.f, 40.f);
    int visibleActivations = 0;
    int hiddenActivations = 0;
    int collapsedActivations = 0;

    auto add = [&](float x, Visibility visibility, int& activations) -> PaintProbe* {
        auto probe = std::make_unique<PaintProbe>();
        PaintProbe* result = probe.get();
        probe->setRect({x, 10.f, 20.f, 20.f}).setVisibility(visibility).setOnActivate([&activations](Widget&) { ++activations; });
        surface.mount(std::move(probe));
        return result;
    };

    PaintProbe* visible = add(0.f, Visibility::Visible, visibleActivations);
    PaintProbe* hidden = add(30.f, Visibility::Hidden, hiddenActivations);
    PaintProbe* collapsed = add(60.f, Visibility::Collapsed, collapsedActivations);
    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_EQ(visible->paints, 1);
    EXPECT_EQ(hidden->paints, 0);
    EXPECT_EQ(collapsed->paints, 0);

    EXPECT_TRUE(surface.pointerDown({{10.f, 20.f}, PointerButton::Left}));
    surface.pointerUp({{10.f, 20.f}, PointerButton::Left});
    EXPECT_FALSE(surface.pointerDown({{40.f, 20.f}, PointerButton::Left}));
    EXPECT_FALSE(surface.pointerDown({{70.f, 20.f}, PointerButton::Left}));
    EXPECT_EQ(visibleActivations, 1);
    EXPECT_EQ(hiddenActivations, 0);
    EXPECT_EQ(collapsedActivations, 0);

    surface.clearInteractionState();
    EXPECT_TRUE(surface.keyDown({kKeyTab}));
    EXPECT_TRUE(visible->hasState(WidgetState::Focused));
    EXPECT_FALSE(hidden->hasState(WidgetState::Focused));
    EXPECT_FALSE(collapsed->hasState(WidgetState::Focused));
}

TEST(SurfaceTest, InvalidatesAncestorLayoutAfterStateChanges) {
    StyleSheet styleSheet;
    constexpr char kStateLayout[] = "panel { flow: row; } switch { width: 20px; height: 10px; } "
                                    "switch:checked { width: 40px; } label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = std::make_unique<Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = std::make_unique<Switch>();
    Switch* target = control.get();
    panel->addChild(std::move(control));
    auto label = std::make_unique<Label>("after");
    Label* after = label.get();
    panel->addChild(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    EXPECT_EQ(after->rect().left(), 20.f);
    target->setChecked(true);
    surface.updateLayout();
    EXPECT_EQ(after->rect().left(), 40.f);
}

TEST(SurfaceTest, PreservesOrderedPaintingHitTestingAndFocus) {
    StyleSheet styleSheet;
    constexpr char kOrderedOverlap[] = "panel { flow: row; width: 40px; height: 20px; } "
                                       "#early { order: -1; width: 20px; height: 20px; } "
                                       "#late { order: 2; width: 20px; height: 20px; margin: 0px 0px 0px -20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOrderedOverlap).ok());
    Surface surface(styleSheet);
    surface.setViewport(40.f, 20.f);
    auto panel = std::make_unique<Panel>();
    std::vector<std::string> paintOrder;
    auto early = std::make_unique<OrderedPaintProbe>("early", paintOrder);
    auto late = std::make_unique<OrderedPaintProbe>("late", paintOrder);
    early->setId("early");
    late->setId("late");
    OrderedPaintProbe* earlyTarget = early.get();
    OrderedPaintProbe* lateTarget = late.get();
    panel->addChild(std::move(late));
    panel->addChild(std::move(early));
    surface.mount(std::move(panel));

    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_EQ(paintOrder, std::vector<std::string>({"late", "early"}));
    EXPECT_TRUE(surface.pointerDown({{5.f, 5.f}, PointerButton::Left}));
    EXPECT_TRUE(earlyTarget->hasState(WidgetState::Active));
    surface.pointerUp({{5.f, 5.f}, PointerButton::Left});
    surface.clearInteractionState();
    EXPECT_TRUE(surface.keyDown({kKeyTab}));
    EXPECT_TRUE(lateTarget->hasState(WidgetState::Focused));

    paintOrder.clear();
    earlyTarget->setVisibility(Visibility::Collapsed);
    surface.paint(recording);
    EXPECT_EQ(paintOrder, std::vector<std::string>({"late"}));
    paintOrder.clear();
    earlyTarget->setVisibility(Visibility::Visible);
    surface.paint(recording);
    EXPECT_EQ(paintOrder, std::vector<std::string>({"late", "early"}));
}

TEST(SurfaceTest, InvalidatesCachedTraversalAfterChildMutation) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto panel = std::make_unique<Panel>();
    Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    surface.root().addChild(std::move(panel));
    surface.updateLayout();

    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    parent->addChild(std::move(button));
    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(target->hasState(WidgetState::Focused));

    parent->clearChildren();
    EXPECT_FALSE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
}

} // namespace
