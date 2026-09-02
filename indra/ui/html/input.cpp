/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/input.h"
#include <algorithm>
#include "binding/binder.h"
#include "dom/elementinternal.h"
#include "html/elementnames.h"
#include "nativeappearance.h"
#include "render/paintcontext.h"
#include "resource/elementdefinition.h"
#include "style/style.h"
#include "surface/surface.h"

namespace radia::ui {
using detail::ElementCompilerAccess;
using detail::ElementDefinitions;

bool HTMLInputElement::isCheckableType(std::string_view type) {
    const std::string key = canonicalizeHTMLName(type);
    return key == "checkbox" || key == "radio";
}

bool HTMLInputElement::isCheckboxType() const {
    return canonicalizeHTMLName(mType) == "checkbox";
}

bool HTMLInputElement::isRadioType() const {
    return canonicalizeHTMLName(mType) == "radio";
}

bool HTMLInputElement::isSwitchType() const {
    return mSwitchMode && isCheckboxType();
}

HTMLInputElement::HTMLInputElement()
    : HTMLElement(kInputTag.localName), mSliderTrack(PseudoElementType::SliderTrack, *this),
      mSliderFill(PseudoElementType::SliderFill, *this, &mSliderTrack), mSliderThumb(PseudoElementType::SliderThumb, *this),
      mCheckmark(PseudoElementType::Checkmark, *this) {
    mSliderTrack.addGeneratedPseudoElement(mSliderFill);
    setAttribute("type", mType);
}

void HTMLInputElement::constrainResolvedStyle(Style& style) const {
    if (style.appearance != AppearanceMode::Base || !isCheckableType(mType) || isSwitchType() || style.borderWidthSet) return;
    style.borderWidth = {1.f, 1.f, 1.f, 1.f};
    if (!style.borderColorSet) {
        style.borderColor = {};
        style.borderColorLightDark.reset();
        style.borderGradient.reset();
        style.borderColorCurrent = true;
    }
}

Vec2 HTMLInputElement::intrinsicSize(const StyleSheet&, const Style& style, const TextMetrics&, const IntrinsicSizeConstraints& constraints) const {
    if (!isCheckableType(mType)) return {};
    if (style.appearance == AppearanceMode::Base && !isSwitchType()) {
        const float size = std::max(24.f, style.fontSize);
        return {std::max(0.f, size - style.padding.horizontal() - style.borderWidth.horizontal()),
                std::max(0.f, size - style.padding.vertical() - style.borderWidth.vertical())};
    }
    if (style.appearance != AppearanceMode::Auto) return {};
    const NativeInputControl control = isRadioType() ? NativeInputControl::Radio
        : isSwitchType()                             ? NativeInputControl::Switch
                                                     : NativeInputControl::Checkbox;
    const Surface* owner = surface();
    const NativeAppearance& appearance = constraints.nativeAppearance ? *constraints.nativeAppearance
        : owner                                                       ? owner->nativeAppearance()
                                                                      : defaultNativeAppearance();
    return appearance.inputMetrics(control).intrinsicSize;
}

void HTMLInputElement::paint(PaintContext& context, const Style& style, float scale) const {
    if (style.appearance != AppearanceMode::Auto || !isCheckableType(mType)) {
        Element::paint(context, style, scale);
        if (style.appearance == AppearanceMode::Base) {
            const bool clipsX = style.overflowX != Overflow::Visible;
            const bool clipsY = style.overflowY != Overflow::Visible;
            const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
            if (clipsX || clipsY) context.pushClip(detail::ElementInternalAccess::scrollport(*this), scale, clipAxes);
            const auto paintPseudoElement = [&context, scale](const PseudoElement& pseudoElement, float inheritedOpacity,
                                                              const auto& paintChildren) -> void {
                const Style& pseudoStyle = pseudoElement.style();
                if (pseudoStyle.display == DisplayMode::NoneValue || pseudoElement.rect().empty()) return;
                Style paintedStyle = pseudoStyle;
                applyOpacity(paintedStyle, inheritedOpacity);
                const bool clipsX = paintedStyle.overflowX != Overflow::Visible;
                const bool clipsY = paintedStyle.overflowY != Overflow::Visible;
                const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
                if (clipsX || clipsY) context.pushClip(pseudoElement.rect(), scale, clipAxes);
                if (paintedStyle.visibility == Visibility::Visible) {
                    context.paintBox(pseudoElement.rect(), paintedStyle);
                    if (paintedStyle.content && !paintedStyle.content->empty()) {
                        const EdgeInsets contentInsets{
                            paintedStyle.padding.top + paintedStyle.borderWidth.top,
                            paintedStyle.padding.right + paintedStyle.borderWidth.right,
                            paintedStyle.padding.bottom + paintedStyle.borderWidth.bottom,
                            paintedStyle.padding.left + paintedStyle.borderWidth.left,
                        };
                        context.paintText(*paintedStyle.content, insetRect(pseudoElement.rect(), contentInsets), paintedStyle);
                    }
                }
                for (const PseudoElement* child : pseudoElement.generatedPseudoElements())
                    if (child) paintChildren(*child, paintedStyle.opacity, paintChildren);
                if (clipsX || clipsY) context.popClip();
            };
            if (isSwitchType()) {
                if (const PseudoElement* sliderTrack = this->sliderTrack(); sliderTrack)
                    paintPseudoElement(*sliderTrack, style.opacity, paintPseudoElement);
                if (const PseudoElement* sliderThumb = this->sliderThumb(); sliderThumb)
                    paintPseudoElement(*sliderThumb, style.opacity, paintPseudoElement);
            } else if (const PseudoElement* checkmark = this->checkmark()) {
                paintPseudoElement(*checkmark, style.opacity, paintPseudoElement);
            }
            if (clipsX || clipsY) context.popClip();
        }
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

HTMLInputElement& HTMLInputElement::type(std::string type) {
    if (type.empty()) type = "text";
    const bool wasRadio = isRadioType();
    const std::string oldName = mName;
    if (wasRadio) refreshRadioGroup(oldName, this);
    mType = std::move(type);
    setAttribute("type", mType);
    ElementCompilerAccess::setStyleAttribute(*this, "type", mType);
    if (!isCheckboxType()) resetIndeterminateState();

    if (isRadioType()) refreshRadioGroup();
    else refreshIndeterminateState();
    return *this;
}

HTMLInputElement& HTMLInputElement::name(std::string name) {
    if (mName == name) return *this;
    const std::string oldName = mName;
    if (isRadioType()) refreshRadioGroup(oldName, this);
    mName = std::move(name);
    if (mName.empty()) removeAttribute("name");
    else setAttribute("name", mName);
    ElementCompilerAccess::setStyleAttribute(*this, "name", mName);
    if (isRadioType()) refreshRadioGroup();
    return *this;
}

HTMLInputElement& HTMLInputElement::switchMode(bool enabled) {
    if (mSwitchMode == enabled) return *this;

    mSwitchMode = enabled;
    if (enabled) setAttribute("switch");
    else removeAttribute("switch");
    if (enabled) ElementCompilerAccess::setStyleAttribute(*this, "switch", "true");
    else ElementCompilerAccess::removeStyleAttribute(*this, "switch");

    return *this;
}

HTMLInputElement& HTMLInputElement::checked(bool checked) {
    if (!isCheckableType(mType)) return *this;
    const bool changed = updateCheckedState(checked);
    mValueState.value = checked;
    if (checked) setAttribute("checked");
    else removeAttribute("checked");
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    if (changed) notifyValueState();
    return *this;
}

void HTMLInputElement::initializeChecked(bool checked) {
    if (!isCheckableType(mType)) return;
    mValueState = {checked, checked, std::nullopt};
    updateCheckedState(checked);
    if (checked) setAttribute("checked");
    else removeAttribute("checked");
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

bool HTMLInputElement::updateCheckedState(bool checked) {
    const bool changed = checked != this->checked();
    setState(ElementState::Checked, checked);
    if (changed) invalidateArrange();
    return changed;
}

HTMLInputElement& HTMLInputElement::setSettingName(std::string name) {
    mValueBindingRequest = ValueBindingRequest{std::move(name)};
    return *this;
}

InputValueState HTMLInputElement::valueState() const {
    InputValueState result;
    result.dirty = mValueState.dirty();
    result.validationStatus = mValueState.validationStatus();
    if (const std::string* message = mValueState.validationMessage()) result.validationMessage = *message;
    return result;
}

ValueBindingSubscription HTMLInputElement::observeValueState(ValueStateObserver observer) {
    const std::size_t id = mNextValueObserver++;
    mValueObservers.emplace(id, std::move(observer));
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    return ValueBindingSubscription([this, lifetime, id] {
        if (!lifetime.expired()) mValueObservers.erase(id);
    });
}

void HTMLInputElement::notifyValueState() {
    const InputValueState state = valueState();
    const auto observers = mValueObservers;
    const std::weak_ptr<char> lifetime = mValueObserverLifetime;
    for (const auto& [id, observer] : observers) {
        if (lifetime.expired()) return;
        if (mValueObservers.find(id) != mValueObservers.end()) observer(state);
    }
}

void HTMLInputElement::applyValueState(ValueState<bool> state) {
    mValueState = std::move(state);
    updateCheckedState(mValueState.value);
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    notifyValueState();
}

void HTMLInputElement::prepareValueBinding(Binder& binder) {
    if (mValueBindingRequest) binder.requireValueBinding(*mValueBindingRequest, mBinding);
}

ValueBindingSubscription HTMLInputElement::commitValueBinding() {
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

HTMLInputElement& HTMLInputElement::setOnCheckedChanged(std::function<void(bool)> callback) {
    mOnCheckedChanged = std::move(callback);
    return *this;
}

void HTMLInputElement::activateChecked(bool checked) {
    ElementRef<HTMLInputElement> self(this);
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
    HTMLInputElement* current = self.get();
    if (!current || current->checked() == previous) return;
    if (current->mOnCheckedChanged) current->mOnCheckedChanged(current->checked());
    current = self.get();
    if (!current) return;
    Event input(kInputEvent, *current, current->checked());
    current->dispatchEvent(input);
    current = self.get();
    if (!current) return;
    Event change(kChangeEvent, *current, current->checked());
    current->dispatchEvent(change);
}

void HTMLInputElement::onTreeAttached() {
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

void HTMLInputElement::onTreeDetached() {
    if (isRadioType()) refreshRadioGroup(mName, this);
}

void HTMLInputElement::onActivate() {
    const std::string key = canonicalizeHTMLName(mType);
    if (key == "checkbox") {
        indeterminate(false);
        if (isSwitchType()) activateSwitch();
        else activateCheckbox();
    } else if (key == "radio") activateRadio();
}

std::vector<PseudoElement*> HTMLInputElement::generatedPseudoElements() const {
    if (isSwitchType()) return {&mSliderTrack, &mSliderThumb};
    if (isCheckableType(mType)) return {&mCheckmark};
    return {};
}
} // namespace radia::ui
