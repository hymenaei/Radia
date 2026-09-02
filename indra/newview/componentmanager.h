/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "componentinstancekey.h"
#include "diagnostic.h"
#include "system.h"

namespace radia::ui {
class Document;
class HTMLFloaterElement;
class SettingResolver;
class SkinGeneration;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::HTMLFloaterElement;
using radia::ui::PublicationCommit;
using radia::ui::SettingResolver;
using radia::ui::SkinGeneration;
using radia::ui::System;

class DocumentController;

struct ComponentOpenResult : DiagnosticResult {
    HTMLFloaterElement* floater = nullptr;
    bool ok() const { return !hasErrors() && floater; }
};

class ComponentManager final {
public:
    class PreparedReplacement final : public PublicationCommit {
    public:
        PreparedReplacement() = default;
        ~PreparedReplacement();
        PreparedReplacement(PreparedReplacement&&) noexcept;
        PreparedReplacement& operator=(PreparedReplacement&&) noexcept;
        PreparedReplacement(const PreparedReplacement&) = delete;
        PreparedReplacement& operator=(const PreparedReplacement&) = delete;

        explicit operator bool() const { return static_cast<bool>(mState); }
        bool commit() override;
        DiagnosticResult takeDiagnostics();

    private:
        friend class ComponentManager;
        struct State;
        explicit PreparedReplacement(std::unique_ptr<State> state);
        std::unique_ptr<State> mState;
        DiagnosticResult mDiagnostics;
    };

    struct ReplacementResult : DiagnosticResult {
        PreparedReplacement replacement;
        bool ok() const { return !hasErrors() && replacement; }
    };

    class Host {
    public:
        struct ReplacementRequest {
            HTMLFloaterElement* current = nullptr;
            Document* replacement = nullptr;
        };

        virtual ~Host() = default;
        virtual void mount(Document& document) = 0;
        virtual bool unmount(HTMLFloaterElement& root) = 0;
        virtual bool replaceAll(std::vector<ReplacementRequest> replacements) = 0;
        virtual bool clearAll(std::vector<HTMLFloaterElement*> roots) = 0;
        virtual void present(HTMLFloaterElement& root) = 0;
    };

    using ControllerFactory = std::function<std::unique_ptr<DocumentController>(System& system, Document& document)>;

    ComponentManager(System& system, Host& host, SettingResolver& settingResolver);
    ~ComponentManager();
    ComponentManager(const ComponentManager&) = delete;
    ComponentManager& operator=(const ComponentManager&) = delete;

    bool registerDefinition(std::string definitionId, std::string resource, ControllerFactory factory);
    ComponentOpenResult open(const std::string& definitionId, const std::string& instanceKey = {});

    using OpenComponentCallback = std::function<void(const ComponentInstanceKey&, HTMLFloaterElement&)>;
    void forEachOpen(const OpenComponentCallback& callback) const;
    std::optional<ComponentInstanceKey> componentKeyFor(const HTMLFloaterElement& floater) const;
    ReplacementResult prepareReplacement(std::shared_ptr<const SkinGeneration> generation, std::string locale);
    bool clearInstances();
    void idle();
    void reportReloadSucceeded();
    void reportReloadFailed(const DiagnosticResult& diagnostics);

private:
    struct Impl;
    std::shared_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
