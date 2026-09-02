/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>
#include "binding/valuebinding.h"
#include "html/element.h"
#include "style/pseudoelement.h"

namespace radia::ui {
class Binder;

struct InputValueState {
    bool dirty = false;
    ValueValidationStatus validationStatus = ValueValidationStatus::Valid;
    std::optional<std::string> validationMessage;
};

class HTMLInputElement : public HTMLElement {
    friend class detail::ElementDefinitions;
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;
    friend class Binder;

public:
    const std::string& type() const { return mType; }
    HTMLInputElement& type(std::string type);
    const std::string& name() const { return mName; }
    HTMLInputElement& name(std::string name);
    bool switchMode() const { return mSwitchMode; }
    HTMLInputElement& switchMode(bool enabled);
    HTMLInputElement& checked(bool checked);
    bool checked() const { return hasState(ElementState::Checked); }
    HTMLInputElement& indeterminate(bool indeterminate);
    bool indeterminate() const { return isCheckboxType() && hasState(ElementState::Indeterminate); }
    HTMLInputElement& setOnCheckedChanged(std::function<void(bool)> callback);
    PseudoElement* sliderTrack() { return isSwitchType() ? &mSliderTrack : nullptr; }
    const PseudoElement* sliderTrack() const { return isSwitchType() ? &mSliderTrack : nullptr; }
    PseudoElement* sliderFill() { return isSwitchType() ? &mSliderFill : nullptr; }
    const PseudoElement* sliderFill() const { return isSwitchType() ? &mSliderFill : nullptr; }
    PseudoElement* sliderThumb() { return isSwitchType() ? &mSliderThumb : nullptr; }
    const PseudoElement* sliderThumb() const { return isSwitchType() ? &mSliderThumb : nullptr; }
    PseudoElement* checkmark() { return isCheckableType(mType) && !isSwitchType() ? &mCheckmark : nullptr; }
    const PseudoElement* checkmark() const { return isCheckableType(mType) && !isSwitchType() ? &mCheckmark : nullptr; }

    std::optional<ValueBindingRequest> valueBindingRequest() const { return mValueBindingRequest; }
    InputValueState valueState() const;
    using ValueStateObserver = std::function<void(const InputValueState&)>;
    ValueBindingSubscription observeValueState(ValueStateObserver observer);

    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    Vec2 intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                       const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const override;
    void paint(PaintContext& context, const Style& style, float scale) const override;

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onActivate() override;
    void onTreeAttached() override;
    void onTreeDetached() override;
    std::vector<PseudoElement*> generatedPseudoElements() const override;

private:
    HTMLInputElement();

    static bool isCheckableType(std::string_view type);
    bool isCheckboxType() const;
    bool isRadioType() const;
    bool isSwitchType() const;
    void activateCheckbox();
    void activateRadio();
    void activateSwitch();
    HTMLInputElement& setSettingName(std::string name);
    void initializeChecked(bool checked);
    void activateChecked(bool checked);
    void prepareValueBinding(Binder& binder);
    ValueBindingSubscription commitValueBinding();
    bool updateCheckedState(bool checked);
    void updateRadioGroup();
    void refreshRadioGroup();
    void refreshRadioGroup(std::string_view name, const HTMLInputElement* excluded = nullptr);
    void resetIndeterminateState();
    void refreshIndeterminateState();
    bool updateIndeterminateState(bool indeterminate);
    void setCheckedFromRadioGroup(bool checked);
    void applyValueState(ValueState<bool> state);
    void notifyValueState();

    std::string mType = "text";
    std::string mName;
    bool mSwitchMode = false;
    bool mIndeterminate = false;
    mutable PseudoElement mSliderTrack;
    mutable PseudoElement mSliderFill;
    mutable PseudoElement mSliderThumb;
    mutable PseudoElement mCheckmark;
    std::function<void(bool)> mOnCheckedChanged;
    std::optional<ValueBindingRequest> mValueBindingRequest;
    ValueBindingRef<bool> mBinding;
    ValueState<bool> mValueState{false, false, std::nullopt};
    std::map<std::size_t, ValueStateObserver> mValueObservers;
    std::size_t mNextValueObserver = 1;
    std::shared_ptr<char> mValueObserverLifetime = std::make_shared<char>(0);
};
} // namespace radia::ui
