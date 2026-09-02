/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstddef>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include "binding/valuebinding.h"
#include "dom/elementinternal.h"
#include "event.h"
#include "html/input.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "nativeappearance.h"
#include "render/recordingpaintcontext.h"
#include "style/style.h"
#include "style/stylesheet.h"
#include "surface/surface.h"
#include "text/metrics.h"

namespace {
using radia::ui::AccentColor;
using radia::ui::AppearanceMode;
using radia::ui::Color;
using radia::ui::ColorScheme;
using radia::ui::ElementState;
using radia::ui::Event;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLPanelElement;
using radia::ui::kChangeEvent;
using radia::ui::kInputEvent;
using radia::ui::LayoutDirection;
using radia::ui::NativeAppearanceBase;
using radia::ui::NativeInputControl;
using radia::ui::NativeInputMark;
using radia::ui::NativeInputMetrics;
using radia::ui::NativeInputPaintRequest;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::RecordingPaintContext;
using radia::ui::resolveElementStyle;
using radia::ui::ScrollLayoutOptions;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;
using radia::ui::ValueValidation;
using radia::ui::ValueValidationStatus;
using radia::ui::Vec2;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;

template<typename T> class MemoryValueBinding final : public ValueBinding<T>, public std::enable_shared_from_this<MemoryValueBinding<T>> {
public:
    explicit MemoryValueBinding(ValueState<T> state) : mState(std::move(state)) {}

    ValueState<T> state() const override { return mState; }

    void write(T value) override {
        mState.value = std::move(value);
        publish(mState);
    }

    ValueBindingSubscription observe(typename ValueBinding<T>::Observer observer) override {
        const std::size_t id = ++mNextObserver;
        mObservers.emplace(id, std::move(observer));
        std::weak_ptr<MemoryValueBinding<T>> weak = this->shared_from_this();
        return ValueBindingSubscription([weak, id] {
            if (auto binding = weak.lock()) binding->mObservers.erase(id);
        });
    }

    void publish(ValueState<T> state) {
        mState = std::move(state);
        std::vector<typename ValueBinding<T>::Observer> observers;
        for (const auto& [id, observer] : mObservers) observers.push_back(observer);
        for (const auto& observer : observers) observer(mState);
    }

private:
    ValueState<T> mState;
    std::map<std::size_t, typename ValueBinding<T>::Observer> mObservers;
    std::size_t mNextObserver = 0;
};

} // namespace

TEST(ValueStateTest, ReportsDirtyStateAndValidation) {
    ValueState<int> clean{4, 4, std::nullopt};
    EXPECT_FALSE(clean.dirty()) << "equal value and baseline are clean";
    EXPECT_EQ(clean.validationStatus(), ValueValidationStatus::Valid) << "omitted validation is valid";
    EXPECT_EQ(clean.validationMessage(), nullptr) << "omitted validation has no message";

    ValueState<int> invalid{5, 4, ValueValidation::invalid("Not allowed")};
    EXPECT_TRUE(invalid.dirty()) << "changed value is dirty";
    EXPECT_EQ(invalid.validationStatus(), ValueValidationStatus::Invalid) << "invalid status is retained";
    ASSERT_NE(invalid.validationMessage(), nullptr) << "invalid validation has a message";
    ASSERT_TRUE(invalid.validation.has_value());
    ASSERT_TRUE(invalid.validation->message.has_value());
    EXPECT_EQ(invalid.validationMessage(), &*invalid.validation->message) << "dynamic validation message is retained";

    invalid.validation = ValueValidation::pending();
    EXPECT_EQ(invalid.validationStatus(), ValueValidationStatus::Pending) << "pending validation is distinct";
}

