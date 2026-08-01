/**
 * @file valuebinding_test.cpp
 * @brief
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
#include <map>
#include <memory>
#include <vector>
#include "../test/lltut.h"
#include "binding/valuebinding.h"

namespace tut {
template<typename T> class MemoryValueBinding final : public rdui::ValueBinding<T>, public std::enable_shared_from_this<MemoryValueBinding<T>> {
public:
    explicit MemoryValueBinding(rdui::ValueState<T> state) : mState(std::move(state)) {}

    rdui::ValueState<T> state() const override { return mState; }

    void write(T value) override {
        mState.value = std::move(value);
        publish(mState);
    }

    rdui::ValueBindingSubscription observe(typename rdui::ValueBinding<T>::Observer observer) override {
        const std::size_t id = ++mNextObserver;
        mObservers.emplace(id, std::move(observer));
        std::weak_ptr<MemoryValueBinding<T>> weak = this->shared_from_this();
        return rdui::ValueBindingSubscription([weak, id] {
            if (auto binding = weak.lock()) binding->mObservers.erase(id);
        });
    }

    void publish(rdui::ValueState<T> state) {
        mState = std::move(state);
        std::vector<typename rdui::ValueBinding<T>::Observer> observers;
        for (const auto& [id, observer] : mObservers) observers.push_back(observer);
        for (const auto& observer : observers) observer(mState);
    }

private:
    rdui::ValueState<T> mState;
    std::map<std::size_t, typename rdui::ValueBinding<T>::Observer> mObservers;
    std::size_t mNextObserver = 0;
};

struct rduivaluebinding_data {};
using rduivaluebinding_test = test_group<rduivaluebinding_data>;
using rduivaluebinding_object = rduivaluebinding_test::object;
rduivaluebinding_test rduivaluebinding_testcase("rduivaluebinding");

template<> template<> void rduivaluebinding_object::test<1>() {
    rdui::ValueState<int> clean{4, 4, std::nullopt};
    ensure("equal value and baseline are clean", !clean.dirty());
    ensure("omitted validation is valid", clean.validationStatus() == rdui::ValueValidationStatus::Valid);
    ensure("omitted validation has no message", clean.validationMessage() == nullptr);

    rdui::ValueState<int> invalid{5, 4, rdui::ValueValidation::invalid(rdui::TextSource::text("Not allowed"))};
    ensure("changed value is dirty", invalid.dirty());
    ensure("invalid status is retained", invalid.validationStatus() == rdui::ValueValidationStatus::Invalid);
    ensure_equals("dynamic validation message is retained", invalid.validationMessage()->materialize().plainText(), std::string("Not allowed"));

    invalid.validation = rdui::ValueValidation::pending();
    ensure("pending validation is distinct", invalid.validationStatus() == rdui::ValueValidationStatus::Pending);
}

template<> template<> void rduivaluebinding_object::test<2>() {
    auto binding = std::make_shared<MemoryValueBinding<bool>>(rdui::ValueState<bool>{false, false, std::nullopt});
    int publications = 0;
    bool observed = false;
    rdui::ValueBindingSubscription subscription = binding->observe([&](const rdui::ValueState<bool>& state) {
        ++publications;
        observed = state.value;
    });

    binding->write(true);
    ensure("write reaches the adapter", binding->state().value);
    ensure("write derives dirty state from the baseline", binding->state().dirty());
    ensure("write publication reaches observers", publications == 1 && observed);

    binding->publish({true, true, rdui::ValueValidation::valid()});
    ensure("a new baseline clears dirty state", !binding->state().dirty());
    ensure_equals("asynchronous state updates publish", publications, 2);

    subscription.reset();
    binding->write(false);
    ensure_equals("reset subscription stops publications", publications, 2);
}
} // namespace tut
