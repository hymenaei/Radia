/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/input.h"
#include "binding/binder.h"
#include "elements/elementdefinition.h"
#include "layout/schema.h"
#include "nativeappearance.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "surface/surface.h"

namespace radia::ui {
bool InputElement::isCheckableType(std::string_view type) {
    const std::string key = schemaNameKey(type);
    return key == "checkbox" || key == "radio";
}

bool InputElement::isCheckboxType() const {
    return schemaNameKey(mType) == "checkbox";
}

bool InputElement::isRadioType() const {
    return schemaNameKey(mType) == "radio";
}

bool InputElement::isSwitchType() const {
    return mSwitchMode && isCheckboxType();
}

InputElement::InputElement() : Element("input") {}

Vec2 InputElement::intrinsicSize(const StyleSheet&, const Style& style, const TextMetrics&, const IntrinsicSizeConstraints& constraints) const {
    if (style.appearance == AppearanceMode::Unstyled || !isCheckableType(mType)) return {};
    const NativeInputControl control = isRadioType() ? NativeInputControl::Radio
        : isSwitchType()                             ? NativeInputControl::Switch
                                                     : NativeInputControl::Checkbox;
    const Surface* owner = surface();
    const NativeAppearance& appearance = constraints.nativeAppearance ? *constraints.nativeAppearance
        : owner                                                       ? owner->nativeAppearance()
                                                                      : defaultNativeAppearance();
    return appearance.inputMetrics(control).intrinsicSize;
}

void InputElement::paint(PaintContext& context, const Style& style, float scale) const {
    if (style.appearance == AppearanceMode::Unstyled || !isCheckableType(mType)) {
        Element::paint(context, style, scale);
        return;
    }

    NativeInputPaintRequest request;
    request.control = isRadioType() ? NativeInputControl::Radio : isSwitchType() ? NativeInputControl::Switch : NativeInputControl::Checkbox;
    request.bounds = rect();
    request.checked = checked();
    request.indeterminate = indeterminate();
    request.disabled = disabled();
    request.hovered = hasState(ElementState::Hovered);
    request.pressed = hasState(ElementState::Active);
    if (style.accentColor.kind == AccentColor::Kind::CurrentColor) request.accentColor = style.color;
    else if (style.accentColor.kind == AccentColor::Kind::Color) request.accentColor = style.accentColor.color;
    request.colorScheme = style.colorScheme;
    request.direction = style.direction;
    request.scale = scale;
    context.paintNativeInput(request);
}

InputElement& InputElement::type(std::string type) {
    if (type.empty()) type = "text";
    const bool wasRadio = isRadioType();
    const bool wasSwitch = isSwitchType();
    const std::string oldName = mName;
    if (wasRadio) refreshRadioGroup(oldName, this);
    mType = std::move(type);
    detail::ElementCompilerAccess::setStyleAttribute(*this, "type", mType);
    if (!isCheckboxType()) resetIndeterminateState();

    const bool isSwitch = isSwitchType();
    if (wasSwitch && !isSwitch) replaceChildren();
    else if (!wasSwitch && isSwitch) detail::instantiateCompositeParts(*this, detail::ElementDefinitionFactory::input());
    if (isRadioType()) refreshRadioGroup();
    else refreshIndeterminateState();
    return *this;
}

InputElement& InputElement::name(std::string name) {
    if (mName == name) return *this;
    const std::string oldName = mName;
    if (isRadioType()) refreshRadioGroup(oldName, this);
    mName = std::move(name);
    detail::ElementCompilerAccess::setStyleAttribute(*this, "name", mName);
    if (isRadioType()) refreshRadioGroup();
    return *this;
}

InputElement& InputElement::switchMode(bool enabled) {
    const bool wasSwitch = isSwitchType();
    if (mSwitchMode == enabled) return *this;

    mSwitchMode = enabled;
    if (enabled) detail::ElementCompilerAccess::setStyleAttribute(*this, "switch", "true");
    else detail::ElementCompilerAccess::removeStyleAttribute(*this, "switch");

    const bool isSwitch = isSwitchType();
    if (wasSwitch && !isSwitch) replaceChildren();
    else if (!wasSwitch && isSwitch) detail::instantiateCompositeParts(*this, detail::ElementDefinitionFactory::input());
    return *this;
}

InputElement& InputElement::checked(bool checked) {
    if (!isCheckableType(mType)) return *this;
    const bool changed = updateCheckedState(checked);
    mValueState.value = checked;
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    if (changed) notifyValueState();
    return *this;
}

void InputElement::initializeChecked(bool checked) {
    if (!isCheckableType(mType)) return;
    mValueState = {checked, checked, std::nullopt};
    updateCheckedState(checked);
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

bool InputElement::updateCheckedState(bool checked) {
    const bool changed = checked != this->checked();
    setState(ElementState::Checked, checked);
    if (changed) invalidateArrange();
    return changed;
}

InputElement& InputElement::setSettingName(std::string name) {
    mValueBindingRequest = ValueBindingRequest{std::move(name)};
    return *this;
}

InputValueState InputElement::valueState() const {
    InputValueState result;
    result.dirty = mValueState.dirty();
    result.validationStatus = mValueState.validationStatus();
    if (const std::string* message = mValueState.validationMessage()) result.validationMessage = *message;
    return result;
}

ValueBindingSubscription InputElement::observeValueState(ValueStateObserver observer) {
    const std::size_t id = mNextValueObserver++;
    mValueObservers.emplace(id, std::move(observer));
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    return ValueBindingSubscription([this, lifetime, id] {
        if (!lifetime.expired()) mValueObservers.erase(id);
    });
}

void InputElement::notifyValueState() {
    const InputValueState state = valueState();
    const auto observers = mValueObservers;
    for (const auto& [id, observer] : observers)
        if (mValueObservers.find(id) != mValueObservers.end()) observer(state);
}

void InputElement::applyValueState(ValueState<bool> state) {
    mValueState = std::move(state);
    updateCheckedState(mValueState.value);
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    notifyValueState();
}

void InputElement::prepareValueBinding(Binder& binder) {
    if (mValueBindingRequest) binder.requireValueBinding(*mValueBindingRequest, mBinding);
}

ValueBindingSubscription InputElement::commitValueBinding() {
    if (!mBinding) return {};
    applyValueState(mBinding->state());
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    std::shared_ptr<ValueBinding<bool>> provider = mBinding.shared();
    auto providerSubscription = std::make_shared<ValueBindingSubscription>(provider->observe([this, lifetime](const ValueState<bool>& state) {
        if (!lifetime.expired()) applyValueState(state);
    }));
    return ValueBindingSubscription([this, lifetime, provider = std::move(provider), providerSubscription] {
        providerSubscription->reset();
        if (!lifetime.expired() && mBinding.shared() == provider) mBinding.reset();
    });
}

InputElement& InputElement::setOnCheckedChanged(std::function<void(bool)> callback) {
    mOnCheckedChanged = std::move(callback);
    return *this;
}

void InputElement::activateChecked(bool checked) {
    const bool previous = this->checked();
    if (mBinding) {
        mBinding->write(checked);
    } else {
        mValueState.value = checked;
        updateCheckedState(checked);
        if (isRadioType()) updateRadioGroup();
        else refreshIndeterminateState();
        notifyValueState();
    }
    if (this->checked() == previous) return;
    if (mOnCheckedChanged) mOnCheckedChanged(this->checked());
    Event input(kInputEvent, *this, this->checked());
    dispatchEvent(input);
    Event change(kChangeEvent, *this, this->checked());
    dispatchEvent(change);
}

void InputElement::onTreeAttached() {
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

void InputElement::onTreeDetached() {
    if (isRadioType()) refreshRadioGroup(mName, this);
}

void InputElement::onActivate() {
    const std::string key = schemaNameKey(mType);
    if (key == "checkbox") {
        indeterminate(false);
        if (isSwitchType()) activateSwitch();
        else activateCheckbox();
    } else if (key == "radio") activateRadio();
}
} // namespace radia::ui
