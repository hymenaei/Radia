/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <utility>
#include <vector>
#include "componentmanager.h"
#include "documentcontrollerinternal.h"
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "resourceprovider.h"

namespace radia::viewer::ui {
using radia::ui::ElementRef;

struct ComponentManager::Impl final {
    enum class MutationMode { Normal, PublicationCommit, Cleanup };

    class MutationScope final {
    public:
        MutationScope(Impl& impl, MutationMode mode = MutationMode::Normal) : mImpl(impl), mOwner(impl.beginMutation(mode)) {}

        ~MutationScope() {
            if (mOwner) mImpl.endMutation();
        }

        MutationScope(const MutationScope&) = delete;
        MutationScope& operator=(const MutationScope&) = delete;

        explicit operator bool() const { return mOwner; }
        bool valid() const { return mOwner && !mImpl.mMutationRejected; }

    private:
        Impl& mImpl;
        bool mOwner = false;
    };

    struct OpenComponentSnapshot {
        ComponentInstanceKey key;
        ElementRef<HTMLFloaterElement> root;
    };

    struct Definition {
        radia::ui::ResourceId resource;
        ControllerFactory factory;
    };

    struct Instance {
        Instance(ComponentInstanceKey componentKey, radia::ui::ResourceId resource, std::unique_ptr<Document> document,
                 std::unique_ptr<DocumentController> controller)
            : componentKey(std::move(componentKey)), resource(std::move(resource)), document(std::move(document)), controller(std::move(controller)) {
        }

        ComponentInstanceKey componentKey;
        radia::ui::ResourceId resource;
        std::unique_ptr<Document> document;
        std::unique_ptr<DocumentController> controller;
        HTMLFloaterElement* root = nullptr;
        bool closeNotified = false;
    };

    struct RetainedMount {
        ComponentInstanceKey componentKey;
        std::unique_ptr<Document> document;
        std::unique_ptr<DocumentController> controller;
        ElementRef<HTMLFloaterElement> root;
    };

    Impl(System& system, Host& host, SettingResolver& resolver);
    ~Impl();

    System& system;
    Host& host;
    SettingResolver& settingResolver;
    std::map<std::string, Definition> definitions;
    std::map<ComponentInstanceKey, Instance> instances;
    std::map<const HTMLFloaterElement*, ComponentInstanceKey> rootKeys;
    std::set<ComponentInstanceKey> pendingEvictions;
    std::vector<RetainedMount> retainedMounts;
    std::uint64_t mutationEpoch = 0;
    bool mMutationActive = false;
    bool mMutationRejected = false;
    bool mPublicationReserved = false;
    bool mPublicationMutationRejected = false;
    bool mSuppressCloseControllerCallback = false;

    bool beginMutation(MutationMode mode);
    void endMutation();
    void rejectMutation();
    bool reservePublication();
    void releasePublication();
    bool retryRetainedMounts();
    bool unmountOrRetain(ComponentInstanceKey componentKey, std::unique_ptr<Document> document, std::unique_ptr<DocumentController> controller,
                         HTMLFloaterElement& root);
    bool discardMountedInstance(std::map<ComponentInstanceKey, Instance>::iterator found);
    std::vector<OpenComponentSnapshot> openSnapshot() const;
    void rootClosed(const ComponentInstanceKey& key, HTMLFloaterElement* root);
    bool evictClosed();
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
    std::uint64_t mutationEpoch = 0;
    bool publicationReserved = false;
    bool prepared = false;

    ~State();
    bool prepare();
    bool commit();
    bool prepareCandidate(ComponentManager::Impl& impl);
    bool validateCurrentState(const ComponentManager::Impl& impl);
    bool prepareComponents(ComponentManager::Impl& impl, ComponentManager::Impl::MutationScope& operation);
    bool validatePreparedState(const ComponentManager::Impl& impl);
    bool commitControllers();
    void deactivateReplacementControllers() const;
    bool reactivateCurrentControllers() const;
    bool replaceHost(ComponentManager::Impl& impl);
    bool rollbackHost(ComponentManager::Impl& impl) const;
    bool restoreCurrentState(ComponentManager::Impl& impl) const;
    bool currentRootsOpen(const ComponentManager::Impl& impl) const;
    bool activateComponents(ComponentManager::Impl& impl, ComponentManager::Impl::MutationScope& operation);
};

namespace componentmanager_internal {
inline void attachRootLifecycle(HTMLFloaterElement& root, std::function<void()> onClose = {}) {
    root.setLifecycleCallbacks({}, std::move(onClose));
}

inline bool isClosed(const HTMLFloaterElement& root) {
    return root.closed();
}

inline std::unique_ptr<Document> takeFloaterDocument(radia::ui::ResourceBuildResult buildResult, const std::string& rootError,
                                                     const std::string& sourceName, DiagnosticResult& result) {
    Document* document = buildResult.document.get();
    radia::ui::Element* root = document ? document->documentElement() : nullptr;
    HTMLFloaterElement* candidate = root ? dynamic_cast<HTMLFloaterElement*>(root) : nullptr;
    if (buildResult.ok() && !candidate) buildResult.error("component.root.type_mismatch", rootError, sourceName);
    const bool buildOk = buildResult.ok() && candidate;
    std::unique_ptr<Document> resultDocument = std::move(buildResult.document);
    result.append(std::move(buildResult));
    if (!buildOk) return {};
    return resultDocument;
}
} // namespace componentmanager_internal
} // namespace radia::viewer::ui
