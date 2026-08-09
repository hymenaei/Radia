/**
 * @file floaterplacementstore.cpp
 * @brief Persists attached and detached Floater placements keyed by instance identity.
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
#include "floaterplacementstore.h"
#include <utility>

namespace rdui::viewer {
namespace {
std::optional<FloaterPlacementSize> positiveSize(const LLSD& value, const char* width_key, const char* height_key) {
    const float width = static_cast<float>(value[width_key].asReal());
    const float height = static_cast<float>(value[height_key].asReal());
    if (width <= 0.f || height <= 0.f) return std::nullopt;
    return FloaterPlacementSize{width, height};
}

FloaterPlacement decodePlacement(const LLSD& value) {
    if (value["detached"].asBoolean()) {
        return DetachedFloaterPlacement{value["x"].asInteger(),      value["y"].asInteger(),
                                        value["width"].asInteger(),  value["height"].asInteger(),
                                        value["monitor"].asString(), positiveSize(value, "logical_width", "logical_height")};
    }

    return AttachedFloaterPlacement{static_cast<float>(value["x"].asReal()), static_cast<float>(value["y"].asReal()),
                                    positiveSize(value, "width", "height")};
}

LLSD encodePlacement(const FloaterPlacement& placement) {
    LLSD value = LLSD::emptyMap();
    if (const auto* attached = std::get_if<AttachedFloaterPlacement>(&placement)) {
        value["detached"] = false;
        value["x"] = attached->x;
        value["y"] = attached->y;
        if (attached->size) {
            value["width"] = attached->size->width;
            value["height"] = attached->size->height;
        }
        return value;
    }

    const auto& detached = std::get<DetachedFloaterPlacement>(placement);
    value["detached"] = true;
    value["x"] = detached.x;
    value["y"] = detached.y;
    value["width"] = detached.width;
    value["height"] = detached.height;
    value["monitor"] = detached.monitor;
    if (detached.logicalSize) {
        value["logical_width"] = detached.logicalSize->width;
        value["logical_height"] = detached.logicalSize->height;
    }
    return value;
}
} // namespace

FloaterPlacementStore::FloaterPlacementStore(Persistence& persistence) : mPersistence(persistence) {}

std::optional<FloaterPlacement> FloaterPlacementStore::restore(const FloaterInstanceId& identity) {
    if (identity.empty() || !mRestoredIdentities.insert(identity.value()).second) return std::nullopt;

    LLSD placements = mPersistence.read();
    if (!placements.isMap()) return std::nullopt;
    const LLSD& current = placements[identity.value()];
    if (current.isMap()) return decodePlacement(current);
    return std::nullopt;
}

void FloaterPlacementStore::save(const FloaterInstanceId& identity, FloaterPlacement placement) {
    if (identity.empty() || !mRestoredIdentities.contains(identity.value())) return;
    LLSD placements = mPersistence.read();
    if (!placements.isMap()) placements = LLSD::emptyMap();
    placements[identity.value()] = encodePlacement(placement);
    mPersistence.write(placements);
}
} // namespace rdui::viewer
