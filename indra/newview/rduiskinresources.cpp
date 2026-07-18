#include "llviewerprecompiledheaders.h"
#include "rduiskinresources.h"
#include "lldir.h"
#include <filesystem>

namespace
{
    std::vector<std::filesystem::path> installedRoots()
    {
        if (!gDirUtilp) return {};
        return {
            gDirUtilp->getSkinBaseDir(),
            std::filesystem::path(gDirUtilp->getOSUserAppDir()) / "skins",
        };
    }

    bool samePath(const std::filesystem::path& left, const std::filesystem::path& right)
    {
        std::error_code left_error;
        std::error_code right_error;
        const std::filesystem::path canonical_left = std::filesystem::canonical(left, left_error);
        const std::filesystem::path canonical_right = std::filesystem::canonical(right, right_error);
        return !left_error && !right_error && canonical_left == canonical_right;
    }
}

namespace rdui::viewer
{
    SkinSnapshotResult SkinResources::capture() const
    {
        if (!gDirUtilp)
        {
            SkinSnapshotResult result;
            result.error("skin.filesystem.unavailable", "Skin filesystem is unavailable.");
            return result;
        }
        return SkinResolver().resolve(gDirUtilp->getSkinDir(), installedRoots());
    }

    SkinSnapshotResult SkinResources::captureBundledDefault() const
    {
        if (!gDirUtilp)
        {
            SkinSnapshotResult result;
            result.error("skin.filesystem.unavailable", "Skin filesystem is unavailable.");
            return result;
        }
        return SkinResolver().resolve(gDirUtilp->getDefaultSkinDir(), {gDirUtilp->getSkinBaseDir()});
    }

    bool SkinResources::selectedIsBundledDefault() const
    {
        return gDirUtilp && samePath(gDirUtilp->getSkinDir(), gDirUtilp->getDefaultSkinDir());
    }
}
