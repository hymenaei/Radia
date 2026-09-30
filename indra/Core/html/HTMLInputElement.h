/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>
#include "HTMLElement.h"
#include "PseudoElement.h"
#include "ValueBinding.h"

namespace Core {
class Binding;
class Binder;

struct InputValueState {
    bool dirty = false;
    ValueValidationStatus validationStatus = ValueValidationStatus::Valid;
    std::optional<std::string> validationMessage;
};

class HTMLInputElement : public HTMLElement {
    friend class dom_detail::FragmentParser;
    friend class detail::ElementDefinitions;
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;
    friend class Binding;
    friend class Binder;

public:
    const std::string& type() const { return mType; }
    HTMLInputElement& type(std::string type);
    const std::string& name() const { return mName; }
    HTMLInputElement& name(std::string name);
    bool switchMode() const { return mSwitchMode; }
    HTMLInputElement& switchMode(bool enabled);
    HTMLInputElement& checked(bool checked);
    bool checked() const { return isCheckableType(mType) && mValueState.value; }
    HTMLInputElement& indeterminate(bool indeterminate);
    bool indeterminate() const { return isCheckboxType() && mCheckboxIndeterminate; }
    bool radioGroupIsIndeterminate() const { return isRadioType() && mRadioGroupIndeterminate; }
    bool invalid() const { return mValueState.validationStatus() == ValueValidationStatus::Invalid; }
    HTMLInputElement& setOnCheckedChanged(std::function<void(bool)> callback);
    Style::PseudoElement* sliderTrack() { return isSwitchType() ? &mSliderTrack : nullptr; }
    const Style::PseudoElement* sliderTrack() const { return isSwitchType() ? &mSliderTrack : nullptr; }
    Style::PseudoElement* sliderFill() { return isSwitchType() ? &mSliderFill : nullptr; }
    const Style::PseudoElement* sliderFill() const { return isSwitchType() ? &mSliderFill : nullptr; }
    Style::PseudoElement* sliderThumb() { return isSwitchType() ? &mSliderThumb : nullptr; }
    const Style::PseudoElement* sliderThumb() const { return isSwitchType() ? &mSliderThumb : nullptr; }
    Style::PseudoElement* checkmark() { return isCheckableType(mType) && !isSwitchType() ? &mCheckmark : nullptr; }
    const Style::PseudoElement* checkmark() const { return isCheckableType(mType) && !isSwitchType() ? &mCheckmark : nullptr; }

    std::optional<ValueBindingRequest> valueBindingRequest() const { return mValueBindingRequest; }
    InputValueState valueState() const;
    using ValueStateObserver = std::function<void(const InputValueState&)>;
    ValueBindingSubscription observeValueState(ValueStateObserver observer);

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    AccessibleSemantics accessibleSemantics() const override;
    Layout::Vec2 intrinsicSize(const CSS::StyleSheet& styleSheet, const Style::ComputedStyle& style, const TextMeasurer& textMetrics,
        const Layout::IntrinsicSizeConstraints& constraints = Layout::IntrinsicSizeConstraints()) const override;
    void paint(PaintContext& context, const Style::ComputedStyle& style, float scale) const override;

protected:
    void constrainResolvedStyle(Style::ComputedStyle& style) const override;
    void onAttributeSet(std::string_view name, const std::optional<std::string>& value) override;
    void onAttributeRemoved(std::string_view name) override;
    void onActivate() override;
    void onTreeWillBeDetached() override;
    void onTreeAttached() override;
    std::vector<Style::PseudoElement*> generatedPseudoElements() const override;

private:
    HTMLInputElement();

    static bool isCheckableType(std::string_view type);
    bool isCheckboxType() const;
    bool isRadioType() const;
    bool isSwitchType() const;
    void activateCheckbox();
    void activateRadio();
    void activateSwitch();
    void clearValueBinding();
    HTMLInputElement& setSettingName(std::string name);
    void initializeChecked(bool checked);
    void activateChecked(bool checked);
    void prepareValueBinding(Binder& binder);
    ValueBindingSubscription commitValueBinding(const std::shared_ptr<bool>& bindingActive, Element& root);
    bool updateCheckedState(bool checked);
    void updateRadioGroup();
    void refreshRadioGroup();
    void refreshRadioGroup(std::string_view name, const HTMLInputElement* excluded = nullptr);
    void refreshIndeterminateState();
    bool updateIndeterminateState(bool indeterminate);
    void setCheckedFromRadioGroup(bool checked);
    void applyValueState(ValueState<bool> state);
    void synchronizeValueBinding();
    void notifyValueState();

    std::string mType = "text";
    std::string mName;
    bool mSwitchMode = false;
    bool mCheckboxIndeterminate = false;
    bool mRadioGroupIndeterminate = false;
    mutable Style::PseudoElement mSliderTrack;
    mutable Style::PseudoElement mSliderFill;
    mutable Style::PseudoElement mSliderThumb;
    mutable Style::PseudoElement mCheckmark;
    std::function<void(bool)> mOnCheckedChanged;
    std::optional<ValueBindingRequest> mValueBindingRequest;
    bool mUpdatingAttribute = false;
    ValueBindingRef<bool> mBinding;
    std::weak_ptr<ValueBindingSubscription> mBindingSubscription;
    ValueState<bool> mValueState {false, false, std::nullopt};
    std::map<std::size_t, ValueStateObserver> mValueObservers;
    std::size_t mNextValueObserver = 1;
    std::shared_ptr<char> mValueObserverLifetime = std::make_shared<char>(0);
};
} // namespace Core
