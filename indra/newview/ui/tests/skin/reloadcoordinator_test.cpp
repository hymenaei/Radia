/**
 * @file reloadcoordinator_test.cpp
 * @brief Tests debounced Skin reload requests, diagnostics, and generation commits.
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
#include <chrono>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <typeindex>
#include <utility>
#include <vector>
#include "../test_floater_host.h"
#include "binding/settingresolver.h"
#include "componentcontroller.h"
#include "componentcontrollerregistration.h"
#include "componentmanager.h"
#include "skin/compiler.h"
#include "skin/reloadcoordinator.h"
#include "skin/resolver.h"
#include "system.h"
#include "widgets/floater.h"

namespace {
using radia::ui::Floater;
using radia::ui::ResourceLayer;
using radia::ui::ResourceSnapshot;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::System;
using radia::viewer::ui::ComponentController;
using radia::viewer::ui::ComponentKey;
using radia::viewer::ui::ComponentManager;
using radia::viewer::ui::SkinReloadCoordinator;
using radia::viewer::ui::SkinReloadResult;
using radia::viewer::ui::SkinSnapshotResult;
using radia::viewer::ui::SkinSnapshotSource;
using radia::viewer::ui::test::TestFloaterHost;
using std::chrono_literals::operator""ms;
using std::chrono_literals::operator""s;

SettingResolver& missingSettingResolver() {
    class MissingSettingResolver final : public SettingResolver {
    public:
        SettingResolution resolve(std::string_view, std::type_index) override { return {SettingResolution::ResolutionStatus::Missing, {}}; }
    };
    static MissingSettingResolver resolver;
    return resolver;
}

ResourceSnapshot skinSnapshot(std::string view = "<floater/>", std::string style = {}) {
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n");
    snapshot.add("skin.radia", std::move(style));
    snapshot.add("view.xml", std::move(view));
    return snapshot;
}

ResourceSnapshot importedStyleSnapshot(std::string entrypoint, std::map<std::string, std::string> modules) {
    ResourceSnapshot snapshot = skinSnapshot("<floater/>", entrypoint);
    snapshot.setLayers("skin.radia", {ResourceLayer{"test/skin.radia", std::move(entrypoint), "skin.radia", std::move(modules)}});
    return snapshot;
}

ResourceSnapshot conflictingEventSnapshot() {
    return skinSnapshot(R"XML(<floater><button onClick="shared()"/><switch onChange="shared()"/></floater>)XML");
}

class SkinReloadCoordinatorTest : public ::testing::Test {
protected:
    struct SnapshotSource final : SkinSnapshotSource {
        SkinSnapshotResult capture() const override {
            ++captures;
            SkinSnapshotResult result;
            result.snapshot = snapshot;
            if (rejectCapture) result.error("skin.test.rejected", "Test manifest rejected.");
            return result;
        }

        ResourceSnapshot snapshot = skinSnapshot();
        bool rejectCapture = false;
        mutable int captures = 0;
    };

    struct ControllerState {
        int commits = 0;
    };

    struct Controller final : ComponentController {
        Controller(System& system, ControllerState& state) : ComponentController(system), mState(state) { event("shared", &Controller::shared); }

        void postBuild() override { ++mState.commits; }

        ControllerState& mState;

    private:
        void shared() {}
    };

    using Host = TestFloaterHost;

    void SetUp() override {
        const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot());
        ASSERT_TRUE(prepared.ok());
        system.publish(std::move(prepared.generation));

        ASSERT_TRUE(components.registerDefinition("component", "view.xml",
                                                  [this](System& system) { return std::make_unique<Controller>(system, componentState); }));
        ASSERT_TRUE(components.open("component").ok());
        componentState.commits = 0;
        host.replacements = 0;
    }

    Floater* installed(const std::string& definitionId = "component") const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && components.componentKeyFor(*floater) == ComponentKey{definitionId, {}}) return floater.get();
        return nullptr;
    }

    std::optional<SkinReloadResult> update(SkinReloadCoordinator::TimePoint now = {}) { return coordinator.update(now, components); }

    SnapshotSource snapshotSource;
    System system;
    Host host;
    ControllerState componentState;
    ComponentManager components{system, host, missingSettingResolver()};
    SkinReloadCoordinator coordinator{system, snapshotSource};
};
} // namespace

TEST_F(SkinReloadCoordinatorTest, CommitsRequestedReloadAsOneTransaction) {
    EXPECT_FALSE(update().has_value());
    EXPECT_EQ(snapshotSource.captures, 0);

    coordinator.request();
    const auto result = update();

    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok());
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_EQ(result->generationNumber, 2ULL);
    EXPECT_NE(installed(), nullptr);
    EXPECT_EQ(componentState.commits, 1);
    EXPECT_FALSE(update().has_value());
}

TEST_F(SkinReloadCoordinatorTest, RejectsInvalidCandidateWithoutDisturbingLiveState) {
    coordinator.request();
    const auto baseline = update();
    ASSERT_TRUE(baseline.has_value());
    ASSERT_TRUE(baseline->ok());
    Floater* live = installed();
    const int commits = componentState.commits;

    snapshotSource.snapshot.add("localization.yaml", "defaultLocale: [");
    coordinator.request();
    const auto rejected = update();

    ASSERT_TRUE(rejected.has_value());
    EXPECT_FALSE(rejected->ok());
    EXPECT_FALSE(rejected->errors.empty());
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_EQ(installed(), live);
    EXPECT_EQ(componentState.commits, commits);
}

TEST_F(SkinReloadCoordinatorTest, RejectsControllerPreparationBeforePublishingCandidate) {
    snapshotSource.snapshot = conflictingEventSnapshot();
    Floater* live = installed();
    coordinator.request();

    const auto rejected = update();

    ASSERT_TRUE(rejected.has_value());
    EXPECT_FALSE(rejected->ok());
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(installed(), live);
    EXPECT_EQ(componentState.commits, 0);
    ASSERT_FALSE(rejected->errors.empty());
    EXPECT_EQ(rejected->errors.front().code, "binding.event.kind_mismatch");
}

TEST_F(SkinReloadCoordinatorTest, AutoReloadDetectsChangesAfterWatchingIsEnabled) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot = skinSnapshot("<floater/>", "floater { width: 420px; }");
    coordinator.setSkinAutoReload(true);

    const auto enabled = update(start);
    ASSERT_TRUE(enabled.has_value());
    ASSERT_TRUE(enabled->ok());
    EXPECT_EQ(system.generation(), 2ULL);

    snapshotSource.snapshot = skinSnapshot("<floater/>", "floater { width: 460px; }");
    EXPECT_FALSE(update(start + 250ms).has_value());
    const auto settled = update(start + 500ms);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 3ULL);
    EXPECT_EQ(componentState.commits, 2);
    EXPECT_FALSE(update(start + 750ms).has_value());
}

TEST_F(SkinReloadCoordinatorTest, ReturnsSnapshotResolutionDiagnosticsBeforePreparation) {
    snapshotSource.rejectCapture = true;
    coordinator.request();

    const auto rejected = update();

    ASSERT_TRUE(rejected.has_value());
    EXPECT_FALSE(rejected->ok());
    ASSERT_FALSE(rejected->errors.empty());
    EXPECT_EQ(rejected->errors.front().code, "skin.test.rejected");
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(componentState.commits, 0);
}

TEST_F(SkinReloadCoordinatorTest, ReloadsOnlyImportedStylesheetChanges) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 300px; }"}, {"unused.radia", "floater { width: 500px; }"}});
    coordinator.request();
    const auto baseline = update();
    ASSERT_TRUE(baseline.has_value());
    ASSERT_TRUE(baseline->ok());
    coordinator.setSkinAutoReload(true);

    const auto enabled = update(start);
    ASSERT_TRUE(enabled.has_value());
    ASSERT_TRUE(enabled->ok());
    EXPECT_EQ(system.generation(), 3ULL);

    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 300px; }"}, {"unused.radia", "floater { width: 600px; }"}});
    EXPECT_FALSE(update(start + 250ms).has_value());
    EXPECT_FALSE(update(start + 500ms).has_value());

    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 420px; }"}, {"unused.radia", "floater { width: 600px; }"}});
    EXPECT_FALSE(update(start + 750ms).has_value());
    const auto settled = update(start + 1s);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 4ULL);
}

TEST_F(SkinReloadCoordinatorTest, RetriesARejectedCandidateAfterItsDependencyIsFixed) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"used.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: invalid; }"}});
    coordinator.request();
    const auto baseline = update();
    ASSERT_TRUE(baseline.has_value());
    ASSERT_TRUE(baseline->ok());
    coordinator.setSkinAutoReload(true);

    const auto enabled = update(start);
    ASSERT_TRUE(enabled.has_value());
    ASSERT_TRUE(enabled->ok());
    EXPECT_EQ(system.generation(), 3ULL);

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"new.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: invalid; }"}});
    EXPECT_FALSE(update(start + 250ms).has_value());
    const auto rejected = update(start + 500ms);
    ASSERT_TRUE(rejected.has_value());
    EXPECT_FALSE(rejected->ok());
    EXPECT_EQ(system.generation(), 3ULL);

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"new.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: 440px; }"}});
    EXPECT_FALSE(update(start + 750ms).has_value());
    const auto recovered = update(start + 1s);

    ASSERT_TRUE(recovered.has_value());
    ASSERT_TRUE(recovered->ok());
    EXPECT_EQ(system.generation(), 4ULL);
}

TEST_F(SkinReloadCoordinatorTest, ReplacesEveryOpenComponentInOneGeneration) {
    ControllerState secondComponentState;
    ASSERT_TRUE(components.registerDefinition(
        "second", "view.xml", [&secondComponentState](System& system) { return std::make_unique<Controller>(system, secondComponentState); }));
    ASSERT_TRUE(components.open("second").ok());
    secondComponentState.commits = 0;
    Floater* firstLive = installed();
    Floater* secondLive = installed("second");
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok());
    EXPECT_NE(installed(), firstLive);
    EXPECT_NE(installed("second"), secondLive);
    EXPECT_EQ(componentState.commits, 1);
    EXPECT_EQ(secondComponentState.commits, 1);
    EXPECT_EQ(system.generation(), 2ULL);
}

TEST_F(SkinReloadCoordinatorTest, RollsBackEveryComponentWhenPreparationRejectsOne) {
    ControllerState rejectedComponentState;
    ASSERT_TRUE(components.registerDefinition(
        "second", "view.xml", [&rejectedComponentState](System& system) { return std::make_unique<Controller>(system, rejectedComponentState); }));
    ASSERT_TRUE(components.open("second").ok());
    componentState.commits = 0;
    rejectedComponentState.commits = 0;
    Floater* firstLive = installed();
    Floater* secondLive = installed("second");
    snapshotSource.snapshot = conflictingEventSnapshot();
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    EXPECT_FALSE(result->ok());
    EXPECT_EQ(installed(), firstLive);
    EXPECT_EQ(installed("second"), secondLive);
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(componentState.commits, 0);
    EXPECT_EQ(rejectedComponentState.commits, 0);
}

TEST_F(SkinReloadCoordinatorTest, PreservesLiveStateWhenHostRejectsReplacement) {
    Floater* live = installed();
    host.rejectReplacements = true;
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    EXPECT_FALSE(result->ok());
    ASSERT_FALSE(result->errors.empty());
    EXPECT_EQ(result->errors.front().code, "floater.host.replace_failed");
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(installed(), live);
    EXPECT_EQ(componentState.commits, 0);
}

TEST_F(SkinReloadCoordinatorTest, RollsBackEverySwapWhenHostFailsMidCommit) {
    ControllerState secondComponentState;
    ASSERT_TRUE(components.registerDefinition(
        "second", "view.xml", [&secondComponentState](System& system) { return std::make_unique<Controller>(system, secondComponentState); }));
    ASSERT_TRUE(components.open("second").ok());
    componentState.commits = 0;
    secondComponentState.commits = 0;
    Floater* firstLive = installed();
    Floater* secondLive = installed("second");
    host.failAfterFirst = true;
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    EXPECT_FALSE(result->ok());
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(installed(), firstLive);
    EXPECT_EQ(installed("second"), secondLive);
    EXPECT_FALSE(host.rollbackInvariantViolated);
    EXPECT_EQ(componentState.commits, 0);
    EXPECT_EQ(secondComponentState.commits, 0);
    EXPECT_EQ(host.replacements, 0);
}

TEST_F(SkinReloadCoordinatorTest, HonorsConfiguredAutomaticReloadIntervals) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    ASSERT_TRUE(coordinator.setAutoReloadTiming({40ms, 80ms}));
    coordinator.setSkinAutoReload(true);

    const auto enabled = update(start);
    ASSERT_TRUE(enabled.has_value());
    ASSERT_TRUE(enabled->ok());
    EXPECT_EQ(system.generation(), 2ULL);

    snapshotSource.snapshot = skinSnapshot("<floater/>", "floater { width: 420px; }");
    EXPECT_FALSE(update(start + 40ms).has_value());
    EXPECT_FALSE(update(start + 119ms).has_value());
    const auto settled = update(start + 159ms);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 3ULL);
}
