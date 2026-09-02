/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include "binding/binder.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "eventcall.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "nativeappearance.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace {
using radia::ui::AAIntent;
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::ClipAxes;
using radia::ui::clipsAxis;
using radia::ui::CursorStyle;
using radia::ui::Element;
using radia::ui::ElementState;
using radia::ui::Event;
using radia::ui::EventCall;
using radia::ui::EventHandler;
using radia::ui::EventPhase;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::kChangeEvent;
using radia::ui::kClickEvent;
using radia::ui::kContextMenuEvent;
using radia::ui::kDoubleClickEvent;
using radia::ui::kKeyEnd;
using radia::ui::kKeyHome;
using radia::ui::kKeyPageDown;
using radia::ui::kKeyReturn;
using radia::ui::kKeySpace;
using radia::ui::kKeyTab;
using radia::ui::kModifierShift;
using radia::ui::kPointerDownEvent;
using radia::ui::kPointerMoveEvent;
using radia::ui::kPointerUpEvent;
using radia::ui::setAuthoredEventCall;
using radia::ui::kScrollEvent;
using radia::ui::kWheelEvent;
using radia::ui::LayoutDirection;
using radia::ui::NativeAppearanceBase;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PaintContext;
using radia::ui::PaintTargetKind;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::PreparedBindingResult;
using radia::ui::RecordingPaintContext;
using radia::ui::Rect;
using radia::ui::ResourceSnapshot;
using radia::ui::ScrollbarAxisGeometry;
using radia::ui::ScrollbarPart;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::SurfaceLayer;
using radia::ui::System;
using radia::ui::Text;
using radia::ui::Vec2;
using radia::ui::Visibility;
using radia::ui::WheelEvent;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using radia::ui::detail::makeEventRegistration;
using ::testing::Message;

constexpr char kFloaterInteractionLayout[] = "floater { display: flex; flex-direction: column; } "
                                             "floater > head { height: 30px; } "
                                             "floater > body { flex-grow: 1; } "
                                             "label { height: 20px; }";

const char* noEventArguments(const EventCall& call) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

template<typename Callback> void bindAction(Binder& binder, std::string name, Callback callback) {
    binder.event(
        makeEventRegistration(std::move(name), [callback = std::move(callback)](Event&, const EventCall&) mutable { callback(); }, noEventArguments));
}

template<typename Callback> void bindSemanticEvent(Binder& binder, std::string name, Callback callback) {
    binder.event(makeEventRegistration(
        std::move(name), [callback = std::move(callback)](Event& event, const EventCall&) mutable { callback(static_cast<const Event&>(event)); },
        noEventArguments));
}
} // namespace

namespace {
class InputProbe final : public Element {
public:
    InputProbe() : Element("input_probe") {}

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
    bool defaultScroll(const WheelEvent& event) override {
        lastScrollX = event.dx;
        lastScrollY = event.dy;
        return true;
    }

    uint8_t lastClickCount = 0;
    unsigned int lastCodepoint = 0;
    float lastScrollX = 0.f;
    float lastScrollY = 0.f;
};

class CaptureProbe final : public Element {
public:
    CaptureProbe() : Element("capture_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const PointerEvent&) override { return true; }
    bool endPointerInteraction(const PointerEvent&) override {
        ++ends;
        return true;
    }

    int ends = 0;
};

class PaintProbe final : public Element {
public:
    PaintProbe() : Element("paint_probe") {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(PaintContext&, const Style&, float) const override { ++paints; }

    mutable int paints = 0;
};

class TransformedTextPaintContext final : public PaintContext {
public:
    struct TextRecord {
        std::string value;
        Rect rect;
    };

    Vec2 measureText(const std::string& text, const Style& style) const override { return fixedTextMetrics().measureText(text, style); }
    float usedLetterSpacing(const Style& style) const override { return fixedTextMetrics().usedLetterSpacing(style); }
    void pushClip(const Rect&, float, ClipAxes) override {}
    void popClip() override {}
    void pushTranslation(const Vec2& translation) override {
        mTranslations.push_back(translation);
        mTranslation = mTranslation + translation;
    }
    void popTranslation() override {
        if (mTranslations.empty()) return;
        mTranslation = mTranslation - mTranslations.back();
        mTranslations.pop_back();
    }
    void beginEffects(const Rect&, const Style&, float) override {}
    void endEffects() override {}
    void paintBox(const Rect&, const Style&, std::optional<radia::ui::TopBorderGap>) override {}
    void paintText(const std::string& text, const Rect& rect, const Style&) override {
        mTexts.push_back({text, {rect.x + mTranslation.x, rect.y + mTranslation.y, rect.w, rect.h}});
    }
    void paintIcon(const std::string&, const Rect&, const Style&, float) override {}

    const TextRecord* find(const std::string& value) const {
        const auto found = std::find_if(mTexts.begin(), mTexts.end(), [&value](const TextRecord& text) { return text.value == value; });
        return found == mTexts.end() ? nullptr : &*found;
    }

private:
    Vec2 mTranslation;
    std::vector<Vec2> mTranslations;
    std::vector<TextRecord> mTexts;
};

class OrderedPaintProbe final : public Element {
public:
    OrderedPaintProbe(std::string name, std::vector<std::string>& paintOrder)
        : Element("ordered_probe"), mName(std::move(name)), mPaintOrder(paintOrder) {}

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    void paint(PaintContext&, const Style&, float) const override { mPaintOrder.push_back(mName); }

private:
    std::string mName;
    std::vector<std::string>& mPaintOrder;
};

class RoutedProbe final : public Element {
public:
    RoutedProbe(std::string name, std::vector<std::string>& log) : Element("routed_probe"), mName(std::move(name)), mLog(log) {
        const bool parent = mName == "parent";
        addEventListener(
            kPointerDownEvent,
            [this](Event& event) {
                const char* phase = event.phase() == EventPhase::Capture ? "capture" : event.phase() == EventPhase::Target ? "target" : "bubble";
                mLog.push_back(mName + ":" + phase);
                if (preventDefault && event.phase() == EventPhase::Target) event.preventDefault();
            },
            parent);
        if (parent)
            addEventListener(kPointerDownEvent, [this](Event& event) {
                const char* phase = event.phase() == EventPhase::Capture ? "capture" : event.phase() == EventPhase::Target ? "target" : "bubble";
                mLog.push_back(mName + ":" + phase);
                if (preventDefault && event.phase() == EventPhase::Target) event.preventDefault();
            });
    }

    bool defaultPointerEvents() const override { return true; }
    bool beginPointerInteraction(const PointerEvent&) override {
        ++begins;
        return true;
    }

    bool preventDefault = false;
    int begins = 0;

private:
    std::string mName;
    std::vector<std::string>& mLog;
};

TEST(SurfaceTest, RecordsExplicitPaintTarget) {
    Surface surface;
    surface.setViewport(240.f, 120.f);
    NativeAppearanceBase appearance;
    surface.setScrollLayoutOptions({radia::ui::ScrollbarMode::Classic, &appearance});

    RecordingPaintContext recording;
    surface.paint(recording, 1.25f, {-3.f, 4.f});

    const PaintCommand* frame = recording.last(PaintCommandKind::BeginFrame);
    ASSERT_NE(frame, nullptr);
    EXPECT_FLOAT_EQ(frame->target.bounds.w, 240.f);
    EXPECT_FLOAT_EQ(frame->target.bounds.h, 120.f);
    EXPECT_FLOAT_EQ(frame->target.pixelOrigin.x, -3.f);
    EXPECT_FLOAT_EQ(frame->target.pixelOrigin.y, 4.f);
    EXPECT_FLOAT_EQ(frame->target.scale, 1.25f);
    EXPECT_EQ(frame->target.kind, PaintTargetKind::Direct);
    EXPECT_EQ(frame->target.nativeAppearance, &appearance);
    EXPECT_FALSE(frame->target.opaque);
    EXPECT_EQ(frame->target.shapeAA, AAIntent::Coverage);
    EXPECT_EQ(frame->target.textAA, AAIntent::Coverage);
    EXPECT_EQ(frame->target.clipAA, AAIntent::Coverage);
}

TEST(SurfaceTest, SizesBodyDocumentRootToViewportInsideItsMargin) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(":root { display: block; margin: 8px; background-color: #ff0000; }").ok());

