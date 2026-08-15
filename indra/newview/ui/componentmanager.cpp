/**
 * @file componentmanager.cpp
 * @brief Manages viewer-owned component instances, bindings, and skin replacement.
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
#include "componentmanager.h"
#include <map>
#include <set>
#include <utility>
#include "binding/settingresolver.h"
#include "componentcontrollerinternal.h"
#include "skin/generation.h"
#include "system.h"
#include "widgets/floater.h"

namespace radia::viewer::ui {
using namespace ::radia::ui;
namespace {
void attachRootLifecycle(Floater& root, ComponentController& controller, std::function<void()> onClose = {}) {
    root.setLifecycleCallbacks({}, [&controller, onClose = std::move(onClose)] {
        controller.onClose();
        if (onClose) onClose();
    });
}

bool isClosed(const Floater& root) {
    return root.closed();
}

std::unique_ptr<Floater> takeFloaterRoot(LayoutBuildResult layout, const std::string& rootError, const std::string& source,
                                         DiagnosticResult& result) {
    radia::ui::Widget* root = layout.root.get();
    Floater* candidate = root ? dynamic_cast<Floater*>(root) : nullptr;
    if (layout.ok() && !candidate) layout.error("component.root.type_mismatch", rootError, source);
    const bool layoutOk = layout.ok() && candidate;
    result.append(std::move(layout));
    if (!layoutOk) return {};
    return std::unique_ptr<Floater>(static_cast<Floater*>(layout.root.release()));
}
} // namespace

struct ComponentManager::Impl final {
    struct Definition {
        std::string resourceId;
        ControllerFactory factory;
    };

    struct Instance {
        Instance(ComponentKey componentKey, std::string resourceId, std::unique_ptr<ComponentController> controller)
            : componentKey(std::move(componentKey)), resourceId(std::move(resourceId)), controller(std::move(controller)) {}

        ComponentKey componentKey;
        std::string resourceId;
        std::unique_ptr<ComponentController> controller;
        WidgetRef<Floater> root;
    };

    Impl(System& system, Host& host, SettingResolver& resolver) : system(system), host(host), settingResolver(resolver) {}

    System& system;
    Host& host;
    SettingResolver& settingResolver;
    std::map<std::string, Definition> definitions;
    std::map<ComponentKey, Instance> instances;
    std::map<const Floater*, ComponentKey> rootKeys;
    std::set<ComponentKey> pendingEvictions;

    void evictClosed() {
        for (auto pending = pendingEvictions.begin(); pending != pendingEvictions.end();) {
            const auto found = instances.find(*pending);
            if (found == instances.end() || !found->second.root || !isClosed(*found->second.root)) {
                pending = pendingEvictions.erase(pending);
                continue;
            }

            Floater* root = found->second.root.get();
            std::unique_ptr<Floater> retired = host.unmount(*root);
            if (!retired) {
                LL_WARNS("UI") << "Closed component could not be unmounted: " << pending->persistenceKey() << LL_ENDL;
                ++pending;
                continue;
            }
            if (retired.get() != root)
                LL_ERRS("UI") << "Component host returned the wrong root while unmounting: " << pending->persistenceKey() << LL_ENDL;

            rootKeys.erase(root);
            found->second.root.set(nullptr);
            instances.erase(found);
            pending = pendingEvictions.erase(pending);
        }
    }
};

struct ComponentManager::PreparedReplacement::State {
    struct PendingComponent {
        ComponentKey componentKey;
        std::unique_ptr<Floater> replacement;
        ComponentController::PreparedMount mount;
        ComponentManager::Impl::Instance* instance = nullptr;
        Floater* current = nullptr;
        Floater* candidate = nullptr;
    };

    std::weak_ptr<ComponentManager::Impl> manager;
    std::vector<PendingComponent> components;

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
    std::unique_ptr<State> state = std::move(mState);
    return state && state->commit();
}

bool ComponentManager::PreparedReplacement::State::commit() {
    const std::shared_ptr<ComponentManager::Impl> impl = manager.lock();
    if (!impl) return false;

    std::vector<ComponentManager::Host::ReplacementRequest> requests;
    requests.reserve(components.size());
    for (PendingComponent& component : components) {
        const auto found = impl->instances.find(component.componentKey);
        ComponentManager::Impl::Instance* instance = found == impl->instances.end() ? nullptr : &found->second;
        ComponentController* controller = instance ? instance->controller.get() : nullptr;
        if (found == impl->instances.end()
            || instance != component.instance
            || !instance->root
            || instance->root.get() != component.current
            || !controller
            || !component.replacement
            || !component.candidate
            || !controller->canCommit(component.mount, *component.candidate))
            return false;
        requests.push_back({component.current, std::move(component.replacement)});
    }

    if (!impl->host.replaceAll(std::move(requests))) return false;

    const std::weak_ptr<ComponentManager::Impl> weakManager = impl;
    for (PendingComponent& component : components) {
        ComponentManager::Impl::Instance& instance = *component.instance;
        ComponentController& controller = *instance.controller;
        controller.commit(std::move(component.mount));
        Floater* current = instance.root.get();
        Floater* root = component.candidate;
        impl->rootKeys.erase(current);
        impl->rootKeys[root] = instance.componentKey;
        instance.root.set(root);
        const ComponentKey closedComponentKey = instance.componentKey;
        attachRootLifecycle(*root, controller, [weakManager, closedComponentKey] {
            if (const std::shared_ptr<ComponentManager::Impl> impl = weakManager.lock()) impl->pendingEvictions.insert(closedComponentKey);
        });
    }
    return true;
}

bool ComponentManager::registerDefinition(std::string definitionId, std::string resourceId, ControllerFactory factory) {
    if (!ComponentKey{definitionId, {}}.valid() || resourceId.empty() || !factory) return false;
    return mImpl->definitions.emplace(std::move(definitionId), Impl::Definition{std::move(resourceId), std::move(factory)}).second;
}

ComponentOpenResult ComponentManager::open(const std::string& definitionId, const std::string& instanceKey) {
    ComponentOpenResult result;
    const auto definition = mImpl->definitions.find(definitionId);
    if (definition == mImpl->definitions.end()) {
        result.error("floater.definition.missing", "Unknown Floater definition: " + definitionId + ".");
        return result;
    }

    const ComponentKey component{definitionId, instanceKey};
    if (!component.valid()) {
        result.error("floater.identity.invalid", "Invalid component identity for " + definitionId + ".");
        return result;
    }

    const std::string persistenceKey = component.persistenceKey();
    if (const auto existing = mImpl->instances.find(component); existing != mImpl->instances.end() && existing->second.root) {
        mImpl->pendingEvictions.erase(component);
        result.floater = existing->second.root.get();
        mImpl->host.present(*result.floater);
        existing->second.controller->onOpen();
        return result;
    }

    std::unique_ptr<ComponentController> controller = definition->second.factory(mImpl->system);
    if (!controller) {
        result.error("floater.controller.missing", "Floater controller factory returned no controller: " + definitionId + ".");
        return result;
    }

    const std::string& resourceId = definition->second.resourceId;
    ComponentController::PreparedMountResult prepared;
    std::unique_ptr<Floater> ownedFloater =
        takeFloaterRoot(mImpl->system.buildWidgetTree(resourceId), "Component definition must have a <floater> root.", resourceId, result);
    if (!ownedFloater) return result;
    Floater* floater = ownedFloater.get();
    prepared = controller->prepare(*floater, mImpl->settingResolver);
    const bool preparedOk = prepared.ok();
    result.append(std::move(prepared));
    if (!preparedOk) return result;

    if (!controller->canCommit(prepared.mount, *floater)) {
        result.error("floater.controller.commit_invalid", "Floater controller prepared an invalid mount: " + persistenceKey + ".");
        return result;
    }

    Impl::Instance instance(component, resourceId, std::move(controller));
    mImpl->host.mount(std::move(ownedFloater));
    result.floater = floater;

    mImpl->rootKeys[result.floater] = component;
    instance.root.set(result.floater);
    const std::weak_ptr<Impl> weakImpl = mImpl;
    const ComponentKey closedComponentKey = component;
    attachRootLifecycle(*result.floater, *instance.controller, [weakImpl, closedComponentKey] {
        if (const std::shared_ptr<Impl> impl = weakImpl.lock()) impl->pendingEvictions.insert(closedComponentKey);
    });
    instance.controller->commit(std::move(prepared.mount));
    ComponentController* mountedController = instance.controller.get();
    mImpl->instances.insert_or_assign(component, std::move(instance));
    mImpl->host.present(*result.floater);
    mountedController->onOpen();
    return result;
}

void ComponentManager::forEachOpen(const OpenComponentCallback& callback) const {
    if (!callback) return;
    for (const auto& [key, instance] : mImpl->instances)
        if (Floater* floater = instance.root.get(); floater && !isClosed(*floater)) callback(key, *floater);
}

std::optional<ComponentKey> ComponentManager::componentKeyFor(const Floater& floater) const {
    const auto found = mImpl->rootKeys.find(&floater);
    if (found == mImpl->rootKeys.end()) return std::nullopt;
    return found->second;
}

ComponentManager::ReplacementResult ComponentManager::prepareReplacement(const SkinGeneration& generation, const std::string& locale) {
    ReplacementResult result;
    auto pending = std::make_unique<PreparedReplacement::State>();
    pending->manager = mImpl;
    pending->components.reserve(mImpl->instances.size());
    for (auto& [componentKey, instance] : mImpl->instances) {
        if (!instance.root || isClosed(*instance.root)) continue;

        ComponentController& controller = *instance.controller;
        ComponentController::PreparedMountResult prepared;
        std::unique_ptr<Floater> replacement = takeFloaterRoot(generation.buildWidgetTree(instance.resourceId, locale),
                                                               "Reloaded Component must have a <floater> root.", instance.resourceId, result);
        if (!replacement) return result;
        Floater* candidate = replacement.get();
        prepared = controller.prepare(*candidate, mImpl->settingResolver);
        const bool preparedOk = prepared.ok();
        result.append(std::move(prepared));
        if (!preparedOk) return result;

        PreparedReplacement::State::PendingComponent pendingComponent;
        pendingComponent.componentKey = componentKey;
        pendingComponent.replacement = std::move(replacement);
        pendingComponent.mount = std::move(prepared.mount);
        pendingComponent.instance = &instance;
        pendingComponent.current = instance.root.get();
        pendingComponent.candidate = candidate;
        pending->components.push_back(std::move(pendingComponent));
    }

    result.replacement = PreparedReplacement(std::move(pending));
    return result;
}

bool ComponentManager::clearInstances() {
    std::vector<Floater*> roots;
    roots.reserve(mImpl->instances.size());
    for (auto& entry : mImpl->instances)
        if (Floater* root = entry.second.root.get()) roots.push_back(root);

    if (!mImpl->host.clearAll(std::move(roots))) {
        LL_WARNS("UI") << "Component host rejected account teardown; retaining the current component generation." << LL_ENDL;
        return false;
    }

    for (auto& entry : mImpl->instances) {
        auto& instance = entry.second;
        instance.root.set(nullptr);
        instance.controller.reset();
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
