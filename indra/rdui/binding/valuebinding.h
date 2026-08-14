/**
 * @file valuebinding.h
 * @brief Defines typed value state, validation, observation, and controller binding interfaces.
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

#ifndef LL_RDUI_VALUE_BINDING_H
#define LL_RDUI_VALUE_BINDING_H

#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <typeinfo>
#include <utility>
#include "localization/localization.h"
#include "text/source.h"

namespace rdui {
enum class ValueValidationStatus { Valid, Invalid, Pending };

struct ValueValidation {
    ValueValidationStatus status = ValueValidationStatus::Valid;
    std::optional<TextSource> message;

    static ValueValidation valid() { return {}; }
    static ValueValidation invalid(std::optional<TextSource> message = std::nullopt) { return {ValueValidationStatus::Invalid, std::move(message)}; }
    static ValueValidation pending(std::optional<TextSource> message = std::nullopt) { return {ValueValidationStatus::Pending, std::move(message)}; }
};

template<typename T> struct ValueState {
    T value{};
    T baseline{};
    std::optional<ValueValidation> validation;

    bool dirty() const { return value != baseline; }
    ValueValidationStatus validationStatus() const { return validation ? validation->status : ValueValidationStatus::Valid; }
    const TextSource* validationMessage() const { return validation && validation->message ? &*validation->message : nullptr; }
};

class ValueBindingSubscription {
public:
    ValueBindingSubscription() = default;
    explicit ValueBindingSubscription(std::function<void()> disconnect) : mDisconnect(std::move(disconnect)) {}
    ~ValueBindingSubscription() { reset(); }

    ValueBindingSubscription(const ValueBindingSubscription&) = delete;
    ValueBindingSubscription& operator=(const ValueBindingSubscription&) = delete;

    ValueBindingSubscription(ValueBindingSubscription&& other) noexcept : mDisconnect(std::exchange(other.mDisconnect, {})) {}
    ValueBindingSubscription& operator=(ValueBindingSubscription&& other) noexcept {
        if (this != &other) {
            reset();
            mDisconnect = std::exchange(other.mDisconnect, {});
        }
        return *this;
    }

    explicit operator bool() const { return static_cast<bool>(mDisconnect); }
    void reset() {
        if (!mDisconnect) return;
        auto disconnect = std::move(mDisconnect);
        disconnect();
    }

private:
    std::function<void()> mDisconnect;
};

class ValueBindingBase {
public:
    virtual ~ValueBindingBase() = default;
};

struct ValueBindingRequest {
    std::string settingName;
};

template<typename T> class ValueBinding : public ValueBindingBase {
public:
    using ValueType = T;
    using Observer = std::function<void(const ValueState<T>&)>;

    virtual ValueState<T> state() const = 0;
    virtual void write(T value) = 0;
    virtual ValueBindingSubscription observe(Observer observer) = 0;
};

template<typename T> class ValueBindingRef {
    friend class Binder;

public:
    ValueBinding<T>* get() const { return mBinding.get(); }
    ValueBinding<T>* operator->() const { return get(); }
    ValueBinding<T>& operator*() const { return *get(); }
    explicit operator bool() const { return static_cast<bool>(mBinding); }
    std::shared_ptr<ValueBinding<T>> shared() const { return mBinding; }
    void reset() { mBinding.reset(); }

private:
    void set(std::shared_ptr<ValueBinding<T>> binding) { mBinding = std::move(binding); }
    std::shared_ptr<ValueBinding<T>> mBinding;
};

namespace detail {
template<typename T> const char* valueTypeName() {
    return typeid(T).name();
}

template<> inline const char* valueTypeName<bool>() {
    return "boolean";
}
template<> inline const char* valueTypeName<int>() {
    return "integer";
}
template<> inline const char* valueTypeName<float>() {
    return "number";
}
template<> inline const char* valueTypeName<double>() {
    return "number";
}
template<> inline const char* valueTypeName<std::string>() {
    return "string";
}
} // namespace detail
} // namespace rdui
#endif // LL_RDUI_VALUE_BINDING_H
