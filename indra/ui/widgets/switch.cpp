/**
 * @file switch.cpp
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

#include "linden_common.h"
#include "widgets/switch.h"
#include "binding/binder.h"
#include "layout/schema.h"
#include "style/style.h"
#include "widgets/widgetcontractbuilder.h"

namespace radia::ui {
namespace {
class SwitchThumb final : public Widget {
public:
    SwitchThumb() : Widget("switch-thumb") { setPointerEvents(false); }

    void constrainResolvedStyle(Style& style) const override {
        style.alignSelf = AlignSelf::Stretch;
        style.aspectRatio = 1.f;
    }
};
} // namespace

Switch::Switch() : ValueControl(sElement) {
    detail::instantiateCompositeParts(*this, detail::switchContract());
}

void Switch::onChildrenCleared() {
    mThumb.set(nullptr);
    detail::instantiateCompositeParts(*this, detail::switchContract());
}

Switch& Switch::setChecked(bool checked) {
    const bool changed = updateCheckedState(checked);
    mValueState = {checked, checked, std::nullopt};
    if (changed) notifyValueControlState();
    return *this;
}

bool Switch::setCheckedValue(bool checked) {
    setChecked(checked);
    return true;
}

std::optional<bool> Switch::checkedValue() const {
    return checked();
}

bool Switch::updateCheckedState(bool checked) {
    const bool changed = checked != this->checked();
    setState(WidgetState::Checked, checked);
    if (changed) invalidateArrange();
    return changed;
}

Switch& Switch::setSettingName(std::string name) {
    mValueBindingRequest = ValueBindingRequest{std::move(name)};
    return *this;
}

ValueControlState Switch::valueControlState() const {
    ValueControlState result;
    result.dirty = mValueState.dirty();
    result.validationStatus = mValueState.validationStatus();
    if (const TextSource* message = mValueState.validationMessage()) result.validationMessage = *message;
    return result;
}

ValueBindingSubscription Switch::observeValueControlState(Observer observer) {
    const std::size_t id = mNextValueObserver++;
    mValueObservers.emplace(id, std::move(observer));
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    return ValueBindingSubscription([this, lifetime, id] {
        if (!lifetime.expired()) mValueObservers.erase(id);
    });
}

void Switch::notifyValueControlState() {
    const ValueControlState state = valueControlState();
    const auto observers = mValueObservers;
    for (const auto& [id, observer] : observers)
        if (mValueObservers.find(id) != mValueObservers.end()) observer(state);
}

void Switch::applyValueState(ValueState<bool> state) {
    mValueState = std::move(state);
    updateCheckedState(mValueState.value);
    notifyValueControlState();
}

void Switch::prepareValueBinding(Binder& binder) {
    if (mValueBindingRequest) binder.requireValueBinding(*mValueBindingRequest, mBinding);
}

ValueBindingSubscription Switch::commitValueBinding() {
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

Switch& Switch::setOnCheckedChanged(std::function<void(bool)> callback) {
    mOnCheckedChanged = std::move(callback);
    return *this;
}

void Switch::onActivate() {
    const bool previous = checked();
    if (mBinding) {
        mBinding->write(!previous);
    } else {
        mValueState.value = !previous;
        updateCheckedState(mValueState.value);
        notifyValueControlState();
    }
    if (checked() == previous) return;
    if (mOnCheckedChanged) mOnCheckedChanged(checked());
    emitEvent(ChangeEvent(*this, checked()));
}

void Switch::constrainResolvedStyle(Style& style) const {
    style.flow = Flow::Row;
    style.justifyContent = checked() ? JustifyContent::End : JustifyContent::Start;
}

WidgetContract detail::switchContract() {
    return defineWidget<Switch>(Switch::sElement)
        .attributes({booleanAttribute("checked", &Switch::setChecked), stringAttribute("setting", &Switch::setSettingName)})
        .validate([](const LayoutElement& element, Switch&, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext*) {
            const LayoutAttribute* setting = element.attribute("setting");
            if (setting && setting->value.empty())
                result.error("layout.value.setting_invalid", "Value Control setting must not be empty.", source, setting->source.begin.line,
                             setting->source.begin.column);
            if (setting && element.attribute("checked"))
                result.error("layout.value.multiple_sources", "Switch cannot declare both setting and checked.", source, setting->source.begin.line,
                             setting->source.begin.column);
        })
        .events({WidgetEventKind::Change, WidgetEventKind::DoubleClick, WidgetEventKind::MouseDown, WidgetEventKind::MouseUp,
                 WidgetEventKind::MouseMove, WidgetEventKind::LongClick, WidgetEventKind::ContextMenu})
        .labelable()
        .state(WidgetState::Checked)
        .part<SwitchThumb>("thumb", &Switch::mThumb)
        .build();
}
} // namespace radia::ui
