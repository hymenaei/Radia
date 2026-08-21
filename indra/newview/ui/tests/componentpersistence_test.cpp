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
#include <gtest/gtest.h>
#include <string>
#include <tuple>
#include <vector>
#include "componentpersistence.h"
#include "llcontrol.h"

namespace {
using radia::viewer::ui::ComponentInstanceState;
using radia::viewer::ui::ComponentKey;
using radia::viewer::ui::ComponentOpenState;
using radia::viewer::ui::ComponentPersistence;
using radia::viewer::ui::FloaterPlacement;
using radia::viewer::ui::FloaterLogicalSize;

class ComponentPersistenceTest : public ::testing::Test {
protected:
    void SetUp() override {
        layout.declareLLSD("UILayout", LLSD::emptyMap(), "test layout", LLControlVariable::PERSIST_NO);
        workspace.declareLLSD("UIWorkspace", LLSD::emptyMap(), "test workspace", LLControlVariable::PERSIST_NO);
        layout.setLLSD("UILayout", LLSD::emptyMap());
        workspace.setLLSD("UIWorkspace", LLSD::emptyMap());
    }

    LLControlGroup layout{"component-persistence-layout"};
    LLControlGroup workspace{"component-persistence-workspace"};
};
} // namespace

TEST_F(ComponentPersistenceTest, RoundTripsKeyedAndKeylessWorkspaceEntries) {
    ComponentPersistence persistence(layout, workspace);
    const std::vector<ComponentKey> components{{"inventory", "one"}, {"inventory", "two"}, {"settings", {}}};

    persistence.saveWorkspace({{components[0]}, {components[1]}, {components[2]}});

    const LLSD saved = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(saved.isMap());
    EXPECT_TRUE(saved.has("one@inventory"));
    EXPECT_TRUE(saved.has("two@inventory"));
    EXPECT_TRUE(saved.has("settings"));

    std::vector<ComponentKey> restored = persistence.openComponentKeys();
    const auto order = [](const ComponentKey& left, const ComponentKey& right) {
        return std::tie(left.definitionId, left.instanceKey) < std::tie(right.definitionId, right.instanceKey);
    };
    std::vector<ComponentKey> expected = components;
    std::sort(restored.begin(), restored.end(), order);
    std::sort(expected.begin(), expected.end(), order);
    EXPECT_EQ(restored, expected);
}

TEST_F(ComponentPersistenceTest, IgnoresMalformedWorkspaceIdentities) {
    ComponentPersistence persistence(layout, workspace);
    LLSD saved = LLSD::emptyMap();
    saved["settings"] = LLSD::emptyMap();
    saved["alice@profile"] = LLSD::emptyMap();
    saved["@missing-component"] = LLSD::emptyMap();
    saved["missing-key@"] = LLSD::emptyMap();
    saved["too@many@separators"] = LLSD::emptyMap();
    saved["not-a-map"] = true;
    workspace.setLLSD("UIWorkspace", saved);

    const std::vector<ComponentKey> restored = persistence.openComponentKeys();
    ASSERT_EQ(restored.size(), std::size_t{2});
    EXPECT_TRUE(std::find(restored.begin(), restored.end(), ComponentKey{"settings", {}}) != restored.end());
    EXPECT_TRUE(std::find(restored.begin(), restored.end(), ComponentKey{"profile", "alice"}) != restored.end());
}

TEST_F(ComponentPersistenceTest, WritesLayoutAndWorkspaceToTheirOwningSettings) {
    ComponentPersistence persistence(layout, workspace);
    persistence.savePlacement(ComponentKey{"settings", {}}, FloaterPlacement{96.f, 64.f, FloaterLogicalSize{480.f, 320.f}, false},
                              ComponentOpenState::Open);

    EXPECT_TRUE(layout.getLLSD("UILayout")["settings"].isMap());
    EXPECT_TRUE(workspace.getLLSD("UIWorkspace")["settings"].isMap());
}

TEST_F(ComponentPersistenceTest, EncodesAndDecodesComponentIdentityThroughOneCodec) {
    const ComponentKey component{"profile", "alice"};

    EXPECT_TRUE(component.valid());
    EXPECT_EQ(component.persistenceKey(), "alice@profile");
    EXPECT_EQ(ComponentKey::fromPersistenceKey(component.persistenceKey()), component);
    EXPECT_FALSE(ComponentKey::fromPersistenceKey("alice@profile@extra").has_value());
}

