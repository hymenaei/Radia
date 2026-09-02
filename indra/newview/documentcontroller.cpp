/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "documentcontroller.h"
#include <map>
#include <set>
#include "binding/binder.h"
#include "binding/eventregistration.h"
#include "documentcontrollerinternal.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "system.h"

namespace radia::viewer::ui {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::DiagnosticResult;
using radia::ui::EventRegistrationDescriptor;
using radia::ui::PreparedBinding;
using radia::ui::PreparedBindingResult;
using radia::ui::SettingResolver;
using radia::ui::System;
using radia::ui::detail::ElementIdIndex;
using radia::ui::detail::ElementInternalAccess;
using radia::ui::detail::indexElementsInScope;
using radia::ui::detail::makeEventRegistration;

struct DocumentController::PreparedMount::State {
    DocumentController* controller = nullptr;
    Document* document = nullptr;
    Element* root = nullptr;
    std::weak_ptr<char> rootLifetime;
    PreparedBinding binding;
};

struct DocumentController::Impl {
    std::vector<EventRegistrationDescriptor> registrations;
    bool registrationsSealed = false;
    std::set<std::string> elementIds;
    Binding binding;
};

DocumentController::DocumentController(System& system, Document& document) : mSystem(system), mDocument(document), mImpl(std::make_unique<Impl>()) {}

DocumentController::PreparedMount::PreparedMount() = default;
DocumentController::PreparedMount::~PreparedMount() = default;
DocumentController::PreparedMount::PreparedMount(PreparedMount&&) noexcept = default;
DocumentController::PreparedMount& DocumentController::PreparedMount::operator=(PreparedMount&&) noexcept = default;

DocumentController::PreparedMount::operator bool() const {
    return mState && mState->controller != nullptr;
}

bool DocumentController::PreparedMountResult::ok() const {
    return !hasErrors() && static_cast<bool>(mount);
}

DocumentController::~DocumentController() = default;

Element* DocumentController::getElementById(std::string_view id) {
    if (id.empty()) return nullptr;
    const std::string key(id);
    mImpl->elementIds.emplace(key);
    Element* root = mDocument.documentElement();
    if (!root) return nullptr;
    ElementIdIndex index;
    indexElementsInScope(*root, index);
    if (index.ambiguous.find(key) != index.ambiguous.end()) return nullptr;
    const auto found = index.first.find(key);
    return found == index.first.end() ? nullptr : found->second;
}

LocalizedText DocumentController::t(std::string localizationKey, LocalizationArguments arguments) const {
    return mSystem.t(std::move(localizationKey), std::move(arguments));
}

DocumentController::PreparedMountResult DocumentController::prepare(SettingResolver& settingResolver) {
    PreparedMountResult result;
    mImpl->registrationsSealed = true;
    Element* root = mDocument.documentElement();
    if (!root) {
        result.error("controller.document.empty", "Document Controller cannot prepare an empty Document.");
        return result;
    }
    Binder binder(*root, &settingResolver);
    for (const EventRegistrationDescriptor& registration : mImpl->registrations) binder.event(makeEventRegistration(registration));

    PreparedBindingResult binding = binder.prepare();
    const bool bindingOk = binding.ok();
    result.append(std::move(binding));
    if (!bindingOk) return result;

    ElementIdIndex index;
    indexElementsInScope(*root, index);
    for (const std::string& id : mImpl->elementIds)
        if (index.ambiguous.find(id) != index.ambiguous.end())
            result.error("controller.element.ambiguous", "Controller Element ID is not unique in this skin: " + id + ".");
        else if (index.first.find(id) == index.first.end())
            result.warning("controller.element.missing", "Controller Element ID is not present in this skin: " + id + ".");
    if (result.hasErrors()) return result;

    result.mount.mState = std::make_unique<PreparedMount::State>();
    result.mount.mState->controller = this;
    result.mount.mState->document = &mDocument;
    result.mount.mState->root = root;
    result.mount.mState->rootLifetime = ElementInternalAccess::lifetime(*root);
    result.mount.mState->binding = std::move(binding.binding);
    return result;
}

bool DocumentController::canCommit(const PreparedMount& prepared) const {
    return prepared.mState
        && prepared.mState->controller == this
        && prepared.mState->document == &mDocument
        && prepared.mState->root
        && !prepared.mState->rootLifetime.expired()
        && mDocument.documentElement() == prepared.mState->root
        && static_cast<bool>(prepared.mState->binding);
}

void DocumentController::commit(PreparedMount&& prepared) {
    if (!canCommit(prepared)) LL_ERRS("UI") << "DocumentController committed an invalid prepared mount." << LL_ENDL;
    PreparedMount::State& state = *prepared.mState;
    mImpl->binding = state.binding.commit();
    prepared.mState.reset();
}

void DocumentController::addHandlerRegistration(EventRegistrationDescriptor registration) {
    if (mImpl->registrationsSealed) {
        LL_WARNS("UI") << "Controller Event Handler registration was attempted after preparation began." << LL_ENDL;
        return;
    }
    mImpl->registrations.emplace_back(std::move(registration));
}
} // namespace radia::viewer::ui
