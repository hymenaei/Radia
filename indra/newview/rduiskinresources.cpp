/**
 * @file rduiskinresources.cpp
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

#include "llviewerprecompiledheaders.h"
#include "rduiskinresources.h"
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
    std::error_code left_error;
    std::error_code right_error;
    const std::filesystem::path canonical_left = std::filesystem::canonical(left, left_error);
    const std::filesystem::path canonical_right = std::filesystem::canonical(right, right_error);
    return !left_error && !right_error && canonical_left == canonical_right;
}
} // namespace

namespace rdui::viewer {
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
} // namespace rdui::viewer
