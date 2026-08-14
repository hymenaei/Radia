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
#include <optional>
#include "../test/lltut.h"
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
const char* noEventArguments(const rdui::EventCall& call, rdui::WidgetEventKind) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

template<typename Callback> void bindAction(rdui::Binder& binder, std::string name, Callback callback) {
    binder.event(rdui::detail::makeEventRegistration(
        std::move(name), std::nullopt, [callback = std::move(callback)](const rdui::WidgetEvent&, const rdui::EventCall&) mutable { callback(); },
        noEventArguments));
}

template<typename Event, typename Callback>
void bindSemanticEvent(rdui::Binder& binder, std::string name, std::optional<rdui::WidgetEventKind> kind, Callback callback) {
    binder.event(rdui::detail::makeEventRegistration(
        std::move(name), kind,
        [callback = std::move(callback)](const rdui::WidgetEvent& event, const rdui::EventCall&) mutable {
            callback(static_cast<const Event&>(event));
        },
        noEventArguments));
}
} // namespace

namespace tut {
class InputProbe final : public rdui::Widget {
public:
    InputProbe() : Widget("input_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    bool beginPointerInteraction(const rdui::PointerEvent& event) override {
        lastClickCount = event.clickCount;
        return false;
    }
    bool defaultCharacterInput(unsigned int codepoint) override {
        lastCodepoint = codepoint;
        return true;
    }
    bool defaultScroll(const rdui::ScrollEvent& event) override {
        lastScrollX = event.dx;
        lastScrollY = event.dy;
        return true;
    }

    uint8_t lastClickCount = 0;
    unsigned int lastCodepoint = 0;
    float lastScrollX = 0.f;
    float lastScrollY = 0.f;
};

class CaptureProbe final : public rdui::Widget {
public:
    CaptureProbe() : Widget("capture_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const rdui::PointerEvent&) override { return true; }
    bool endPointerInteraction(const rdui::PointerEvent&) override {
        ++ends;
        return true;
    }

    int ends = 0;
};

class PaintProbe final : public rdui::Widget {
public:
    PaintProbe() : Widget("paint_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(rdui::PaintContext&, const rdui::Style&, float) const override { ++paints; }

    mutable int paints = 0;
};

class OrderedPaintProbe final : public rdui::Widget {
public:
    OrderedPaintProbe(std::string name, std::vector<std::string>& paintOrder)
        : Widget("ordered_probe"), mName(std::move(name)), mPaintOrder(paintOrder) {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(rdui::PaintContext&, const rdui::Style&, float) const override { mPaintOrder.push_back(mName); }

private:
    std::string mName;
    std::vector<std::string>& mPaintOrder;
};

class RoutedProbe final : public rdui::Widget {
public:
    RoutedProbe(std::string name, std::vector<std::string>& log) : Widget("routed_probe"), mName(std::move(name)), mLog(log) {}

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const rdui::PointerEvent&) override {
        ++begins;
        return true;
    }

    bool preventDefault = false;
    int begins = 0;

protected:
    void onEvent(rdui::RoutedEvent& event) override {
        if (event.kind() != rdui::EventKind::PointerDown) return;
        const char* phase = event.phase() == rdui::EventPhase::Capture ? "capture" : event.phase() == rdui::EventPhase::Target ? "target" : "bubble";
        mLog.push_back(mName + ":" + phase);
        if (preventDefault && event.phase() == rdui::EventPhase::Target) event.preventDefault();
    }

private:
    std::string mName;
    std::vector<std::string>& mLog;
};

struct surfaceData {};
using surfaceTest = test_group<surfaceData>;
using surfaceObject = surfaceTest::object;
surfaceTest surfaceTestCase("surface");

template<> template<> void surfaceObject::test<1>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true).setOnActivate([&](rdui::Widget&) { ++activations; });
    context.root().addChild(std::move(button));
    ensure("move consumed", context.pointerMove({{15.f, 15.f}}));
    ensure("hover set", target->hasState(rdui::WidgetState::Hovered));
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure("pressed control active", target->hasState(rdui::WidgetState::Active));
    context.pointerMove({{50.f, 50.f}});
    ensure("leaving pressed control clears active", !target->hasState(rdui::WidgetState::Active));
    context.pointerMove({{15.f, 15.f}});
    ensure("re-entering pressed control restores active", target->hasState(rdui::WidgetState::Active));
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("pointer activates", activations, 1);
    ensure("control focused", context.hasFocus());
    ensure("mouse focus is not focus-visible", !target->hasState(rdui::WidgetState::FocusVisible));

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerLeave();
    ensure("mouse leave clears pressed active", !target->hasState(rdui::WidgetState::Active));
    context.pointerUp({{50.f, 50.f}, rdui::PointerButton::Left});
    ensure_equals("release outside does not activate", activations, 1);
}

template<> template<> void surfaceObject::test<2>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto control = std::make_unique<rdui::Switch>();
    rdui::Switch* target = control.get();
    int changes = 0;
    control->setOnCheckedChanged([&](bool) { ++changes; });
    control->setRect({10.f, 10.f, 40.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(control));
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure("switch toggles itself", target->checked());
    ensure("unmatched activation key-up is ignored", !context.keyUp({rdui::KEY_SPACE}));
    ensure("unmatched key-up does not toggle switch", target->checked());
    context.keyDown({rdui::KEY_SPACE});
    ensure("keyboard active state set", target->hasState(rdui::WidgetState::Active));
    ensure("mismatched activation key-up is ignored", !context.keyUp({rdui::KEY_RETURN}));
    ensure("mismatched key-up preserves held active state", target->hasState(rdui::WidgetState::Active));
    context.keyUp({rdui::KEY_SPACE});
    ensure("keyboard toggles switch", !target->checked());
    ensure("duplicate activation key-up is ignored", !context.keyUp({rdui::KEY_SPACE}));
    ensure_equals("checked callback follows both activations", changes, 2);
}

template<> template<> void surfaceObject::test<3>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(button));
    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure("control focused before mutation", context.hasFocus());
    context.root().clearChildren();
    ensure("tree mutation invalidates interaction references", !context.hasFocus());
    ensure("hover refresh after mutation is safe", !context.pointerMove({{15.f, 15.f}}));
}

