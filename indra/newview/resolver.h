/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <filesystem>
#include <string>
#include <vector>
#include "diagnostic.h"
#include "resourceprovider.h"

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::ResourceSnapshot;

struct SkinSnapshotResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }

    ResourceSnapshot snapshot;
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
} // namespace radia::viewer::ui
