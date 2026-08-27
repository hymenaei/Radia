/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <gtest/gtest.h>
#include <string>
#include <tuple>
#include <vector>
#include "llcontrol.h"
#include "workspacepersistence.h"

namespace {
using radia::viewer::ui::ComponentInstanceKey;
using radia::viewer::ui::ComponentInstanceState;
using radia::viewer::ui::ComponentOpenState;
using radia::viewer::ui::FloaterLogicalSize;
using radia::viewer::ui::FloaterPlacement;
using radia::viewer::ui::WorkspacePersistence;

class WorkspacePersistenceTest : public ::testing::Test {
protected:
    void SetUp() override {
        layout.declareLLSD("UILayout", LLSD::emptyMap(), "test layout", LLControlVariable::PERSIST_NO);
        workspace.declareLLSD("UIWorkspace", LLSD::emptyMap(), "test workspace", LLControlVariable::PERSIST_NO);
        layout.setLLSD("UILayout", LLSD::emptyMap());
        workspace.setLLSD("UIWorkspace", LLSD::emptyMap());
    }

    LLControlGroup layout{"workspace-persistence-layout"};
    LLControlGroup workspace{"workspace-persistence-workspace"};
};
} // namespace

TEST_F(WorkspacePersistenceTest, RoundTripsKeyedAndKeylessWorkspaceEntries) {
    WorkspacePersistence persistence(layout, workspace);
    const std::vector<ComponentInstanceKey> components{{"inventory", "one"}, {"inventory", "two"}, {"settings", {}}};

    persistence.saveWorkspace({{components[0]}, {components[1]}, {components[2]}});

    const LLSD saved = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(saved.isMap());
    EXPECT_TRUE(saved.has("one@inventory"));
    EXPECT_TRUE(saved.has("two@inventory"));
    EXPECT_TRUE(saved.has("settings"));

    std::vector<ComponentInstanceKey> restored = persistence.openComponentKeys();
    const auto order = [](const ComponentInstanceKey& left, const ComponentInstanceKey& right) {
        return std::tie(left.definitionId, left.instanceKey) < std::tie(right.definitionId, right.instanceKey);
    };
    std::vector<ComponentInstanceKey> expected = components;
    std::sort(restored.begin(), restored.end(), order);
    std::sort(expected.begin(), expected.end(), order);
    EXPECT_EQ(restored, expected);
}

TEST_F(WorkspacePersistenceTest, IgnoresMalformedWorkspaceIdentities) {
    WorkspacePersistence persistence(layout, workspace);
    LLSD saved = LLSD::emptyMap();
    saved["settings"] = LLSD::emptyMap();
    saved["alice@profile"] = LLSD::emptyMap();
    saved["@missing-component"] = LLSD::emptyMap();
    saved["missing-key@"] = LLSD::emptyMap();
    saved["too@many@separators"] = LLSD::emptyMap();
    saved["not-a-map"] = true;
    workspace.setLLSD("UIWorkspace", saved);

    const std::vector<ComponentInstanceKey> restored = persistence.openComponentKeys();
    ASSERT_EQ(restored.size(), std::size_t{2});
    EXPECT_TRUE(std::find(restored.begin(), restored.end(), ComponentInstanceKey{"settings", {}}) != restored.end());
    EXPECT_TRUE(std::find(restored.begin(), restored.end(), ComponentInstanceKey{"profile", "alice"}) != restored.end());
}

TEST_F(WorkspacePersistenceTest, WritesLayoutAndWorkspaceToTheirOwningSettings) {
    WorkspacePersistence persistence(layout, workspace);
    persistence.savePlacement(ComponentInstanceKey{"settings", {}}, FloaterPlacement{96.f, 64.f, FloaterLogicalSize{480.f, 320.f}, false},
                              ComponentOpenState::Open);

    EXPECT_TRUE(layout.getLLSD("UILayout")["settings"].isMap());
    EXPECT_TRUE(workspace.getLLSD("UIWorkspace")["settings"].isMap());
}

TEST_F(WorkspacePersistenceTest, EncodesAndDecodesComponentInstanceKeyThroughOneCodec) {
    const ComponentInstanceKey component{"profile", "alice"};

    EXPECT_TRUE(component.valid());
    EXPECT_EQ(component.persistenceKey(), "alice@profile");
    EXPECT_EQ(ComponentInstanceKey::fromPersistenceKey(component.persistenceKey()), component);
    EXPECT_FALSE(ComponentInstanceKey::fromPersistenceKey("alice@profile@extra").has_value());
}

TEST_F(WorkspacePersistenceTest, SavesOnlyCurrentWorkspaceState) {
    WorkspacePersistence persistence(layout, workspace);
    const std::vector<ComponentInstanceState> components{{{"profile", "alice"}, true}, {{"demo", {}}, false}};
    LLSD previous = LLSD::emptyMap();
    previous["stale@orphan"] = LLSD::emptyMap();
    workspace.setLLSD("UIWorkspace", previous);

    persistence.saveWorkspace(components);

    const LLSD saved = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(saved["alice@profile"]["minimized"].asBoolean());
    EXPECT_FALSE(saved.has("stale@orphan"));
}