template<> template<> void surfaceObject::test<4>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    button->setDisabled(true).setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(button));
    ensure("disabled control still blocks pointer", context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure("disabled control is not focused", !context.hasFocus());
}

template<> template<> void surfaceObject::test<5>() {
    rdui::StyleSheet styleSheet;
    styleSheet.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; } label { height: 20px; }");
    rdui::Surface context(styleSheet);
    context.setViewport(200.f, 200.f);

    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* floaterPtr = floater.get();
    floater->setTitle("title").setCanClose(false).setCanMinimize(true);
    auto content = std::make_unique<rdui::Label>("content");
    rdui::Label* contentNode = content.get();
    floater->addChild(std::move(content));
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    context.mountFloater(std::move(floater));
    context.updateLayout();

    ensure("header starts drag", context.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left}));
    ensure("captured move handled", context.pointerMove({{50.f, 120.f}, rdui::PointerButton::Left}));
    context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
    ensure_equals("drag moves x", floaterPtr->rect().x, 40.f);
    ensure_equals("drag moves y", floaterPtr->rect().y, 30.f);

    const float expandedTop = floaterPtr->rect().top();
    const float expandedWidth = floaterPtr->rect().w;
    floaterPtr->setMinimized(true);
    ensure("minimized state is style-visible", floaterPtr->hasState(rdui::WidgetState::Minimized));
    ensure("content box collapsed while minimized", floaterPtr->content()->visibility() == rdui::Visibility::Collapsed);
    ensure("child visibility is preserved while content box collapses", contentNode->visibility() == rdui::Visibility::Visible);
    ensure_equals("minimize preserves top", floaterPtr->rect().top(), expandedTop);
    ensure_equals("minimize uses header height", floaterPtr->rect().h, 30.f);
    ensure("minimize shrinks width to header identity and controls", floaterPtr->rect().w < expandedWidth);
    floaterPtr->setMinimized(false);
    ensure("expanded state clears minimized style", !floaterPtr->hasState(rdui::WidgetState::Minimized));
    ensure("content box visibility restored", floaterPtr->content()->visibility() == rdui::Visibility::Visible);
    ensure("child remains visible after expansion", contentNode->visibility() == rdui::Visibility::Visible);
    ensure_equals("expanded height restored", floaterPtr->rect().h, 100.f);
    ensure_equals("expanded width restored", floaterPtr->rect().w, expandedWidth);

    ensure("header double-click is handled", context.pointerDown({{50.f, 120.f}, rdui::PointerButton::Left, 0, 2}));
    ensure("header double-click minimizes", floaterPtr->minimized());
    context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
    ensure("second header double-click is handled", context.pointerDown({{50.f, 120.f}, rdui::PointerButton::Left, 0, 2}));
    ensure("header double-click restores", !floaterPtr->minimized());
    context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
}

template<> template<> void surfaceObject::test<6>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setOnActivate([&](rdui::Widget&) { ++activations; });
    context.root().addChild(std::move(button));

    for (rdui::PointerButton pointerButton :
         {rdui::PointerButton::Right, rdui::PointerButton::Middle, rdui::PointerButton::Auxiliary1, rdui::PointerButton::Auxiliary2}) {
        ensure("non-left down is consumed over control", context.pointerDown({{15.f, 15.f}, pointerButton}));
        ensure("non-left up is consumed over control", context.pointerUp({{15.f, 15.f}, pointerButton}));
    }
    ensure_equals("non-left buttons do not activate", activations, 0);
    ensure("non-left buttons do not focus", !context.hasFocus());
    ensure("non-left buttons do not set active state", !target->hasState(rdui::WidgetState::Active));

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("left button still activates", activations, 1);
}

