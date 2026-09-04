/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "componentmanager.h"
#include <map>
#include <set>
#include <utility>
#include "binding/settingresolver.h"
#include "documentcontrollerinternal.h"
#include "html/floater.h"
#include "resourceprovider.h"
#include "skin/generation.h"
#include "system.h"

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::HTMLFloaterElement;
using radia::ui::ResourceBuildResult;
using radia::ui::ResourceId;
using radia::ui::SettingResolver;
using radia::ui::SkinGeneration;
using radia::ui::System;

namespace {
void attachRootLifecycle(HTMLFloaterElement& root, DocumentController& controller, std::function<void()> onClose = {}) {
    root.setLifecycleCallbacks({}, [&controller, onClose = std::move(onClose)] {
        controller.onClose();
        if (onClose) onClose();
    });
}

bool isClosed(const HTMLFloaterElement& root) {
    return root.closed();
}

std::unique_ptr<Document> takeFloaterDocument(ResourceBuildResult buildResult, const std::string& rootError, const std::string& sourceName,
                                              DiagnosticResult& result) {
    Document* document = buildResult.document.get();
    Element* root = document ? document->documentElement() : nullptr;
    HTMLFloaterElement* candidate = root ? dynamic_cast<HTMLFloaterElement*>(root) : nullptr;
    if (buildResult.ok() && !candidate) buildResult.error("component.root.type_mismatch", rootError, sourceName);
    const bool buildOk = buildResult.ok() && candidate;
    std::unique_ptr<Document> resultDocument = std::move(buildResult.document);
    result.append(std::move(buildResult));
    if (!buildOk) return {};
    return resultDocument;
}
} // namespace

struct ComponentManager::Impl final {
    struct Definition {
        ResourceId resource;
        ControllerFactory factory;
    };

    struct Instance {
        Instance(ComponentInstanceKey componentKey, ResourceId resource, std::unique_ptr<Document> document,
                 std::unique_ptr<DocumentController> controller)
            : componentKey(std::move(componentKey)), resource(std::move(resource)), document(std::move(document)), controller(std::move(controller)) {
        }

        ComponentInstanceKey componentKey;
        ResourceId resource;
        std::unique_ptr<Document> document;
        std::unique_ptr<DocumentController> controller;
        HTMLFloaterElement* root = nullptr;
    };

    Impl(System& system, Host& host, SettingResolver& resolver) : system(system), host(host), settingResolver(resolver) {}

    ~Impl() {
        std::vector<HTMLFloaterElement*> roots;
        roots.reserve(instances.size());
        for (auto& entry : instances)
            if (entry.second.root) {
                entry.second.controller->deactivate();
                roots.push_back(entry.second.root);
            }

        if (!roots.empty() && !host.clearAll(std::move(roots)))
            LL_ERRS("UI") << "Component host could not detach all roots during manager shutdown." << LL_ENDL;
    }

    System& system;
    Host& host;
    SettingResolver& settingResolver;
    std::map<std::string, Definition> definitions;
    std::map<ComponentInstanceKey, Instance> instances;
    std::map<const HTMLFloaterElement*, ComponentInstanceKey> rootKeys;
    std::set<ComponentInstanceKey> pendingEvictions;

    void evictClosed() {
        for (auto pending = pendingEvictions.begin(); pending != pendingEvictions.end();) {
            const auto found = instances.find(*pending);
            if (found == instances.end() || !found->second.root || !isClosed(*found->second.root)) {
                pending = pendingEvictions.erase(pending);
                continue;
            }

            HTMLFloaterElement* root = found->second.root;
            DocumentController& controller = *found->second.controller;
            controller.deactivate();
            if (!host.unmount(*root)) {
                if (!controller.activate()) LL_ERRS("UI") << "Closed component could not reactivate after host unmount rejection." << LL_ENDL;
                LL_WARNS("UI") << "Closed component could not be unmounted: " << pending->persistenceKey() << LL_ENDL;
                ++pending;
                continue;
            }

            rootKeys.erase(root);
            found->second.root = nullptr;
            instances.erase(found);
            pending = pendingEvictions.erase(pending);
        }
    }
};

struct ComponentManager::PreparedReplacement::State {
    struct PendingComponent {
        ComponentInstanceKey componentKey;
        std::unique_ptr<Document> replacement;
        std::unique_ptr<DocumentController> controller;
        DocumentController::PreparedMount mount;
        ComponentManager::Impl::Instance* instance = nullptr;
        HTMLFloaterElement* current = nullptr;
        HTMLFloaterElement* candidate = nullptr;
    };

    std::weak_ptr<ComponentManager::Impl> manager;
    std::shared_ptr<const SkinGeneration> generation;
    std::string locale;
    std::vector<PendingComponent> components;
    DiagnosticResult diagnostics;

    bool commit();
};

