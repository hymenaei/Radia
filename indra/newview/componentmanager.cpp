/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "componentmanager.h"
#include <cstdint>
#include <map>
#include <set>
#include <utility>
#include <vector>
#include "binding/settingresolver.h"
#include "componentmanagerinternal.h"
#include "documentcontrollerinternal.h"
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "resourceprovider.h"
#include "skin/generation.h"
#include "system.h"

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::HTMLFloaterElement;
using radia::ui::ResourceBuildResult;
using radia::ui::ResourceId;
using radia::ui::SettingResolver;
using radia::ui::SkinGeneration;
using radia::ui::System;

using componentmanager_internal::attachRootLifecycle;
using componentmanager_internal::isClosed;
using componentmanager_internal::takeFloaterDocument;

ComponentManager::Impl::Impl(System& system, Host& host, SettingResolver& resolver) : system(system), host(host), settingResolver(resolver) {}

ComponentManager::Impl::~Impl() {
    MutationScope operation(*this, MutationMode::PublicationCommit);
    if (!operation) LL_ERRS("UI") << "Component manager was destroyed during another mutation." << LL_ENDL;

    std::vector<HTMLFloaterElement*> roots;
    roots.reserve(instances.size() + retainedMounts.size());
    for (auto& entry : instances)
        if (entry.second.root) {
            entry.second.controller->deactivate();
            roots.push_back(entry.second.root);
        }
    for (const RetainedMount& retained : retainedMounts)
        if (HTMLFloaterElement* root = retained.root.get()) roots.push_back(root);
        else LL_ERRS("UI") << "Retained component mount lost its root during manager shutdown." << LL_ENDL;

    if (!roots.empty()) {
        const bool previousSuppressCloseCallback = mSuppressCloseControllerCallback;
        mSuppressCloseControllerCallback = true;
        const bool cleared = host.clearAll(std::move(roots));
        mSuppressCloseControllerCallback = previousSuppressCloseCallback;
        if (!cleared) LL_ERRS("UI") << "Component host could not detach all roots during manager shutdown." << LL_ENDL;
        for (auto& entry : instances)
            if (!entry.second.closeNotified) {
                entry.second.closeNotified = true;
                entry.second.controller->onClose();
            }
    }
    retainedMounts.clear();
}

bool ComponentManager::Impl::beginMutation(MutationMode mode) {
    const bool publicationCommit = mode == MutationMode::PublicationCommit;
    if (mMutationActive
        || (mPublicationReserved && !publicationCommit)
        || (!publicationCommit && system.publicationInProgress())
        || (!retainedMounts.empty() && mode == MutationMode::Normal)) {
        rejectMutation();
        return false;
    }
    mMutationActive = true;
    mMutationRejected = false;
    return true;
}

void ComponentManager::Impl::endMutation() {
    mMutationActive = false;
}

void ComponentManager::Impl::rejectMutation() {
    mMutationRejected = true;
    if (mPublicationReserved) mPublicationMutationRejected = true;
}

bool ComponentManager::Impl::reservePublication() {
    if (mPublicationReserved || mMutationActive || !retainedMounts.empty()) {
        rejectMutation();
        return false;
    }
    mPublicationReserved = true;
    mPublicationMutationRejected = false;
    return true;
}

void ComponentManager::Impl::releasePublication() {
    mPublicationReserved = false;
    mPublicationMutationRejected = false;
}

bool ComponentManager::Impl::retryRetainedMounts() {
    bool allReleased = true;
    for (auto retained = retainedMounts.begin(); retained != retainedMounts.end();) {
        HTMLFloaterElement* root = retained->root.get();
        if (!root) LL_ERRS("UI") << "Retained component mount lost its root during retry." << LL_ENDL;
        if (host.unmount(*root)) retained = retainedMounts.erase(retained);
        else {
            allReleased = false;
            ++retained;
        }
    }
    return allReleased;
}

bool ComponentManager::Impl::unmountOrRetain(ComponentInstanceKey componentKey, std::unique_ptr<Document> document,
                                             std::unique_ptr<DocumentController> controller, HTMLFloaterElement& root) {
    root.setLifecycleCallbacks({}, {});
    if (host.unmount(root)) return true;
    root.close();
    retainedMounts.push_back({std::move(componentKey), std::move(document), std::move(controller), ElementRef<HTMLFloaterElement>(&root)});
    return false;
}