template<> template<> void surfaceObject::test<7>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto probe = std::make_unique<InputProbe>();
    InputProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(probe));

    ensure("double click down handled", context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left, 0, 2}));
    ensure_equals("double click count reaches node", target->lastClickCount, static_cast<uint8_t>(2));
    ensure("double click release handled", context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure("double click release clears active", !target->hasState(rdui::WidgetState::Active));
    ensure("probe receives focus", context.hasFocus());
    ensure("character input handled by focus", context.charInput(0x03A9));
    ensure_equals("Unicode codepoint reaches focus", target->lastCodepoint, 0x03A9u);

    ensure("vertical scroll handled", context.scroll({{15.f, 15.f}, 0.f, 3.f}));
    ensure_equals("vertical scroll reaches node", target->lastScrollY, 3.f);
    ensure("horizontal scroll handled", context.scroll({{15.f, 15.f}, -2.f, 0.f}));
    ensure_equals("horizontal scroll reaches node", target->lastScrollX, -2.f);

    context.clearInteractionState();
    ensure("focus loss clears focus", !context.hasFocus());
    ensure("character input ignored without focus", !context.charInput('x'));

    ensure("hover restored", context.pointerMove({{15.f, 15.f}}));
    context.pointerLeave();
    ensure("mouse leave clears hover", !target->hasState(rdui::WidgetState::Hovered));
}

template<> template<> void surfaceObject::test<8>() {
    rdui::StyleSheet styleSheet;
    styleSheet.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; }");
    rdui::Surface context(styleSheet);
    context.setViewport(200.f, 200.f);
    auto floater = std::make_unique<rdui::Floater>();
    floater->setTitle("title").setCanClose(false).setCanMinimize(false);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    context.mountFloater(std::move(floater));
    context.updateLayout();

    ensure("header starts capture", context.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left}));
    ensure("context owns capture", context.hasPointerCapture());
    ensure("captured move outside viewport handled", context.pointerMove({{-50.f, -50.f}, rdui::PointerButton::Left}));
    ensure("captured release outside viewport handled", context.pointerUp({{-50.f, -50.f}, rdui::PointerButton::Left}));
    ensure("release ends capture", !context.hasPointerCapture());

    ensure("second header press starts capture", context.pointerDown({{10.f, 90.f}, rdui::PointerButton::Left}));
    context.clearInteractionState();
    ensure("capture loss clears capture", !context.hasPointerCapture());
}

