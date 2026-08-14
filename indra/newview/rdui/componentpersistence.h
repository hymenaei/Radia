/**
 * @file componentpersistence.h
 * @brief Persists user-wide component layout and per-account workspace state.
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

#ifndef RD_COMPONENTPERSISTENCE_H
#define RD_COMPONENTPERSISTENCE_H

#include <optional>
#include <variant>
#include <vector>
#include "componentidentity.h"
#include "llsd.h"

class LLControlGroup;
namespace rdui { class Floater; }

namespace rdui::viewer {
enum class ComponentOpenState { Closed, Open };

struct FloaterLogicalSize {
    float width = 0.f;
    float height = 0.f;
};

struct AttachedFloaterPlacement {
    float x = 0.f;
    float y = 0.f;
    std::optional<FloaterLogicalSize> size;
    bool minimized = false;
};

struct DetachedFloaterPlacement {
    int x = 0;
    int y = 0;
    std::optional<FloaterLogicalSize> size;
    bool minimized = false;
};

using FloaterPlacement = std::variant<AttachedFloaterPlacement, DetachedFloaterPlacement>;

struct ComponentInstanceState {
    ComponentKey componentKey;
    bool minimized = false;
    bool detached = false;
};

class ComponentPersistence final {
public:
    ComponentPersistence(LLControlGroup& layout, LLControlGroup& workspace);

    std::vector<ComponentKey> openComponentKeys() const;
    void saveWorkspace(const std::vector<ComponentInstanceState>& states, const std::vector<ComponentKey>& preserved = {});
    std::optional<FloaterPlacement> restorePlacement(const ComponentKey& componentKey) const;
    void saveAttachedPlacement(const ComponentKey& componentKey, const Floater& floater);
    void savePlacement(const ComponentKey& componentKey, FloaterPlacement placement, ComponentOpenState state);

private:
    LLSD readLayout() const;
    LLSD readWorkspace() const;
    void writeLayout(const LLSD& layout);
    void writeWorkspace(const LLSD& workspace);

    LLControlGroup& mLayout;
    LLControlGroup& mWorkspace;
};
} // namespace rdui::viewer
#endif // RD_COMPONENTPERSISTENCE_H
