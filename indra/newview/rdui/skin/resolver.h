/**
 * @file resolver.h
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

#ifndef LL_RDUI_SKIN_RESOLVER_H
#define LL_RDUI_SKIN_RESOLVER_H

#include <filesystem>
#include <string>
#include <vector>
#include "diagnostic.h"
#include "resourceprovider.h"

namespace rdui::viewer {
struct SkinSnapshotResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }

    ResourceSnapshot snapshot;
    std::string skin_id;
};

class SkinSnapshotSource {
public:
    virtual ~SkinSnapshotSource() = default;
    virtual SkinSnapshotResult capture() const = 0;
};

class SkinResolver final {
public:
    SkinSnapshotResult resolve(const std::filesystem::path& selected_root, const std::vector<std::filesystem::path>& installed_roots) const;
};
} // namespace rdui::viewer
#endif // LL_RDUI_SKIN_RESOLVER_H