    Surface surface(styleSheet);
    surface.setViewport(100.f, 80.f);
    auto body = makeElement<Element>("body");
    Element* bodyPtr = body.get();
    body->append(makeElement<HTMLButtonElement>());
    surface.mount(std::move(body));
    surface.updateLayout();

    EXPECT_FLOAT_EQ(bodyPtr->rect().x, 8.f);
    EXPECT_FLOAT_EQ(bodyPtr->rect().y, 8.f);
    EXPECT_FLOAT_EQ(bodyPtr->rect().w, 84.f);
    EXPECT_FLOAT_EQ(bodyPtr->rect().h, 64.f);
}

TEST(SurfaceTest, PaintsBodyDocumentRootBackgroundAcrossViewport) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(":root { display: block; margin: 8px; background-color: #ff0000; }").ok());

    Surface surface(styleSheet);
    surface.setViewport(100.f, 80.f);
    auto body = makeElement<Element>("body");
    body->append(makeElement<HTMLButtonElement>());
    surface.mount(std::move(body));

    RecordingPaintContext recording;
    surface.paint(recording);

    const auto command = std::find_if(recording.commands().begin(), recording.commands().end(), [](const PaintCommand& candidate) {
        return candidate.kind == PaintCommandKind::Box
            && candidate.rect.x == 0.f
            && candidate.rect.y == 0.f
            && candidate.rect.w == 100.f
            && candidate.rect.h == 80.f
            && candidate.style.backgroundColor.r == 1.f
            && candidate.style.backgroundColor.g == 0.f
            && candidate.style.backgroundColor.b == 0.f;
    });
    ASSERT_NE(command, recording.commands().end());
}

TEST(SurfaceTest, HandlesPointerHoverPressAndRelease) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true).setOnActivate([&](Element&) { ++activations; });
    context.mount(std::move(button));
    EXPECT_TRUE(context.pointerMove({{15.f, 15.f}}));
    EXPECT_TRUE(target->hasState(ElementState::Hovered));
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->hasState(ElementState::Active));
    context.pointerMove({{50.f, 50.f}});
    EXPECT_FALSE(target->hasState(ElementState::Active));
    context.pointerMove({{15.f, 15.f}});
    EXPECT_TRUE(target->hasState(ElementState::Active));
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_EQ(activations, 1);
    EXPECT_TRUE(context.hasFocus());
    EXPECT_FALSE(target->hasState(ElementState::FocusVisible));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerLeave();
    EXPECT_FALSE(target->hasState(ElementState::Active));
    context.pointerUp({{50.f, 50.f}, PointerButton::Left});
    EXPECT_EQ(activations, 1);
}

TEST(SurfaceTest, ActivatesSwitchWithMouseAndKeyboard) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto control = makeElement<HTMLInputElement>();
    HTMLInputElement* target = control.get();
    control->type("checkbox").switchMode(true);
    int changes = 0;
    control->setOnCheckedChanged([&](bool) { ++changes; });
    control->setRect({10.f, 10.f, 40.f, 20.f}).setPointerEvents(true);
    context.mount(std::move(control));
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->checked());
    EXPECT_FALSE(context.keyUp({kKeySpace}));
    EXPECT_TRUE(target->checked());
    context.keyDown({kKeySpace});
    EXPECT_TRUE(target->hasState(ElementState::Active));
    EXPECT_FALSE(context.keyUp({kKeyReturn}));
    EXPECT_TRUE(target->hasState(ElementState::Active));
    context.keyUp({kKeySpace});
    EXPECT_FALSE(target->checked());
    EXPECT_FALSE(context.keyUp({kKeySpace}));
    EXPECT_EQ(changes, 2);
}

TEST(SurfaceTest, ClearsInteractionAfterTreeMutation) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    HTMLButtonElement* mounted = button.get();
    context.mount(std::move(button));
    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(context.hasFocus());
    ASSERT_NE(context.unmount(*mounted), nullptr);
    EXPECT_FALSE(context.hasFocus());
    EXPECT_FALSE(context.pointerMove({{15.f, 15.f}}));
}

TEST(SurfaceTest, BlocksDisabledControlsWithoutFocusingThem) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    button->disabled(true).setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.mount(std::move(button));
    EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_FALSE(context.hasFocus());
}

TEST(SurfaceTest, DragsMinimizesAndRestoresFloaters) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterInteractionLayout).ok());
    Surface context(styleSheet);
    context.setViewport(200.f, 200.f);

    auto floater = radia::ui::test::makeFloater(false, true);
    HTMLFloaterElement* floaterPtr = floater.get();
    auto content = makeElement<HTMLLabelElement>("content");
    HTMLLabelElement* contentNode = content.get();
    floater->body()->append(std::move(content));
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
    EXPECT_TRUE(floaterPtr->hasState(ElementState::Minimized));
    EXPECT_EQ(floaterPtr->body()->visibility(), Visibility::Visible);
    EXPECT_EQ(contentNode->visibility(), Visibility::Visible);
    EXPECT_EQ(floaterPtr->rect().top(), expandedTop);
    EXPECT_EQ(floaterPtr->rect().h, 30.f);
    EXPECT_LT(floaterPtr->rect().w, expandedWidth);
    floaterPtr->setMinimized(false);
    EXPECT_FALSE(floaterPtr->hasState(ElementState::Minimized));
    EXPECT_EQ(floaterPtr->body()->visibility(), Visibility::Visible);
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
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    int activations = 0;
    button->setRect({10.f, 10.f, 20.f, 20.f}).setOnActivate([&](Element&) { ++activations; });
    context.mount(std::move(button));

    for (PointerButton pointerButton : {PointerButton::Right, PointerButton::Middle, PointerButton::Auxiliary1, PointerButton::Auxiliary2}) {
        SCOPED_TRACE(Message() << "pointer button: " << static_cast<int>(pointerButton));
        EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, pointerButton}));
        EXPECT_TRUE(context.pointerUp({{15.f, 15.f}, pointerButton}));
    }
    EXPECT_EQ(activations, 0);
    EXPECT_FALSE(context.hasFocus());
    EXPECT_FALSE(target->hasState(ElementState::Active));

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
    context.mount(std::move(probe));

    EXPECT_TRUE(context.pointerDown({{15.f, 15.f}, PointerButton::Left, 0, 2}));
    EXPECT_EQ(target->lastClickCount, static_cast<uint8_t>(2));
    EXPECT_TRUE(context.pointerUp({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_FALSE(target->hasState(ElementState::Active));
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
    EXPECT_FALSE(target->hasState(ElementState::Hovered));
}

TEST(SurfaceTest, RoutesWheelInputWithWheelPayload) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto probe = std::make_unique<InputProbe>();
    InputProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    std::size_t wheelEvents = 0;
    float observedDeltaX = 0.f;
    float observedDeltaY = 0.f;
    target->addEventListener(kWheelEvent, [&](Event& event) {
        ++wheelEvents;
        EXPECT_EQ(event.type(), kWheelEvent);
        EXPECT_FALSE(event.defaultPrevented());
        const WheelEvent* payload = event.wheel();
        ASSERT_NE(payload, nullptr);
        observedDeltaX = payload->dx;
        observedDeltaY = payload->dy;
    });
    context.mount(std::move(probe));

    EXPECT_TRUE(context.scroll({{15.f, 15.f}, -2.f, 3.f}));
    EXPECT_EQ(wheelEvents, std::size_t{1});
    EXPECT_EQ(observedDeltaX, -2.f);
    EXPECT_EQ(observedDeltaY, 3.f);
    EXPECT_EQ(target->lastScrollX, -2.f);
    EXPECT_EQ(target->lastScrollY, 3.f);
    EXPECT_NE(kWheelEvent, kScrollEvent);
}

TEST(SurfaceTest, PreventsDefaultWheelAction) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto probe = std::make_unique<InputProbe>();
    InputProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    target->addEventListener(kWheelEvent, [](Event& event) { event.preventDefault(); });
    context.mount(std::move(probe));

    EXPECT_TRUE(context.scroll({{15.f, 15.f}, 0.f, 3.f}));
    EXPECT_EQ(target->lastScrollX, 0.f);
    EXPECT_EQ(target->lastScrollY, 0.f);
}

