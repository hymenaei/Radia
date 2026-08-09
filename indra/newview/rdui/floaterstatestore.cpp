/**
 * @file floaterstatestore.cpp
 * @brief Persists open Floater documents and their placement state through viewer settings.
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
#include "floaterstatestore.h"
#include <utility>
#include "llviewercontrol.h"

namespace rdui::viewer {
std::vector<FloaterDocumentId> FloaterStateStore::openDocuments() const {
    std::vector<FloaterDocumentId> result;
    const LLSD saved = gSavedSettings.getLLSD("RduiOpenFloaters");
    if (!saved.isArray()) {
        LL_WARNS("rdui") << "RduiOpenFloaters is not an array; ignoring it." << LL_ENDL;
        return result;
    }

    for (LLSD::array_const_iterator entry = saved.beginArray(); entry != saved.endArray(); ++entry) {
        if (!entry->isMap()) continue;
        const std::string definition_id = (*entry)["definition"].asString();
        if (definition_id.empty()) continue;
        result.push_back({definition_id, (*entry)["instance"].asString()});
    }
    return result;
}

void FloaterStateStore::saveOpenDocuments(const std::vector<FloaterDocumentId>& documents) {
    LLSD saved = LLSD::emptyArray();
    for (const FloaterDocumentId& document : documents) {
        LLSD entry = LLSD::emptyMap();
        entry["definition"] = document.definitionId;
        entry["instance"] = document.instanceKey;
        saved.append(std::move(entry));
    }
    gSavedSettings.setLLSD("RduiOpenFloaters", saved);
}

LLSD FloaterStateStore::read() const {
    return gSavedSettings.getLLSD("RduiFloaterPlacements");
}

void FloaterStateStore::write(const LLSD& placements) {
    gSavedSettings.setLLSD("RduiFloaterPlacements", placements);
}
} // namespace rdui::viewer
