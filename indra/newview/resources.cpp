/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "resources.h"
#include <filesystem>
#include "lldir.h"

namespace {
std::vector<std::filesystem::path> installedRoots() {
    if (!gDirUtilp) return {};
    return {
        gDirUtilp->getSkinBaseDir(),
        std::filesystem::path(gDirUtilp->getOSUserAppDir()) / "skins",
    };
}

bool samePath(const std::filesystem::path& left, const std::filesystem::path& right) {
    std::error_code leftError;
    std::error_code rightError;
    const std::filesystem::path canonicalLeft = std::filesystem::canonical(left, leftError);
    const std::filesystem::path canonicalRight = std::filesystem::canonical(right, rightError);
    return !leftError && !rightError && canonicalLeft == canonicalRight;
}
} // namespace

namespace radia::viewer::ui {
SkinSnapshotResult SkinResources::capture() const {
    if (!gDirUtilp) {
        SkinSnapshotResult result;
        result.error("skin.filesystem.unavailable", "Skin filesystem is unavailable.");
        return result;
    }
    return SkinResolver().resolve(gDirUtilp->getSkinDir(), installedRoots());
}

SkinSnapshotResult SkinResources::captureBundledDefault() const {
    if (!gDirUtilp) {
        SkinSnapshotResult result;
        result.error("skin.filesystem.unavailable", "Skin filesystem is unavailable.");
        return result;
    }
    return SkinResolver().resolve(gDirUtilp->getDefaultSkinDir(), {gDirUtilp->getSkinBaseDir()});
}

bool SkinResources::selectedIsBundledDefault() const {
    return gDirUtilp && samePath(gDirUtilp->getSkinDir(), gDirUtilp->getDefaultSkinDir());
}
} // namespace radia::viewer::ui