TEST(SurfaceTest, HitTestsScrolledChildAtPaintedLocation) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: none; } "
                               "#target { pointer-events: auto; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    int activations = 0;

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto target = makeElement<HTMLButtonElement>();
    target->setId("target").setRect({130.f, 10.f, 20.f, 20.f}).setOnActivate([&activations](Element&) { ++activations; });
    viewport->append(std::move(target));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();
    ASSERT_GT(viewportPtr->scrollMetrics().maxScrollLeft, 0.f);
    viewportPtr->scrollTo(50.f, 0.f);

    EXPECT_TRUE(surface.pointerDown({{90.f, 20.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerUp({{90.f, 20.f}, PointerButton::Left}));
    EXPECT_EQ(activations, 1);
}

TEST(SurfaceTest, HitTestsVerticallyScrolledChildAtPaintedLocation) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: none; } "
                               "#target { pointer-events: auto; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    int activations = 0;

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto target = makeElement<HTMLButtonElement>();
    target->setId("target").setRect({10.f, -80.f, 20.f, 20.f}).setOnActivate([&activations](Element&) { ++activations; });
    viewport->append(std::move(target));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();
    ASSERT_GT(viewportPtr->scrollMetrics().maxScrollTop, 0.f);
    viewportPtr->scrollTo(0.f, viewportPtr->scrollMetrics().maxScrollTop);

    EXPECT_TRUE(surface.pointerDown({{15.f, 10.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerUp({{15.f, 10.f}, PointerButton::Left}));
    EXPECT_EQ(activations, 1);
}

TEST(SurfaceTest, ClipsScrolledContentFromHitTesting) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: none; } "
                               "#target { pointer-events: auto; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto target = makeElement<HTMLButtonElement>();
    target->setId("target").setRect({130.f, 10.f, 20.f, 20.f});
    viewport->append(std::move(target));
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_FALSE(surface.pointerDown({{135.f, 20.f}, PointerButton::Left}));
}

TEST(SurfaceTest, ScrollsScrollableElementWithWheel) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 200.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 25.f);
}

TEST(SurfaceTest, ShiftWheelScrollsHorizontallyAndPreservesWheelPayload) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 200.f, 200.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    float observedDeltaX = 0.f;
    float observedDeltaY = 0.f;
    viewportPtr->addEventListener(kWheelEvent, [&](Event& event) {
        const WheelEvent* payload = event.wheel();
        ASSERT_NE(payload, nullptr);
        observedDeltaX = payload->dx;
        observedDeltaY = payload->dy;
    });
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f, kModifierShift}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollLeft(), 25.f);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 0.f);
    EXPECT_FLOAT_EQ(observedDeltaX, 0.f);
    EXPECT_FLOAT_EQ(observedDeltaY, 25.f);
}

TEST(SurfaceTest, RecordsSemanticFallbackScrollbarRequest) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#viewport { display: block; overflow: scroll; scrollbar-mode: classic; "
                               "scrollbar-color: #112233 #445566; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 180.f, 180.f});
    viewport->append(std::move(content));
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);

    ASSERT_EQ(recording.count(PaintCommandKind::Scrollbar), 1u);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const auto& request = *command->scrollbar;
    EXPECT_EQ(request.mode, radia::ui::ScrollbarMode::Classic);
    EXPECT_EQ(request.direction, radia::ui::LayoutDirection::LeftToRight);
    EXPECT_EQ(request.appearanceRevision, 1u);
    EXPECT_FLOAT_EQ(request.metrics.thickness, 15.f);
    EXPECT_TRUE(request.geometry.horizontal.visible);
    EXPECT_TRUE(request.geometry.vertical.visible);
    EXPECT_FALSE(request.colors.automatic);
    EXPECT_NEAR(request.colors.thumb.r, 0x11 / 255.f, 1.0e-6f);
    EXPECT_NEAR(request.colors.thumb.g, 0x22 / 255.f, 1.0e-6f);
    EXPECT_NEAR(request.colors.track.b, 0x66 / 255.f, 1.0e-6f);
}

