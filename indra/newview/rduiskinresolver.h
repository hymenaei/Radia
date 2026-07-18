#ifndef LL_RDUI_SKIN_RESOLVER_H
#define LL_RDUI_SKIN_RESOLVER_H

#include "rduidiagnostic.h"
#include "rduiresourceprovider.h"

#include <filesystem>
#include <string>
#include <vector>

namespace rdui::viewer
{
    struct SkinSnapshotResult : DiagnosticResult
    {
        bool ok() const { return !hasErrors(); }

        ResourceSnapshot snapshot;
        std::string skin_id;
    };

    class SkinSnapshotSource
    {
        public:
            virtual ~SkinSnapshotSource() = default;
            virtual SkinSnapshotResult capture() const = 0;
    };

    class SkinResolver final
    {
        public:
            SkinSnapshotResult resolve(
                const std::filesystem::path& selected_root,
                const std::vector<std::filesystem::path>& installed_roots) const;
    };
}

#endif // LL_RDUI_SKIN_RESOLVER_H