bool ComponentManager::Impl::discardMountedInstance(std::map<ComponentInstanceKey, Instance>::iterator found) {
    Instance& instance = found->second;
    const ComponentInstanceKey componentKey = instance.componentKey;
    HTMLFloaterElement* root = instance.root;
    if (!root || !instance.document || !instance.controller) LL_ERRS("UI") << "Mounted component instance lost an owner before rollback." << LL_ENDL;
    std::unique_ptr<Document> document = std::move(instance.document);
    std::unique_ptr<DocumentController> controller = std::move(instance.controller);
    const auto rootKey = rootKeys.find(root);
    if (rootKey != rootKeys.end() && rootKey->second == componentKey) rootKeys.erase(rootKey);
    pendingEvictions.erase(componentKey);
    instances.erase(found);
    return unmountOrRetain(componentKey, std::move(document), std::move(controller), *root);
}

std::vector<ComponentManager::Impl::OpenComponentSnapshot> ComponentManager::Impl::openSnapshot() const {
    std::vector<OpenComponentSnapshot> snapshot;
    snapshot.reserve(instances.size());
    for (const auto& [key, instance] : instances) {
        if (!instance.root || isClosed(*instance.root)) continue;
        snapshot.push_back({key, ElementRef<HTMLFloaterElement>(instance.root)});
    }
    return snapshot;
}

void ComponentManager::Impl::rootClosed(const ComponentInstanceKey& key, HTMLFloaterElement* root) {
    const auto found = instances.find(key);
    if (found == instances.end() || found->second.root != root || !root || !isClosed(*root)) return;
    pendingEvictions.insert(key);
    ++mutationEpoch;
    if (mMutationActive || mSuppressCloseControllerCallback || mPublicationReserved || system.publicationInProgress()) return;

    MutationScope operation(*this);
    if (!operation) {
        LL_WARNS("UI") << "Rejected a nested component close operation." << LL_ENDL;
        return;
    }

    const auto current = instances.find(key);
    if (current == instances.end() || current->second.root != root || !isClosed(*root)) return;
    if (!current->second.closeNotified) {
        current->second.closeNotified = true;
        current->second.controller->onClose();
        if (!isClosed(*root)) {
            current->second.closeNotified = false;
            pendingEvictions.erase(key);
        }
    }
}

bool ComponentManager::Impl::evictClosed() {
    bool allEvicted = true;
    const std::vector<ComponentInstanceKey> pending(pendingEvictions.begin(), pendingEvictions.end());
    for (const ComponentInstanceKey& key : pending) {
        const auto found = instances.find(key);
        if (found == instances.end() || !found->second.root || !isClosed(*found->second.root)) {
            pendingEvictions.erase(key);
            continue;
        }

        HTMLFloaterElement* root = found->second.root;
        DocumentController& controller = *found->second.controller;
        if (!found->second.closeNotified) {
            found->second.closeNotified = true;
            controller.onClose();
            if (!isClosed(*root)) {
                found->second.closeNotified = false;
                pendingEvictions.erase(key);
                continue;
            }
            if (mMutationRejected) {
                allEvicted = false;
                break;
            }
        }
        controller.deactivate();
        if (!host.unmount(*root)) {
            if (!controller.activate()) LL_WARNS("UI") << "Closed component could not reactivate after host unmount rejection." << LL_ENDL;
            LL_WARNS("UI") << "Closed component could not be unmounted: " << key.persistenceKey() << LL_ENDL;
            allEvicted = false;
            continue;
        }

        rootKeys.erase(root);
        found->second.root = nullptr;
        instances.erase(found);
        pendingEvictions.erase(key);
        ++mutationEpoch;
    }
    return allEvicted;
}

ComponentManager::ComponentManager(System& system, Host& host, SettingResolver& settingResolver)
    : mImpl(std::make_shared<Impl>(system, host, settingResolver)) {}

ComponentManager::~ComponentManager() = default;

ComponentManager::PreparedReplacement::PreparedReplacement(std::unique_ptr<State> state) : mState(std::move(state)) {}

ComponentManager::PreparedReplacement::~PreparedReplacement() = default;
ComponentManager::PreparedReplacement::PreparedReplacement(PreparedReplacement&&) noexcept = default;
ComponentManager::PreparedReplacement& ComponentManager::PreparedReplacement::operator=(PreparedReplacement&&) noexcept = default;

