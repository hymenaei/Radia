/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <vector>
#include "ComponentInstanceKey.h"

class LLControlGroup;
namespace Core {
class HTMLFloaterElement;
}

namespace Viewer {
enum class ComponentOpenState {
    Closed,
    Open
};

struct FloaterLogicalSize {
    float width = 0.f;
    float height = 0.f;
};

struct FloaterPlacement {
    float x = 0.f;
    float y = 0.f;
    std::optional<FloaterLogicalSize> size;
    bool minimized = false;
};

struct ComponentInstanceState {
    ComponentInstanceKey componentKey;
    bool minimized = false;
};

class WorkspacePersistence final {
public:
    WorkspacePersistence(LLControlGroup& layout, LLControlGroup& workspace);

    std::vector<ComponentInstanceKey> openComponentKeys() const;
    void saveWorkspace(const std::vector<ComponentInstanceState>& states, const std::vector<ComponentInstanceKey>& preserved = {});
    std::optional<FloaterPlacement> restorePlacement(const ComponentInstanceKey& componentKey) const;
    void saveFloaterPlacement(const ComponentInstanceKey& componentKey, const Core::HTMLFloaterElement& floater);
    void savePlacement(const ComponentInstanceKey& componentKey, FloaterPlacement placement, ComponentOpenState state);

private:
    LLControlGroup& mLayout;
    LLControlGroup& mWorkspace;
};
} // namespace Viewer
