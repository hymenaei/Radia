/**
 * @file reloadcoordinator.h
 * @brief Coordinates debounced skin reloads and commits complete UI skin generations.
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

#ifndef RD_SKIN_RELOADCOORDINATOR_H
#define RD_SKIN_RELOADCOORDINATOR_H

#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>
#include "diagnostic.h"

namespace rdui {
class System;
class ResourceSnapshot;

namespace viewer {
class SkinSnapshotSource;
class FloaterDocumentManager;

struct SkinReloadResult : DiagnosticResult {
    bool committed = false;
    std::uint64_t generation = 0;

    bool ok() const { return committed && !hasErrors(); }
};

class SkinReloadCoordinator final {
public:
    using TimePoint = std::chrono::steady_clock::time_point;
    SkinReloadCoordinator(System& system, const SkinSnapshotSource& snapshots);
    ~SkinReloadCoordinator();

    SkinReloadCoordinator(const SkinReloadCoordinator&) = delete;
    SkinReloadCoordinator& operator=(const SkinReloadCoordinator&) = delete;

    void setAuthoringEnabled(bool enabled);
    void request();
    std::optional<SkinReloadResult> update(TimePoint now, FloaterDocumentManager& documents);

private:
    class Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace viewer
} // namespace rdui
#endif // RD_SKIN_RELOADCOORDINATOR_H