TEST(MemoryValueBindingTest, PublishesStateChangesUntilSubscriptionReset) {
    auto binding = std::make_shared<MemoryValueBinding<bool>>(ValueState<bool>{false, false, std::nullopt});
    int publications = 0;
    bool observed = false;
    ValueBindingSubscription subscription = binding->observe([&](const ValueState<bool>& state) {
        ++publications;
        observed = state.value;
    });

    binding->write(true);
    EXPECT_TRUE(binding->state().value) << "write reaches the adapter";
    EXPECT_TRUE(binding->state().dirty()) << "write derives dirty state from the baseline";
    EXPECT_EQ(publications, 1) << "write publication reaches observers";
    EXPECT_TRUE(observed) << "write publication carries the new value";

    binding->publish({true, true, ValueValidation::valid()});
    EXPECT_FALSE(binding->state().dirty()) << "a new baseline clears dirty state";
    EXPECT_EQ(publications, 2) << "asynchronous state updates publish";

    subscription.reset();
    binding->write(false);
    EXPECT_FALSE(binding->state().value) << "write still updates state after unsubscribe";
    EXPECT_EQ(publications, 2) << "reset subscription stops publications";
}

TEST(InputTest, KeepsOneElementIdentityAcrossInputTypes) {
    auto input = makeElementValue<HTMLInputElement>();

    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.type(), "text");

    input.type("checkbox");
    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.sliderThumb(), nullptr);
    ASSERT_NE(input.checkmark(), nullptr);
    EXPECT_EQ(input.checkmark()->name(), "checkmark");
    EXPECT_EQ(&input.checkmark()->originatingElement(), &input);

    input.type("checkbox").switchMode(true);
    EXPECT_EQ(input.elementName(), "input");
    ASSERT_NE(input.sliderTrack(), nullptr);
    ASSERT_NE(input.sliderFill(), nullptr);
    ASSERT_NE(input.sliderThumb(), nullptr);
    EXPECT_EQ(input.checkmark(), nullptr);
    EXPECT_EQ(input.sliderTrack()->name(), "slider-track");
    EXPECT_EQ(input.sliderFill()->name(), "slider-fill");
    EXPECT_EQ(input.sliderThumb()->name(), "slider-thumb");
    EXPECT_EQ(&input.sliderTrack()->originatingElement(), &input);
    EXPECT_EQ(input.sliderTrack()->parentPseudoElement(), nullptr);
    EXPECT_EQ(input.sliderFill()->parentPseudoElement(), input.sliderTrack());
    EXPECT_EQ(input.sliderThumb()->parentPseudoElement(), nullptr);

    input.type("radio");
    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.sliderTrack(), nullptr);
    EXPECT_EQ(input.sliderThumb(), nullptr);
    ASSERT_NE(input.checkmark(), nullptr);
    EXPECT_EQ(input.checkmark()->name(), "checkmark");
}

TEST(InputTest, AppearanceSelectsNativeCommandOrOrdinaryPaint) {
    auto input = makeElementValue<HTMLInputElement>();
    RecordingPaintContext recording;
    Style style;

    input.type("checkbox").setRect({0.f, 0.f, 13.f, 13.f});
    input.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::NativeInput), std::size_t{1});
    ASSERT_NE(recording.last(PaintCommandKind::NativeInput), nullptr);
    ASSERT_TRUE(recording.last(PaintCommandKind::NativeInput)->nativeInput.has_value());
    EXPECT_EQ(recording.last(PaintCommandKind::NativeInput)->nativeInput->control, NativeInputControl::Checkbox);

    recording.clear();
    style.appearance = AppearanceMode::Unstyled;
    style.backgroundColor = {0.2f, 0.3f, 0.4f, 1.f};
    input.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{1});
    ASSERT_NE(recording.last(PaintCommandKind::Box), nullptr);
    EXPECT_FLOAT_EQ(recording.last(PaintCommandKind::Box)->style.backgroundColor.r, 0.2f);

    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true).setRect({0.f, 0.f, 36.f, 20.f});
    recording.clear();
    control.paint(recording, Style{}, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::NativeInput), std::size_t{1});
    ASSERT_NE(recording.last(PaintCommandKind::NativeInput), nullptr);
    ASSERT_TRUE(recording.last(PaintCommandKind::NativeInput)->nativeInput.has_value());
    EXPECT_EQ(recording.last(PaintCommandKind::NativeInput)->nativeInput->control, NativeInputControl::Switch);
}

