/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "workspacepersistence.h"
#include <cmath>
#include <string_view>
#include <utility>
#include "elements/floater.h"
#include "llcontrol.h"
#include "llsdutil.h"

namespace radia::viewer::ui {
using radia::ui::FloaterElement;

namespace {
constexpr std::string_view kUILayout = "UILayout";
constexpr std::string_view kUIWorkspace = "UIWorkspace";

LLSD normalizedMap(LLSD value) {
    return value.isMap() ? value : LLSD::emptyMap();
}

bool isNumber(const LLSD& value) {
    return value.isInteger() || value.isReal();
}

bool readPair(const LLSD& value, float& first, float& second) {
    if (!value.isArray() || value.size() != 2 || !isNumber(value[0]) || !isNumber(value[1])) return false;
    first = static_cast<float>(value[0].asReal());
    second = static_cast<float>(value[1].asReal());
    return std::isfinite(first) && std::isfinite(second);
}

LLSD makePair(float first, float second) {
    LLSD result = LLSD::emptyArray();
    result.append(first);
    result.append(second);
    return result;
}

bool readFlag(const LLSD& entry, const char* name) {
    return entry.isMap() && entry.has(name) && entry[name].asBoolean();
}

void setOptionalFlag(LLSD& entry, const char* name, bool value) {
    if (value) entry[name] = true;
    else entry.erase(name);
}

void writeGeometry(LLSD& entry, const FloaterPlacement& placement) {
    entry["position"] = makePair(placement.x, placement.y);
    if (placement.size) entry["size"] = makePair(placement.size->width, placement.size->height);
    else entry.erase("size");
}

void writePlacement(LLSD& entry, const FloaterPlacement& placement) {
    writeGeometry(entry, placement);
    setOptionalFlag(entry, "minimized", placement.minimized);
}

void writeLayoutPlacement(LLSD& entry, const FloaterPlacement& placement) {
    writePlacement(entry, placement);
    entry.erase("minimized");
}

std::optional<FloaterPlacement> decodePlacement(const LLSD& value) {
    if (!value.isMap()) return std::nullopt;

    float width = 0.f;
    float height = 0.f;
    const bool hasSize = readPair(value["size"], width, height) && width > 0.f && height > 0.f;
    const bool minimized = readFlag(value, "minimized");

    std::optional<FloaterLogicalSize> size;
    if (hasSize) size = FloaterLogicalSize{width, height};

    float x = 0.f;
    float y = 0.f;
    if (!readPair(value["position"], x, y)) return std::nullopt;
    return FloaterPlacement{x, y, size, minimized};
}

LLSD mergedPlacement(const LLSD& layout, const LLSD& workspaceEntry) {
    LLSD merged = layout.isMap() ? layout : LLSD::emptyMap();
    if (!workspaceEntry.isMap()) return merged;
    for (const char* field : {"position", "size", "minimized"})
        if (workspaceEntry.has(field)) merged[field] = workspaceEntry[field];
    return merged;
}
} // namespace

WorkspacePersistence::WorkspacePersistence(LLControlGroup& layout, LLControlGroup& workspace) : mLayout(layout), mWorkspace(workspace) {}

std::vector<ComponentInstanceKey> WorkspacePersistence::openComponentKeys() const {
    std::vector<ComponentInstanceKey> result;
    const LLSD workspace = readWorkspace();
    for (LLSD::map_const_iterator entry = workspace.beginMap(); entry != workspace.endMap(); ++entry) {
        if (!entry->second.isMap()) continue;
        if (const std::optional<ComponentInstanceKey> component = ComponentInstanceKey::fromPersistenceKey(entry->first))
            result.push_back(*component);
    }
    return result;
}

void WorkspacePersistence::saveWorkspace(const std::vector<ComponentInstanceState>& states, const std::vector<ComponentInstanceKey>& preserved) {
    const LLSD previousWorkspace = readWorkspace();
    LLSD workspace = LLSD::emptyMap();
    auto retain = [&](const ComponentInstanceKey& component) {
        if (!component.valid()) return;
        const std::string key = component.persistenceKey();
        const LLSD& previous = previousWorkspace[key];
        workspace[key] = previous.isMap() ? previous : LLSD::emptyMap();
    };
    for (const ComponentInstanceState& state : states) retain(state.componentKey);
    for (const ComponentInstanceKey& component : preserved) retain(component);

    for (const ComponentInstanceState& state : states) {
        if (!state.componentKey.valid()) continue;
        const std::string key = state.componentKey.persistenceKey();
        if (!workspace.has(key)) continue;
        LLSD& entry = workspace[key];
        if (!entry.isMap()) entry = LLSD::emptyMap();
        if (state.minimized) entry["minimized"] = true;
        else entry.erase("minimized");
    }

    if (workspace == previousWorkspace) return;
    writeWorkspace(workspace);
}

std::optional<FloaterPlacement> WorkspacePersistence::restorePlacement(const ComponentInstanceKey& componentKey) const {
    if (!componentKey.valid()) return std::nullopt;
    const LLSD layout = readLayout();
    const LLSD workspace = readWorkspace();
    return decodePlacement(mergedPlacement(layout[componentKey.definitionId], workspace[componentKey.persistenceKey()]));
}

void WorkspacePersistence::saveFloaterPlacement(const ComponentInstanceKey& componentKey, const FloaterElement& floater) {
    const auto& placementRect = floater.minimized() ? floater.expandedRect() : floater.rect();
    std::optional<FloaterLogicalSize> size;
    if (floater.resizeable()) size = FloaterLogicalSize{placementRect.w, placementRect.h};
    savePlacement(componentKey, FloaterPlacement{placementRect.x, placementRect.y, size, floater.minimized()},
                  floater.closed() ? ComponentOpenState::Closed : ComponentOpenState::Open);
}

void WorkspacePersistence::savePlacement(const ComponentInstanceKey& componentKey, FloaterPlacement placement, ComponentOpenState state) {
    if (!componentKey.valid()) return;

    LLSD layout = readLayout();
    LLSD workspace = readWorkspace();
    const LLSD previousLayout = layout;
    const LLSD previousWorkspace = workspace;
    const std::string key = componentKey.persistenceKey();

    if (state == ComponentOpenState::Closed) {
        workspace.erase(key);
        if (componentKey.instanceKey.empty()) {
            LLSD& componentLayout = layout[componentKey.definitionId];
            if (!componentLayout.isMap()) componentLayout = LLSD::emptyMap();
            writeLayoutPlacement(componentLayout, placement);
        }
    } else if (componentKey.instanceKey.empty()) {
        LLSD& componentLayout = layout[componentKey.definitionId];
        if (!componentLayout.isMap()) componentLayout = LLSD::emptyMap();
        writeLayoutPlacement(componentLayout, placement);

        LLSD& workspaceEntry = workspace[key];
        if (!workspaceEntry.isMap()) workspaceEntry = LLSD::emptyMap();
        workspaceEntry.erase("position");
        workspaceEntry.erase("size");
        setOptionalFlag(workspaceEntry, "minimized", placement.minimized);
    } else {
        LLSD& workspaceEntry = workspace[key];
        if (!workspaceEntry.isMap()) workspaceEntry = LLSD::emptyMap();
        writePlacement(workspaceEntry, placement);
    }

    if (layout != previousLayout) writeLayout(layout);
    if (workspace != previousWorkspace) writeWorkspace(workspace);
}

LLSD WorkspacePersistence::readLayout() const {
    if (!mLayout.controlExists(std::string(kUILayout))) return LLSD::emptyMap();
    return normalizedMap(mLayout.getLLSD(kUILayout));
}

LLSD WorkspacePersistence::readWorkspace() const {
    if (!mWorkspace.controlExists(std::string(kUIWorkspace))) return LLSD::emptyMap();
    return normalizedMap(mWorkspace.getLLSD(kUIWorkspace));
}

void WorkspacePersistence::writeLayout(const LLSD& layout) {
    if (mLayout.controlExists(std::string(kUILayout))) mLayout.setLLSD(kUILayout, layout);
}

void WorkspacePersistence::writeWorkspace(const LLSD& workspace) {
    if (mWorkspace.controlExists(std::string(kUIWorkspace))) mWorkspace.setLLSD(kUIWorkspace, workspace);
}
} // namespace radia::viewer::ui
