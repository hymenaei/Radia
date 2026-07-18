#ifndef LL_RDUI_SKIN_RELOAD_COORDINATOR_H
#define LL_RDUI_SKIN_RELOAD_COORDINATOR_H

#include "rduibinder.h"
#include "rduidiagnostic.h"

#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace rdui
{
    class Floater;
    class System;
    class ResourceSnapshot;

    namespace viewer
    {
        class SkinSnapshotSource;

        class ReloadableFloater
        {
            public:
                virtual ~ReloadableFloater() = default;
                virtual std::string reloadResourceId() const = 0;
                virtual PreparedBindingResult prepareBindings(Floater& floater) = 0;
                virtual void commitBindings(PreparedBinding&& binding) = 0;
                virtual void idle() {}
                virtual void reportReloadSucceeded() {}
                virtual void reportReloadFailed(const DiagnosticResult&) {}
        };

        struct FloaterReloadTarget
        {
            ReloadableFloater* document = nullptr;
            std::function<void(std::unique_ptr<Floater>)> install;
        };

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
                    TimePoint now, const std::vector<FloaterReloadTarget>& targets);

            private:
                class Impl;
                std::unique_ptr<Impl> mImpl;
        };
    }
}

#endif // LL_RDUI_SKIN_RELOAD_COORDINATOR_H