TEST(InputTest, BaseAppearancePaintsCheckmarkContent) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("input[type=checkbox] { appearance: base; width: 20px; height: 20px; } "
                               "input[type=checkbox]::checkmark { content: \"\\2713\" / \"\"; width: 10px; height: 10px; visibility: visible; }")
                    .ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").checked(true).setRect({0.f, 0.f, 20.f, 20.f});
    layoutTree(input, stylesheet, fixedTextMetrics());

    RecordingPaintContext recording;
    input.paint(recording, resolveElementStyle(stylesheet, input), 1.f);

    const PaintCommand* text = recording.last(PaintCommandKind::Text);
    ASSERT_NE(text, nullptr);
    EXPECT_EQ(text->textOrIconName, "\xE2\x9C\x93");
}

TEST(InputTest, NativeRequestCarriesResolvedAccentColor) {
    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").setRect({0.f, 0.f, 13.f, 13.f});
    RecordingPaintContext recording;
    Style style;
    style.accentColor = AccentColor::fromColor({.2f, .4f, .6f, .8f});

    input.paint(recording, style, 1.f);

    const PaintCommand* command = recording.last(PaintCommandKind::NativeInput);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->nativeInput.has_value());
    ASSERT_TRUE(command->nativeInput->accentColor.has_value());
    EXPECT_NEAR(command->nativeInput->accentColor->r, .2f, 1.0e-6f);
    EXPECT_NEAR(command->nativeInput->accentColor->g, .4f, 1.0e-6f);
    EXPECT_NEAR(command->nativeInput->accentColor->b, .6f, 1.0e-6f);
    EXPECT_NEAR(command->nativeInput->accentColor->a, .8f, 1.0e-6f);
}

TEST(InputTest, NativeRequestCarriesResolvedColorScheme) {
    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").setRect({0.f, 0.f, 13.f, 13.f});
    RecordingPaintContext recording;
    Style style;
    style.colorScheme = ColorScheme::Light;

    input.paint(recording, style, 1.f);

    const PaintCommand* command = recording.last(PaintCommandKind::NativeInput);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->nativeInput.has_value());
    EXPECT_EQ(command->nativeInput->colorScheme, ColorScheme::Light);
}

TEST(InputTest, NativeAppearanceBasePaintsCenteredCheckboxAndVectorMark) {
    NativeAppearanceBase appearance;
    NativeInputPaintRequest request;
    request.control = NativeInputControl::Checkbox;
    request.bounds = {0.f, 0.f, 20.f, 12.f};
    request.checked = true;
    request.accentColor = Color{.2f, .4f, .6f, 1.f};
    RecordingPaintContext recording;

    appearance.paintInput(recording, request);

    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* checkedBox = recording.last(PaintCommandKind::Box);
    ASSERT_NE(checkedBox, nullptr);
    EXPECT_FLOAT_EQ(checkedBox->rect.x, 4.f);
    EXPECT_FLOAT_EQ(checkedBox->rect.y, 0.f);
    EXPECT_FLOAT_EQ(checkedBox->rect.w, 12.f);
    EXPECT_FLOAT_EQ(checkedBox->rect.h, 12.f);
    const PaintCommand* mark = recording.last(PaintCommandKind::NativeInputMark);
    ASSERT_NE(mark, nullptr);
    ASSERT_TRUE(mark->nativeInputMark.has_value());
    EXPECT_EQ(mark->nativeInputMark->mark, NativeInputMark::Check);
    EXPECT_FLOAT_EQ(mark->nativeInputMark->strokeWidth, 12.f * .16f);
    ASSERT_EQ(mark->nativeInputMark->path.commands().size(), std::size_t{3});
    const auto& path = mark->nativeInputMark->path.commands();
    EXPECT_EQ(path[0].verb, radia::ui::PathVerb::MoveTo);
    EXPECT_EQ(path[1].verb, radia::ui::PathVerb::LineTo);
    EXPECT_EQ(path[2].verb, radia::ui::PathVerb::LineTo);
    EXPECT_FLOAT_EQ(path[0].p0.x, 6.4f);
    EXPECT_FLOAT_EQ(path[0].p0.y, 6.f);
    EXPECT_FLOAT_EQ(path[1].p0.x, 8.8f);
    EXPECT_FLOAT_EQ(path[1].p0.y, 3.6f);
    EXPECT_FLOAT_EQ(path[2].p0.x, 13.6f);
    EXPECT_FLOAT_EQ(path[2].p0.y, 9.6f);

    request.checked = false;
    request.indeterminate = true;
    recording.clear();
    appearance.paintInput(recording, request);

    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    mark = recording.last(PaintCommandKind::NativeInputMark);
    ASSERT_NE(mark, nullptr);
    ASSERT_TRUE(mark->nativeInputMark.has_value());
    EXPECT_EQ(mark->nativeInputMark->mark, NativeInputMark::Dash);
    EXPECT_TRUE(mark->nativeInputMark->path.empty());
}