template<> template<> void surfaceObject::test<9>() {
    rdui::Surface context;
    context.setViewport(200.f, 200.f);

    auto first = std::make_unique<rdui::Button>();
    rdui::Button* firstTarget = first.get();
    first->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.root().addChild(std::move(first));

    auto hidden = std::make_unique<rdui::Button>();
    rdui::Button* hiddenTarget = hidden.get();
    hidden->setVisibility(rdui::Visibility::Hidden).setRect({40.f, 10.f, 20.f, 20.f});
    context.root().addChild(std::move(hidden));

    auto disabled = std::make_unique<rdui::Button>();
    rdui::Button* disabledTarget = disabled.get();
    disabled->setDisabled(true).setRect({70.f, 10.f, 20.f, 20.f});
    context.root().addChild(std::move(disabled));

    auto last = std::make_unique<rdui::Switch>();
    rdui::Switch* lastTarget = last.get();
    last->setRect({100.f, 10.f, 40.f, 20.f});
    context.root().addChild(std::move(last));

    ensure("Tab focuses first control", context.keyDown({rdui::KEY_TAB}));
    ensure("first is focused", firstTarget->hasState(rdui::WidgetState::Focused));
    ensure("Tab focus is visible", firstTarget->hasState(rdui::WidgetState::FocusVisible));
    ensure("Tab key-up consumed", context.keyUp({rdui::KEY_TAB}));

    context.keyDown({rdui::KEY_TAB});
    ensure("Tab skips hidden and disabled controls", lastTarget->hasState(rdui::WidgetState::Focused));
    ensure("hidden control not focused", !hiddenTarget->hasState(rdui::WidgetState::Focused));
    ensure("disabled control not focused", !disabledTarget->hasState(rdui::WidgetState::Focused));

    context.keyDown({rdui::KEY_TAB});
    ensure("forward traversal wraps", firstTarget->hasState(rdui::WidgetState::Focused));
    context.keyDown({rdui::KEY_TAB, rdui::MODIFIER_SHIFT});
    ensure("Shift+Tab traverses backward and wraps", lastTarget->hasState(rdui::WidgetState::Focused));

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure("mouse moves focus", firstTarget->hasState(rdui::WidgetState::Focused));
    ensure("mouse focus clears focus-visible", !firstTarget->hasState(rdui::WidgetState::FocusVisible));
    firstTarget->setVisibility(rdui::Visibility::Hidden);
    ensure("hidden focused node rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
    ensure("hidden focused node clears focus", !context.hasFocus());
    firstTarget->setVisibility(rdui::Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    firstTarget->setDisabled(true);
    ensure("disabled focused node rejects character input", !context.charInput('x'));
    ensure("disabled focused node clears focus", !context.hasFocus());
    firstTarget->setDisabled(false);
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.clearInteractionState();
    ensure("focus loss clears focused state", !firstTarget->hasState(rdui::WidgetState::Focused));
    ensure("focus loss clears focus-visible state", !firstTarget->hasState(rdui::WidgetState::FocusVisible));
}

template<> template<> void surfaceObject::test<10>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto panel = std::make_unique<rdui::Panel>();
    rdui::Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    panel->addChild(std::move(button));
    context.root().addChild(std::move(panel));

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    context.keyDown({rdui::KEY_SPACE});
    ensure("keyboard press active before capture loss", target->hasState(rdui::WidgetState::Active));
    context.clearInteractionState();
    ensure("capture loss clears hover", !target->hasState(rdui::WidgetState::Hovered));
    ensure("capture loss clears keyboard active", !target->hasState(rdui::WidgetState::Active));
    ensure("capture loss clears focus", !context.hasFocus());

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure("pointer press active before capture loss", target->hasState(rdui::WidgetState::Active));
    context.clearInteractionState();
    ensure("capture loss clears pointer active", !target->hasState(rdui::WidgetState::Active));

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    parent->setVisibility(rdui::Visibility::Hidden);
    ensure("hidden ancestor rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
    ensure("hidden ancestor clears descendant focus", !context.hasFocus());
    parent->setVisibility(rdui::Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    parent->setDisabled(true);
    ensure("disabled ancestor rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
    ensure("disabled ancestor clears descendant focus", !context.hasFocus());
}

template<> template<> void surfaceObject::test<11>() {
    rdui::Surface context;
    context.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    button->setEventCall(rdui::WidgetEventKind::MouseDown, rdui::EventCall("press"));
    button->setEventCall(rdui::WidgetEventKind::MouseUp, rdui::EventCall("release"));
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("click"));
    button->setEventCall(rdui::WidgetEventKind::DoubleClick, rdui::EventCall("doubleClick"));
    button->setEventCall(rdui::WidgetEventKind::ContextMenu, rdui::EventCall("contextMenu"));
    context.root().addChild(std::move(button));

    std::vector<std::string> events;
    rdui::Binder binder(context.root());
    bindSemanticEvent<rdui::MouseWidgetEvent>(binder, "press", std::nullopt, [&](const rdui::MouseWidgetEvent& event) {
        ensure("mouse context reports button", event.mouse.button != rdui::PointerButton::NoButton);
        events.push_back("down");
    });
    bindAction(binder, "release", [&] { events.push_back("up"); });
    bindAction(binder, "click", [&] { events.push_back("click"); });
    bindSemanticEvent<rdui::MouseWidgetEvent>(binder, "doubleClick", std::nullopt, [&](const rdui::MouseWidgetEvent& event) {
        ensure_equals("double click preserves native count", event.mouse.clickCount, 2);
        events.push_back("double");
    });
    bindSemanticEvent<rdui::MouseWidgetEvent>(binder, "contextMenu", std::nullopt, [&](const rdui::MouseWidgetEvent& event) {
        ensure("context menu reports right button", event.mouse.button == rdui::PointerButton::Right);
        events.push_back("context");
    });
    rdui::PreparedBindingResult prepared = binder.prepare();
    const bool bindingPrepared = prepared.ok();
    rdui::Binding binding = bindingPrepared ? prepared.binding.commit() : rdui::Binding{};
    ensure("mouse actions bind", bindingPrepared && binding);

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("down up click order", events.size(), 3U);
    ensure_equals("down first", events[0], "down");
    ensure_equals("up second", events[1], "up");
    ensure_equals("click last", events[2], "click");

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    context.pointerUp({{50.f, 50.f}, rdui::PointerButton::Left});
    ensure_equals("release outside still emits up without click", events.size(), 5U);
    ensure_equals("outside release ends pair", events.back(), "up");

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Right});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Right});
    ensure_equals("context menu follows right mouse up", events.size(), 8U);
    ensure_equals("context menu is last", events.back(), "context");

    context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left, 0, 2});
    context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("second click emits click then double click", events.size(), 12U);
    ensure_equals("second click precedes double click", events[10], "click");
    ensure_equals("double click is last", events[11], "double");
}

