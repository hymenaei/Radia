/**
 * @file componentmanager.h
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

#ifndef RD_COMPONENTMANAGER_H
#define RD_COMPONENTMANAGER_H

#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "componentidentity.h"
#include "diagnostic.h"
#include "system.h"

namespace radia::ui {
class Floater;
class SettingResolver;
class SkinGeneration;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::Floater;
using radia::ui::PublicationCommit;
using radia::ui::SettingResolver;
using radia::ui::SkinGeneration;
using radia::ui::System;

class ComponentController;

struct ComponentOpenResult : DiagnosticResult {
    Floater* floater = nullptr;
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

    private:
        friend class ComponentManager;
        struct State;
        explicit PreparedReplacement(std::unique_ptr<State> state);
        std::unique_ptr<State> mState;
    };

    struct ReplacementResult : DiagnosticResult {
        PreparedReplacement replacement;
        bool ok() const { return !hasErrors() && replacement; }
    };

    class Host {
    public:
        struct ReplacementRequest {
            Floater* current = nullptr;
            std::unique_ptr<Floater> replacement;
        };

        virtual ~Host() = default;
        virtual void mount(std::unique_ptr<Floater> root) = 0;
        virtual std::unique_ptr<Floater> unmount(Floater& root) = 0;
        virtual bool replaceAll(std::vector<ReplacementRequest> replacements) = 0;
        virtual bool clearAll(std::vector<Floater*> roots) = 0;
        virtual void present(Floater& root) = 0;
    };

    using ControllerFactory = std::function<std::unique_ptr<ComponentController>(System& system)>;

    ComponentManager(System& system, Host& host, SettingResolver& settingResolver);
    ~ComponentManager();
    ComponentManager(const ComponentManager&) = delete;
    ComponentManager& operator=(const ComponentManager&) = delete;

    bool registerDefinition(std::string definitionId, std::string resourceId, ControllerFactory factory);
    ComponentOpenResult open(const std::string& definitionId, const std::string& instanceKey = {});

    using OpenComponentCallback = std::function<void(const ComponentKey&, Floater&)>;
    void forEachOpen(const OpenComponentCallback& callback) const;
    std::optional<ComponentKey> componentKeyFor(const Floater& floater) const;
    ReplacementResult prepareReplacement(const SkinGeneration& generation, const std::string& locale);
    bool clearInstances();
    void idle();
    void reportReloadSucceeded();
    void reportReloadFailed(const DiagnosticResult& diagnostics);

private:
    struct Impl;
    std::shared_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
#endif // RD_COMPONENTMANAGER_H