TEST(InputTest, NativeAppearancePaintsRadioDotAsRoundCircle) {
    NativeAppearanceBase appearance;
    NativeInputPaintRequest request;
    request.control = NativeInputControl::Radio;
    request.bounds = {0.f, 0.f, 13.f, 13.f};
    request.checked = true;
    RecordingPaintContext recording;

    appearance.paintInput(recording, request);

    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{3});
    const PaintCommand* dot = recording.last(PaintCommandKind::Box);
    ASSERT_NE(dot, nullptr);
    EXPECT_FLOAT_EQ(dot->rect.w, 7.8f);
    EXPECT_FLOAT_EQ(dot->rect.h, 7.8f);
    EXPECT_FLOAT_EQ(dot->style.borderRadius.topLeft.horizontal.pixels, 3.9f);
    EXPECT_FLOAT_EQ(dot->style.borderRadius.topLeft.vertical.pixels, 3.9f);
}

TEST(InputTest, NativeAppearanceUsesDistinctLightAndDarkInputPalettes) {
    NativeAppearanceBase appearance;
    NativeInputPaintRequest request;
    request.control = NativeInputControl::Checkbox;
    request.bounds = {0.f, 0.f, 13.f, 13.f};
    RecordingPaintContext recording;

    request.colorScheme = ColorScheme::Light;
    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.commands().size(), std::size_t{2});
    const Color lightBackground = recording.commands().front().style.backgroundColor;
    const Color lightBorder = recording.commands().back().style.borderColor;

    request.colorScheme = ColorScheme::Dark;
    recording.clear();
    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.commands().size(), std::size_t{2});
    const Color darkBackground = recording.commands().front().style.backgroundColor;
    const Color darkBorder = recording.commands().back().style.borderColor;

    EXPECT_GT(lightBackground.r, darkBackground.r);
    EXPECT_GT(lightBackground.g, darkBackground.g);
    EXPECT_GT(lightBackground.b, darkBackground.b);
    EXPECT_NE(lightBorder.r, darkBorder.r);
    EXPECT_NE(lightBorder.g, darkBorder.g);
    EXPECT_NE(lightBorder.b, darkBorder.b);
}