template<> template<> void surfaceObject::test<12>() {
    rdui::Surface context;
    context.setViewport(100.f, 80.f);
    auto panel = std::make_unique<rdui::Panel>();
    rdui::Panel* mounted = panel.get();
    context.mount(std::move(panel));
    ensure("mounted widget unmounts", context.unmount(*mounted) != nullptr);

    ensure_equals("unmount leaves valid root width", context.root().rect().w, 100.f);
    ensure_equals("unmount leaves valid root height", context.root().rect().h, 80.f);
    ensure("input remains safe after unmount", !context.pointerDown({{10.f, 10.f}, rdui::PointerButton::Left}));
}

template<> template<> void surfaceObject::test<13>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    auto probe = std::make_unique<CaptureProbe>();
    CaptureProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    surface.root().addChild(std::move(probe));

    ensure("probe captures pointer", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure("surface records capture", surface.hasPointerCapture());
    target->setDisabled(true);
    ensure("disabling widget immediately clears capture", !surface.hasPointerCapture());
    ensure_equals("capture cancellation ends widget interaction", target->ends, 1);
}

template<> template<> void surfaceObject::test<14>() {
    rdui::System system;
    ensure("global delay accepts positive duration", system.setLongClickDelay(std::chrono::milliseconds(600)));
    std::unique_ptr<rdui::Surface> ownedSurface = system.createSurface(rdui::fixedTextMetrics());
    rdui::Surface& surface = *ownedSurface;
    surface.setViewport(100.f, 100.f);

    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    button->setEventCall(rdui::WidgetEventKind::LongClick, rdui::EventCall("hold"));
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("tap"));
    button->setEventCall(rdui::WidgetEventKind::MouseUp, rdui::EventCall("release"));
    surface.root().addChild(std::move(button));

    int holds = 0;
    int taps = 0;
    int releases = 0;
    std::chrono::milliseconds heldFor{0};
    rdui::Binder binder(surface.root());
    bindSemanticEvent<rdui::LongClickEvent>(binder, "hold", rdui::WidgetEventKind::LongClick, [&](const rdui::LongClickEvent& event) {
        heldFor = event.heldFor;
        ++holds;
    });
    bindAction(binder, "tap", [&] { ++taps; });
    bindAction(binder, "release", [&] { ++releases; });
    rdui::PreparedBindingResult prepared = binder.prepare();
    const bool bindingPrepared = prepared.ok();
    rdui::Binding binding = bindingPrepared ? prepared.binding.commit() : rdui::Binding{};
    ensure("long click actions bind", bindingPrepared && binding);

    surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    surface.update(std::chrono::milliseconds(599));
    ensure_equals("global threshold not early", holds, 0);
    surface.update(std::chrono::milliseconds(1));
    ensure_equals("global threshold fires once", holds, 1);
    ensure_equals("long click reports held duration", heldFor.count(), 600LL);
    surface.update(std::chrono::milliseconds(500));
    ensure_equals("held action does not repeat", holds, 1);
    surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("release still fires after long click", releases, 1);
    ensure_equals("long click suppresses click", taps, 0);

    target->setLongClickDelay(std::chrono::milliseconds(200));
    surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    surface.update(std::chrono::milliseconds(200));
    ensure_equals("widget delay overrides global threshold", holds, 2);
    ensure_equals("override duration reaches typed event", heldFor.count(), 200LL);
    surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("override release still fires", releases, 2);
    ensure_equals("override long click also suppresses click", taps, 0);
}

template<> template<> void surfaceObject::test<15>() {
    rdui::StyleSheet styleSheet;
    ensure("pointer policy stylesheet compiles", styleSheet.loadRadia("button { pointer-events: none; } panel { pointer-events: auto; }").ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto button = std::make_unique<rdui::Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(button));
    auto panel = std::make_unique<rdui::Panel>();
    panel->setRect({40.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(panel));

    ensure("style can disable interactive widget without layout", !surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure("style can enable noninteractive widget without layout", surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));
}

template<> template<> void surfaceObject::test<16>() {
    rdui::StyleSheet styleSheet;
    ensure("automatic layout stylesheet compiles", styleSheet.loadRadia("panel { flow: row; } label { height: 10px; }").ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = std::make_unique<rdui::Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = std::make_unique<rdui::Label>("a");
    rdui::Label* text = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));

    surface.updateLayout();
    const float shortWidth = text->rect().w;
    text->setText("a much longer label");
    surface.updateLayout();
    ensure("intrinsic mutation automatically remeasures Surface", text->rect().w > shortWidth);
}

template<> template<> void surfaceObject::test<17>() {
    rdui::StyleSheet styleSheet;
    ensure("initial generated stylesheet compiles", styleSheet.loadRadia("label { width: 10px; height: 10px; }").ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = std::make_unique<rdui::Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = std::make_unique<rdui::Label>("text");
    rdui::Label* text = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));
    surface.updateLayout();
    ensure_equals("initial stylesheet generation arranged", text->rect().w, 10.f);

    ensure("replacement stylesheet compiles", styleSheet.loadRadia("label { width: 30px; height: 10px; }").ok());
    surface.updateLayout();
    ensure_equals("stylesheet generation invalidates cached measurement", text->rect().w, 30.f);
}

