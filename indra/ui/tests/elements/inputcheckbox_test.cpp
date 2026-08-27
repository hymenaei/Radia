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
#include "elements/input.h"
#include "elements/panel.h"
#include "event.h"
#include "render/recordingpaintcontext.h"
#include "style/style.h"

namespace {
using radia::ui::AppearanceMode;
using radia::ui::ElementState;
using radia::ui::Event;
using radia::ui::InputElement;
using radia::ui::kChangeEvent;
using radia::ui::kInputEvent;
using radia::ui::LayoutDirection;
using radia::ui::PaintCommandKind;
using radia::ui::PaintCommand;
using radia::ui::PanelElement;
using radia::ui::RecordingPaintContext;
using radia::ui::Style;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;
using radia::ui::ValueValidation;
using radia::ui::ValueValidationStatus;

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
    InputElement input;

    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.type(), "text");

    input.type("checkbox");
    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.thumb(), nullptr);

    input.type("checkbox").switchMode(true);
    EXPECT_EQ(input.elementName(), "input");
    ASSERT_NE(input.track(), nullptr);
    ASSERT_NE(input.thumb(), nullptr);
    EXPECT_EQ(input.track()->part(), "track");
    EXPECT_EQ(input.track()->parentElement(), &input);
    EXPECT_EQ(input.thumb()->part(), "thumb");
    EXPECT_EQ(input.thumb()->parentElement(), &input);

    input.type("radio");
    EXPECT_EQ(input.elementName(), "input");
    EXPECT_EQ(input.track(), nullptr);
    EXPECT_EQ(input.thumb(), nullptr);
}

TEST(InputTest, AppearanceSelectsBuiltInOrOrdinaryPaint) {
    InputElement input;
    RecordingPaintContext recording;
    Style style;

    input.type("checkbox").setRect({0.f, 0.f, 13.f, 13.f});
    input.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), std::size_t{1});

    recording.clear();
    style.appearance = AppearanceMode::Unstyled;
    style.backgroundColor = {0.2f, 0.3f, 0.4f, 1.f};
    input.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{1});
    ASSERT_NE(recording.last(PaintCommandKind::Box), nullptr);
    EXPECT_FLOAT_EQ(recording.last(PaintCommandKind::Box)->style.backgroundColor.r, 0.2f);

    InputElement control;
    control.type("checkbox").switchMode(true).setRect({0.f, 0.f, 36.f, 20.f});
    recording.clear();
    control.paint(recording, Style{}, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
}

TEST(InputTest, BuiltInSwitchPaintUsesResolvedDirection) {
    InputElement control;
    control.type("checkbox").switchMode(true).setRect({10.f, 0.f, 36.f, 20.f});
    RecordingPaintContext recording;
    Style style;

    control.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* uncheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(uncheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(uncheckedThumb->rect.x, 12.f);
    EXPECT_NE(uncheckedThumb->style.backgroundColor.r, recording.commands().front().style.backgroundColor.r);

    control.checked(true);
    recording.clear();
    control.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* checkedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(checkedThumb, nullptr);
    EXPECT_FLOAT_EQ(checkedThumb->rect.x, 28.f);

    style.direction = LayoutDirection::RightToLeft;
    control.checked(false);
    recording.clear();
    control.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* rtlUncheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(rtlUncheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(rtlUncheckedThumb->rect.x, 28.f);

    control.checked(true);
    recording.clear();
    control.paint(recording, style, 1.f);
    ASSERT_EQ(recording.count(PaintCommandKind::Box), std::size_t{2});
    const PaintCommand* rtlCheckedThumb = recording.last(PaintCommandKind::Box);
    ASSERT_NE(rtlCheckedThumb, nullptr);
    EXPECT_FLOAT_EQ(rtlCheckedThumb->rect.x, 12.f);
}

TEST(InputTest, UsesTypeSpecificCheckableActivation) {
    InputElement checkbox;
    checkbox.type("checkbox");
    checkbox.activate();
    EXPECT_TRUE(checkbox.checked());
    checkbox.activate();
    EXPECT_FALSE(checkbox.checked());

    InputElement radio;
    radio.type("radio");
    radio.activate();
    EXPECT_TRUE(radio.checked());
    radio.activate();
    EXPECT_TRUE(radio.checked());
}

TEST(InputTest, ClearsCheckboxIndeterminateStateOnActivation) {
    InputElement checkbox;
    checkbox.type("checkbox").indeterminate(true);

    EXPECT_TRUE(checkbox.indeterminate());
    EXPECT_TRUE(checkbox.hasState(ElementState::Indeterminate));

    checkbox.activate();

    EXPECT_TRUE(checkbox.checked());
    EXPECT_FALSE(checkbox.indeterminate());
    EXPECT_FALSE(checkbox.hasState(ElementState::Indeterminate));
}

TEST(InputTest, DispatchesInputBeforeChangeForUserActivation) {
    InputElement checkbox;
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
    PanelElement root;
    auto first = std::make_unique<InputElement>();
    auto second = std::make_unique<InputElement>();
    InputElement* firstPtr = first.get();
    InputElement* secondPtr = second.get();
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
    PanelElement root;
    auto named = std::make_unique<InputElement>();
    auto different = std::make_unique<InputElement>();
    auto unnamed = std::make_unique<InputElement>();
    InputElement* namedPtr = named.get();
    InputElement* differentPtr = different.get();
    InputElement* unnamedPtr = unnamed.get();
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

TEST(SwitchTest, ExposesCheckedProperty) {
    InputElement control;
    control.type("checkbox").switchMode(true);

    EXPECT_FALSE(control.checked()) << "switch starts unchecked";

    control.checked(true);
    EXPECT_TRUE(control.checked()) << "checked setter updates the element state";
    control.checked(false);
    EXPECT_FALSE(control.checked()) << "checked setter clears the element state";
}

TEST(SwitchTest, ActivationTogglesAndNotifiesCheckedChange) {
    InputElement control;
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

TEST(SwitchTest, PublishesInputValueStateUntilSubscriptionReset) {
    InputElement control;
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