TEST_F(ComponentPersistenceTest, SavesOnlyCurrentWorkspaceStateAndClearsLegacyFlags) {
    ComponentPersistence persistence(layout, workspace);
    const std::vector<ComponentInstanceState> components{{{"profile", "alice"}, true}, {{"demo", {}}, false}};
    LLSD previous = LLSD::emptyMap();
    previous["stale@orphan"] = LLSD::emptyMap();
    workspace.setLLSD("UIWorkspace", previous);

    persistence.saveWorkspace(components);

    const LLSD saved = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(saved["alice@profile"]["minimized"].asBoolean());
    EXPECT_FALSE(saved["alice@profile"].has("detached"));
    EXPECT_FALSE(saved["demo"].has("detached"));
    EXPECT_FALSE(saved.has("stale@orphan"));
}

TEST_F(ComponentPersistenceTest, RestoresKeylessPlacementFromUserWideLayout) {
    LLSD saved = LLSD::emptyMap();
    saved["demo"]["position"].append(31.f);
    saved["demo"]["position"].append(42.f);
    saved["demo"]["size"].append(320.f);
    saved["demo"]["size"].append(240.f);
    layout.setLLSD("UILayout", saved);

    ComponentPersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"demo", {}}, FloaterPlacement{});
    ASSERT_TRUE(placement.has_value());
    const auto& restored = *placement;
    EXPECT_FLOAT_EQ(restored.x, 31.f);
    EXPECT_FLOAT_EQ(restored.y, 42.f);
    ASSERT_TRUE(restored.size.has_value());
    EXPECT_FLOAT_EQ(restored.size->width, 320.f);
    EXPECT_FLOAT_EQ(restored.size->height, 240.f);
}

TEST_F(ComponentPersistenceTest, LetsKeyedPlacementOverrideDefaultsWithoutChangingThem) {
    LLSD layoutValue = LLSD::emptyMap();
    layoutValue["profile"]["position"].append(10.f);
    layoutValue["profile"]["position"].append(20.f);
    layoutValue["profile"]["size"].append(400.f);
    layoutValue["profile"]["size"].append(300.f);
    layout.setLLSD("UILayout", layoutValue);

    LLSD workspaceValue = LLSD::emptyMap();
    workspaceValue["alice@profile"]["position"].append(100.f);
    workspaceValue["alice@profile"]["position"].append(200.f);
    workspaceValue["alice@profile"]["size"].append(500.f);
    workspaceValue["alice@profile"]["size"].append(600.f);
    workspace.setLLSD("UIWorkspace", workspaceValue);

    ComponentPersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"profile", "alice"}, FloaterPlacement{});
    ASSERT_TRUE(placement.has_value());
    const auto& restored = *placement;
    EXPECT_FLOAT_EQ(restored.x, 100.f);
    ASSERT_TRUE(restored.size.has_value());
    EXPECT_FLOAT_EQ(restored.size->width, 500.f);

    persistence.savePlacement({"profile", "alice"}, restored, ComponentOpenState::Open);
    EXPECT_DOUBLE_EQ(layout.getLLSD("UILayout")["profile"]["position"][0].asReal(), 10.0);
}

TEST_F(ComponentPersistenceTest, RestoresLegacyDetachedPlacementUsingAttachedFallback) {
    ComponentPersistence persistence(layout, workspace);
    const ComponentKey identity{"profile", "alice"};
    const FloaterPlacement fallback{12.f, 24.f, std::nullopt, false};
    LLSD legacy = LLSD::emptyMap();
    legacy["position"].append(100);
    legacy["position"].append(200);
    legacy["size"].append(640.f);
    legacy["size"].append(480.f);
    legacy["detached"] = true;
    legacy["minimized"] = true;
    LLSD saved = LLSD::emptyMap();
    saved[identity.persistenceKey()] = legacy;
    workspace.setLLSD("UIWorkspace", saved);

    const LLSD encoded = workspace.getLLSD("UIWorkspace")["alice@profile"];
    EXPECT_TRUE(encoded["detached"].asBoolean());
    EXPECT_TRUE(encoded["position"].isArray());
    EXPECT_TRUE(encoded["size"].isArray());
    EXPECT_TRUE(encoded["minimized"].asBoolean());

    const auto restored = persistence.restorePlacement(identity, fallback);
    ASSERT_TRUE(restored.has_value());
    EXPECT_FLOAT_EQ(restored->x, fallback.x);
    EXPECT_FLOAT_EQ(restored->y, fallback.y);
    ASSERT_TRUE(restored->size.has_value());
    EXPECT_FLOAT_EQ(restored->size->width, 640.f);
    EXPECT_TRUE(restored->minimized);

    persistence.savePlacement(identity, *restored, ComponentOpenState::Open);
    EXPECT_FALSE(workspace.getLLSD("UIWorkspace")["alice@profile"].has("detached"));
    EXPECT_DOUBLE_EQ(workspace.getLLSD("UIWorkspace")["alice@profile"]["position"][0].asReal(), static_cast<double>(fallback.x));
    EXPECT_DOUBLE_EQ(workspace.getLLSD("UIWorkspace")["alice@profile"]["position"][1].asReal(), static_cast<double>(fallback.y));
}

