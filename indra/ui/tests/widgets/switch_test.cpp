/**
 * @file switch_test.cpp
 * @brief Tests Switch state, interaction, and typed value binding behavior.
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
#include <cstddef>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include "binding/valuebinding.h"
#include "widgets/switch.h"

namespace {
using radia::ui::Switch;
using radia::ui::TextSource;
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

    ValueState<int> invalid{5, 4, ValueValidation::invalid(TextSource::text("Not allowed"))};
    EXPECT_TRUE(invalid.dirty()) << "changed value is dirty";
    EXPECT_EQ(invalid.validationStatus(), ValueValidationStatus::Invalid) << "invalid status is retained";
    ASSERT_NE(invalid.validationMessage(), nullptr) << "invalid validation has a message";
    EXPECT_EQ(invalid.validationMessage()->materialize().plainText(), "Not allowed") << "dynamic validation message is retained";

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

TEST(SwitchTest, StartsUncheckedAndTracksCheckedValue) {
    Switch control;

    EXPECT_FALSE(control.checked()) << "switch starts unchecked";
    const auto initialValue = control.checkedValue();
    ASSERT_TRUE(initialValue.has_value()) << "switch exposes an initial value";
    EXPECT_FALSE(*initialValue) << "initial value is unchecked";

    control.setChecked(true);
    EXPECT_TRUE(control.checked()) << "setChecked updates the widget state";
    const auto checkedValue = control.checkedValue();
    ASSERT_TRUE(checkedValue.has_value()) << "checked switch exposes a value";
    EXPECT_TRUE(*checkedValue) << "checked value is true";

    EXPECT_TRUE(control.setCheckedValue(false)) << "boolean value is accepted";
    EXPECT_FALSE(control.checked()) << "setCheckedValue updates the widget state";
}

TEST(SwitchTest, ActivationTogglesAndNotifiesCheckedChange) {
    Switch control;
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

TEST(SwitchTest, PublishesValueControlStateUntilSubscriptionReset) {
    Switch control;
    std::size_t publications = 0;
    bool observed = false;
    bool observedDirty = false;
    ValueValidationStatus observedValidation = ValueValidationStatus::Pending;

    ValueBindingSubscription subscription = control.observeValueControlState([&](const auto& state) {
        ++publications;
        observed = true;
        observedDirty = state.dirty;
        observedValidation = state.validationStatus;
    });

    control.activate();
    ASSERT_TRUE(observed) << "activation publishes value control state";
    EXPECT_EQ(publications, std::size_t{1}) << "first activation publishes once";
    EXPECT_TRUE(observedDirty) << "activation changes the value from its baseline";
    EXPECT_EQ(observedValidation, ValueValidationStatus::Valid) << "unvalidated state remains valid";

    subscription.reset();
    control.activate();
    EXPECT_EQ(publications, std::size_t{1}) << "reset subscription stops value state publications";
}
