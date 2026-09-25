/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "settingsadapter.h"
#include <memory>
#include "llcontrol.h"

namespace radia::viewer::ui {
using radia::ui::SettingResolution;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;

namespace {
template<typename T> struct SavedSettingTraits;

template<> struct SavedSettingTraits<bool> {
    static bool read(const LLSD& value) { return value.asBoolean(); }
};

template<> struct SavedSettingTraits<int> {
    static int read(const LLSD& value) { return value.asInteger(); }
};

template<> struct SavedSettingTraits<float> {
    static float read(const LLSD& value) { return static_cast<float>(value.asReal()); }
};

template<> struct SavedSettingTraits<double> {
    static double read(const LLSD& value) { return value.asReal(); }
};

template<> struct SavedSettingTraits<std::string> {
    static std::string read(const LLSD& value) { return value.asString(); }
};

LLControlVariablePtr requireControl(LLControlVariablePtr control) {
    llassert(control);
    return control;
}

template<typename T> class ViewerSettingBinding final : public ValueBinding<T> {
public:
    explicit ViewerSettingBinding(LLControlVariablePtr control)
        : mControl(requireControl(std::move(control))), mBaseline(SavedSettingTraits<T>::read(mControl->get())) {}

    ValueState<T> state() const override {
        const T value = SavedSettingTraits<T>::read(mControl->get());
        return {value, mBaseline, std::nullopt};
    }

    void write(T value) override { mControl->set(LLSD(std::move(value))); }

    ValueBindingSubscription observe(typename ValueBinding<T>::Observer observer) override {
        const T baseline = mBaseline;
        auto connection = std::make_shared<boost::signals2::connection>(
            mControl->getSignal()->connect([observer = std::move(observer), baseline](LLControlVariable*, const LLSD& value, const LLSD&) {
                const T current = SavedSettingTraits<T>::read(value);
                observer(ValueState<T>{current, baseline, std::nullopt});
            }));
        return ValueBindingSubscription([connection = std::move(connection)] { connection->disconnect(); });
    }

private:
    LLControlVariablePtr mControl;
    const T mBaseline;
};

std::type_index controlTypeIndex(eControlType type) {
    switch (type) {
        case TYPE_BOOLEAN: return typeid(bool);
        case TYPE_S32: return typeid(int);
        case TYPE_F32: return typeid(float);
        case TYPE_STRING: return typeid(std::string);
        default: return typeid(void);
    }
}
} // namespace

SettingResolution SettingsAdapter::resolve(std::string_view settingName, std::type_index requestedType) {
    if (settingName.empty()) return {SettingResolution::ResolutionStatus::Invalid, {}};

    LLControlVariablePtr control = mSettings.getControl(settingName);
    if (!control) return {SettingResolution::ResolutionStatus::Missing, {}};

    const std::type_index availableType = controlTypeIndex(control->type());
    const bool compatibleFloatingPoint = control->type() == TYPE_F32 && (requestedType == typeid(float) || requestedType == typeid(double));
    if (availableType == typeid(void) || (!compatibleFloatingPoint && requestedType != availableType))
        return {SettingResolution::ResolutionStatus::TypeMismatch, {}};

    if (requestedType == typeid(bool))
        return {SettingResolution::ResolutionStatus::Found, std::make_shared<ViewerSettingBinding<bool>>(std::move(control))};
    if (requestedType == typeid(int))
        return {SettingResolution::ResolutionStatus::Found, std::make_shared<ViewerSettingBinding<int>>(std::move(control))};
    if (requestedType == typeid(float))
        return {SettingResolution::ResolutionStatus::Found, std::make_shared<ViewerSettingBinding<float>>(std::move(control))};
    if (requestedType == typeid(double))
        return {SettingResolution::ResolutionStatus::Found, std::make_shared<ViewerSettingBinding<double>>(std::move(control))};
    if (requestedType == typeid(std::string))
        return {SettingResolution::ResolutionStatus::Found, std::make_shared<ViewerSettingBinding<std::string>>(std::move(control))};
    return {SettingResolution::ResolutionStatus::TypeMismatch, {}};
}
} // namespace radia::viewer::ui