template<> template<> void surfaceObject::test<18>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    std::vector<std::string> log;

    auto parent = std::make_unique<RoutedProbe>("parent", log);
    parent->setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<RoutedProbe>("child", log);
    child->setRect({10.f, 10.f, 20.f, 20.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    ensure("routed press handled", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure_equals("capture, target, bubble each run once", log.size(), std::size_t(3));
    ensure_equals("ancestor captures first", log[0], std::string("parent:capture"));
    ensure_equals("target runs second", log[1], std::string("child:target"));
    ensure_equals("ancestor bubbles last", log[2], std::string("parent:bubble"));
}

template<> template<> void surfaceObject::test<19>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    std::vector<std::string> log;
    auto probe = std::make_unique<RoutedProbe>("target", log);
    RoutedProbe* target = probe.get();
    target->preventDefault = true;
    target->setRect({10.f, 10.f, 20.f, 20.f});
    surface.root().addChild(std::move(probe));

    ensure("prevented routed press still consumed", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure_equals("preventDefault skips widget default behavior", target->begins, 0);
    ensure("preventDefault skips pointer capture", !surface.hasPointerCapture());
}

template<> template<> void surfaceObject::test<20>() {
    rdui::StyleSheet styleSheet;
    ensure("cursor stylesheet compiles",
           styleSheet.loadRadia("#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; }").ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto parent = std::make_unique<rdui::Panel>();
    parent->setId("parent").setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<rdui::Panel>();
    child->setId("child").setRect({10.f, 10.f, 20.f, 20.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    ensure("child receives hover", surface.pointerMove({{15.f, 15.f}}));
    ensure_equals("auto cursor inherits nearest explicit ancestor", static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Grab));

    ensure("explicit auto cursor compiles",
           styleSheet.loadRadia("#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; cursor: auto; }").ok());
    ensure_equals("explicit auto cursor stops inheritance", static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Default));

    ensure("cursor override compiles",
           styleSheet.loadRadia("#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; cursor: text; }").ok());
    ensure_equals("hovered widget overrides inherited cursor", static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Text));
}

template<> template<> void surfaceObject::test<21>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    int contentActivations = 0;
    int floaterActivations = 0;
    int popupActivations = 0;
    int tooltipActivations = 0;
    int dragActivations = 0;
    int modalActivations = 0;

    auto mountButton = [&](rdui::SurfaceLayer layer, int& activations, const rdui::Rect& rect) {
        auto button = std::make_unique<rdui::Button>();
        button->setRect(rect).setOnActivate([&activations](rdui::Widget&) { ++activations; });
        surface.mount(std::move(button), layer);
    };
    mountButton(rdui::SurfaceLayer::Content, contentActivations, {0.f, 0.f, 100.f, 100.f});
    mountButton(rdui::SurfaceLayer::Floater, floaterActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(rdui::SurfaceLayer::Popup, popupActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(rdui::SurfaceLayer::Tooltip, tooltipActivations, {10.f, 10.f, 30.f, 30.f});
    mountButton(rdui::SurfaceLayer::Drag, dragActivations, {10.f, 10.f, 30.f, 30.f});

    surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("popup precedes floater and content", popupActivations, 1);
    ensure_equals("tooltip layer is input transparent", tooltipActivations, 0);
    ensure_equals("drag adornment layer is input transparent", dragActivations, 0);

    mountButton(rdui::SurfaceLayer::Modal, modalActivations, {10.f, 10.f, 30.f, 30.f});
    surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("modal precedes every lower layer", modalActivations, 1);

    ensure("modal backdrop consumes outside press", surface.pointerDown({{80.f, 80.f}, rdui::PointerButton::Left}));
    surface.pointerUp({{80.f, 80.f}, rdui::PointerButton::Left});
    ensure_equals("modal backdrop blocks content activation", contentActivations, 0);

    surface.clearLayer(rdui::SurfaceLayer::Modal);
    surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
    surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
    ensure_equals("clearing modal restores popup precedence", popupActivations, 2);
}

template<> template<> void surfaceObject::test<22>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    int firstActivations = 0;
    int secondActivations = 0;

    auto first = std::make_unique<rdui::Panel>();
    first->setRect({0.f, 0.f, 50.f, 50.f});
    auto firstButton = std::make_unique<rdui::Button>();
    firstButton->setRect({0.f, 0.f, 50.f, 50.f}).setOnActivate([&firstActivations](rdui::Widget&) { ++firstActivations; });
    first->addChild(std::move(firstButton));
    surface.mount(std::move(first), rdui::SurfaceLayer::Floater);

    auto second = std::make_unique<rdui::Panel>();
    second->setRect({25.f, 0.f, 50.f, 50.f});
    auto secondButton = std::make_unique<rdui::Button>();
    secondButton->setRect({25.f, 0.f, 50.f, 50.f}).setOnActivate([&secondActivations](rdui::Widget&) { ++secondActivations; });
    second->addChild(std::move(secondButton));
    surface.mount(std::move(second), rdui::SurfaceLayer::Floater);

    surface.pointerDown({{10.f, 10.f}, rdui::PointerButton::Left});
    surface.pointerUp({{10.f, 10.f}, rdui::PointerButton::Left});
    surface.pointerDown({{30.f, 10.f}, rdui::PointerButton::Left});
    surface.pointerUp({{30.f, 10.f}, rdui::PointerButton::Left});
    ensure_equals("press raises containing floater", firstActivations, 2);
    ensure_equals("previously top floater remains behind", secondActivations, 0);
}

template<> template<> void surfaceObject::test<23>() {
    rdui::StyleSheet stylesheet;
    ensure("visible overflow compiles",
           stylesheet.loadRadia("#parent { overflow: visible; pointer-events: none; } #child { pointer-events: auto; }").ok());
    rdui::Surface surface(stylesheet);
    surface.setViewport(100.f, 100.f);
    auto parent = std::make_unique<rdui::Panel>();
    parent->setId("parent").setRect({10.f, 10.f, 20.f, 20.f});
    auto child = std::make_unique<rdui::Panel>();
    child->setId("child").setRect({40.f, 10.f, 10.f, 10.f});
    parent->addChild(std::move(child));
    surface.root().addChild(std::move(parent));

    ensure("visible overflow permits descendant hit outside parent", surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, rdui::PointerButton::Left});

    const char* kVerticalOverflow = "#parent { overflow-x: visible; overflow-y: hidden; pointer-events: none; } #child { pointer-events: auto; }";
    ensure("vertical overflow longhand compiles", stylesheet.loadRadia(kVerticalOverflow).ok());
    ensure("vertical clipping permits a horizontally overflowing descendant", surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, rdui::PointerButton::Left});

    rdui::RecordingPaintContext verticalRecording;
    surface.paint(verticalRecording);
    const rdui::PaintCommand* verticalClip = verticalRecording.last(rdui::PaintCommandKind::PushClip);
    ensure("vertical overflow clip recorded", verticalClip != nullptr);
    ensure("vertical overflow clip leaves x visible", !rdui::clipsAxis(verticalClip->clipAxes, rdui::ClipAxes::X));
    ensure("vertical overflow clip clips y", rdui::clipsAxis(verticalClip->clipAxes, rdui::ClipAxes::Y));

    const char* kHorizontalOverflow = "#parent { overflow-x: hidden; overflow-y: visible; pointer-events: none; } #child { pointer-events: auto; }";
    ensure("horizontal overflow longhand compiles", stylesheet.loadRadia(kHorizontalOverflow).ok());
    ensure("horizontal clipping rejects a horizontally overflowing descendant", !surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));

    rdui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure_equals("paint clip stack balances", recording.clipDepth(), 0);
    ensure_equals("surface and overflow clips nest", recording.maxClipDepth(), 2);
    const rdui::PaintCommand* overflowClip = recording.last(rdui::PaintCommandKind::PushClip);
    ensure("overflow clip recorded", overflowClip != nullptr);
    ensure_equals("overflow clip uses parent width", overflowClip->rect.w, 20.f);
    ensure("horizontal overflow clip clips x", rdui::clipsAxis(overflowClip->clipAxes, rdui::ClipAxes::X));
    ensure("horizontal overflow clip leaves y visible", !rdui::clipsAxis(overflowClip->clipAxes, rdui::ClipAxes::Y));
}

