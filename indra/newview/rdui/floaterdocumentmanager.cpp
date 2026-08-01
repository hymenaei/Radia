/**
 * @file floaterdocumentmanager.cpp
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

#include "llviewerprecompiledheaders.h"
#include "floaterdocumentmanager.h"
#include <map>
#include <utility>
#include "skin/generation.h"
#include "system.h"
#include "widgets/floater.h"

namespace rdui::viewer {
namespace {
std::string instanceId(const std::string& definition_id, const std::string& instance_key) {
    return instance_key.empty() ? definition_id : definition_id + "/" + instance_key;
}
} // namespace

struct FloaterDocumentManager::Impl {
    struct Instance {
        Instance(std::string definition_id, std::string instance_key, FloaterInstanceId instance_identity,
                 std::unique_ptr<FloaterController> instance_controller)
            : definitionId(std::move(definition_id)), instanceKey(std::move(instance_key)), identity(std::move(instance_identity)),
              controller(std::move(instance_controller)) {}

        std::string definitionId;
        std::string instanceKey;
        FloaterInstanceId identity;
        std::unique_ptr<FloaterController> controller;
        WidgetRef<Floater> floater;
    };

    Impl(System& system_value, Host& host_value) : system(system_value), host(host_value) {}

    void installReplacement(const std::string& identity, std::unique_ptr<Floater> replacement) {
        const auto found = instances.find(identity);
        if (found == instances.end() || !replacement) return;
        Instance& instance = *found->second;
        Floater* current = instance.floater.get();
        if (!current) return;
        instance.floater.set(host.replace(instance.identity, *current, std::move(replacement)));
    }

    System& system;
    Host& host;
    std::map<std::string, ControllerFactory> definitions;
    std::map<std::string, std::unique_ptr<Instance>> instances;
};

FloaterDocumentManager::FloaterDocumentManager(System& system, Host& host) : mImpl(std::make_unique<Impl>(system, host)) {}

FloaterDocumentManager::~FloaterDocumentManager() = default;

FloaterDocumentManager::PreparedReplacement::PreparedReplacement(std::function<void()> commit) : mCommit(std::move(commit)) {}

FloaterDocumentManager::PreparedReplacement::~PreparedReplacement() = default;
FloaterDocumentManager::PreparedReplacement::PreparedReplacement(PreparedReplacement&&) noexcept = default;
FloaterDocumentManager::PreparedReplacement& FloaterDocumentManager::PreparedReplacement::operator=(PreparedReplacement&&) noexcept = default;

bool FloaterDocumentManager::PreparedReplacement::commit() {
    if (!mCommit) return false;
    std::function<void()> commit = std::exchange(mCommit, {});
    commit();
    return true;
}

bool FloaterDocumentManager::registerDefinition(std::string definition_id, ControllerFactory factory) {
    if (definition_id.empty() || definition_id.find('/') != std::string::npos || !factory) return false;
    return mImpl->definitions.emplace(std::move(definition_id), std::move(factory)).second;
}

FloaterDocumentOpenResult FloaterDocumentManager::open(const std::string& definition_id, const std::string& instance_key) {
    FloaterDocumentOpenResult result;
    const auto definition = mImpl->definitions.find(definition_id);
    if (definition == mImpl->definitions.end()) {
        result.error("floater.definition.missing", "Unknown RDUI Floater definition: " + definition_id + ".");
        return result;
    }

    const std::string identity_value = instanceId(definition_id, instance_key);
    if (const auto existing = mImpl->instances.find(identity_value); existing != mImpl->instances.end() && existing->second->floater) {
        result.floater = existing->second->floater.get();
        result.floater->open();
        mImpl->host.show(*result.floater);
        return result;
    }

    std::unique_ptr<FloaterController> controller = definition->second(mImpl->system);
    if (!controller) {
        result.error("floater.controller.missing", "RDUI Floater controller factory returned no controller: " + definition_id + ".");
        return result;
    }

    const std::string resource_id = controller->resourceId();
    ViewBuildResult view = mImpl->system.createView(resource_id);
    Floater* candidate = view.rootAs<Floater>();
    if (view.ok() && !candidate) view.error("view.root.type_mismatch", "RDUI Floater definition must have a <floater> root.", resource_id);
    const bool view_ok = view.ok() && candidate;
    result.append(std::move(view));
    if (!view_ok) return result;

    PreparedBindingResult binding = controller->prepareBindings(*candidate);
    const bool binding_ok = binding.ok();
    result.append(std::move(binding));
    if (!binding_ok) return result;

    auto instance = std::make_unique<Impl::Instance>(definition_id, instance_key, FloaterInstanceId(identity_value), std::move(controller));
    std::unique_ptr<Floater> floater(static_cast<Floater*>(view.root.release()));
    result.floater = mImpl->host.mount(instance->identity, std::move(floater));
    if (!result.floater) {
        result.error("floater.host.mount_failed", "RDUI Floater host rejected the instance: " + identity_value + ".");
        return result;
    }

    instance->floater.set(result.floater);
    instance->controller->commitBindings(std::move(binding.binding));
    mImpl->instances.insert_or_assign(identity_value, std::move(instance));
    return result;
}

const FloaterInstanceId* FloaterDocumentManager::identity(const Floater& floater) const {
    for (const auto& [key, instance] : mImpl->instances)
        if (instance->floater.get() == &floater) return &instance->identity;
    return nullptr;
}

std::vector<Floater*> FloaterDocumentManager::floaters() const {
    std::vector<Floater*> result;
    result.reserve(mImpl->instances.size());
    for (const auto& [key, instance] : mImpl->instances)
        if (instance->floater) result.push_back(instance->floater.get());
    return result;
}

std::vector<FloaterDocumentId> FloaterDocumentManager::openDocuments() const {
    std::vector<FloaterDocumentId> result;
    result.reserve(mImpl->instances.size());
    for (const auto& [key, instance] : mImpl->instances)
        if (instance->floater && !instance->floater->closed()) result.push_back({instance->definitionId, instance->instanceKey});
    return result;
}

FloaterDocumentManager::ReplacementResult FloaterDocumentManager::prepareReplacement(const SkinGeneration& generation, const std::string& locale) {
    struct PendingDocument {
        std::string identity;
        std::unique_ptr<Floater> floater;
        PreparedBinding binding;
        FloaterController* controller = nullptr;
    };

    struct PendingCommit {
        std::vector<PendingDocument> documents;
    };

    ReplacementResult result;
    auto pending = std::make_shared<PendingCommit>();
    pending->documents.reserve(mImpl->instances.size());
    for (const auto& [identity, instance] : mImpl->instances) {
        if (!instance->floater) continue;

        FloaterController& controller = *instance->controller;
        const std::string resource_id = controller.resourceId();
        ViewBuildResult view = generation.createView(resource_id, locale);
        Floater* candidate = view.rootAs<Floater>();
        if (view.ok() && !candidate) view.error("view.root.type_mismatch", "Reloaded View must have a <floater> root.", resource_id);
        const bool view_ok = view.ok() && candidate;
        result.append(std::move(view));
        if (!view_ok) return result;

        PreparedBindingResult binding = controller.prepareBindings(*candidate);
        const bool binding_ok = binding.ok();
        result.append(std::move(binding));
        if (!binding_ok) return result;

        pending->documents.push_back(
            {identity, std::unique_ptr<Floater>(static_cast<Floater*>(view.root.release())), std::move(binding.binding), &controller});
    }

    Impl* impl = mImpl.get();
    result.replacement = PreparedReplacement([impl, pending] {
        for (PendingDocument& document : pending->documents) impl->installReplacement(document.identity, std::move(document.floater));
        for (PendingDocument& document : pending->documents) document.controller->commitBindings(std::move(document.binding));
    });
    return result;
}

void FloaterDocumentManager::idle() {
    for (const auto& [key, instance] : mImpl->instances) instance->controller->idle();
}

void FloaterDocumentManager::reportReloadSucceeded() {
    for (const auto& [key, instance] : mImpl->instances) instance->controller->reportReloadSucceeded();
}

void FloaterDocumentManager::reportReloadFailed(const DiagnosticResult& diagnostics) {
    for (const auto& [key, instance] : mImpl->instances) instance->controller->reportReloadFailed(diagnostics);
}

std::size_t FloaterDocumentManager::size() const {
    return mImpl->instances.size();
}
} // namespace rdui::viewer
