#ifndef LL_RDUI_SKIN_RELOAD_COORDINATOR_H
#define LL_RDUI_SKIN_RELOAD_COORDINATOR_H

#include "rduidiagnostic.h"

#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>

namespace rdui
{
    class System;
    class ResourceSnapshot;

    namespace viewer
    {
        class SkinSnapshotSource;
        class FloaterDocumentManager;

        struct SkinReloadResult : DiagnosticResult
        {
            bool committed = false;
            std::uint64_t generation = 0;

            bool ok() const { return committed && !hasErrors(); }
        };

        class SkinReloadCoordinator final
        {
            public:
                using TimePoint = std::chrono::steady_clock::time_point;
                SkinReloadCoordinator(System& system, const SkinSnapshotSource& snapshots);
                ~SkinReloadCoordinator();

                SkinReloadCoordinator(const SkinReloadCoordinator&) = delete;
                SkinReloadCoordinator& operator=(const SkinReloadCoordinator&) = delete;

                void setAuthoringEnabled(bool enabled);
                void request();
                std::optional<SkinReloadResult> update(
                    TimePoint now, FloaterDocumentManager& documents);

            private:
                class Impl;
                std::unique_ptr<Impl> mImpl;
        };
    }
}

#endif // LL_RDUI_SKIN_RELOAD_COORDINATOR_H
