/**
 * @file switch.h
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

#ifndef LL_RDUI_SWITCH_H
#define LL_RDUI_SWITCH_H

#include <map>
#include "binding/valuecontrol.h"

namespace rdui {
struct WidgetContract;

namespace detail { WidgetContract switchContract(); }

class Switch : public ValueControl {
    friend WidgetContract detail::switchContract();

public:
    static constexpr const char* ELEMENT = "switch";

    Switch();
    Switch& setChecked(bool checked);
    Switch& setOnCheckedChanged(std::function<void(bool)> callback);
    bool checked() const { return hasState(WidgetState::Checked); }
    Widget* thumb() { return mThumb.get(); }
    const Widget* thumb() const { return mThumb.get(); }
    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }

    const std::string& bindingId() const override { return mBindingId; }
    ValueControlState valueControlState() const override;
    ValueBindingSubscription observeValueControlState(Observer observer) override;

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onActivate() override;
    void onChildrenCleared() override;

private:
    Switch& setBindingId(std::string id);
    void prepareValueBinding(Binder& binder) override;
    ValueBindingSubscription commitValueBinding() override;
    void applyValueState(ValueState<bool> state);
    void notifyValueState();

    WidgetRef<Widget> mThumb;
    std::function<void(bool)> mOnCheckedChanged;
    std::string mBindingId;
    ValueBindingRef<bool> mBinding;
    ValueState<bool> mValueState{false, false, std::nullopt};
    std::map<std::size_t, Observer> mValueObservers;
    std::size_t mNextValueObserver = 1;
    std::shared_ptr<char> mValueObserverLifetime = std::make_shared<char>(0);
};
} // namespace rdui
#endif // LL_RDUI_SWITCH_H