TEST(SurfaceTest, ScrollbarThumbCapturesPointerAndReachesBothEndpoints) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    int activations = 0;
    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 240.f});
    viewport->append(std::move(content));
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({85.f, 20.f, 15.f, 20.f}).setOnActivate([&activations](Element&) { ++activations; });
    viewport->append(std::move(button));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const Rect verticalBounds = command->scrollbar->geometry.vertical.bounds;

    const auto center = [](const Rect& rect) { return Vec2{rect.x + rect.w * .5f, rect.y + rect.h * .5f}; };
    const Vec2 thumbStart = center(command->scrollbar->geometry.vertical.thumb);
    EXPECT_TRUE(surface.pointerDown({thumbStart, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    EXPECT_TRUE(surface.pointerMove({{thumbStart.x, verticalBounds.bottom()}, PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), viewportPtr->scrollMetrics().maxScrollTop);
    EXPECT_TRUE(surface.pointerUp({{thumbStart.x, verticalBounds.bottom()}, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
    EXPECT_EQ(activations, 0);

    RecordingPaintContext atEndRecording;
    surface.paint(atEndRecording);
    const PaintCommand* atEndCommand = atEndRecording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(atEndCommand, nullptr);
    ASSERT_TRUE(atEndCommand->scrollbar.has_value());
    const Rect thumbAtEnd = atEndCommand->scrollbar->geometry.vertical.thumb;
    EXPECT_TRUE(surface.pointerDown({center(thumbAtEnd), PointerButton::Left}));
    EXPECT_TRUE(surface.pointerMove({{center(thumbAtEnd).x, verticalBounds.top()}, PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 0.f);
    EXPECT_TRUE(surface.pointerUp({{center(thumbAtEnd).x, verticalBounds.top()}, PointerButton::Left}));
}

TEST(SurfaceTest, RtlScrollbarHitTestingAndHorizontalTrackClicks) {
    constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {}}, ar: {strings: {}}}\n";
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", kLocalization);
    snapshot.add("skin.css", "#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }");
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({20.f, 20.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 240.f, 240.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface->mount(std::move(viewport));
    surface->updateLayout();
    ASSERT_TRUE(system.setLocale("ar"));
    surface->updateLayout();

    RecordingPaintContext recording;
    surface->paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const auto& request = *command->scrollbar;
    EXPECT_EQ(request.direction, LayoutDirection::RightToLeft);
    ASSERT_TRUE(request.geometry.vertical.visible);
    ASSERT_TRUE(request.geometry.horizontal.visible);

    const auto center = [](const Rect& rect) { return Vec2{rect.x + rect.w * .5f, rect.y + rect.h * .5f}; };
    const Vec2 verticalPoint = center(request.geometry.vertical.bounds);
    EXPECT_TRUE(surface->pointerDown({verticalPoint, PointerButton::Left}));
    EXPECT_TRUE(surface->hasPointerCapture());
    EXPECT_TRUE(surface->pointerUp({verticalPoint, PointerButton::Left}));

    const ScrollbarAxisGeometry& horizontal = request.geometry.horizontal;
    const float initialThumbX = horizontal.thumb.x;
    const Vec2 leftTrackPoint{horizontal.track.left() + horizontal.track.w * .25f, horizontal.track.y + horizontal.track.h * .5f};
    ASSERT_FALSE(horizontal.thumb.contains(leftTrackPoint));
    EXPECT_FLOAT_EQ(viewportPtr->scrollLeft(), 0.f);
    EXPECT_TRUE(surface->pointerDown({leftTrackPoint, PointerButton::Left}));
    EXPECT_GT(viewportPtr->scrollLeft(), 0.f);
    EXPECT_TRUE(surface->pointerUp({leftTrackPoint, PointerButton::Left}));

    RecordingPaintContext afterClickRecording;
    surface->paint(afterClickRecording);
    const PaintCommand* afterClickCommand = afterClickRecording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(afterClickCommand, nullptr);
    ASSERT_TRUE(afterClickCommand->scrollbar.has_value());
    EXPECT_LT(afterClickCommand->scrollbar->geometry.horizontal.thumb.x, initialThumbX);
}

TEST(SurfaceTest, RtlHorizontalWheelReversesNormalizedScrollDirection) {
    constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {}}, ar: {strings: {}}}\n";
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", kLocalization);
    snapshot.add("skin.css", "#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }");
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({20.f, 20.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 240.f, 240.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface->mount(std::move(viewport));
    ASSERT_TRUE(system.setLocale("ar"));
    surface->updateLayout();
    ASSERT_GT(viewportPtr->scrollMetrics().maxScrollLeft, 0.f);

    viewportPtr->scrollTo(viewportPtr->scrollMetrics().maxScrollLeft * .5f, 0.f);
    RecordingPaintContext initialPaint;
    surface->paint(initialPaint);
    const PaintCommand* initialCommand = initialPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(initialCommand, nullptr);
    ASSERT_TRUE(initialCommand->scrollbar.has_value());
    const float initialScrollLeft = viewportPtr->scrollLeft();
    const float initialThumbX = initialCommand->scrollbar->geometry.horizontal.thumb.x;

    EXPECT_TRUE(surface->scroll({{70.f, 60.f}, 20.f, 0.f}));
    EXPECT_LT(viewportPtr->scrollLeft(), initialScrollLeft);

    RecordingPaintContext afterWheelPaint;
    surface->paint(afterWheelPaint);
    const PaintCommand* afterWheelCommand = afterWheelPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(afterWheelCommand, nullptr);
    ASSERT_TRUE(afterWheelCommand->scrollbar.has_value());
    EXPECT_GT(afterWheelCommand->scrollbar->geometry.horizontal.thumb.x, initialThumbX);
}

TEST(SurfaceTest, RtlScrollTransformMirrorsHorizontalContentTranslation) {
    constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {}}, ar: {strings: {}}}\n";
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", kLocalization);
    snapshot.add("skin.css", "#viewport { display: block; overflow: hidden; scrollbar-width: none; } ");
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto target = makeElement<HTMLButtonElement>();
    target->setId("target").setRect({-130.f, 10.f, 20.f, 20.f});
    viewport->append(std::move(target));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface->mount(std::move(viewport));
    ASSERT_TRUE(system.setLocale("ar"));
    surface->updateLayout();

    viewportPtr->scrollTo(20.f, 0.f);
    RecordingPaintContext recording;
    surface->paint(recording);
    const auto translation = std::find_if(recording.commands().begin(), recording.commands().end(),
                                          [](const PaintCommand& command) { return command.kind == PaintCommandKind::PushTranslation; });
    ASSERT_NE(translation, recording.commands().end());
    EXPECT_FLOAT_EQ(translation->translation.x, 20.f);
    EXPECT_FLOAT_EQ(translation->translation.y, 0.f);
}

TEST(SurfaceTest, ScrollbarArrowsAndTrackPageByInputPolicy) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 300.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const auto& geometry = command->scrollbar->geometry.vertical;
    const auto center = [](const Rect& rect) { return Vec2{rect.x + rect.w * .5f, rect.y + rect.h * .5f}; };

    EXPECT_TRUE(surface.pointerDown({center(geometry.endArrow), PointerButton::Left}));
    EXPECT_TRUE(surface.pointerUp({center(geometry.endArrow), PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 40.f);

    viewportPtr->scrollTo(0.f, 0.f);
    const Vec2 trackPoint{geometry.track.x + geometry.track.w * .5f, geometry.track.y + geometry.track.h * .2f};
    EXPECT_TRUE(surface.pointerDown({trackPoint, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerUp({trackPoint, PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), viewportPtr->clientHeight() - 40.f);
}

TEST(SurfaceTest, HeldScrollbarArrowRepeatsUntilRelease) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 300.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const auto center = [](const Rect& rect) { return Vec2{rect.x + rect.w * .5f, rect.y + rect.h * .5f}; };
    const Vec2 endArrowPoint = center(command->scrollbar->geometry.vertical.endArrow);

    EXPECT_TRUE(surface.pointerDown({endArrowPoint, PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 40.f);
    surface.advanceScrollbarInteraction(.39f);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 40.f);
    surface.advanceScrollbarInteraction(.02f);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 80.f);

    EXPECT_TRUE(surface.pointerUp({endArrowPoint, PointerButton::Left}));
    const float afterRelease = viewportPtr->scrollTop();
    surface.advanceScrollbarInteraction(1.f);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), afterRelease);
}

TEST(SurfaceTest, ScrollbarTrackClickContinuesIntoThumbDrag) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 300.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const ScrollbarAxisGeometry& geometry = command->scrollbar->geometry.vertical;
    const Vec2 trackPoint{geometry.track.x + geometry.track.w * .5f, geometry.track.bottom() + geometry.track.h * .25f};
    const Vec2 dragPoint{trackPoint.x, geometry.bounds.bottom()};

    EXPECT_TRUE(surface.pointerDown({trackPoint, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    EXPECT_GT(viewportPtr->scrollTop(), 0.f);
    EXPECT_TRUE(surface.pointerMove({dragPoint, PointerButton::Left}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), viewportPtr->scrollMetrics().maxScrollTop);
    EXPECT_TRUE(surface.pointerUp({dragPoint, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
}

TEST(SurfaceTest, KeepsDefaultCursorOverScrollbar) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 240.f});
    viewport->append(std::move(content));
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const PaintCommand* command = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->scrollbar.has_value());
    const Rect thumb = command->scrollbar->geometry.vertical.thumb;
    const Vec2 center{thumb.x + thumb.w * .5f, thumb.y + thumb.h * .5f};

    EXPECT_TRUE(surface.pointerMove({center}));
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
    EXPECT_TRUE(surface.pointerDown({center, PointerButton::Left}));
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
    EXPECT_TRUE(surface.pointerUp({center, PointerButton::Left}));
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
}

TEST(SurfaceTest, ReportsPartSpecificScrollbarHoverAndPressedState) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: scroll; scrollbar-mode: classic; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 300.f});
    viewport->append(std::move(content));
    surface.mount(std::move(viewport));
    surface.updateLayout();

    RecordingPaintContext initialPaint;
    surface.paint(initialPaint);
    const PaintCommand* initialCommand = initialPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(initialCommand, nullptr);
    ASSERT_TRUE(initialCommand->scrollbar.has_value());
    const auto center = [](const Rect& rect) { return Vec2{rect.x + rect.w * .5f, rect.y + rect.h * .5f}; };
    const Vec2 startArrowPoint = center(initialCommand->scrollbar->geometry.vertical.startArrow);

    surface.pointerMove({startArrowPoint});
    RecordingPaintContext arrowHoverPaint;
    surface.paint(arrowHoverPaint);
    const PaintCommand* arrowHoverCommand = arrowHoverPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(arrowHoverCommand, nullptr);
    ASSERT_TRUE(arrowHoverCommand->scrollbar.has_value());
    EXPECT_EQ(arrowHoverCommand->scrollbar->vertical.hoveredPart, ScrollbarPart::StartArrow);
    EXPECT_EQ(arrowHoverCommand->scrollbar->vertical.pressedPart, ScrollbarPart::NoneValue);

    EXPECT_TRUE(surface.pointerDown({startArrowPoint, PointerButton::Left}));
    RecordingPaintContext arrowPressedPaint;
    surface.paint(arrowPressedPaint);
    const PaintCommand* arrowPressedCommand = arrowPressedPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(arrowPressedCommand, nullptr);
    ASSERT_TRUE(arrowPressedCommand->scrollbar.has_value());
    EXPECT_EQ(arrowPressedCommand->scrollbar->vertical.hoveredPart, ScrollbarPart::StartArrow);
    EXPECT_EQ(arrowPressedCommand->scrollbar->vertical.pressedPart, ScrollbarPart::StartArrow);
    EXPECT_TRUE(surface.pointerUp({startArrowPoint, PointerButton::Left}));

    RecordingPaintContext thumbGeometryPaint;
    surface.paint(thumbGeometryPaint);
    const PaintCommand* thumbGeometryCommand = thumbGeometryPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(thumbGeometryCommand, nullptr);
    ASSERT_TRUE(thumbGeometryCommand->scrollbar.has_value());
    const Vec2 thumbPoint = center(thumbGeometryCommand->scrollbar->geometry.vertical.thumb);

    surface.pointerMove({thumbPoint});
    RecordingPaintContext thumbHoverPaint;
    surface.paint(thumbHoverPaint);
    const PaintCommand* thumbHoverCommand = thumbHoverPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(thumbHoverCommand, nullptr);
    ASSERT_TRUE(thumbHoverCommand->scrollbar.has_value());
    EXPECT_EQ(thumbHoverCommand->scrollbar->vertical.hoveredPart, ScrollbarPart::Thumb);
    EXPECT_EQ(thumbHoverCommand->scrollbar->vertical.pressedPart, ScrollbarPart::NoneValue);

    EXPECT_TRUE(surface.pointerDown({thumbPoint, PointerButton::Left}));
    RecordingPaintContext thumbPressedPaint;
    surface.paint(thumbPressedPaint);
    const PaintCommand* thumbPressedCommand = thumbPressedPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(thumbPressedCommand, nullptr);
    ASSERT_TRUE(thumbPressedCommand->scrollbar.has_value());
    EXPECT_EQ(thumbPressedCommand->scrollbar->vertical.hoveredPart, ScrollbarPart::Thumb);
    EXPECT_EQ(thumbPressedCommand->scrollbar->vertical.pressedPart, ScrollbarPart::Thumb);
    EXPECT_TRUE(surface.pointerUp({thumbPoint, PointerButton::Left}));
}

TEST(SurfaceTest, ScrollsFocusedAncestorWithKeyboard) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 240.f});
    viewport->append(std::move(content));
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 20.f, 20.f, 20.f});
    HTMLButtonElement* buttonPtr = button.get();
    viewport->append(std::move(button));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_TRUE(surface.pointerDown({{15.f, 30.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerUp({{15.f, 30.f}, PointerButton::Left}));
    EXPECT_TRUE(buttonPtr->hasState(ElementState::Focused));
    EXPECT_TRUE(surface.keyDown({kKeyPageDown}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), viewportPtr->clientHeight() - 40.f);
    EXPECT_TRUE(surface.keyDown({kKeyEnd}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), viewportPtr->scrollMetrics().maxScrollTop);
    EXPECT_TRUE(surface.keyDown({kKeyHome}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 0.f);
}

TEST(SurfaceTest, ScrollsColumnContentDownAndMovesPaintedText) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#viewport { display: flex; flex-direction: column; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; } "
                               "#first, #second, #third { height: 60px; pointer-events: none; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto appendItem = [&viewport](const char* id, const char* value) {
        auto item = makeElement<HTMLPanelElement>();
        item->setId(id).textContent(value);
        viewport->append(std::move(item));
    };
    appendItem("first", "first");
    appendItem("second", "second");
    appendItem("third", "third");
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    ASSERT_GT(viewportPtr->scrollMetrics().maxScrollTop, 0.f);
    TransformedTextPaintContext initialPaint;
    surface.paint(initialPaint);
    const TransformedTextPaintContext::TextRecord* initialText = initialPaint.find("third");
    ASSERT_NE(initialText, nullptr);

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 25.f);

    TransformedTextPaintContext scrolledPaint;
    surface.paint(scrolledPaint);
    const TransformedTextPaintContext::TextRecord* scrolledText = scrolledPaint.find("third");
    ASSERT_NE(scrolledText, nullptr);
    EXPECT_FLOAT_EQ(scrolledText->rect.x, initialText->rect.x);
    EXPECT_FLOAT_EQ(scrolledText->rect.y, initialText->rect.y + 25.f);
}

TEST(SurfaceTest, PreventsDefaultWheelScrollingWhenCanceled) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 200.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    viewportPtr->addEventListener(kWheelEvent, [](Event& event) { event.preventDefault(); });
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 0.f);
}

