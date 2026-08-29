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
#include "binding/valuebinding.h"
#include "elements/element.h"

namespace radia::ui {
class Binder;

struct InputValueState {
    bool dirty = false;
    ValueValidationStatus validationStatus = ValueValidationStatus::Valid;
    std::optional<std::string> validationMessage;
};

class InputElement : public Element {
    friend class detail::ElementDefinitionFactory;
    friend class Binder;

public:
    InputElement();

    const std::string& type() const { return mType; }
    InputElement& type(std::string type);
    const std::string& name() const { return mName; }
    InputElement& name(std::string name);
    bool switchMode() const { return mSwitchMode; }
    InputElement& switchMode(bool enabled);
    InputElement& checked(bool checked);
    bool checked() const { return hasState(ElementState::Checked); }
    InputElement& indeterminate(bool indeterminate);
    bool indeterminate() const { return isCheckboxType() && hasState(ElementState::Indeterminate); }
    InputElement& setOnCheckedChanged(std::function<void(bool)> callback);
    Element* track() { return mTrack; }
    const Element* track() const { return mTrack; }
    Element* thumb() { return mThumb; }
    const Element* thumb() const { return mThumb; }

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
    void onActivate() override;
    bool shouldPaintChild(const Element& child, const Style& style) const override;
    void onTreeAttached() override;
    void onTreeDetached() override;
    void onChildrenCleared() override;

private:
    static bool isCheckableType(std::string_view type);
    bool isCheckboxType() const;
    bool isRadioType() const;
    bool isSwitchType() const;
    void activateCheckbox();
    void activateRadio();
    void activateSwitch();
    InputElement& setSettingName(std::string name);
    void initializeChecked(bool checked);
    void activateChecked(bool checked);
    void prepareValueBinding(Binder& binder);
    ValueBindingSubscription commitValueBinding();
    bool updateCheckedState(bool checked);
    void updateRadioGroup();
    void refreshRadioGroup();
    void refreshRadioGroup(std::string_view name, const InputElement* excluded = nullptr);
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
    Element* mTrack = nullptr;
    Element* mThumb = nullptr;
    std::function<void(bool)> mOnCheckedChanged;
    std::optional<ValueBindingRequest> mValueBindingRequest;
    ValueBindingRef<bool> mBinding;
    ValueState<bool> mValueState{false, false, std::nullopt};
    std::map<std::size_t, ValueStateObserver> mValueObservers;
    std::size_t mNextValueObserver = 1;
    std::shared_ptr<char> mValueObserverLifetime = std::make_shared<char>(0);
};
} // namespace radia::ui