bool ComponentManager::registerDefinition(std::string definitionId, std::string resource, ControllerFactory factory) {
    const ResourceId id(resource);
    if (!ComponentInstanceKey{definitionId, {}}.valid() || !id.valid() || !factory) return false;
    Impl::MutationScope operation(*mImpl);
    if (!operation) return false;
    const bool inserted = mImpl->definitions.emplace(std::move(definitionId), Impl::Definition{id, std::move(factory)}).second;
    if (inserted) ++mImpl->mutationEpoch;
    return inserted;
}

ComponentOpenResult ComponentManager::open(const std::string& definitionId, const std::string& instanceKey) {
    ComponentOpenResult result;
    Impl::MutationScope operation(*mImpl);
    if (!operation) {
        result.error("floater.transaction.busy", "The Component Manager is already processing another operation.");
        return result;
    }

    const auto definition = mImpl->definitions.find(definitionId);
    if (definition == mImpl->definitions.end()) {
        result.error("floater.definition.missing", "Unknown HTMLFloaterElement definition: " + definitionId + ".");
        return result;
    }

    const ComponentInstanceKey component{definitionId, instanceKey};
    if (!component.valid()) {
        result.error("floater.identity.invalid", "Invalid component identity for " + definitionId + ".");
        return result;
    }

    const std::string persistenceKey = component.persistenceKey();
    if (const auto existing = mImpl->instances.find(component); existing != mImpl->instances.end() && existing->second.root) {
        Impl::Instance& live = existing->second;
        HTMLFloaterElement* root = live.root;
        const bool wasClosed = isClosed(*root);
        const bool wasPending = mImpl->pendingEvictions.contains(component);
        const bool wasCloseNotified = live.closeNotified;
        mImpl->pendingEvictions.erase(component);
        live.closeNotified = false;
        result.floater = root;
        mImpl->host.present(*result.floater);
        if (!operation.valid() || isClosed(*root)) {
            if (wasClosed && !isClosed(*root)) root->close();
            live.closeNotified = wasCloseNotified;
            if (isClosed(*root) || wasPending) mImpl->pendingEvictions.insert(component);
            else mImpl->pendingEvictions.erase(component);
            result.floater = nullptr;
            result.error("floater.transaction.reentrant", "A component mutation was requested while presenting a component.");
            return result;
        }
        live.controller->onOpen();
        if (!operation.valid() || isClosed(*root)) {
            if (wasClosed) {
                if (!isClosed(*root)) root->close();
                live.controller->onClose();
                live.closeNotified = true;
            } else {
                live.closeNotified = wasCloseNotified;
            }
            if (isClosed(*root) || wasPending) mImpl->pendingEvictions.insert(component);
            else mImpl->pendingEvictions.erase(component);
            result.floater = nullptr;
            result.error("floater.transaction.reentrant", "A component mutation was requested while opening a component.");
        }
        if (wasClosed && result.ok()) ++mImpl->mutationEpoch;
        return result;
    }

    ResourceBuildResult buildResult = mImpl->system.buildElementTree(definition->second.resource);
    std::unique_ptr<Document> document =
        takeFloaterDocument(std::move(buildResult), "Component definition must have a <floater> root.", definition->second.resource.value(), result);
    if (!document) return result;

    std::unique_ptr<DocumentController> controller = definition->second.factory(mImpl->system, *document);
    if (!controller) {
        result.error("floater.controller.missing", "HTMLFloaterElement controller factory returned no controller: " + definitionId + ".");
        return result;
    }
    if (!operation.valid()) {
        result.error("floater.transaction.reentrant", "A component mutation was requested while creating a component controller.");
        return result;
    }

    DocumentController::PreparedMountResult prepared;
    HTMLFloaterElement* floater = document->documentElement() ? dynamic_cast<HTMLFloaterElement*>(document->documentElement()) : nullptr;
    prepared = controller->prepare(mImpl->settingResolver);
    const bool preparedOk = prepared.ok();
    result.append(std::move(prepared));
    if (!preparedOk) return result;
    if (!operation.valid()) {
        result.error("floater.transaction.reentrant", "A component mutation was requested while preparing a component.");
        return result;
    }

    if (!controller->canCommit(prepared.mount)) {
        result.error("floater.controller.commit_invalid", "HTMLFloaterElement controller prepared an invalid mount: " + persistenceKey + ".");
        return result;
    }

    if (!controller->commit(std::move(prepared.mount))) {
        result.error("floater.controller.commit_invalid",
                     "HTMLFloaterElement controller could not commit its prepared binding: " + persistenceKey + ".");
        return result;
    }

    if (!mImpl->host.mount(*document)) {
        result.error("floater.host.mount_failed", "Component host could not mount HTMLFloaterElement: " + persistenceKey + ".");
        return result;
    }
    if (!operation.valid()) {
        controller->deactivate();
        mImpl->unmountOrRetain(component, std::move(document), std::move(controller), *floater);
        result.error("floater.transaction.reentrant", "A component mutation was requested while mounting a component.");
        return result;
    }
    if (!controller->activate()) {
        controller->deactivate();
        mImpl->unmountOrRetain(component, std::move(document), std::move(controller), *floater);
        result.error("floater.controller.activation_invalid",
                     "HTMLFloaterElement controller could not activate its mounted binding: " + persistenceKey + ".");
        return result;
    }

    Impl::Instance instance(component, definition->second.resource, std::move(document), std::move(controller));
    instance.root = floater;
    result.floater = instance.root;

    auto [inserted, insertedNew] =
        mImpl->instances.try_emplace(component, component, instance.resource, std::move(instance.document), std::move(instance.controller));
    if (!insertedNew) {
        instance.controller->deactivate();
        mImpl->unmountOrRetain(component, std::move(instance.document), std::move(instance.controller), *floater);
        result.floater = nullptr;
        result.error("floater.transaction.identity_conflict", "The component identity was claimed during mount: " + persistenceKey + ".");
        return result;
    }

    Impl::Instance& live = inserted->second;
    live.root = floater;
    if (!mImpl->rootKeys.emplace(floater, component).second) {
        live.controller->deactivate();
        mImpl->discardMountedInstance(inserted);
        result.floater = nullptr;
        result.error("floater.transaction.identity_conflict", "The component root identity was already registered: " + persistenceKey + ".");
        return result;
    }

    const std::weak_ptr<Impl> weakImpl = mImpl;
    const ComponentInstanceKey closedComponentKey = component;
    attachRootLifecycle(*result.floater, [weakImpl, closedComponentKey, floater] {
        if (const std::shared_ptr<Impl> impl = weakImpl.lock()) impl->rootClosed(closedComponentKey, floater);
    });
    mImpl->host.present(*result.floater);
    if (!operation.valid() || isClosed(*floater)) {
        result.error("floater.transaction.reentrant", "A component mutation was requested while presenting a component.");
        live.controller->deactivate();
        mImpl->discardMountedInstance(inserted);
        result.floater = nullptr;
        return result;
    }
    live.controller->onOpen();
    if (!operation.valid() || isClosed(*floater)) {
        result.error("floater.transaction.reentrant", "A component mutation was requested while opening a component.");
        live.controller->onClose();
        live.controller->deactivate();
        mImpl->discardMountedInstance(inserted);
        result.floater = nullptr;
        return result;
    }
    ++mImpl->mutationEpoch;
    return result;
}