TEST(SurfaceTest, DoesNotWheelScrollHiddenOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: hidden; pointer-events: auto; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 200.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    surface.mount(std::move(viewport));
    surface.updateLayout();

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f}));
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 0.f);
    viewportPtr->scrollTo(0.f, 25.f);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 25.f);
}

TEST(SurfaceTest, ChainsWheelDeltaFromInnerToOuterScroller) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("#outer { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; } "
                               "#inner { display: block; overflow: auto; scrollbar-mode: overlay; pointer-events: auto; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);

    auto outer = makeElement<HTMLPanelElement>();
    outer->setId("outer").setRect({0.f, 0.f, 100.f, 100.f});
    auto inner = makeElement<HTMLPanelElement>();
    inner->setId("inner").setRect({0.f, 0.f, 100.f, 200.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 300.f});
    inner->append(std::move(content));
    HTMLPanelElement* outerPtr = outer.get();
    HTMLPanelElement* innerPtr = inner.get();
    outer->append(std::move(inner));
    surface.mount(std::move(outer));
    surface.updateLayout();
    ASSERT_FLOAT_EQ(outerPtr->scrollMetrics().maxScrollTop, 100.f);
    ASSERT_FLOAT_EQ(innerPtr->scrollMetrics().maxScrollTop, 100.f);
    innerPtr->scrollTo(0.f, 90.f);

    EXPECT_TRUE(surface.scroll({{50.f, 50.f}, 0.f, 25.f}));
    EXPECT_FLOAT_EQ(innerPtr->scrollTop(), 100.f);
    EXPECT_FLOAT_EQ(outerPtr->scrollTop(), 15.f);
}

TEST(SurfaceTest, DispatchesCoalescedTargetOnlyScrollNotification) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("#viewport { display: block; overflow: auto; scrollbar-mode: overlay; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    int targetNotifications = 0;
    int parentNotifications = 0;

    auto parent = makeElement<HTMLPanelElement>();
    parent->setRect({0.f, 0.f, 100.f, 100.f});
    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 100.f, 200.f});
    viewport->append(std::move(content));
    HTMLPanelElement* viewportPtr = viewport.get();
    parent->addEventListener(kScrollEvent, [&parentNotifications](Event&) { ++parentNotifications; });
    viewportPtr->addEventListener(kScrollEvent, [&](Event& event) {
        ++targetNotifications;
        EXPECT_EQ(event.phase(), EventPhase::Target);
        EXPECT_EQ(event.currentTarget(), viewportPtr);
        EXPECT_EQ(event.target(), viewportPtr);
        EXPECT_FALSE(event.cancelable());
        event.preventDefault();
        EXPECT_FALSE(event.defaultPrevented());
    });
    parent->append(std::move(viewport));
    surface.mount(std::move(parent));
    surface.updateLayout();

    viewportPtr->scrollTo(0.f, 10.f);
    viewportPtr->scrollTo(0.f, 20.f);
    surface.updateLayout();

    EXPECT_EQ(targetNotifications, 1);
    EXPECT_EQ(parentNotifications, 0);
    EXPECT_FLOAT_EQ(viewportPtr->scrollTop(), 20.f);
}

TEST(SurfaceTest, ReleasesPointerCaptureWhenInteractionStateClears) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterInteractionLayout).ok());
    Surface context(styleSheet);
    context.setViewport(200.f, 200.f);
    auto floater = radia::ui::test::makeFloater();
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

    auto first = makeElement<HTMLButtonElement>();
    HTMLButtonElement* firstTarget = first.get();
    first->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    context.mount(std::move(first));

    auto hidden = makeElement<HTMLButtonElement>();
    HTMLButtonElement* hiddenTarget = hidden.get();
    hidden->setVisibility(Visibility::Hidden).setRect({40.f, 10.f, 20.f, 20.f});
    context.mount(std::move(hidden));

    auto disabled = makeElement<HTMLButtonElement>();
    HTMLButtonElement* disabledTarget = disabled.get();
    disabled->disabled(true).setRect({70.f, 10.f, 20.f, 20.f});
    context.mount(std::move(disabled));

    auto last = makeElement<HTMLInputElement>();
    HTMLInputElement* lastTarget = last.get();
    last->type("checkbox").switchMode(true);
    last->setRect({100.f, 10.f, 40.f, 20.f});
    context.mount(std::move(last));

    EXPECT_TRUE(context.keyDown({kKeyTab}));
    EXPECT_TRUE(firstTarget->hasState(ElementState::Focused));
    EXPECT_TRUE(firstTarget->hasState(ElementState::FocusVisible));
    EXPECT_TRUE(context.keyUp({kKeyTab}));

    context.keyDown({kKeyTab});
    EXPECT_TRUE(lastTarget->hasState(ElementState::Focused));
    EXPECT_FALSE(hiddenTarget->hasState(ElementState::Focused));
    EXPECT_FALSE(disabledTarget->hasState(ElementState::Focused));

    context.keyDown({kKeyTab});
    EXPECT_TRUE(firstTarget->hasState(ElementState::Focused));
    context.keyDown({kKeyTab, kModifierShift});
    EXPECT_TRUE(lastTarget->hasState(ElementState::Focused));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(firstTarget->hasState(ElementState::Focused));
    EXPECT_FALSE(firstTarget->hasState(ElementState::FocusVisible));
    firstTarget->setVisibility(Visibility::Hidden);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
    firstTarget->setVisibility(Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    firstTarget->disabled(true);
    EXPECT_FALSE(context.charInput('x'));
    EXPECT_FALSE(context.hasFocus());
    firstTarget->disabled(false);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.clearInteractionState();
    EXPECT_FALSE(firstTarget->hasState(ElementState::Focused));
    EXPECT_FALSE(firstTarget->hasState(ElementState::FocusVisible));
}

