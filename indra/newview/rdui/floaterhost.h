/**
 * @file floaterhost.h
 * @brief Hosts viewer-owned Floaters for the component manager.
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

#ifndef RD_FLOATERHOST_H
#define RD_FLOATERHOST_H

#include "componentmanager.h"

namespace rdui {
class Floater;
class Surface;
} // namespace rdui

namespace rdui::viewer {
class DetachedFloaterManager;

class FloaterHost final : public ComponentManager::Host {
public:
    FloaterHost(rdui::Surface& attachedSurface, DetachedFloaterManager& detachedManager);

    void mount(std::unique_ptr<rdui::Floater> root) override;
    std::unique_ptr<rdui::Floater> unmount(rdui::Floater& root) override;
    bool replaceAll(std::vector<ReplacementRequest> replacements) override;
    bool clearAll(std::vector<rdui::Floater*> roots) override;
    void present(rdui::Floater& root) override;

private:
    rdui::Surface& mAttachedSurface;
    DetachedFloaterManager& mDetachedManager;
};
} // namespace rdui::viewer
#endif // RD_FLOATERHOST_H