TEST_F(WorkspacePersistenceTest, RestoresKeylessPlacementFromUserWideLayout) {
    LLSD saved = LLSD::emptyMap();
    saved["demo"]["position"].append(31.f);
    saved["demo"]["position"].append(42.f);
    saved["demo"]["size"].append(320.f);
    saved["demo"]["size"].append(240.f);
    layout.setLLSD("UILayout", saved);

    WorkspacePersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"demo", {}});
    ASSERT_TRUE(placement.has_value());
    const auto& restored = *placement;
    EXPECT_FLOAT_EQ(restored.x, 31.f);
    EXPECT_FLOAT_EQ(restored.y, 42.f);
    ASSERT_TRUE(restored.size.has_value());
    EXPECT_FLOAT_EQ(restored.size->width, 320.f);
    EXPECT_FLOAT_EQ(restored.size->height, 240.f);
}

TEST_F(WorkspacePersistenceTest, LetsKeyedPlacementOverrideDefaultsWithoutChangingThem) {
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

    WorkspacePersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"profile", "alice"});
    ASSERT_TRUE(placement.has_value());
    const auto& restored = *placement;
    EXPECT_FLOAT_EQ(restored.x, 100.f);
    ASSERT_TRUE(restored.size.has_value());
    EXPECT_FLOAT_EQ(restored.size->width, 500.f);

    persistence.savePlacement({"profile", "alice"}, restored, ComponentOpenState::Open);
    EXPECT_DOUBLE_EQ(layout.getLLSD("UILayout")["profile"]["position"][0].asReal(), 10.0);
}

TEST_F(WorkspacePersistenceTest, CombinesWorkspaceFlagsWithDefaultLayout) {
    LLSD layoutValue = LLSD::emptyMap();
    layoutValue["demo"]["position"].append(1.f);
    layoutValue["demo"]["position"].append(2.f);
    layout.setLLSD("UILayout", layoutValue);

    LLSD workspaceValue = LLSD::emptyMap();
    workspaceValue["demo"]["minimized"] = true;
    workspace.setLLSD("UIWorkspace", workspaceValue);

    WorkspacePersistence persistence(layout, workspace);
    const auto placement = persistence.restorePlacement({"demo", {}});
    ASSERT_TRUE(placement.has_value());
    EXPECT_TRUE(placement->minimized);
}

TEST_F(WorkspacePersistenceTest, StoresKeylessFloaterGeometryInLayoutAndOpenStateInWorkspace) {
    WorkspacePersistence persistence(layout, workspace);
    const ComponentInstanceKey identity{"demo", {}};
    persistence.savePlacement(identity, FloaterPlacement{760.f, 120.f, FloaterLogicalSize{540.f, 680.f}, true}, ComponentOpenState::Open);

    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("demo"));

    const auto restored = persistence.restorePlacement(identity);
    ASSERT_TRUE(restored.has_value());
    EXPECT_FLOAT_EQ(restored->x, 760.f);
}

TEST_F(WorkspacePersistenceTest, ClearsStaleKeylessWorkspaceGeometryWhileKeepingMinimizedState) {
    LLSD saved = LLSD::emptyMap();
    saved["demo"]["position"].append(900.f);
    saved["demo"]["position"].append(700.f);
    saved["demo"]["size"].append(640.f);
    saved["demo"]["size"].append(480.f);
    workspace.setLLSD("UIWorkspace", saved);

    WorkspacePersistence persistence(layout, workspace);
    persistence.savePlacement({"demo", {}}, FloaterPlacement{12.f, 24.f, std::nullopt, true}, ComponentOpenState::Open);

    const LLSD updated = workspace.getLLSD("UIWorkspace");
    EXPECT_TRUE(updated.has("demo"));
    EXPECT_FALSE(updated["demo"].has("position"));
    EXPECT_FALSE(updated["demo"].has("size"));
    EXPECT_TRUE(updated["demo"]["minimized"].asBoolean());
}

TEST_F(WorkspacePersistenceTest, RemovesClosedKeyedPlacementFromWorkspace) {
    WorkspacePersistence persistence(layout, workspace);
    const ComponentInstanceKey identity{"profile", "alice"};
    persistence.savePlacement(identity, FloaterPlacement{10.f, 20.f, std::nullopt, false}, ComponentOpenState::Open);
    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("alice@profile"));

    persistence.savePlacement(identity, FloaterPlacement{30.f, 40.f, std::nullopt, false}, ComponentOpenState::Closed);
    EXPECT_FALSE(workspace.getLLSD("UIWorkspace").has("alice@profile"));
}

TEST_F(WorkspacePersistenceTest, PreservesUnavailableWorkspaceEntriesExplicitly) {
    LLSD saved = LLSD::emptyMap();
    saved["alice@profile"] = LLSD::emptyMap();
    workspace.setLLSD("UIWorkspace", saved);

    WorkspacePersistence persistence(layout, workspace);
    persistence.saveWorkspace({}, {{"profile", "alice"}});

    EXPECT_TRUE(workspace.getLLSD("UIWorkspace").has("alice@profile"));
}