TEST(SurfaceTest, ClearsInteractionWhenDescendantsBecomeUnavailable) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    HTMLPanelElement* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    panel->append(std::move(button));
    context.mount(std::move(panel));

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    context.pointerUp({{15.f, 15.f}, PointerButton::Left});
    context.keyDown({kKeySpace});
    EXPECT_TRUE(target->hasState(ElementState::Active));
    context.clearInteractionState();
    EXPECT_FALSE(target->hasState(ElementState::Hovered));
    EXPECT_FALSE(target->hasState(ElementState::Active));
    EXPECT_FALSE(context.hasFocus());

    context.pointerMove({{15.f, 15.f}});
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    EXPECT_TRUE(target->hasState(ElementState::Active));
    context.clearInteractionState();
    EXPECT_FALSE(target->hasState(ElementState::Active));

    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    parent->setVisibility(Visibility::Hidden);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
    parent->setVisibility(Visibility::Visible);
    context.pointerDown({{15.f, 15.f}, PointerButton::Left});
    parent->disabled(true);
    EXPECT_FALSE(context.keyDown({kKeySpace}));
    EXPECT_FALSE(context.hasFocus());
}

TEST(SurfaceTest, DispatchesMouseBindingsInExpectedOrder) {
    Surface context;
    context.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    setAuthoredEventCall(*button, kPointerDownEvent, EventCall("press"));
    setAuthoredEventCall(*button, kPointerUpEvent, EventCall("release"));
    setAuthoredEventCall(*button, kClickEvent, EventCall("click"));
    setAuthoredEventCall(*button, kDoubleClickEvent, EventCall("doubleClick"));
    setAuthoredEventCall(*button, kContextMenuEvent, EventCall("contextMenu"));
    HTMLButtonElement* mounted = button.get();
    context.mount(std::move(button));

    std::vector<std::string> events;
    Binder binder(*mounted);
    bindSemanticEvent(binder, "press", [&](const Event& event) {
        ASSERT_NE(event.pointer(), nullptr);
        EXPECT_NE(event.pointer()->button, PointerButton::NoButton);
        events.push_back("down");
    });
    bindAction(binder, "release", [&] { events.push_back("up"); });
    bindAction(binder, "click", [&] { events.push_back("click"); });
    bindSemanticEvent(binder, "doubleClick", [&](const Event& event) {
        ASSERT_NE(event.pointer(), nullptr);
        EXPECT_EQ(event.pointer()->clickCount, 2);
        events.push_back("double");
    });
    bindSemanticEvent(binder, "contextMenu", [&](const Event& event) {
        ASSERT_NE(event.pointer(), nullptr);
        EXPECT_EQ(event.pointer()->button, PointerButton::Right);
        events.push_back("context");
    });
    PreparedBindingResult prepared = binder.prepare();
    ASSERT_TRUE(prepared.ok());
    Binding binding = prepared.binding.commit();
    ASSERT_TRUE(static_cast<bool>(binding));

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

TEST(SurfaceTest, UnmountsRootElementsSafely) {
    Surface context;
    context.setViewport(100.f, 80.f);
    auto panel = makeElement<HTMLPanelElement>();
    HTMLPanelElement* mounted = panel.get();
    context.mount(std::move(panel));
    ASSERT_NE(context.unmount(*mounted), nullptr);

    EXPECT_EQ(context.width(), 100.f);
    EXPECT_EQ(context.height(), 80.f);
    EXPECT_FALSE(context.pointerDown({{10.f, 10.f}, PointerButton::Left}));
}

TEST(SurfaceTest, ClearsPointerCaptureWhenElementBecomesDisabled) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto probe = std::make_unique<CaptureProbe>();
    CaptureProbe* target = probe.get();
    probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    surface.mount(std::move(probe));

    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    target->disabled(true);
    EXPECT_FALSE(surface.hasPointerCapture());
    EXPECT_EQ(target->ends, 1);
}

TEST(SurfaceTest, AppliesPointerPolicyStylesWithoutLayout) {
    StyleSheet styleSheet;
    constexpr char kPointerPolicyStyles[] = "button { pointer-events: none; } "
                                            "panel { pointer-events: auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPointerPolicyStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    surface.mount(std::move(button));
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({40.f, 10.f, 20.f, 20.f});
    surface.mount(std::move(panel));

    EXPECT_FALSE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
}

TEST(SurfaceTest, RemeasuresAfterIntrinsicContentChanges) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; } "
                                  "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = makeElement<HTMLLabelElement>("a");
    HTMLLabelElement* text = label.get();
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    const float shortWidth = text->rect().w;
    text->textContent("a much longer label");
    surface.updateLayout();
    EXPECT_GT(text->rect().w, shortWidth);
}

TEST(SurfaceTest, RemeasuresAfterTextNodeDataChanges) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; } label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = makeElement<HTMLLabelElement>("a");
    HTMLLabelElement* labelElement = label.get();
    ASSERT_NE(label->firstChild(), nullptr);
    Text* text = label->firstChild()->asText();
    ASSERT_NE(text, nullptr);
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    const float shortWidth = labelElement->rect().w;
    text->setData("a much longer label");
    surface.updateLayout();
    EXPECT_GT(labelElement->rect().w, shortWidth);
}

TEST(SurfaceTest, InvalidatesLayoutAfterStylesheetGenerationChanges) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kExpandedLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);

    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto label = makeElement<HTMLLabelElement>("text");
    HTMLLabelElement* text = label.get();
    panel->append(std::move(label));
    surface.mount(std::move(panel));
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
    parent->append(std::move(child));
    surface.mount(std::move(parent));

    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    ASSERT_EQ(log.size(), std::size_t(3));
    EXPECT_EQ(log[0], std::string("parent:capture"));
    EXPECT_EQ(log[1], std::string("child:target"));
    EXPECT_EQ(log[2], std::string("parent:bubble"));
}

TEST(SurfaceTest, PreservesEventHandlerIdentityAndSuppressesDuplicates) {
    auto button = makeElementValue<HTMLButtonElement>();
    int calls = 0;
    EventHandler handler([&](Event& event) {
        EXPECT_EQ(event.type(), kClickEvent);
        ++calls;
    });

    button.addEventListener(kClickEvent, handler);
    button.addEventListener(kClickEvent, handler);
    button.activate();
    EXPECT_EQ(calls, 1);

    button.removeEventListener(kClickEvent, handler);
    button.activate();
    EXPECT_EQ(calls, 1);
}

TEST(SurfaceTest, EventListenerRemovalTakesEffectDuringTheCurrentDispatch) {
    auto button = makeElementValue<HTMLButtonElement>();
    int firstCalls = 0;
    int removedCalls = 0;
    EventHandler removed([&](Event&) { ++removedCalls; });
    EventHandler first([&](Event&) {
        ++firstCalls;
        button.removeEventListener(kClickEvent, removed);
    });
    button.addEventListener(kClickEvent, first);
    button.addEventListener(kClickEvent, removed);

    button.activate();

    EXPECT_EQ(firstCalls, 1);
    EXPECT_EQ(removedCalls, 0);
}

TEST(SurfaceTest, EventListenerAddedDuringDispatchWaitsForTheNextDispatch) {
    auto button = makeElementValue<HTMLButtonElement>();
    int addedCalls = 0;
    EventHandler added([&](Event&) { ++addedCalls; });
    EventHandler installer([&](Event&) { button.addEventListener(kClickEvent, added); });
    button.addEventListener(kClickEvent, installer);

    button.activate();
    EXPECT_EQ(addedCalls, 0);
    button.activate();
    EXPECT_EQ(addedCalls, 1);
}

TEST(SurfaceTest, EventRoutingUsesOneListenerSnapshotForTheWholeRoute) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto parent = makeElement<HTMLPanelElement>();
    parent->setRect({0.f, 0.f, 100.f, 100.f});
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    parent->append(std::move(button));

    int lateCalls = 0;
    EventHandler late([&](Event&) { ++lateCalls; });
    parent->addEventListener(kPointerDownEvent, [&](Event&) { target->addEventListener(kPointerDownEvent, late); }, true);
    surface.mount(std::move(parent));

    ASSERT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_EQ(lateCalls, 0);
    surface.pointerUp({{15.f, 15.f}, PointerButton::Left});
    ASSERT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_EQ(lateCalls, 1);
}