template<> template<> void surfaceObject::test<24>() {
    rdui::Surface first;
    rdui::Surface second;
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto button = std::make_unique<rdui::Button>();
    rdui::Button* transferred = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    first.mount(std::move(button), rdui::SurfaceLayer::Floater);
    first.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});

    std::unique_ptr<rdui::Widget> detached = first.unmount(*transferred);
    ensure("mounted root can be transferred", detached && detached.get() == transferred);
    ensure("unmount clears source interaction", !first.hasPointerCapture());
    ensure("unmounted widget leaves source hierarchy", transferred->parent() == nullptr);
    second.mount(std::move(detached), rdui::SurfaceLayer::Floater);
    ensure("transferred widget enters destination hierarchy", transferred->parent() != nullptr);
    ensure("unmount rejects nested or absent widget", !second.unmount(*transferred->parent()));
}

template<> template<> void surfaceObject::test<25>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 40.f);
    int visibleActivations = 0;
    int hiddenActivations = 0;
    int collapsedActivations = 0;

    auto add = [&](float x, rdui::Visibility visibility, int& activations) -> PaintProbe* {
        auto probe = std::make_unique<PaintProbe>();
        PaintProbe* result = probe.get();
        probe->setRect({x, 10.f, 20.f, 20.f}).setVisibility(visibility).setOnActivate([&activations](rdui::Widget&) { ++activations; });
        surface.mount(std::move(probe));
        return result;
    };

    PaintProbe* visible = add(0.f, rdui::Visibility::Visible, visibleActivations);
    PaintProbe* hidden = add(30.f, rdui::Visibility::Hidden, hiddenActivations);
    PaintProbe* collapsed = add(60.f, rdui::Visibility::Collapsed, collapsedActivations);
    rdui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure_equals("Visible participates in paint", visible->paints, 1);
    ensure_equals("Hidden does not paint", hidden->paints, 0);
    ensure_equals("Collapsed does not paint", collapsed->paints, 0);

    ensure("Visible participates in hit testing", surface.pointerDown({{10.f, 20.f}, rdui::PointerButton::Left}));
    surface.pointerUp({{10.f, 20.f}, rdui::PointerButton::Left});
    ensure("Hidden is absent from hit testing", !surface.pointerDown({{40.f, 20.f}, rdui::PointerButton::Left}));
    ensure("Collapsed is absent from hit testing", !surface.pointerDown({{70.f, 20.f}, rdui::PointerButton::Left}));
    ensure_equals("only Visible activates", visibleActivations, 1);
    ensure_equals("Hidden never activates", hiddenActivations, 0);
    ensure_equals("Collapsed never activates", collapsedActivations, 0);

    surface.clearInteractionState();
    ensure("Tab finds a Visible focus target", surface.keyDown({rdui::KEY_TAB}));
    ensure("Visible receives focus", visible->hasState(rdui::WidgetState::Focused));
    ensure("Hidden does not receive focus", !hidden->hasState(rdui::WidgetState::Focused));
    ensure("Collapsed does not receive focus", !collapsed->hasState(rdui::WidgetState::Focused));
}

