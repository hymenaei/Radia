/**
 * @file switch.h
 * @brief Defines the two-state Switch Value Control.
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

#ifndef RD_WIDGETS_SWITCH_H
#define RD_WIDGETS_SWITCH_H

#include <map>
#include "binding/valuecontrol.h"

namespace radia::ui {
struct WidgetContract;

namespace detail { WidgetContract switchContract(); }

class Switch : public ValueControl {
    friend WidgetContract detail::switchContract();

public:
    static constexpr const char* sElement = "switch";

    Switch();
    Switch& setChecked(bool checked);
    bool setCheckedValue(bool checked) override;
    std::optional<bool> checkedValue() const override;
    Switch& setOnCheckedChanged(std::function<void(bool)> callback);
    bool checked() const { return hasState(WidgetState::Checked); }
    Widget* thumb() { return mThumb.get(); }
    const Widget* thumb() const { return mThumb.get(); }
    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }

    std::optional<ValueBindingRequest> valueBindingRequest() const override { return mValueBindingRequest; }
    ValueControlState valueControlState() const override;
    ValueBindingSubscription observeValueControlState(Observer observer) override;

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onActivate() override;
    void onChildrenCleared() override;

private:
    Switch& setSettingName(std::string name);
    void prepareValueBinding(Binder& binder) override;
    ValueBindingSubscription commitValueBinding() override;
    bool updateCheckedState(bool checked);
    void applyValueState(ValueState<bool> state);
    void notifyValueControlState();

    WidgetRef<Widget> mThumb;
    std::function<void(bool)> mOnCheckedChanged;
    std::optional<ValueBindingRequest> mValueBindingRequest;
    ValueBindingRef<bool> mBinding;
    ValueState<bool> mValueState{false, false, std::nullopt};
    std::map<std::size_t, Observer> mValueObservers;
    std::size_t mNextValueObserver = 1;
    std::shared_ptr<char> mValueObserverLifetime = std::make_shared<char>(0);
};
} // namespace radia::ui
#endif // RD_WIDGETS_SWITCH_H