TEST(SurfaceTest, StopImmediatePropagationSkipsLaterListeners) {
    auto button = makeElementValue<HTMLButtonElement>();
    int skipped = 0;
    button.addEventListener(kClickEvent, [](Event& event) { event.stopImmediatePropagation(); });
    button.addEventListener(kClickEvent, [&](Event&) { ++skipped; });

    button.activate();

    EXPECT_EQ(skipped, 0);
}

TEST(SurfaceTest, HonorsPreventDefaultDuringPointerRouting) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    std::vector<std::string> log;
    auto probe = std::make_unique<RoutedProbe>("target", log);
    RoutedProbe* target = probe.get();
    target->preventDefault = true;
    target->setRect({10.f, 10.f, 20.f, 20.f});
    surface.mount(std::move(probe));

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

    auto parent = makeElement<HTMLPanelElement>();
    parent->setId("parent").setRect({0.f, 0.f, 100.f, 100.f});
    auto child = makeElement<HTMLPanelElement>();
    child->setId("child").setRect({10.f, 10.f, 20.f, 20.f});
    parent->append(std::move(child));
    surface.mount(std::move(parent));

    EXPECT_TRUE(surface.pointerMove({{15.f, 15.f}}));
    EXPECT_EQ(surface.cursor(), CursorStyle::Grab);

    ASSERT_TRUE(styleSheet.loadRadia(kDefaultChildCursorLayout).ok());
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);

    ASSERT_TRUE(styleSheet.loadRadia(kTextChildCursorLayout).ok());
    EXPECT_EQ(surface.cursor(), CursorStyle::Text);
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
        auto button = makeElement<HTMLButtonElement>();
        button->setRect(rect).setOnActivate([&activations](Element&) { ++activations; });
        surface.mount(std::move(button), layer);
    };
    mountButton(SurfaceLayer::Base, contentActivations, {0.f, 0.f, 100.f, 100.f});
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

    auto first = makeElement<HTMLPanelElement>();
    first->setRect({0.f, 0.f, 50.f, 50.f});
    auto firstButton = makeElement<HTMLButtonElement>();
    firstButton->setRect({0.f, 0.f, 50.f, 50.f}).setOnActivate([&firstActivations](Element&) { ++firstActivations; });
    first->append(std::move(firstButton));
    surface.mount(std::move(first), SurfaceLayer::Floater);

    auto second = makeElement<HTMLPanelElement>();
    second->setRect({25.f, 0.f, 50.f, 50.f});
    auto secondButton = makeElement<HTMLButtonElement>();
    secondButton->setRect({25.f, 0.f, 50.f, 50.f}).setOnActivate([&secondActivations](Element&) { ++secondActivations; });
    second->append(std::move(secondButton));
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
    auto parent = makeElement<HTMLPanelElement>();
    parent->setId("parent").setRect({10.f, 10.f, 20.f, 20.f});
    auto child = makeElement<HTMLPanelElement>();
    child->setId("child").setRect({40.f, 10.f, 10.f, 10.f});
    parent->append(std::move(child));
    surface.mount(std::move(parent));

    EXPECT_TRUE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, PointerButton::Left});

    constexpr char kVerticalOverflow[] = "#parent { overflow-x: visible; overflow-y: hidden; pointer-events: none; } "
                                         "#child { pointer-events: auto; }";
    ASSERT_TRUE(stylesheet.loadRadia(kVerticalOverflow).ok());
    EXPECT_FALSE(surface.pointerDown({{45.f, 15.f}, PointerButton::Left}));
    surface.pointerUp({{45.f, 15.f}, PointerButton::Left});

    RecordingPaintContext verticalRecording;
    surface.paint(verticalRecording);
    const PaintCommand* verticalClip = verticalRecording.last(PaintCommandKind::PushClip);
    ASSERT_NE(verticalClip, nullptr);
    EXPECT_TRUE(clipsAxis(verticalClip->clipAxes, ClipAxes::X));
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
    EXPECT_TRUE(clipsAxis(overflowClip->clipAxes, ClipAxes::Y));
}

TEST(SurfaceTest, PaintsNestedScrollersWithBalancedViewportClipsAndTranslations) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("#outer { display: block; overflow: auto; scrollbar-mode: overlay; scrollbar-width: none; } "
                               "#inner { display: block; overflow: auto; scrollbar-mode: overlay; scrollbar-width: none; }")
                    .ok());
    Surface surface(stylesheet);
    surface.setViewport(100.f, 100.f);

    auto outer = makeElement<HTMLPanelElement>();
    outer->setId("outer").setRect({0.f, 0.f, 100.f, 100.f});
    HTMLPanelElement* outerPtr = outer.get();
    auto inner = makeElement<HTMLPanelElement>();
    inner->setId("inner").setRect({40.f, 40.f, 120.f, 120.f});
    HTMLPanelElement* innerPtr = inner.get();
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({40.f, 40.f, 180.f, 180.f});
    HTMLPanelElement* contentPtr = content.get();
    inner->append(std::move(content));
    outer->append(std::move(inner));
    surface.mount(std::move(outer));

    surface.updateLayout();
    const Rect contentLayoutRect = contentPtr->rect();
    outerPtr->scrollTo(5.f, 7.f);
    innerPtr->scrollTo(11.f, 13.f);

    RecordingPaintContext recording;
    surface.paint(recording);

    const std::vector<PaintCommandKind> expectedKinds{
        PaintCommandKind::BeginFrame,     PaintCommandKind::PushClip,        PaintCommandKind::Box,
        PaintCommandKind::PushClip,       PaintCommandKind::PushTranslation, PaintCommandKind::Box,
        PaintCommandKind::PushClip,       PaintCommandKind::PushTranslation, PaintCommandKind::Box,
        PaintCommandKind::PopTranslation, PaintCommandKind::PopClip,         PaintCommandKind::PopTranslation,
        PaintCommandKind::PopClip,        PaintCommandKind::PopClip,         PaintCommandKind::EndFrame,
    };
    ASSERT_EQ(recording.commands().size(), expectedKinds.size());
    for (std::size_t index = 0; index < expectedKinds.size(); ++index) {
        SCOPED_TRACE(Message() << "paint command index: " << index);
        EXPECT_EQ(recording.commands()[index].kind, expectedKinds[index]);
    }

    EXPECT_EQ(recording.clipDepth(), 0);
    EXPECT_EQ(recording.maxClipDepth(), 3);
    EXPECT_EQ(recording.translationDepth(), 0);
    EXPECT_EQ(recording.maxTranslationDepth(), 2);
    EXPECT_FLOAT_EQ(recording.commands()[3].rect.x, 0.f);
    EXPECT_FLOAT_EQ(recording.commands()[3].rect.y, 0.f);
    EXPECT_FLOAT_EQ(recording.commands()[3].rect.w, 100.f);
    EXPECT_FLOAT_EQ(recording.commands()[3].rect.h, 100.f);
    EXPECT_FLOAT_EQ(recording.commands()[4].translation.x, -5.f);
    EXPECT_FLOAT_EQ(recording.commands()[4].translation.y, 7.f);
    EXPECT_FLOAT_EQ(recording.commands()[6].rect.x, 40.f);
    EXPECT_FLOAT_EQ(recording.commands()[6].rect.y, 40.f);
    EXPECT_FLOAT_EQ(recording.commands()[6].rect.w, 120.f);
    EXPECT_FLOAT_EQ(recording.commands()[6].rect.h, 120.f);
    EXPECT_FLOAT_EQ(recording.commands()[7].translation.x, -11.f);
    EXPECT_FLOAT_EQ(recording.commands()[7].translation.y, 13.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().x, contentLayoutRect.x);
    EXPECT_FLOAT_EQ(contentPtr->rect().y, contentLayoutRect.y);
    EXPECT_FLOAT_EQ(contentPtr->rect().w, contentLayoutRect.w);
    EXPECT_FLOAT_EQ(contentPtr->rect().h, contentLayoutRect.h);
}