void ComponentManager::forEachOpen(const OpenComponentCallback& callback) const {
    if (!callback) return;
    const std::vector<Impl::OpenComponentSnapshot> snapshot = mImpl->openSnapshot();
    for (const Impl::OpenComponentSnapshot& entry : snapshot) {
        const auto found = mImpl->instances.find(entry.key);
        HTMLFloaterElement* floater = entry.root.get();
        if (found == mImpl->instances.end() || found->second.root != floater || !floater || isClosed(*floater)) continue;
        callback(entry.key, *floater);
    }
}

std::optional<ComponentInstanceKey> ComponentManager::componentKeyFor(const HTMLFloaterElement& floater) const {
    const auto found = mImpl->rootKeys.find(&floater);
    if (found == mImpl->rootKeys.end()) return std::nullopt;
    return found->second;
}

ComponentManager::ReplacementResult ComponentManager::prepareReplacement(std::shared_ptr<const SkinGeneration> generation, std::string locale) {
    ReplacementResult result;
    if (!generation) {
        result.error("floater.generation.missing", "Cannot prepare a component replacement without a Skin Generation.");
        return result;
    }

    if (mImpl->mMutationActive || mImpl->mPublicationReserved || mImpl->system.publicationInProgress() || !mImpl->retainedMounts.empty()) {
        mImpl->rejectMutation();
        result.error("floater.transaction.busy", "The Component Manager is already processing another operation.");
        return result;
    }

    auto pending = std::make_unique<PreparedReplacement::State>();
    pending->manager = mImpl;
    pending->generation = std::move(generation);
    pending->locale = std::move(locale);
    pending->mutationEpoch = mImpl->mutationEpoch;
    pending->components.reserve(mImpl->instances.size());
    for (auto& [componentKey, instance] : mImpl->instances) {
        if (!instance.root || isClosed(*instance.root)) continue;

        PreparedReplacement::State::PendingComponent pendingComponent;
        pendingComponent.componentKey = componentKey;
        pendingComponent.instance = &instance;
        pendingComponent.current = instance.root;
        pending->components.push_back(std::move(pendingComponent));
    }

    result.replacement = PreparedReplacement(std::move(pending));
    return result;
}

