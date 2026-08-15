/**
 * @file componentcontroller.cpp
 * @brief Defines the viewer ComponentController contract for mounted UI roots.
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

#include "llviewerprecompiledheaders.h"
#include "componentcontroller.h"
#include <set>
#include "binding/binder.h"
#include "binding/eventregistration.h"
#include "componentcontrollerevents.h"
#include "componentcontrollerinternal.h"
#include "layout/schema.h"
#include "system.h"

namespace radia::viewer::ui::detail {
using radia::ui::Widget;
using radia::ui::WidgetRef;

struct ControllerWidgetSlot {
    std::string id;
    WidgetRef<Widget> current;
    std::set<std::string> warnedOperations;
};
} // namespace radia::viewer::ui::detail

namespace radia::viewer::ui {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::DiagnosticResult;
using radia::ui::EventRegistrationDescriptor;
using radia::ui::isWidgetIdentifier;
using radia::ui::PreparedBinding;
using radia::ui::PreparedBindingResult;
using radia::ui::SettingResolver;
using radia::ui::System;
using radia::ui::TextSource;
using radia::ui::WidgetRef;
using radia::ui::detail::indexWidgetsInScope;
using radia::ui::detail::makeEventRegistration;
using RuntimeWidget = radia::ui::Widget;

struct ComponentController::PreparedMount::State {
    ComponentController* controller = nullptr;
    RuntimeWidget* root = nullptr;
    PreparedWidgets widgets;
    PreparedBinding binding;
};

struct ComponentController::Impl {
    struct WidgetEntry {
        explicit WidgetEntry(std::string id) : slot(std::make_shared<detail::ControllerWidgetSlot>()), facade(slot) { slot->id = std::move(id); }

        std::shared_ptr<detail::ControllerWidgetSlot> slot;
        Widget facade;
    };

    std::vector<EventRegistrationDescriptor> registrations;
    bool registrationsSealed = false;
    std::map<std::string, WidgetEntry> widgets;
    Binding binding;
};

ComponentController::ComponentController(System& system) : mSystem(system), mImpl(std::make_unique<Impl>()) {}

ComponentController::PreparedMount::PreparedMount() = default;
ComponentController::PreparedMount::~PreparedMount() = default;
ComponentController::PreparedMount::PreparedMount(PreparedMount&&) noexcept = default;
ComponentController::PreparedMount& ComponentController::PreparedMount::operator=(PreparedMount&&) noexcept = default;

ComponentController::PreparedMount::operator bool() const {
    return mState && mState->controller != nullptr;
}

bool ComponentController::PreparedMountResult::ok() const {
    return !hasErrors() && static_cast<bool>(mount);
}

namespace {
void warnUnsupported(const std::shared_ptr<detail::ControllerWidgetSlot>& slot, const char* operation, const RuntimeWidget& widget) {
    if (!slot || !slot->warnedOperations.emplace(operation).second) return;
    LL_WARNS("UI") << "Controller Widget " << slot->id << " does not support " << operation << " on <" << widget.elementName() << ">." << LL_ENDL;
}

} // namespace

RuntimeWidget* Widget::runtimeWidget() const noexcept {
    return mSlot ? mSlot->current.get() : nullptr;
}

Widget::operator bool() const noexcept {
    return runtimeWidget() != nullptr;
}

std::string_view Widget::id() const noexcept {
    return mSlot ? std::string_view(mSlot->id) : std::string_view();
}

Widget Widget::fromEventSource(RuntimeWidget& source) {
    auto slot = std::make_shared<detail::ControllerWidgetSlot>();
    slot->id = source.id();
    slot->current.set(&source);
    return Widget(std::move(slot));
}

Widget& Widget::setContent(TextSource content) {
    if (RuntimeWidget* widget = runtimeWidget(); widget && !widget->setTextContent(std::move(content))) warnUnsupported(mSlot, "setContent", *widget);
    return *this;
}

Widget& Widget::setDisabled(bool disabled) {
    if (RuntimeWidget* widget = runtimeWidget()) widget->setDisabled(disabled);
    return *this;
}

Widget& Widget::setHidden(bool hidden) {
    if (RuntimeWidget* widget = runtimeWidget()) widget->setHidden(hidden);
    return *this;
}

Widget& Widget::setChecked(bool checked) {
    if (RuntimeWidget* widget = runtimeWidget(); widget && !widget->setCheckedValue(checked)) warnUnsupported(mSlot, "setChecked", *widget);
    return *this;
}

bool Widget::isDisabled() const noexcept {
    const RuntimeWidget* widget = runtimeWidget();
    return widget && widget->disabled();
}

bool Widget::isChecked() const {
    const RuntimeWidget* widget = runtimeWidget();
    if (widget) {
        const std::optional<bool> checked = widget->checkedValue();
        if (checked) return *checked;
        warnUnsupported(mSlot, "isChecked", *widget);
    }
    return false;
}

ComponentController::~ComponentController() {
    for (auto& [id, entry] : mImpl->widgets) entry.slot->current.set(nullptr);
}

Widget& ComponentController::getWidgetById(std::string_view id) {
    const std::string key(id);
    auto found = mImpl->widgets.find(key);
    if (found == mImpl->widgets.end()) found = mImpl->widgets.emplace(key, Impl::WidgetEntry(key)).first;
    return found->second.facade;
}

TextSource ComponentController::localize(std::string localizationKey) const {
    return mSystem.localize(std::move(localizationKey));
}

ComponentController::PreparedWidgets ComponentController::prepareWidgets(RuntimeWidget& root, DiagnosticResult& result) {
    PreparedWidgets prepared;
    prepared.mController = this;
    prepared.mRoot = &root;
    indexWidgetsInScope(root, prepared.mIndex);
    prepared.mTargets.reserve(mImpl->widgets.size());
    for (const auto& [id, entry] : mImpl->widgets) {
        if (!isWidgetIdentifier(id)) {
            result.warning("controller.widget.name_invalid", "Controller Widget ID is not a valid Widget identifier: " + id + ".");
            prepared.mTargets.push_back({entry.slot, nullptr});
            continue;
        }
        const auto indexed = prepared.mIndex.find(id);
        RuntimeWidget* widget = indexed == prepared.mIndex.end() ? nullptr : indexed->second;
        if (!widget) result.warning("controller.widget.missing", "Controller Widget ID is not present in this skin: " + id + ".");
        prepared.mTargets.push_back({entry.slot, widget});
    }
    return prepared;
}

void ComponentController::commitWidgets(PreparedWidgets&& prepared) {
    if (prepared.mController != this || !prepared.mRoot) return;
    for (PreparedWidgets::Target& target : prepared.mTargets) {
        target.slot->warnedOperations.clear();
        target.slot->current.set(target.widget);
    }
    prepared.mController = nullptr;
    prepared.mRoot = nullptr;
    prepared.mTargets.clear();
}

ComponentController::PreparedMountResult ComponentController::prepare(RuntimeWidget& root, SettingResolver& settingResolver) {
    PreparedMountResult result;
    mImpl->registrationsSealed = true;
    Binder binder(root, &settingResolver);
    for (const EventRegistrationDescriptor& registration : mImpl->registrations) binder.event(makeEventRegistration(registration));

    PreparedBindingResult binding = binder.prepare();
    const bool bindingOk = binding.ok();
    result.append(std::move(binding));
    if (!bindingOk) return result;

    PreparedWidgets widgets = prepareWidgets(root, result);
    if (result.hasErrors()) return result;

    result.mount.mState = std::make_unique<PreparedMount::State>();
    result.mount.mState->controller = this;
    result.mount.mState->root = &root;
    result.mount.mState->widgets = std::move(widgets);
    result.mount.mState->binding = std::move(binding.binding);
    return result;
}

bool ComponentController::canCommit(const PreparedMount& prepared, const RuntimeWidget& root) const {
    return prepared.mState && prepared.mState->controller == this && prepared.mState->root == &root;
}

void ComponentController::commit(PreparedMount&& prepared) {
    if (!prepared.mState || prepared.mState->controller != this || !prepared.mState->root)
        LL_ERRS("UI") << "ComponentController committed an invalid prepared mount." << LL_ENDL;
    PreparedMount::State& state = *prepared.mState;
    commitWidgets(std::move(state.widgets));
    mImpl->binding = state.binding.commit();
    prepared.mState.reset();
    postBuild();
}

void ComponentController::addEventRegistration(EventRegistrationDescriptor registration) {
    if (mImpl->registrationsSealed) {
        LL_WARNS("UI") << "Controller Event Handler registration was attempted after preparation began." << LL_ENDL;
        return;
    }
    mImpl->registrations.emplace_back(std::move(registration));
}
} // namespace radia::viewer::ui