TEST(SurfaceTest, TransfersMountedElementsBetweenSurfaces) {
    Surface first;
    Surface second;
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* transferred = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    first.mount(std::move(button), SurfaceLayer::Floater);
    first.pointerDown({{15.f, 15.f}, PointerButton::Left});

    std::unique_ptr<Element> detached = first.unmount(*transferred);
    ASSERT_NE(detached, nullptr);
    EXPECT_EQ(detached.get(), transferred);
    EXPECT_FALSE(first.hasPointerCapture());
    EXPECT_EQ(transferred->parentElement(), nullptr);
    second.mount(std::move(detached), SurfaceLayer::Floater);
    EXPECT_EQ(transferred->parentElement(), nullptr);
    EXPECT_NE(second.unmount(*transferred), nullptr);
}

TEST(SurfaceTest, KeepsMountedRootsIndependent) {
    Surface surface;
    surface.setViewport(100.f, 100.f);

    auto first = makeElement<HTMLPanelElement>();
    HTMLPanelElement* firstRoot = first.get();
    auto firstChild = makeElement<HTMLButtonElement>();
    HTMLButtonElement* firstDescendant = firstChild.get();
    first->append(std::move(firstChild));

    auto second = makeElement<HTMLPanelElement>();
    HTMLPanelElement* secondRoot = second.get();

    surface.mount(std::move(first), SurfaceLayer::Floater);
    surface.mount(std::move(second), SurfaceLayer::Floater);

    radia::ui::ElementRef<Element> firstHandle(firstRoot);
    radia::ui::ElementRef<Element> firstDescendantHandle(firstDescendant);
    radia::ui::ElementRef<Element> secondHandle(secondRoot);

    EXPECT_EQ(firstRoot->parentElement(), nullptr);
    EXPECT_EQ(secondRoot->parentElement(), nullptr);
    EXPECT_EQ(firstDescendant->parentElement(), firstRoot);

    std::unique_ptr<Element> retired = surface.unmount(*firstRoot);
    ASSERT_NE(retired, nullptr);
    EXPECT_EQ(retired.get(), firstRoot);
    EXPECT_EQ(firstRoot->parentElement(), nullptr);
    EXPECT_EQ(secondRoot->parentElement(), nullptr);
    EXPECT_EQ(firstHandle.get(), firstRoot);
    EXPECT_EQ(firstHandle.getMounted(), nullptr);
    EXPECT_EQ(firstDescendantHandle.getMounted(), nullptr);
    EXPECT_EQ(secondHandle.getMounted(), secondRoot);
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
        probe->setRect({x, 10.f, 20.f, 20.f}).setVisibility(visibility).setOnActivate([&activations](Element&) { ++activations; });
        surface.mount(std::move(probe));
        return result;
    };

    PaintProbe* visible = add(0.f, Visibility::Visible, visibleActivations);
    PaintProbe* hidden = add(30.f, Visibility::Hidden, hiddenActivations);
    PaintProbe* collapsed = add(60.f, Visibility::Collapse, collapsedActivations);
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
    EXPECT_TRUE(visible->hasState(ElementState::Focused));
    EXPECT_FALSE(hidden->hasState(ElementState::Focused));
    EXPECT_FALSE(collapsed->hasState(ElementState::Focused));
}

TEST(SurfaceTest, HonorsStylesheetDisplayAndVisibility) {
    StyleSheet styleSheet;
    constexpr char kVisibilityStyles[] = ".hidden { visibility: hidden; } .collapse { visibility: collapse; } .none { display: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibilityStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 40.f);

    auto add = [&](float x, const char* className) {
        auto probe = std::make_unique<PaintProbe>();
        PaintProbe* result = probe.get();
        probe->setRect({x, 10.f, 20.f, 20.f}).addClass(className);
        surface.mount(std::move(probe));
        return result;
    };

    PaintProbe* visible = add(0.f, "visible");
    PaintProbe* hidden = add(30.f, "hidden");
    PaintProbe* collapse = add(60.f, "collapse");
    PaintProbe* none = add(80.f, "none");

    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_EQ(visible->paints, 1);
    EXPECT_EQ(hidden->paints, 0);
    EXPECT_EQ(collapse->paints, 0);
    EXPECT_EQ(none->paints, 0);

    EXPECT_TRUE(surface.pointerDown({{10.f, 20.f}, PointerButton::Left}));
    surface.pointerUp({{10.f, 20.f}, PointerButton::Left});
    EXPECT_FALSE(surface.pointerDown({{40.f, 20.f}, PointerButton::Left}));
    EXPECT_FALSE(surface.pointerDown({{70.f, 20.f}, PointerButton::Left}));
    EXPECT_FALSE(surface.pointerDown({{90.f, 20.f}, PointerButton::Left}));

    surface.clearInteractionState();
    EXPECT_TRUE(surface.keyDown({kKeyTab}));
    EXPECT_TRUE(visible->hasState(ElementState::Focused));
    EXPECT_FALSE(hidden->hasState(ElementState::Focused));
    EXPECT_FALSE(collapse->hasState(ElementState::Focused));
    EXPECT_FALSE(none->hasState(ElementState::Focused));
}

TEST(SurfaceTest, InvalidatesAncestorLayoutAfterStateChanges) {
    StyleSheet styleSheet;
    constexpr char kStateLayout[] = "panel { display: flex; flex-direction: row; } input { width: 20px; height: 10px; } "
                                    "input[switch]:checked { width: 40px; } label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = makeElement<HTMLInputElement>();
    HTMLInputElement* target = control.get();
    control->type("checkbox").switchMode(true);
    panel->append(std::move(control));
    auto label = makeElement<HTMLLabelElement>("after");
    HTMLLabelElement* after = label.get();
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    EXPECT_EQ(after->rect().left(), 20.f);
    target->checked(true);
    surface.updateLayout();
    EXPECT_EQ(after->rect().left(), 40.f);
}

TEST(SurfaceTest, PreservesOrderedPaintingHitTestingAndFocus) {
    StyleSheet styleSheet;
    constexpr char kOrderedOverlap[] = "panel { display: flex; flex-direction: row; width: 40px; height: 20px; } "
                                       "#early { order: -1; width: 20px; height: 20px; } "
                                       "#late { order: 2; width: 20px; height: 20px; margin: 0px 0px 0px -20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOrderedOverlap).ok());
    Surface surface(styleSheet);
    surface.setViewport(40.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    std::vector<std::string> paintOrder;
    auto early = std::make_unique<OrderedPaintProbe>("early", paintOrder);
    auto late = std::make_unique<OrderedPaintProbe>("late", paintOrder);
    early->setId("early");
    late->setId("late");
    OrderedPaintProbe* earlyTarget = early.get();
    OrderedPaintProbe* lateTarget = late.get();
    panel->append(std::move(late));
    panel->append(std::move(early));
    surface.mount(std::move(panel));

    RecordingPaintContext recording;
    surface.paint(recording);
    const std::vector<std::string> expectedInitialPaintOrder{"late", "early"};
    EXPECT_EQ(paintOrder, expectedInitialPaintOrder);
    EXPECT_TRUE(surface.pointerDown({{5.f, 5.f}, PointerButton::Left}));
    EXPECT_TRUE(earlyTarget->hasState(ElementState::Active));
    surface.pointerUp({{5.f, 5.f}, PointerButton::Left});
    surface.clearInteractionState();
    EXPECT_TRUE(surface.keyDown({kKeyTab}));
    EXPECT_TRUE(lateTarget->hasState(ElementState::Focused));

    paintOrder.clear();
    earlyTarget->setVisibility(Visibility::Collapse);
    surface.paint(recording);
    const std::vector<std::string> expectedCollapsedPaintOrder{"late"};
    EXPECT_EQ(paintOrder, expectedCollapsedPaintOrder);
    paintOrder.clear();
    earlyTarget->setVisibility(Visibility::Visible);
    surface.paint(recording);
    EXPECT_EQ(paintOrder, expectedInitialPaintOrder);
}

TEST(SurfaceTest, InvalidatesCachedTraversalAfterChildMutation) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    HTMLPanelElement* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    surface.mount(std::move(panel));
    surface.updateLayout();

    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
    parent->append(std::move(button));
    EXPECT_TRUE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
    EXPECT_TRUE(target->hasState(ElementState::Focused));

    parent->replaceChildren();
    EXPECT_FALSE(surface.pointerDown({{15.f, 15.f}, PointerButton::Left}));
}
} // namespace