bool ComponentManager::clearInstances() {
    Impl::MutationScope operation(*mImpl, Impl::MutationMode::Cleanup);
    if (!operation) return false;

    struct PendingClear {
        ComponentInstanceKey key;
        HTMLFloaterElement* root = nullptr;
        DocumentController* controller = nullptr;
        bool wasClosed = false;
        bool closeNotified = false;
    };

    const std::set<ComponentInstanceKey> previousPendingEvictions = mImpl->pendingEvictions;
    std::vector<PendingClear> pending;
    pending.reserve(mImpl->instances.size());
    std::vector<HTMLFloaterElement*> roots;
    roots.reserve(mImpl->instances.size() + mImpl->retainedMounts.size());
    for (auto& [key, instance] : mImpl->instances)
        if (instance.root) {
            pending.push_back({key, instance.root, instance.controller.get(), isClosed(*instance.root), instance.closeNotified});
            instance.controller->deactivate();
            roots.push_back(instance.root);
        }
    for (const Impl::RetainedMount& retained : mImpl->retainedMounts)
        if (HTMLFloaterElement* root = retained.root.get()) roots.push_back(root);
        else LL_ERRS("UI") << "Retained component mount lost its root during account teardown." << LL_ENDL;

    const bool previousSuppressCloseCallback = mImpl->mSuppressCloseControllerCallback;
    mImpl->mSuppressCloseControllerCallback = true;
    const bool cleared = mImpl->host.clearAll(std::move(roots));
    mImpl->mSuppressCloseControllerCallback = previousSuppressCloseCallback;
    if (!cleared) {
        mImpl->pendingEvictions = previousPendingEvictions;
        for (PendingClear& entry : pending) {
            if (!entry.root) continue;
            if (!entry.wasClosed && isClosed(*entry.root)) entry.root->open();
            if (!entry.controller->activate())
                LL_WARNS("UI") << "Component controller could not reactivate after account teardown rejection." << LL_ENDL;
        }
        LL_WARNS("UI") << "Component host rejected account teardown; retaining the current component generation." << LL_ENDL;
        return false;
    }

    for (const PendingClear& entry : pending)
        if (!entry.closeNotified) entry.controller->onClose();

    for (auto& [key, instance] : mImpl->instances) {
        instance.root = nullptr;
        instance.controller.reset();
        instance.document.reset();
    }
    mImpl->instances.clear();
    mImpl->rootKeys.clear();
    mImpl->pendingEvictions.clear();
    mImpl->retainedMounts.clear();
    ++mImpl->mutationEpoch;
    return true;
}

void ComponentManager::idle() {
    Impl::MutationScope operation(*mImpl, Impl::MutationMode::Cleanup);
    if (!operation) return;
    mImpl->retryRetainedMounts();
    mImpl->evictClosed();
}

void ComponentManager::reportReloadSucceeded() {
    Impl::MutationScope operation(*mImpl);
    if (!operation) return;
    const std::vector<Impl::OpenComponentSnapshot> snapshot = mImpl->openSnapshot();
    for (const Impl::OpenComponentSnapshot& entry : snapshot) {
        const auto found = mImpl->instances.find(entry.key);
        HTMLFloaterElement* root = entry.root.get();
        if (found == mImpl->instances.end() || found->second.root != root || !root || isClosed(*root)) continue;
        found->second.controller->onReloadSucceeded();
        if (!operation.valid()) break;
    }
}

void ComponentManager::reportReloadFailed(const DiagnosticResult& diagnostics) {
    Impl::MutationScope operation(*mImpl);
    if (!operation) return;
    const std::vector<Impl::OpenComponentSnapshot> snapshot = mImpl->openSnapshot();
    for (const Impl::OpenComponentSnapshot& entry : snapshot) {
        const auto found = mImpl->instances.find(entry.key);
        HTMLFloaterElement* root = entry.root.get();
        if (found == mImpl->instances.end() || found->second.root != root || !root || isClosed(*root)) continue;
        found->second.controller->onReloadFailed(diagnostics);
        if (!operation.valid()) break;
    }
}
} // namespace radia::viewer::ui
