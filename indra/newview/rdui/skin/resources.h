/**
 * @file resources.h
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

#ifndef LL_RDUI_SKIN_RESOURCES_H
#define LL_RDUI_SKIN_RESOURCES_H

#include "skin/resolver.h"

namespace rdui::viewer {
class SkinResources final : public SkinSnapshotSource {
public:
    SkinSnapshotResult capture() const override;
    SkinSnapshotResult captureBundledDefault() const;
    bool selectedIsBundledDefault() const;
};
} // namespace rdui::viewer
#endif // LL_RDUI_SKIN_RESOURCES_H
