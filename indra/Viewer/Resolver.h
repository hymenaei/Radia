/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/Diagnostic.h>
#include <Core/ResourceProvider.h>
#include <filesystem>
#include <string>
#include <vector>

namespace Viewer {
struct SkinSnapshotResult : Core::DiagnosticResult {
    bool ok() const { return !hasErrors(); }

    Core::ResourceSnapshot snapshot;
    std::string skinId;
};

class SkinSnapshotSource {
public:
    virtual ~SkinSnapshotSource() = default;
    virtual SkinSnapshotResult capture() const = 0;
};

class SkinResolver final {
public:
    SkinSnapshotResult resolve(const std::filesystem::path& selectedRoot, const std::vector<std::filesystem::path>& installedRoots) const;
};
} // namespace Viewer
