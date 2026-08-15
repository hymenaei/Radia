/**
 * @file resources.cpp
 * @brief Captures viewer skin resources and identifies the bundled default skin.
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
#include "skin/resources.h"
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
