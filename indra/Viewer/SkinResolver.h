/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <filesystem>
#include <vector>
#include "SkinSnapshot.h"

namespace Viewer {
class SkinResolver final {
public:
    SkinSnapshotResult resolve(const std::filesystem::path& selectedRoot, const std::vector<std::filesystem::path>& installedRoots) const;
};
} // namespace Viewer
