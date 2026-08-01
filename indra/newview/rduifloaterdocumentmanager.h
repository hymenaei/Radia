/**
 * @file rduifloaterdocumentmanager.h
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

#ifndef LL_RDUI_FLOATER_DOCUMENT_MANAGER_H
#define LL_RDUI_FLOATER_DOCUMENT_MANAGER_H

#include <functional>
#include <memory>
#include <string>
#include <vector>
#include "rduidiagnostic.h"
#include "rduifloatercontroller.h"
#include "rduifloaterplacementstore.h"

namespace rdui {
class Floater;
class SkinGeneration;
class System;
} // namespace rdui

namespace rdui::viewer {
struct FloaterDocumentId {
    std::string definitionId;
    std::string instanceKey;

    bool operator==(const FloaterDocumentId&) const = default;
};

struct FloaterDocumentOpenResult : DiagnosticResult {
    Floater* floater = nullptr;
    bool ok() const { return !hasErrors() && floater; }
};

class FloaterDocumentManager final {
public:
    class PreparedReplacement final {
    public:
        PreparedReplacement() = default;
        ~PreparedReplacement();
        PreparedReplacement(PreparedReplacement&&) noexcept;
        PreparedReplacement& operator=(PreparedReplacement&&) noexcept;
        PreparedReplacement(const PreparedReplacement&) = delete;
        PreparedReplacement& operator=(const PreparedReplacement&) = delete;

        explicit operator bool() const { return static_cast<bool>(mCommit); }
        bool commit();

    private:
        friend class FloaterDocumentManager;
        explicit PreparedReplacement(std::function<void()> commit);
        std::function<void()> mCommit;
    };

    struct ReplacementResult : DiagnosticResult {
        PreparedReplacement replacement;
        bool ok() const { return !hasErrors() && replacement; }
    };

    class Host {
    public:
        virtual ~Host() = default;
        virtual Floater* mount(const FloaterInstanceId& identity, std::unique_ptr<Floater> floater) = 0;
        virtual Floater* replace(const FloaterInstanceId& identity, Floater& current, std::unique_ptr<Floater> replacement) = 0;
        virtual void show(Floater& floater) = 0;
    };

    using ControllerFactory = std::function<std::unique_ptr<FloaterController>(System& system)>;

    FloaterDocumentManager(System& system, Host& host);
    ~FloaterDocumentManager();
    FloaterDocumentManager(const FloaterDocumentManager&) = delete;
    FloaterDocumentManager& operator=(const FloaterDocumentManager&) = delete;

    bool registerDefinition(std::string definition_id, ControllerFactory factory);
    FloaterDocumentOpenResult open(const std::string& definition_id, const std::string& instance_key = {});

    const FloaterInstanceId* identity(const Floater& floater) const;
    std::vector<Floater*> floaters() const;
    std::vector<FloaterDocumentId> openDocuments() const;
    ReplacementResult prepareReplacement(const SkinGeneration& generation, const std::string& locale);
    void idle();
    void reportReloadSucceeded();
    void reportReloadFailed(const DiagnosticResult& diagnostics);
    std::size_t size() const;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui::viewer
#endif // LL_RDUI_FLOATER_DOCUMENT_MANAGER_H
