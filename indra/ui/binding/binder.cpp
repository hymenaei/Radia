/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "binding/binder.h"
#include <map>
#include <set>
#include "elements/elementevent.h"
#include "elements/elementinternal.h"
#include "layout/schema.h"

namespace radia::ui {
Binding::~Binding() {
    clearEventListeners();
}

Binding::Binding(Binding&& other) noexcept
    : mEventAttachments(std::move(other.mEventAttachments)), mValueSubscriptions(std::move(other.mValueSubscriptions)), mCommitted(other.mCommitted) {
    other.mCommitted = false;
}

Binding& Binding::operator=(Binding&& other) noexcept {
    if (this == &other) return *this;
    clearEventListeners();
    mEventAttachments = std::move(other.mEventAttachments);
    mValueSubscriptions = std::move(other.mValueSubscriptions);
    mCommitted = other.mCommitted;
    other.mCommitted = false;
    return *this;
}

void Binding::clearEventListeners() noexcept {
    for (const EventAttachment& attachment : mEventAttachments)
        if (attachment.element && !attachment.lifetime.expired())
            attachment.element->removeEventListener(attachment.type, attachment.handler, attachment.capture);
    mEventAttachments.clear();
}

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
    std::string name = std::move(registration).takeName();
    EventRegistrationDescriptor::Invoke invoke = std::move(registration).takeInvoke();
    EventHandlerRegistration::ArgumentError argumentError = std::move(registration).takeArgumentError();
    mPendingEventHandlers.push_back({std::move(name), std::move(invoke), std::move(argumentError), valid});
}

void Binder::event(const EventHandlerRegistration& registration) {
    event(registration.copy());
}

namespace {
void collectEventCalls(Element& element, std::map<std::string, std::vector<Binder::EventDeclaration>>& declarations) {
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        const std::string type(descriptor.type);
        const EventCall* eventCall = element.eventCall(type);
        if (!eventCall) continue;
        const std::string& name = eventCall->name();
        declarations[name].push_back({&element, type, eventCall});
    }
    for (Element* child : element.children())
        if (!child->idScopeRoot()) collectEventCalls(*child, declarations);
}

void collectBoundInputs(Element& element, std::vector<InputElement*>& inputs) {
    if (auto* input = dynamic_cast<InputElement*>(&element); input && input->valueBindingRequest()) inputs.push_back(input);
    for (Element* child : element.children())
        if (!child->idScopeRoot()) collectBoundInputs(*child, inputs);
}
} // namespace

void Binder::validate(Element& root, DiagnosticResult& result) {
    mBoundInputs.clear();
    collectBoundInputs(root, mBoundInputs);
    for (InputElement* input : mBoundInputs) input->prepareValueBinding(*this);

    mEventDeclarations.clear();
    collectEventCalls(root, mEventDeclarations);

    std::set<std::string> registeredHandlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) {
        if (!pending.valid)
            result.error("binding.event.registration_invalid", "Controller Event Handler registration is incomplete: " + pending.name + ".");
        if (!isEventHandlerName(pending.name))
            result.error("binding.event.name_invalid", "Controller Event Handler name must use lower-camel-case: " + pending.name + ".");
        const auto inserted = registeredHandlers.emplace(pending.name);
        if (!inserted.second) result.error("binding.event.duplicate", "Controller Event Handler is registered more than once: " + pending.name + ".");
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
            if (pending.argumentError) {
                if (const char* error = pending.argumentError(*declaration.call))
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

void Binder::commit(Element&, Binding& binding) {
    binding.mCommitted = true;

    for (PendingValueRequirement& pending : mPendingValueRequirements) pending.commit(pending.resolved);
    for (InputElement* input : mBoundInputs) {
        ValueBindingSubscription subscription = input->commitValueBinding();
        if (subscription) binding.mValueSubscriptions.push_back(std::move(subscription));
    }
    std::map<std::string, PendingEventHandler*> handlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) handlers.emplace(pending.name, &pending);
    for (const auto& [name, declarations] : mEventDeclarations) {
        const auto found = handlers.find(name);
        if (found == handlers.end()) continue;
        PendingEventHandler& pending = *found->second;
        for (const EventDeclaration& declaration : declarations)
            if (!pending.argumentError || !pending.argumentError(*declaration.call)) {
                const EventCall call = *declaration.call;
                EventHandler listener([invoke = pending.invoke, call](Event& event) mutable { invoke(event, call); });
                declaration.element->addEventListener(declaration.type, listener);
                binding.mEventAttachments.push_back({declaration.element, detail::ElementInternalAccess::lifetime(*declaration.element),
                                                     declaration.type, std::move(listener), false});
            }
    }
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
