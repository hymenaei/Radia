/**
 * @file floaterstatestore.h
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

#ifndef LL_RDUI_FLOATER_STATE_STORE_H
#define LL_RDUI_FLOATER_STATE_STORE_H

#include <vector>
#include "floaterdocumentmanager.h"
#include "floaterplacementstore.h"

namespace rdui::viewer {
class FloaterStateStore final : private FloaterPlacementStore::Persistence {
public:
    std::vector<FloaterDocumentId> openDocuments() const;
    void saveOpenDocuments(const std::vector<FloaterDocumentId>& documents);

    FloaterPlacementStore::Persistence& placementPersistence() { return *this; }

private:
    LLSD read() const override;
    void write(const LLSD& placements) override;
};
} // namespace rdui::viewer
#endif // LL_RDUI_FLOATER_STATE_STORE_H