ComponentManager::ComponentManager(System& system, Host& host, SettingResolver& settingResolver)
    : mImpl(std::make_shared<Impl>(system, host, settingResolver)) {}

ComponentManager::~ComponentManager() = default;

ComponentManager::PreparedReplacement::PreparedReplacement(std::unique_ptr<State> state) : mState(std::move(state)) {}

ComponentManager::PreparedReplacement::~PreparedReplacement() = default;
ComponentManager::PreparedReplacement::PreparedReplacement(PreparedReplacement&&) noexcept = default;
ComponentManager::PreparedReplacement& ComponentManager::PreparedReplacement::operator=(PreparedReplacement&&) noexcept = default;

bool ComponentManager::PreparedReplacement::commit() {
    if (!mState) return false;
    std::unique_ptr<State> state = std::move(mState);
    const bool committed = state->commit();
    mDiagnostics.append(std::move(state->diagnostics));
    return committed;
}

DiagnosticResult ComponentManager::PreparedReplacement::takeDiagnostics() {
    DiagnosticResult result;
    result.append(std::move(mDiagnostics));
    return result;
}

bool ComponentManager::PreparedReplacement::State::commit() {
    const std::shared_ptr<ComponentManager::Impl> impl = manager.lock();
    if (!impl || !generation) return false;

    for (const PendingComponent& component : components) {
        const auto found = impl->instances.find(component.componentKey);
        const ComponentManager::Impl::Instance* instance = found == impl->instances.end() ? nullptr : &found->second;
        if (!instance || instance != component.instance || !instance->root || instance->root != component.current) return false;
    }

    for (PendingComponent& component : components) {
        const auto definition = impl->definitions.find(component.componentKey.definitionId);
        if (definition == impl->definitions.end()) {
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

        component.controller = definition->second.factory(impl->system, *component.replacement);
        if (!component.controller) {
            diagnostics.error(
                "floater.controller.missing",
                "HTMLFloaterElement controller factory returned no controller during reload: " + component.componentKey.definitionId + ".");
            return false;
        }

        DocumentController::PreparedMountResult prepared = component.controller->prepare(impl->settingResolver);
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

    std::vector<ComponentManager::Host::ReplacementRequest> requests;
    requests.reserve(components.size());
    for (PendingComponent& component : components) {
        const auto found = impl->instances.find(component.componentKey);
        ComponentManager::Impl::Instance* instance = found == impl->instances.end() ? nullptr : &found->second;
        DocumentController* controller = component.controller.get();
        if (found == impl->instances.end()
            || instance != component.instance
            || !instance->root
            || instance->root != component.current
            || !controller
            || !component.replacement
            || !component.controller
            || !component.candidate
            || !controller->canCommit(component.mount))
            return false;
        requests.push_back({component.current, component.replacement.get()});
    }

    for (PendingComponent& component : components) {
        if (!component.controller->commit(std::move(component.mount))) {
            diagnostics.error(
                "floater.controller.commit_invalid",
                "HTMLFloaterElement controller could not commit its prepared binding: " + component.componentKey.persistenceKey() + ".");
            return false;
        }
    }

    for (const PendingComponent& component : components) component.instance->controller->deactivate();

    const auto reactivateCurrentControllers = [&]() {
        for (const PendingComponent& component : components)
            if (!component.instance->controller->activate())
                LL_ERRS("UI") << "Component controller could not reactivate after host replacement rollback." << LL_ENDL;
    };

    if (!impl->host.replaceAll(std::move(requests))) {
        reactivateCurrentControllers();
        return false;
    }

    const auto rollbackHost = [&]() {
        std::vector<ComponentManager::Host::ReplacementRequest> rollbackRequests;
        rollbackRequests.reserve(components.size());
        for (const PendingComponent& component : components) rollbackRequests.push_back({component.candidate, component.instance->document.get()});
        if (!impl->host.replaceAll(std::move(rollbackRequests)))
            LL_ERRS("UI") << "Component host could not roll back a failed replacement publication." << LL_ENDL;
    };

    std::size_t activatedControllers = 0;
    for (PendingComponent& component : components) {
        if (!component.controller->activate()) {
            diagnostics.error(
                "floater.controller.activation_invalid",
                "HTMLFloaterElement controller could not activate its mounted binding: " + component.componentKey.persistenceKey() + ".");
            for (std::size_t index = 0; index < activatedControllers; ++index) components[index].controller->deactivate();
            rollbackHost();
            reactivateCurrentControllers();
            return false;
        }
        ++activatedControllers;
    }

    const std::weak_ptr<ComponentManager::Impl> weakManager = impl;
    for (PendingComponent& component : components) {
        ComponentManager::Impl::Instance& instance = *component.instance;
        instance.document = std::move(component.replacement);
        instance.controller = std::move(component.controller);
        DocumentController& controller = *instance.controller;
        HTMLFloaterElement* current = instance.root;
        HTMLFloaterElement* root = component.candidate;
        impl->rootKeys.erase(current);
        impl->rootKeys[root] = instance.componentKey;
        instance.root = root;
        const ComponentInstanceKey closedComponentKey = instance.componentKey;
        attachRootLifecycle(*root, controller, [weakManager, closedComponentKey] {
            if (const std::shared_ptr<ComponentManager::Impl> impl = weakManager.lock()) impl->pendingEvictions.insert(closedComponentKey);
        });
    }
    return true;
}

bool ComponentManager::registerDefinition(std::string definitionId, std::string resource, ControllerFactory factory) {
    const ResourceId id(resource);
    if (!ComponentInstanceKey{definitionId, {}}.valid() || !id.valid() || !factory) return false;
    return mImpl->definitions.emplace(std::move(definitionId), Impl::Definition{id, std::move(factory)}).second;
}

ComponentOpenResult ComponentManager::open(const std::string& definitionId, const std::string& instanceKey) {
    ComponentOpenResult result;
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
        mImpl->pendingEvictions.erase(component);
        result.floater = existing->second.root;
        mImpl->host.present(*result.floater);
        existing->second.controller->onOpen();
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

    DocumentController::PreparedMountResult prepared;
    HTMLFloaterElement* floater = document->documentElement() ? dynamic_cast<HTMLFloaterElement*>(document->documentElement()) : nullptr;
    prepared = controller->prepare(mImpl->settingResolver);
    const bool preparedOk = prepared.ok();
    result.append(std::move(prepared));
    if (!preparedOk) return result;

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
    if (!controller->activate()) {
        if (!mImpl->host.unmount(*floater)) LL_ERRS("UI") << "Component host could not roll back a failed HTMLFloaterElement mount." << LL_ENDL;
        result.error("floater.controller.activation_invalid",
                     "HTMLFloaterElement controller could not activate its mounted binding: " + persistenceKey + ".");
        return result;
    }

    Impl::Instance instance(component, definition->second.resource, std::move(document), std::move(controller));
    instance.root = floater;
    result.floater = instance.root;

    mImpl->rootKeys[result.floater] = component;
    const std::weak_ptr<Impl> weakImpl = mImpl;
    const ComponentInstanceKey closedComponentKey = component;
    attachRootLifecycle(*result.floater, *instance.controller, [weakImpl, closedComponentKey] {
        if (const std::shared_ptr<Impl> impl = weakImpl.lock()) impl->pendingEvictions.insert(closedComponentKey);
    });
    DocumentController* mountedController = instance.controller.get();
    mImpl->instances.insert_or_assign(component, std::move(instance));
    mImpl->host.present(*result.floater);
    mountedController->onOpen();
    return result;
}

void ComponentManager::forEachOpen(const OpenComponentCallback& callback) const {
    if (!callback) return;
    for (const auto& [key, instance] : mImpl->instances)
        if (HTMLFloaterElement* floater = instance.root; floater && !isClosed(*floater)) callback(key, *floater);
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

    auto pending = std::make_unique<PreparedReplacement::State>();
    pending->manager = mImpl;
    pending->generation = std::move(generation);
    pending->locale = std::move(locale);
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
    std::vector<HTMLFloaterElement*> roots;
    roots.reserve(mImpl->instances.size());
    for (auto& entry : mImpl->instances)
        if (HTMLFloaterElement* root = entry.second.root) {
            entry.second.controller->deactivate();
            roots.push_back(root);
        }

    if (!mImpl->host.clearAll(std::move(roots))) {
        for (auto& entry : mImpl->instances)
            if (entry.second.root && !entry.second.controller->activate())
                LL_ERRS("UI") << "Component controller could not reactivate after account teardown rejection." << LL_ENDL;
        LL_WARNS("UI") << "Component host rejected account teardown; retaining the current component generation." << LL_ENDL;
        return false;
    }

    for (auto& entry : mImpl->instances) {
        auto& instance = entry.second;
        instance.root = nullptr;
        instance.controller.reset();
        instance.document.reset();
    }
    mImpl->instances.clear();
    mImpl->rootKeys.clear();
    mImpl->pendingEvictions.clear();
    return true;
}

void ComponentManager::idle() {
    mImpl->evictClosed();
}

void ComponentManager::reportReloadSucceeded() {
    for (const auto& [key, instance] : mImpl->instances)
        if (instance.root && !isClosed(*instance.root)) instance.controller->onReloadSucceeded();
}

void ComponentManager::reportReloadFailed(const DiagnosticResult& diagnostics) {
    for (const auto& [key, instance] : mImpl->instances)
        if (instance.root && !isClosed(*instance.root)) instance.controller->onReloadFailed(diagnostics);
}
} // namespace radia::viewer::ui
