/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/Diagnostic.h>
#include <Core/System.h>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "ComponentInstanceKey.h"

namespace Core {
class Document;
class HTMLFloaterElement;
class SettingResolver;
class SkinGeneration;
} // namespace Core

namespace Viewer {
class DocumentController;

struct ComponentOpenResult : Core::DiagnosticResult {
    Core::HTMLFloaterElement* floater = nullptr;
    bool ok() const { return !hasErrors() && floater; }
};

class ComponentManager final {
public:
    class PreparedReplacement final : public Core::PublicationCommit {
    public:
        PreparedReplacement() = default;
        ~PreparedReplacement();
        PreparedReplacement(PreparedReplacement&&) noexcept;
        PreparedReplacement& operator=(PreparedReplacement&&) noexcept;
        PreparedReplacement(const PreparedReplacement&) = delete;
        PreparedReplacement& operator=(const PreparedReplacement&) = delete;

        explicit operator bool() const { return static_cast<bool>(mState); }
        bool prepare() override;
        bool commit() override;
        void finalize() override;
        Core::DiagnosticResult takeDiagnostics();

    private:
        friend class ComponentManager;
        struct State;
        explicit PreparedReplacement(std::unique_ptr<State> state);
        std::unique_ptr<State> mState;
        Core::DiagnosticResult mDiagnostics;
    };

    struct ReplacementResult : Core::DiagnosticResult {
        PreparedReplacement replacement;
        bool ok() const { return !hasErrors() && replacement; }
    };

    class Host {
    public:
        struct ReplacementRequest {
            Core::HTMLFloaterElement* current = nullptr;
            Core::Document* replacement = nullptr;
        };

        virtual ~Host() = default;
        virtual bool mount(Core::Document& document) = 0;
        virtual bool unmount(Core::HTMLFloaterElement& root) = 0;
        virtual bool replaceAll(std::vector<ReplacementRequest> replacements) = 0;
        virtual bool clearAll(std::vector<Core::HTMLFloaterElement*> roots) = 0;
        virtual void present(Core::HTMLFloaterElement& root) = 0;
    };

    using ControllerFactory = std::function<std::unique_ptr<DocumentController>(Core::System& system, Core::Document& document)>;

    ComponentManager(Core::System& system, Host& host, Core::SettingResolver& settingResolver);
    ~ComponentManager();
    ComponentManager(const ComponentManager&) = delete;
    ComponentManager& operator=(const ComponentManager&) = delete;

    bool registerDefinition(std::string definitionId, std::string resource, ControllerFactory factory);
    ComponentOpenResult open(const std::string& definitionId, const std::string& instanceKey = {});

    using OpenComponentCallback = std::function<void(const ComponentInstanceKey&, Core::HTMLFloaterElement&)>;
    void forEachOpen(const OpenComponentCallback& callback) const;
    std::optional<ComponentInstanceKey> componentKeyFor(const Core::HTMLFloaterElement& floater) const;
    ReplacementResult prepareReplacement(std::shared_ptr<const Core::SkinGeneration> generation, std::string locale);
    bool clearInstances();
    void idle();
    void reportReloadSucceeded();
    void reportReloadFailed(const Core::DiagnosticResult& diagnostics);

private:
    struct Impl;
    std::shared_ptr<Impl> mImpl;
};
} // namespace Viewer