TEST(InputTest, NativeAppearanceBasePaintsSwitchInResolvedDirection) {
    NativeAppearanceBase appearance;
    NativeInputPaintRequest request;
    request.control = NativeInputControl::Switch;
    request.bounds = {10.f, 0.f, 36.f, 20.f};
    RecordingPaintContext recording;

    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* uncheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(uncheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(uncheckedThumb->rect.x, 12.f);
    EXPECT_FLOAT_EQ(uncheckedThumb->rect.y, 2.f);

    request.checked = true;
    recording.clear();
    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* checkedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(checkedThumb, nullptr);
    EXPECT_FLOAT_EQ(checkedThumb->rect.x, 28.f);
    EXPECT_FLOAT_EQ(checkedThumb->rect.y, 2.f);

    request.direction = LayoutDirection::RightToLeft;
    request.checked = false;
    recording.clear();
    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* rtlUncheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(rtlUncheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(rtlUncheckedThumb->rect.x, 28.f);
    EXPECT_FLOAT_EQ(rtlUncheckedThumb->rect.y, 2.f);

    request.checked = true;
    recording.clear();
    appearance.paintInput(recording, request);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* rtlCheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(rtlCheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(rtlCheckedThumb->rect.x, 12.f);
    EXPECT_FLOAT_EQ(rtlCheckedThumb->rect.y, 2.f);
}

class SizedNativeAppearance final : public NativeAppearanceBase {
public:
    NativeInputMetrics inputMetrics(NativeInputControl) const override { return {{21.f, 22.f}}; }
};

TEST(InputTest, IntrinsicSizeUsesSurfaceNativeAppearanceMetrics) {
    StyleSheet styleSheet;
    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio");
    SizedNativeAppearance appearance;
    Surface surface(styleSheet);
    surface.setScrollLayoutOptions({radia::ui::ScrollbarMode::Classic, &appearance});
    surface.mount(input);

    const Vec2 size = input.intrinsicSize(styleSheet, Style{}, fixedTextMetrics());
    EXPECT_FLOAT_EQ(size.x, 21.f);
    EXPECT_FLOAT_EQ(size.y, 22.f);
}

TEST(InputTest, DetachedLayoutUsesRequestedNativeAppearanceMetrics) {
    StyleSheet styleSheet;
    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio");
    SizedNativeAppearance appearance;
    ScrollLayoutOptions options;
    options.nativeAppearance = &appearance;

    radia::ui::layoutTree(input, styleSheet, fixedTextMetrics(), LayoutDirection::LeftToRight, options);

    EXPECT_FLOAT_EQ(input.desiredSize().x, 21.f);
    EXPECT_FLOAT_EQ(input.desiredSize().y, 22.f);
}

TEST(InputTest, UsesTypeSpecificCheckableActivation) {
    auto checkbox = makeElementValue<HTMLInputElement>();
    checkbox.type("checkbox");
    checkbox.activate();
    EXPECT_TRUE(checkbox.checked());
    checkbox.activate();
    EXPECT_FALSE(checkbox.checked());

    auto radio = makeElementValue<HTMLInputElement>();
    radio.type("radio");
    radio.activate();
    EXPECT_TRUE(radio.checked());
    radio.activate();
    EXPECT_TRUE(radio.checked());
}

TEST(InputTest, ClearsCheckboxIndeterminateStateOnActivation) {
    auto checkbox = makeElementValue<HTMLInputElement>();
    checkbox.type("checkbox").indeterminate(true);

    EXPECT_TRUE(checkbox.indeterminate());
    EXPECT_TRUE(checkbox.hasState(ElementState::Indeterminate));

    checkbox.activate();

    EXPECT_TRUE(checkbox.checked());
    EXPECT_FALSE(checkbox.indeterminate());
    EXPECT_FALSE(checkbox.hasState(ElementState::Indeterminate));
}

TEST(InputTest, DispatchesInputBeforeChangeForUserActivation) {
    auto checkbox = makeElementValue<HTMLInputElement>();
    checkbox.type("checkbox");
    std::vector<std::string> events;
    std::vector<bool> values;
    checkbox.addEventListener(kInputEvent, [&](Event& event) {
        events.emplace_back(event.type());
        values.push_back(event.checked());
    });
    checkbox.addEventListener(kChangeEvent, [&](Event& event) {
        events.emplace_back(event.type());
        values.push_back(event.checked());
    });

    checkbox.activate();

    const std::vector<std::string> expectedEvents{"input", "change"};
    const std::vector<bool> expectedValues{true, true};
    ASSERT_EQ(events, expectedEvents);
    ASSERT_EQ(values, expectedValues);
    checkbox.checked(false);
    EXPECT_EQ(events.size(), std::size_t{2}) << "programmatic checked changes do not synthesize DOM events";
}

TEST(InputTest, GroupsRadioInputsByNameWithinTheirTree) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto first = makeElement<HTMLInputElement>();
    auto second = makeElement<HTMLInputElement>();
    HTMLInputElement* firstPtr = first.get();
    HTMLInputElement* secondPtr = second.get();
    first->type("radio").name("choice");
    second->type("radio").name("choice");
    root.append(std::move(first));
    root.append(std::move(second));

    firstPtr->checked(true);
    EXPECT_TRUE(firstPtr->checked());
    EXPECT_FALSE(secondPtr->checked());
    EXPECT_FALSE(firstPtr->hasState(ElementState::Indeterminate));
    EXPECT_FALSE(secondPtr->hasState(ElementState::Indeterminate));

    secondPtr->checked(true);
    EXPECT_FALSE(firstPtr->checked());
    EXPECT_TRUE(secondPtr->checked());

    secondPtr->checked(false);
    EXPECT_TRUE(firstPtr->hasState(ElementState::Indeterminate));
    EXPECT_TRUE(secondPtr->hasState(ElementState::Indeterminate));

    secondPtr->checked(true);
    secondPtr->name("other");
    EXPECT_TRUE(firstPtr->hasState(ElementState::Indeterminate));
}

TEST(InputTest, DoesNotGroupRadioInputsWithDifferentOrEmptyNames) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto named = makeElement<HTMLInputElement>();
    auto different = makeElement<HTMLInputElement>();
    auto unnamed = makeElement<HTMLInputElement>();
    HTMLInputElement* namedPtr = named.get();
    HTMLInputElement* differentPtr = different.get();
    HTMLInputElement* unnamedPtr = unnamed.get();
    named->type("radio").name("one");
    different->type("radio").name("two");
    unnamed->type("radio");
    root.append(std::move(named));
    root.append(std::move(different));
    root.append(std::move(unnamed));

    namedPtr->checked(true);
    differentPtr->checked(true);
    unnamedPtr->checked(true);

    EXPECT_TRUE(namedPtr->checked());
    EXPECT_TRUE(differentPtr->checked());
    EXPECT_TRUE(unnamedPtr->checked());
}

TEST(SwitchTest, ActivationTogglesAndNotifiesCheckedChange) {
    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    std::vector<bool> changes;
    control.setOnCheckedChanged([&changes](bool checked) { changes.push_back(checked); });

    control.activate();
    ASSERT_EQ(changes.size(), std::size_t{1}) << "first activation notifies once";
    EXPECT_TRUE(control.checked()) << "first activation checks the switch";
    EXPECT_TRUE(changes.front()) << "first notification carries the checked value";

    control.activate();
    ASSERT_EQ(changes.size(), std::size_t{2}) << "second activation notifies once";
    EXPECT_FALSE(control.checked()) << "second activation unchecks the switch";
    EXPECT_FALSE(changes.back()) << "second notification carries the unchecked value";
}

TEST(SwitchTest, ExposesGeneratedSliderPseudoElementsWithoutDomChildren) {
    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);

    ASSERT_TRUE(control.children().empty());
    ASSERT_TRUE(control.childNodes().empty());
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderFill(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    auto* sliderTrack = control.sliderTrack();
    auto* sliderFill = control.sliderFill();
    auto* sliderThumb = control.sliderThumb();
    control.replaceChildren();
    EXPECT_TRUE(control.children().empty());
    EXPECT_EQ(control.sliderTrack(), sliderTrack);
    EXPECT_EQ(control.sliderFill(), sliderFill);
    EXPECT_EQ(control.sliderThumb(), sliderThumb);
}

TEST(SwitchTest, PublishesInputValueStateUntilSubscriptionReset) {
    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    std::size_t publications = 0;
    bool observed = false;
    bool observedDirty = false;
    ValueValidationStatus observedValidation = ValueValidationStatus::Pending;

    ValueBindingSubscription subscription = control.observeValueState([&](const auto& state) {
        ++publications;
        observed = true;
        observedDirty = state.dirty;
        observedValidation = state.validationStatus;
    });

    control.activate();
    ASSERT_TRUE(observed) << "activation publishes input value state";
    EXPECT_EQ(publications, std::size_t{1}) << "first activation publishes once";
    EXPECT_TRUE(observedDirty) << "activation changes the value from its baseline";
    EXPECT_EQ(observedValidation, ValueValidationStatus::Valid) << "unvalidated state remains valid";

    subscription.reset();
    control.activate();
    EXPECT_EQ(publications, std::size_t{1}) << "reset subscription stops value state publications";
}

TEST(SwitchTest, StopsValueStateNotificationWhenObserverDestroysInput) {
    auto control = makeElement<HTMLInputElement>();
    control->type("checkbox").switchMode(true);
    std::size_t laterObserverCalls = 0;

    ValueBindingSubscription destroyingObserver = control->observeValueState([&control](const auto&) { control.reset(); });
    ValueBindingSubscription laterObserver = control->observeValueState([&laterObserverCalls](const auto&) { ++laterObserverCalls; });

    control->activate();

    EXPECT_EQ(control, nullptr);
    EXPECT_EQ(laterObserverCalls, std::size_t{0});
}
