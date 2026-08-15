/**
 * @file binder.cpp
 * @brief Validates and commits controller bindings to UI Widget trees.
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
#include "binding/binder.h"
#include <map>
#include <set>
#include "binding/valuecontrol.h"
#include "layout/schema.h"

namespace radia::ui {
PreparedBinding::PreparedBinding() = default;
PreparedBinding::~PreparedBinding() = default;
PreparedBinding::PreparedBinding(PreparedBinding&&) noexcept = default;
PreparedBinding& PreparedBinding::operator=(PreparedBinding&&) noexcept = default;

Binding PreparedBinding::commit() {
    Binding binding;
    if (!mBinder) return binding;
    mBinder->commit(*mBinder->mRoot, binding);
    mBinder.reset();
    return binding;
}

void Binder::event(EventHandlerRegistration&& registration) {
    const bool valid = registration.valid();
    auto handler = std::make_shared<detail::EventHandler>();
    const std::optional<WidgetEventKind> kind = registration.kind();
    handler->kind = kind;
    handler->invoke = std::move(registration).takeInvoke();
    mPendingEventHandlers.push_back(
        {std::move(registration).takeName(), kind, std::move(handler), std::move(registration).takeArgumentError(), valid});
}

void Binder::event(const EventHandlerRegistration& registration) {
    event(registration.copy());
}

namespace {
void collectEventCalls(Widget& widget, std::map<std::string, std::vector<Binder::EventDeclaration>>& declarations, DiagnosticResult& result) {
    for (WidgetEventKind kind : kAllWidgetEventKinds) {
        const EventCall* eventCall = widget.eventCall(kind);
        if (!eventCall) continue;
        const std::string& name = eventCall->name();
        auto& byName = declarations[name];
        if (!byName.empty() && byName.front().kind != kind)
            result.error("binding.event.kind_mismatch", "Event Handler " + name + " is declared for multiple Event kinds.");
        byName.push_back({&widget, kind, eventCall});
    }
    for (const auto& child : widget.children())
        if (!child->idScopeRoot()) collectEventCalls(*child, declarations, result);
}

void collectValueControls(Widget& widget, std::vector<ValueControl*>& controls) {
    if (auto* control = dynamic_cast<ValueControl*>(&widget); control && control->valueBindingRequest()) controls.push_back(control);
    for (const auto& child : widget.children())
        if (!child->idScopeRoot()) collectValueControls(*child, controls);
}
} // namespace

void Binder::validate(Widget& root, DiagnosticResult& result) {
    mValueControls.clear();
    collectValueControls(root, mValueControls);
    for (ValueControl* control : mValueControls) control->prepareValueBinding(*this);

    mEventDeclarations.clear();
    collectEventCalls(root, mEventDeclarations, result);

    std::map<std::string, std::optional<WidgetEventKind>> registeredHandlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) {
        if (!pending.valid)
            result.error("binding.event.registration_invalid", "Controller Event Handler registration is incomplete: " + pending.name + ".");
        if (!isEventHandlerName(pending.name))
            result.error("binding.event.name_invalid", "Controller Event Handler name must use lower-camel-case: " + pending.name + ".");
        const auto inserted = registeredHandlers.emplace(pending.name, pending.kind);
        if (!inserted.second) result.error("binding.event.duplicate", "Controller Event Handler is registered more than once: " + pending.name + ".");
        const auto declared = mEventDeclarations.find(pending.name);
        if (declared != mEventDeclarations.end() && pending.kind && !declared->second.empty() && declared->second.front().kind != *pending.kind)
            result.error("binding.event.kind_mismatch", "Controller Event kind does not match Layout Resource call: " + pending.name + ".");
    }

    for (const auto& entry : mEventDeclarations) {
        const std::string& name = entry.first;
        if (registeredHandlers.find(name) == registeredHandlers.end())
            result.warning("binding.event.unhandled", "Event Handler Call has no Controller Handler: " + name + ".");
    }

    for (PendingEventHandler& pending : mPendingEventHandlers) {
        const auto found = mEventDeclarations.find(pending.name);
        if (found == mEventDeclarations.end()) continue;
        for (const EventDeclaration& declaration : found->second) {
            if (pending.kind && *pending.kind != declaration.kind) continue;
            if (pending.argumentError) {
                if (const char* error = pending.argumentError(*declaration.call, declaration.kind))
                    result.warning(error, "Event Handler Call " + pending.name + " does not match its registered Handler signature.");
            }
        }
    }

    std::set<std::string> settingDiagnostics;
    for (PendingValueRequirement& pending : mPendingValueRequirements) {
        const auto reportSettingError = [&](const char* code, std::string message) {
            std::string key = code;
            key.push_back('\0');
            key += pending.settingName;
            key.push_back('\0');
            key += pending.typeName;
            if (settingDiagnostics.emplace(std::move(key)).second) result.error(code, std::move(message));
        };
        if (!mSettingResolver) {
            reportSettingError("binding.setting.resolver_missing", "No SettingResolver was provided for setting: " + pending.settingName + ".");
            continue;
        }

        const SettingResolution resolution = mSettingResolver->resolve(pending.settingName, pending.type);
        switch (resolution.status) {
            case SettingResolution::ResolutionStatus::Found:
                if (!resolution.binding || !pending.matches || !pending.matches(resolution.binding)) {
                    reportSettingError("binding.setting.type_mismatch",
                                       "Setting " + pending.settingName + " does not provide " + pending.typeName + ".");
                    continue;
                }
                pending.resolved = resolution.binding;
                break;
            case SettingResolution::ResolutionStatus::Missing:
                reportSettingError("binding.setting.missing", "Setting is not available in this viewer: " + pending.settingName + ".");
                break;
            case SettingResolution::ResolutionStatus::TypeMismatch:
                reportSettingError("binding.setting.type_mismatch", "Setting " + pending.settingName + " is not a " + pending.typeName + ".");
                break;
            case SettingResolution::ResolutionStatus::Invalid:
                reportSettingError("binding.setting.name_invalid", "Setting is not a valid setting name: " + pending.settingName + ".");
                break;
        }
    }
}

void Binder::commit(Widget&, Binding& binding) {
    binding.mCommitted = true;

    for (PendingValueRequirement& pending : mPendingValueRequirements) pending.commit(pending.resolved);
    for (ValueControl* control : mValueControls) {
        ValueBindingSubscription subscription = control->commitValueBinding();
        if (subscription) binding.mValueSubscriptions.push_back(std::move(subscription));
    }
    std::map<std::string, PendingEventHandler*> handlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) handlers.emplace(pending.name, &pending);
    for (const auto& [name, declarations] : mEventDeclarations) {
        const auto found = handlers.find(name);
        if (found == handlers.end()) continue;
        PendingEventHandler& pending = *found->second;
        for (const EventDeclaration& declaration : declarations)
            if ((!pending.kind || declaration.kind == *pending.kind)
                && (!pending.argumentError || !pending.argumentError(*declaration.call, declaration.kind)))
                declaration.widget->bindEventHandler(declaration.kind, pending.handler);
    }
    for (PendingEventHandler& pending : mPendingEventHandlers) binding.mHandlers.push_back(std::move(pending.handler));
}

PreparedBindingResult Binder::prepare() {
    PreparedBindingResult result;
    if (mFinished) {
        result.error("binding.already_finished", "Binder transaction was already finished.");
        return result;
    }
    mFinished = true;

    validate(*mRoot, result);
    if (!result.hasErrors()) {
        auto binder = std::unique_ptr<Binder>(new Binder(std::move(*this)));
        result.binding.mBinder = std::move(binder);
    }
    return result;
}
} // namespace radia::ui
