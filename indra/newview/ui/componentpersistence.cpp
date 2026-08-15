/**
 * @file componentpersistence.cpp
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

#include "llviewerprecompiledheaders.h"
#include "componentpersistence.h"
#include <cmath>
#include <string_view>
#include <type_traits>
#include <utility>
#include "llcontrol.h"
#include "llsdutil.h"
#include "widgets/floater.h"

namespace radia::viewer::ui {
using namespace ::radia::ui;
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

template<typename Placement> void writeGeometry(LLSD& entry, const Placement& placement) {
    entry["position"] = makePair(static_cast<float>(placement.x), static_cast<float>(placement.y));
    if (placement.size) entry["size"] = makePair(placement.size->width, placement.size->height);
    else entry.erase("size");
    if constexpr (std::is_same_v<Placement, DetachedFloaterPlacement>) entry["detached"] = true;
    else entry.erase("detached");
}

void writePlacement(LLSD& entry, const FloaterPlacement& placement) {
    std::visit(
        [&entry](const auto& value) {
            writeGeometry(entry, value);
            setOptionalFlag(entry, "minimized", value.minimized);
        },
        placement);
}

void writeLayoutPlacement(LLSD& entry, const FloaterPlacement& placement) {
    writePlacement(entry, placement);
    entry.erase("minimized");
}

std::optional<FloaterPlacement> decodePlacement(const LLSD& value) {
    if (!value.isMap()) return std::nullopt;

    float x = 0.f;
    float y = 0.f;
    if (!readPair(value["position"], x, y)) return std::nullopt;

    float width = 0.f;
    float height = 0.f;
    const bool hasSize = readPair(value["size"], width, height) && width > 0.f && height > 0.f;
    const bool minimized = readFlag(value, "minimized");
    if (value["detached"].asBoolean()) {
        std::optional<FloaterLogicalSize> size;
        if (hasSize) size = FloaterLogicalSize{width, height};
        return DetachedFloaterPlacement{static_cast<int>(std::lround(x)), static_cast<int>(std::lround(y)), size, minimized};
    }

    std::optional<FloaterLogicalSize> size;
    if (hasSize) size = FloaterLogicalSize{width, height};
    return AttachedFloaterPlacement{x, y, size, minimized};
}

LLSD mergedPlacement(const LLSD& layout, const LLSD& workspaceEntry) {
    LLSD merged = layout.isMap() ? layout : LLSD::emptyMap();
    if (!workspaceEntry.isMap()) return merged;
    for (const char* field : {"position", "size", "detached", "minimized"})
        if (workspaceEntry.has(field)) merged[field] = workspaceEntry[field];
    return merged;
}
} // namespace

ComponentPersistence::ComponentPersistence(LLControlGroup& layout, LLControlGroup& workspace) : mLayout(layout), mWorkspace(workspace) {}

std::vector<ComponentKey> ComponentPersistence::openComponentKeys() const {
    std::vector<ComponentKey> result;
    const LLSD workspace = readWorkspace();
    for (LLSD::map_const_iterator entry = workspace.beginMap(); entry != workspace.endMap(); ++entry) {
        if (!entry->second.isMap()) continue;
        if (const std::optional<ComponentKey> component = ComponentKey::fromPersistenceKey(entry->first)) result.push_back(*component);
    }
    return result;
}

void ComponentPersistence::saveWorkspace(const std::vector<ComponentInstanceState>& states, const std::vector<ComponentKey>& preserved) {
    const LLSD previousWorkspace = readWorkspace();
    LLSD workspace = LLSD::emptyMap();
    auto retain = [&](const ComponentKey& component) {
        if (!component.valid()) return;
        const std::string key = component.persistenceKey();
        const LLSD& previous = previousWorkspace[key];
        workspace[key] = previous.isMap() ? previous : LLSD::emptyMap();
    };
    for (const ComponentInstanceState& state : states) retain(state.componentKey);
    for (const ComponentKey& component : preserved) retain(component);

    for (const ComponentInstanceState& state : states) {
        if (!state.componentKey.valid()) continue;
        const std::string key = state.componentKey.persistenceKey();
        if (!workspace.has(key)) continue;
        LLSD& entry = workspace[key];
        if (!entry.isMap()) entry = LLSD::emptyMap();
        if (state.minimized) entry["minimized"] = true;
        else entry.erase("minimized");
        if (!state.componentKey.instanceKey.empty() && state.detached) entry["detached"] = true;
        else entry.erase("detached");
    }

    if (workspace == previousWorkspace) return;
    writeWorkspace(workspace);
}

std::optional<FloaterPlacement> ComponentPersistence::restorePlacement(const ComponentKey& componentKey) const {
    if (!componentKey.valid()) return std::nullopt;
    const LLSD layout = readLayout();
    const LLSD workspace = readWorkspace();
    return decodePlacement(mergedPlacement(layout[componentKey.definitionId], workspace[componentKey.persistenceKey()]));
}

void ComponentPersistence::saveAttachedPlacement(const ComponentKey& componentKey, const Floater& floater) {
    std::optional<FloaterLogicalSize> size;
    if (floater.canResize()) size = FloaterLogicalSize{floater.rect().w, floater.rect().h};
    savePlacement(componentKey, AttachedFloaterPlacement{floater.rect().x, floater.rect().y, size, floater.minimized()},
                  floater.closed() ? ComponentOpenState::Closed : ComponentOpenState::Open);
}

void ComponentPersistence::savePlacement(const ComponentKey& componentKey, FloaterPlacement placement, ComponentOpenState state) {
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
        workspaceEntry.erase("detached");
        const bool minimized = std::visit([](const auto& value) { return value.minimized; }, placement);
        setOptionalFlag(workspaceEntry, "minimized", minimized);
    } else {
        LLSD& workspaceEntry = workspace[key];
        if (!workspaceEntry.isMap()) workspaceEntry = LLSD::emptyMap();
        writePlacement(workspaceEntry, placement);
    }

    if (layout != previousLayout) writeLayout(layout);
    if (workspace != previousWorkspace) writeWorkspace(workspace);
}

LLSD ComponentPersistence::readLayout() const {
    if (!mLayout.controlExists(std::string(kUILayout))) return LLSD::emptyMap();
    return normalizedMap(mLayout.getLLSD(kUILayout));
}

LLSD ComponentPersistence::readWorkspace() const {
    if (!mWorkspace.controlExists(std::string(kUIWorkspace))) return LLSD::emptyMap();
    return normalizedMap(mWorkspace.getLLSD(kUIWorkspace));
}

void ComponentPersistence::writeLayout(const LLSD& layout) {
    if (mLayout.controlExists(std::string(kUILayout))) mLayout.setLLSD(kUILayout, layout);
}

void ComponentPersistence::writeWorkspace(const LLSD& workspace) {
    if (mWorkspace.controlExists(std::string(kUIWorkspace))) mWorkspace.setLLSD(kUIWorkspace, workspace);
}
} // namespace radia::viewer::ui