template<> template<> void surfaceObject::test<26>() {
    rdui::StyleSheet styleSheet;
    const char* kStateLayout =
        "panel { flow: row; } switch { width: 20px; height: 10px; } switch:checked { width: 40px; } label { width: 10px; height: 10px; }";
    ensure("state layout stylesheet compiles", styleSheet.loadRadia(kStateLayout).ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = std::make_unique<rdui::Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = std::make_unique<rdui::Switch>();
    rdui::Switch* target = control.get();
    panel->addChild(std::move(control));
    auto label = std::make_unique<rdui::Label>("after");
    rdui::Label* after = label.get();
    panel->addChild(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    ensure_equals("unchecked RSL state has initial width", after->rect().left(), 20.f);
    target->setChecked(true);
    surface.updateLayout();
    ensure_equals("RSL state change invalidates ancestor layout", after->rect().left(), 40.f);
}

template<> template<> void surfaceObject::test<27>() {
    rdui::StyleSheet styleSheet;
    const char* kOrderedOverlap =
        "panel { flow: row; width: 40px; height: 20px; } #early { order: -1; width: 20px; height: 20px; } #late { order: 2; width: 20px; height: 20px; margin: 0px 0px 0px -20px; }";
    ensure("ordered overlap stylesheet compiles", styleSheet.loadRadia(kOrderedOverlap).ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(40.f, 20.f);
    auto panel = std::make_unique<rdui::Panel>();
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

    rdui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure("paint preserves source order", paintOrder == std::vector<std::string>({"late", "early"}));
    ensure("overlapping source-later child receives the hit", surface.pointerDown({{5.f, 5.f}, rdui::PointerButton::Left}));
    ensure("hit testing follows source stacking order", earlyTarget->hasState(rdui::WidgetState::Active));
    surface.pointerUp({{5.f, 5.f}, rdui::PointerButton::Left});
    surface.clearInteractionState();
    ensure("Tab focus follows source order", surface.keyDown({rdui::KEY_TAB}));
    ensure("first source child receives focus", lateTarget->hasState(rdui::WidgetState::Focused));

    paintOrder.clear();
    earlyTarget->setVisibility(rdui::Visibility::Collapsed);
    surface.paint(recording);
    ensure("visibility changes refresh ordered traversal", paintOrder == std::vector<std::string>({"late"}));
    paintOrder.clear();
    earlyTarget->setVisibility(rdui::Visibility::Visible);
    surface.paint(recording);
    ensure("restoring visibility refreshes source traversal", paintOrder == std::vector<std::string>({"late", "early"}));
}

template<> template<> void surfaceObject::test<28>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    auto panel = std::make_unique<rdui::Panel>();
    rdui::Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    surface.root().addChild(std::move(panel));
    surface.updateLayout();

    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    parent->addChild(std::move(button));
    ensure("adding a child after a traversal invalidates cached order", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
    ensure("new child receives focus after cached-order invalidation", target->hasState(rdui::WidgetState::Focused));

    parent->clearChildren();
    ensure("clearing children after a traversal removes the cached target", !surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
}

} // namespace tut
