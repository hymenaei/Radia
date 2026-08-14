/**
 * @file componentpersistence_test.cpp
 * @brief Tests persistence of component layout and per-account workspace state.
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

#include "linden_common.h"
#include <algorithm>
#include <tuple>
#include "../test/lltut.h"
#include "componentpersistence.h"
#include "llcontrol.h"

LLControlGroup gSavedSettings("test");

namespace tut {
struct componentPersistenceData {
    componentPersistenceData() {
        if (!gSavedSettings.controlExists("UILayout")) gSavedSettings.declareLLSD("UILayout", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        if (!gSavedSettings.controlExists("UIWorkspace"))
            gSavedSettings.declareLLSD("UIWorkspace", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        gSavedSettings.setLLSD("UILayout", LLSD::emptyMap());
        gSavedSettings.setLLSD("UIWorkspace", LLSD::emptyMap());
    }
};
using componentPersistenceTest = test_group<componentPersistenceData>;
using componentPersistenceObject = componentPersistenceTest::object;
componentPersistenceTest componentPersistenceTestCase("UIComponentPersistence");

template<> template<> void componentPersistenceObject::test<1>() {
    set_test_name("workspace uses key-first instance identities");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const std::vector<rdui::viewer::ComponentKey> components{{"inventory", "one"}, {"inventory", "two"}, {"settings", {}}};

    persistence.saveWorkspace({{components[0]}, {components[1]}, {components[2]}});

    const LLSD saved = gSavedSettings.getLLSD("UIWorkspace");
    ensure("workspace is a map", saved.isMap());
    ensure("keyed instance uses key-first identity", saved.has("one@inventory"));
    ensure("second keyed instance uses key-first identity", saved.has("two@inventory"));
    ensure("keyless instance uses its component name", saved.has("settings"));
    std::vector<rdui::viewer::ComponentKey> restored = persistence.openComponentKeys();
    const auto order = [](const rdui::viewer::ComponentKey& left, const rdui::viewer::ComponentKey& right) {
        return std::tie(left.definitionId, left.instanceKey) < std::tie(right.definitionId, right.instanceKey);
    };
    std::vector<rdui::viewer::ComponentKey> expected = components;
    std::sort(restored.begin(), restored.end(), order);
    std::sort(expected.begin(), expected.end(), order);
    ensure("components round trip", restored == expected);
}

template<> template<> void componentPersistenceObject::test<2>() {
    set_test_name("malformed workspace identities are ignored");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    LLSD saved = LLSD::emptyMap();
    saved["settings"] = LLSD::emptyMap();
    saved["alice@profile"] = LLSD::emptyMap();
    saved["@missing-component"] = LLSD::emptyMap();
    saved["missing-key@"] = LLSD::emptyMap();
    saved["too@many@separators"] = LLSD::emptyMap();
    saved["not-a-map"] = true;
    gSavedSettings.setLLSD("UIWorkspace", saved);

    const std::vector<rdui::viewer::ComponentKey> restored = persistence.openComponentKeys();
    ensure_equals("only valid workspace entries are restored", restored.size(), std::size_t{2});
    ensure("keyless component is restored",
           std::find(restored.begin(), restored.end(), rdui::viewer::ComponentKey{"settings", {}}) != restored.end());
    ensure("keyed component is restored",
           std::find(restored.begin(), restored.end(), rdui::viewer::ComponentKey{"profile", "alice"}) != restored.end());
}

template<> template<> void componentPersistenceObject::test<3>() {
    set_test_name("layout and workspace are written to their separate settings");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    LLSD workspace = LLSD::emptyMap();
    workspace["settings"] = LLSD::emptyMap();
    persistence.savePlacement(rdui::viewer::ComponentKey{"settings", {}},
                              rdui::viewer::AttachedFloaterPlacement{96.f, 64.f, rdui::viewer::FloaterLogicalSize{480.f, 320.f}, false},
                              rdui::viewer::ComponentOpenState::Open);
    gSavedSettings.setLLSD("UIWorkspace", workspace);

    ensure("layout is stored in the layout setting", gSavedSettings.getLLSD("UILayout")["settings"].isMap());
    ensure("workspace is stored in the workspace setting", gSavedSettings.getLLSD("UIWorkspace")["settings"].isMap());
}

template<> template<> void componentPersistenceObject::test<4>() {
    set_test_name("component identity has one persistence codec");
    const rdui::viewer::ComponentKey component{"profile", "alice"};
    ensure("component is valid", component.valid());
    ensure_equals("persistence identity is instance first", component.persistenceKey(), std::string("alice@profile"));
    ensure("persistence identity round trips", rdui::viewer::ComponentKey::fromPersistenceKey(component.persistenceKey()) == component);
    ensure("invalid persistence separators are rejected", !rdui::viewer::ComponentKey::fromPersistenceKey("alice@profile@extra"));
}

template<> template<> void componentPersistenceObject::test<5>() {
    set_test_name("workspace state is saved in one combined update");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const std::vector<rdui::viewer::ComponentKey> components{{"profile", "alice"}, {"demo", {}}};
    LLSD previous = LLSD::emptyMap();
    previous["stale@orphan"] = LLSD::emptyMap();
    gSavedSettings.setLLSD("UIWorkspace", previous);
    persistence.saveWorkspace({{components[0], true, true}, {components[1], false, true}});

    const LLSD workspace = gSavedSettings.getLLSD("UIWorkspace");
    ensure("keyed minimized state is persisted", workspace["alice@profile"]["minimized"].asBoolean());
    ensure("keyed detached override is persisted", workspace["alice@profile"]["detached"].asBoolean());
    ensure("keyless detached state is omitted", !workspace["demo"].has("detached"));
    ensure("stale state does not create an open entry", !workspace.has("stale@orphan"));
}

template<> template<> void componentPersistenceObject::test<6>() {
    set_test_name("keyless placement restores user-wide layout defaults");
    LLSD layout = LLSD::emptyMap();
    layout["demo"]["position"].append(31.f);
    layout["demo"]["position"].append(42.f);
    layout["demo"]["size"].append(320.f);
    layout["demo"]["size"].append(240.f);
    gSavedSettings.setLLSD("UILayout", layout);

    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const auto placement = persistence.restorePlacement({"demo", {}});
    const auto* attached = std::get_if<rdui::viewer::AttachedFloaterPlacement>(&*placement);
    ensure("default placement is attached", attached != nullptr);
    ensure_equals("saved x preserved", attached->x, 31.f);
    ensure_equals("saved y preserved", attached->y, 42.f);
    ensure("saved size preserved", attached->size.has_value());
}

template<> template<> void componentPersistenceObject::test<7>() {
    set_test_name("keyed placement overrides layout without changing it");
    LLSD layout = LLSD::emptyMap();
    layout["profile"]["position"].append(10.f);
    layout["profile"]["position"].append(20.f);
    layout["profile"]["size"].append(400.f);
    layout["profile"]["size"].append(300.f);
    LLSD workspace = LLSD::emptyMap();
    workspace["alice@profile"]["position"].append(100.f);
    workspace["alice@profile"]["position"].append(200.f);
    workspace["alice@profile"]["size"].append(500.f);
    workspace["alice@profile"]["size"].append(600.f);
    gSavedSettings.setLLSD("UILayout", layout);
    gSavedSettings.setLLSD("UIWorkspace", workspace);

    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const auto placement = persistence.restorePlacement({"profile", "alice"});
    const auto* attached = std::get_if<rdui::viewer::AttachedFloaterPlacement>(&*placement);
    ensure("keyed placement is attached", attached != nullptr);
    ensure_equals("instance position overrides default", attached->x, 100.f);
    ensure_equals("instance size overrides default", attached->size->width, 500.f);

    persistence.savePlacement({"profile", "alice"}, *attached, rdui::viewer::ComponentOpenState::Open);
    ensure_equals("default position remains unchanged", gSavedSettings.getLLSD("UILayout")["profile"]["position"][0].asReal(), 10.0);
}

template<> template<> void componentPersistenceObject::test<8>() {
    set_test_name("detached placement round trips through compact arrays");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const rdui::viewer::ComponentKey identity{"profile", "alice"};
    persistence.savePlacement(identity, rdui::viewer::DetachedFloaterPlacement{100, 200, rdui::viewer::FloaterLogicalSize{640.f, 480.f}, true},
                              rdui::viewer::ComponentOpenState::Open);

    const LLSD encoded = gSavedSettings.getLLSD("UIWorkspace")["alice@profile"];
    ensure("detached marker is present", encoded["detached"].asBoolean());
    ensure("detached position is an array", encoded["position"].isArray());
    ensure("detached size is an array", encoded["size"].isArray());
    ensure("minimized marker is present", encoded["minimized"].asBoolean());

    const auto restored = persistence.restorePlacement(identity);
    const auto* detached = std::get_if<rdui::viewer::DetachedFloaterPlacement>(&*restored);
    ensure("detached placement decoded", detached != nullptr);
    ensure_equals("x round trips", detached->x, 100);
    ensure_equals("y round trips", detached->y, 200);
    ensure_equals("width round trips", detached->size->width, 640.f);
    ensure(detached->minimized);
}

template<> template<> void componentPersistenceObject::test<9>() {
    set_test_name("workspace flags combine with default layout");
    LLSD layout = LLSD::emptyMap();
    layout["demo"]["position"].append(1.f);
    layout["demo"]["position"].append(2.f);
    LLSD workspace = LLSD::emptyMap();
    workspace["demo"]["minimized"] = true;
    gSavedSettings.setLLSD("UILayout", layout);
    gSavedSettings.setLLSD("UIWorkspace", workspace);

    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const auto placement = persistence.restorePlacement({"demo", {}});
    const auto* attached = std::get_if<rdui::viewer::AttachedFloaterPlacement>(&*placement);
    ensure("default placement is restored", attached != nullptr);
    ensure("workspace minimized flag is restored", attached->minimized);
}

template<> template<> void componentPersistenceObject::test<10>() {
    set_test_name("placement saves update layout and workspace through one owner");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const rdui::viewer::ComponentKey identity{"demo", {}};
    persistence.savePlacement(identity, rdui::viewer::DetachedFloaterPlacement{760, 120, rdui::viewer::FloaterLogicalSize{540.f, 680.f}, true},
                              rdui::viewer::ComponentOpenState::Open);

    ensure("detached placement is user-wide", gSavedSettings.getLLSD("UILayout")["demo"]["detached"].asBoolean());
    ensure("workspace contains the open marker", gSavedSettings.getLLSD("UIWorkspace").has("demo"));
    ensure("workspace does not duplicate detached state", !gSavedSettings.getLLSD("UIWorkspace")["demo"].has("detached"));
    const auto restored = persistence.restorePlacement(identity);
    const auto* detached = std::get_if<rdui::viewer::DetachedFloaterPlacement>(&*restored);
    ensure("detached state survives a fresh read", detached != nullptr);
    ensure_equals("global detached x survives", detached->x, 760);
}

template<> template<> void componentPersistenceObject::test<11>() {
    set_test_name("keyless placement clears stale workspace geometry");
    LLSD workspace = LLSD::emptyMap();
    workspace["demo"]["position"].append(900.f);
    workspace["demo"]["position"].append(700.f);
    workspace["demo"]["size"].append(640.f);
    workspace["demo"]["size"].append(480.f);
    workspace["demo"]["detached"] = true;
    gSavedSettings.setLLSD("UIWorkspace", workspace);

    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    persistence.savePlacement({"demo", {}}, rdui::viewer::AttachedFloaterPlacement{12.f, 24.f, std::nullopt, true},
                              rdui::viewer::ComponentOpenState::Open);

    const LLSD saved = gSavedSettings.getLLSD("UIWorkspace");
    ensure("workspace open marker remains", saved.has("demo"));
    ensure("stale position is removed", !saved["demo"].has("position"));
    ensure("stale size is removed", !saved["demo"].has("size"));
    ensure("stale detached flag is removed", !saved["demo"].has("detached"));
    ensure("minimized state is retained", saved["demo"]["minimized"].asBoolean());
}

template<> template<> void componentPersistenceObject::test<12>() {
    set_test_name("closing a keyed placement removes its workspace marker");
    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    const rdui::viewer::ComponentKey identity{"profile", "alice"};
    persistence.savePlacement(identity, rdui::viewer::AttachedFloaterPlacement{10.f, 20.f, std::nullopt, false},
                              rdui::viewer::ComponentOpenState::Open);
    ensure("open placement creates workspace marker", gSavedSettings.getLLSD("UIWorkspace").has("alice@profile"));
    persistence.savePlacement(identity, rdui::viewer::AttachedFloaterPlacement{30.f, 40.f, std::nullopt, false},
                              rdui::viewer::ComponentOpenState::Closed);
    ensure("closed placement removes workspace marker", !gSavedSettings.getLLSD("UIWorkspace").has("alice@profile"));
}

template<> template<> void componentPersistenceObject::test<13>() {
    set_test_name("unavailable workspace entries are retained explicitly");
    LLSD workspace = LLSD::emptyMap();
    workspace["alice@profile"] = LLSD::emptyMap();
    gSavedSettings.setLLSD("UIWorkspace", workspace);

    rdui::viewer::ComponentPersistence persistence(gSavedSettings, gSavedSettings);
    persistence.saveWorkspace({}, {{"profile", "alice"}});
    ensure("unavailable component remains restorable", gSavedSettings.getLLSD("UIWorkspace").has("alice@profile"));
}
} // namespace tut
