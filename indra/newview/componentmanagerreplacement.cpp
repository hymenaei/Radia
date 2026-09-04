/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <utility>
#include <vector>
#include "componentmanagerinternal.h"
#include "documentcontrollerinternal.h"
#include "resourceprovider.h"
#include "skin/generation.h"
#include "system.h"

namespace radia::viewer::ui {
using componentmanager_internal::attachRootLifecycle;
using componentmanager_internal::isClosed;
using componentmanager_internal::takeFloaterDocument;
using radia::ui::ResourceBuildResult;

ComponentManager::PreparedReplacement::State::~State() {
    if (!publicationReserved) return;
    if (const std::shared_ptr<ComponentManager::Impl> impl = manager.lock()) impl->releasePublication();
}

bool ComponentManager::PreparedReplacement::State::prepare() {
    const std::shared_ptr<ComponentManager::Impl> impl = manager.lock();
    if (!impl || !generation) {
        diagnostics.error("floater.transaction.manager_missing", "The Component Manager no longer owns the prepared replacement.");
        return false;
    }
    if (!impl->system.publicationInProgress()) {
        diagnostics.error("floater.transaction.publication_required", "A prepared component replacement must be committed by a System publication.");
        return false;
    }
    if (publicationReserved || prepared) {
        diagnostics.error("floater.transaction.busy", "The prepared component replacement was already prepared.");
        return false;
    }
    if (!impl->reservePublication()) {
        diagnostics.error("floater.transaction.busy", "The Component Manager is already processing another operation.");
        return false;
    }
    publicationReserved = true;
    if (prepareCandidate(*impl)) return true;
    impl->releasePublication();
    publicationReserved = false;
    return false;
}

bool ComponentManager::PreparedReplacement::prepare() {
    if (!mState) return false;
    const bool prepared = mState->prepare();
    if (prepared) return true;
    mDiagnostics.append(std::move(mState->diagnostics));
    mState.reset();
    return false;
}

bool ComponentManager::PreparedReplacement::commit() {
    if (!mState) return false;
    const bool committed = mState->commit();
    mDiagnostics.append(std::move(mState->diagnostics));
    if (!mState->publicationReserved) mState.reset();
    return committed;
}

void ComponentManager::PreparedReplacement::finalize() {
    mState.reset();
}

DiagnosticResult ComponentManager::PreparedReplacement::takeDiagnostics() {
    DiagnosticResult result;
    result.append(std::move(mDiagnostics));
    return result;
}

bool ComponentManager::PreparedReplacement::State::prepareCandidate(ComponentManager::Impl& impl) {
    ComponentManager::Impl::MutationScope operation(impl, ComponentManager::Impl::MutationMode::PublicationCommit);
    if (!operation) {
        diagnostics.error("floater.transaction.busy", "The Component Manager is already processing another operation.");
        return false;
    }
    if (!validateCurrentState(impl)) return false;
    if (!prepareComponents(impl, operation)) return false;
    if (!operation.valid()) {
        diagnostics.error("floater.transaction.reentrant", "A component mutation was requested during replacement preparation.");
        return false;
    }
    if (!validatePreparedState(impl)) return false;
    prepared = true;
    return true;
}

bool ComponentManager::PreparedReplacement::State::validateCurrentState(const ComponentManager::Impl& impl) {
    for (const PendingComponent& component : components) {
        const auto found = impl.instances.find(component.componentKey);
        const ComponentManager::Impl::Instance* instance = found == impl.instances.end() ? nullptr : &found->second;
        if (!instance || instance != component.instance || !instance->root || instance->root != component.current || isClosed(*instance->root)) {
            diagnostics.error("floater.transaction.stale", "An open component changed before its replacement was committed.");
            return false;
        }
    }
    return true;
}

bool ComponentManager::PreparedReplacement::State::prepareComponents(ComponentManager::Impl& impl, ComponentManager::Impl::MutationScope& operation) {
    for (PendingComponent& component : components) {
        const auto definition = impl.definitions.find(component.componentKey.definitionId);
        if (definition == impl.definitions.end()) {
            diagnostics.error("floater.definition.missing",
                              "Unknown HTMLFloaterElement definition during reload: " + component.componentKey.definitionId + ".");
            return false;
        }

        ResourceBuildResult buildResult = generation->buildElementTree(component.instance->resource, locale);
        component.replacement = takeFloaterDocument(std::move(buildResult), "Reloaded Component must have a <floater> root.",
                                                    component.instance->resource.value(), diagnostics);
        if (!component.replacement) return false;
        component.candidate =
            component.replacement->documentElement() ? dynamic_cast<HTMLFloaterElement*>(component.replacement->documentElement()) : nullptr;

        component.controller = definition->second.factory(impl.system, *component.replacement);
        if (!component.controller) {
            diagnostics.error(
                "floater.controller.missing",
                "HTMLFloaterElement controller factory returned no controller during reload: " + component.componentKey.definitionId + ".");
            return false;
        }

        if (!operation.valid()) {
            diagnostics.error("floater.transaction.reentrant", "A component mutation was requested during replacement preparation.");
            return false;
        }

        DocumentController::PreparedMountResult prepared = component.controller->prepare(impl.settingResolver);
        const bool preparedOk = prepared.ok();
        diagnostics.append(std::move(prepared));
        if (!preparedOk) return false;
        if (!component.controller->canCommit(prepared.mount)) {
            diagnostics.error("floater.controller.commit_invalid",
                              "HTMLFloaterElement controller prepared an invalid mount: " + component.componentKey.persistenceKey() + ".");
            return false;
        }
        component.mount = std::move(prepared.mount);
    }
    return true;
}

bool ComponentManager::PreparedReplacement::State::validatePreparedState(const ComponentManager::Impl& impl) {
    for (const PendingComponent& component : components) {
        const auto found = impl.instances.find(component.componentKey);
        const ComponentManager::Impl::Instance* instance = found == impl.instances.end() ? nullptr : &found->second;
        DocumentController* controller = component.controller.get();
        if (found == impl.instances.end()
            || instance != component.instance
            || !instance->root
            || instance->root != component.current
            || !controller
            || !component.replacement
            || !component.controller
            || !component.candidate
            || isClosed(*instance->root)
            || impl.pendingEvictions.contains(component.componentKey)
            || !controller->canCommit(component.mount)) {
            diagnostics.error("floater.transaction.stale", "A component changed while its replacement was being prepared.");
            return false;
        }
    }
    return true;
}

bool ComponentManager::PreparedReplacement::State::commitControllers() {
    for (PendingComponent& component : components) {
        if (component.controller->commit(std::move(component.mount))) continue;
        diagnostics.error("floater.controller.commit_invalid",
                          "HTMLFloaterElement controller could not commit its prepared binding: " + component.componentKey.persistenceKey() + ".");
        deactivateReplacementControllers();
        return false;
    }
    return true;
}

void ComponentManager::PreparedReplacement::State::deactivateReplacementControllers() const {
    for (const PendingComponent& component : components)
        if (component.controller) component.controller->deactivate();
}

bool ComponentManager::PreparedReplacement::State::reactivateCurrentControllers() const {
    bool result = true;
    for (const PendingComponent& component : components)
        if (!component.instance->controller->activate()) {
            LL_WARNS("UI") << "Component controller could not reactivate after host replacement rollback." << LL_ENDL;
            result = false;
        }
    return result;
}

bool ComponentManager::PreparedReplacement::State::replaceHost(ComponentManager::Impl& impl) {
    std::vector<ComponentManager::Host::ReplacementRequest> requests;
    requests.reserve(components.size());
    for (const PendingComponent& component : components) requests.push_back({component.current, component.replacement.get()});

    if (impl.host.replaceAll(std::move(requests))) return true;

    deactivateReplacementControllers();
    if (!reactivateCurrentControllers())
        diagnostics.error("floater.transaction.rollback_failed", "The current component bindings could not be restored.");
    diagnostics.error("floater.host.replace_failed", "The component host rejected the replacement transaction.");
    return false;
}

bool ComponentManager::PreparedReplacement::State::rollbackHost(ComponentManager::Impl& impl) const {
    std::vector<ComponentManager::Host::ReplacementRequest> requests;
    requests.reserve(components.size());
    for (const PendingComponent& component : components) requests.push_back({component.candidate, component.instance->document.get()});
    if (!impl.host.replaceAll(std::move(requests))) LL_ERRS("UI") << "Component host violated the replacement rollback contract." << LL_ENDL;
    return true;
}

bool ComponentManager::PreparedReplacement::State::restoreCurrentState(ComponentManager::Impl& impl) const {
    deactivateReplacementControllers();
    rollbackHost(impl);
    return reactivateCurrentControllers();
}

bool ComponentManager::PreparedReplacement::State::currentRootsOpen(const ComponentManager::Impl& impl) const {
    for (const PendingComponent& component : components)
        if (impl.pendingEvictions.contains(component.componentKey) || !component.instance->root || isClosed(*component.instance->root)) return false;
    return true;
}

bool ComponentManager::PreparedReplacement::State::activateComponents(ComponentManager::Impl& impl,
                                                                      ComponentManager::Impl::MutationScope& operation) {
    for (PendingComponent& component : components) {
        if (operation.valid() && currentRootsOpen(impl) && !isClosed(*component.candidate) && component.controller->activate()) continue;
        diagnostics.error("floater.controller.activation_invalid",
                          "HTMLFloaterElement controller could not activate its mounted binding: " + component.componentKey.persistenceKey() + ".");
        if (!restoreCurrentState(impl))
            diagnostics.error("floater.transaction.rollback_failed", "The component replacement could not restore the previous state.");
        return false;
    }

    if (operation.valid() && currentRootsOpen(impl)) return true;
    diagnostics.error("floater.transaction.reentrant", "A component mutation was requested during replacement commit.");
    if (!restoreCurrentState(impl))
        diagnostics.error("floater.transaction.rollback_failed", "The component replacement could not restore the previous state.");
    return false;
}

bool ComponentManager::PreparedReplacement::State::commit() {
    const std::shared_ptr<ComponentManager::Impl> impl = manager.lock();
    if (!impl || !generation) {
        diagnostics.error("floater.transaction.manager_missing", "The Component Manager no longer owns the prepared replacement.");
        return false;
    }
    if (publicationReserved && !impl->system.publicationInProgress()) {
        diagnostics.error("floater.transaction.publication_required",
                          "A prepared component replacement must be committed by its active System publication.");
        return false;
    }
    if (impl->mPublicationMutationRejected) {
        diagnostics.error("floater.transaction.reentrant", "A component mutation was requested during publication.");
        return false;
    }
    if (impl->mutationEpoch != mutationEpoch) {
        diagnostics.error("floater.transaction.stale", "The prepared component replacement is no longer current.");
        return false;
    }

    const ComponentManager::Impl::MutationMode mode =
        publicationReserved ? ComponentManager::Impl::MutationMode::PublicationCommit : ComponentManager::Impl::MutationMode::Normal;
    ComponentManager::Impl::MutationScope operation(*impl, mode);
    if (!operation) {
        diagnostics.error("floater.transaction.busy", "The Component Manager is already processing another operation.");
        return false;
    }
    if (!validateCurrentState(*impl)) return false;
    if (!prepared) {
        if (!prepareComponents(*impl, operation)) return false;
        if (!operation.valid()) {
            diagnostics.error("floater.transaction.reentrant", "A component mutation was requested during replacement preparation.");
            return false;
        }
    }
    if (!validatePreparedState(*impl)) return false;
    if (!commitControllers()) return false;

    for (const PendingComponent& component : components) component.instance->controller->deactivate();
    if (!replaceHost(*impl)) return false;
    if (!activateComponents(*impl, operation)) return false;

    const std::weak_ptr<ComponentManager::Impl> weakManager = impl;
    for (PendingComponent& component : components) {
        ComponentManager::Impl::Instance& instance = *component.instance;
        instance.document = std::move(component.replacement);
        instance.controller = std::move(component.controller);
        HTMLFloaterElement* current = instance.root;
        HTMLFloaterElement* root = component.candidate;
        impl->rootKeys.erase(current);
        impl->rootKeys[root] = instance.componentKey;
        instance.root = root;
        instance.closeNotified = false;
        const ComponentInstanceKey closedComponentKey = instance.componentKey;
        attachRootLifecycle(*root, [weakManager, closedComponentKey, root] {
            if (const std::shared_ptr<ComponentManager::Impl> impl = weakManager.lock()) impl->rootClosed(closedComponentKey, root);
        });
    }
    ++impl->mutationEpoch;
    return true;
}
} // namespace radia::viewer::ui