TEST_F(ComponentPersistenceTest, CombinesWorkspaceFlagsWithDefaultLayout) {
    LLSD layoutValue = LLSD::emptyMap();
    layoutValue["demo"]["position"].append(1.f);
    layoutValue["demo"]["position"].append(2.f);
    layout.setLLSD("UILayout", layoutValue);

    LLSD workspaceValue = LLSD::emptyMap();
    workspaceValue["demo"]["minimized"] = true;
    workspace.setLLSD("UIWorkspace", workspaceValue);

    ComponentPersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"demo", {}}, FloaterPlacement{});
    ASSERT_TRUE(placement.has_value());
    EXPECT_TRUE(placement->minimized);
}

TEST_F(ComponentPersistenceTest, StoresKeylessFloaterGeometryInLayoutAndOpenStateInWorkspace) {
    ComponentPersistence persistence(layout, workspace);
    const ComponentKey identity{"demo", {}};
    persistence.savePlacement(identity, FloaterPlacement{760.f, 120.f, FloaterLogicalSize{540.f, 680.f}, true}, ComponentOpenState::Open);

    EXPECT_FALSE(layout.getLLSD("UILayout")["demo"].has("detached"));
    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("demo"));
    EXPECT_FALSE(workspace.getLLSD("UIWorkspace")["demo"].has("detached"));

    const auto restored = persistence.restorePlacement(identity, FloaterPlacement{});
    ASSERT_TRUE(restored.has_value());
    EXPECT_FLOAT_EQ(restored->x, 760.f);
}

TEST_F(ComponentPersistenceTest, ClearsStaleKeylessWorkspaceGeometryWhileKeepingMinimizedState) {
    LLSD saved = LLSD::emptyMap();
    saved["demo"]["position"].append(900.f);
    saved["demo"]["position"].append(700.f);
    saved["demo"]["size"].append(640.f);
    saved["demo"]["size"].append(480.f);
    saved["demo"]["detached"] = true;
    workspace.setLLSD("UIWorkspace", saved);

    ComponentPersistence persistence(layout, workspace);
    persistence.savePlacement({"demo", {}}, FloaterPlacement{12.f, 24.f, std::nullopt, true}, ComponentOpenState::Open);

    const LLSD updated = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(updated.has("demo"));
    EXPECT_FALSE(updated["demo"].has("position"));
    EXPECT_FALSE(updated["demo"].has("size"));
    EXPECT_FALSE(updated["demo"].has("detached"));
    EXPECT_TRUE(updated["demo"]["minimized"].asBoolean());
}

TEST_F(ComponentPersistenceTest, RemovesClosedKeyedPlacementFromWorkspace) {
    ComponentPersistence persistence(layout, workspace);
    const ComponentKey identity{"profile", "alice"};
    persistence.savePlacement(identity, FloaterPlacement{10.f, 20.f, std::nullopt, false}, ComponentOpenState::Open);
    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("alice@profile"));

    persistence.savePlacement(identity, FloaterPlacement{30.f, 40.f, std::nullopt, false}, ComponentOpenState::Closed);
    EXPECT_FALSE(workspace.getLLSD("UIWorkspace").has("alice@profile"));
}

TEST_F(ComponentPersistenceTest, PreservesUnavailableWorkspaceEntriesExplicitly) {
    LLSD saved = LLSD::emptyMap();
    saved["alice@profile"] = LLSD::emptyMap();
    workspace.setLLSD("UIWorkspace", saved);

    ComponentPersistence persistence(layout, workspace);
    persistence.saveWorkspace({}, {{"profile", "alice"}});

    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("alice@profile"));
}
