/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
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
#include "binding/settingresolver.h"
#include "componentmanager.h"
#include "controllerregistration.h"
#include "documentcontroller.h"
#include "elements/floater.h"
#include "reloadcoordinator.h"
#include "resolver.h"
#include "skin/compiler.h"
#include "system.h"
#include "test_floater_host.h"

namespace {
using radia::ui::Document;
using radia::ui::FloaterElement;
using radia::ui::ResourceLayer;
using radia::ui::ResourceSnapshot;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::System;
using radia::viewer::ui::ComponentInstanceKey;
using radia::viewer::ui::ComponentManager;
using radia::viewer::ui::DocumentController;
using radia::viewer::ui::SkinReloadCoordinator;
using radia::viewer::ui::SkinReloadResult;
using radia::viewer::ui::SkinSnapshotResult;
using radia::viewer::ui::SkinSnapshotSource;
using radia::viewer::ui::test::TestFloaterHost;
using std::chrono_literals::operator""ms;
using std::chrono_literals::operator""s;

constexpr char kEmptyFloaterView[] = "<floater><head><title>reload</title></head><body/></floater>";

SettingResolver& missingSettingResolver() {
    class MissingSettingResolver final : public SettingResolver {
    public:
        SettingResolution resolve(std::string_view, std::type_index) override { return {SettingResolution::ResolutionStatus::Missing, {}}; }
    };
    static MissingSettingResolver resolver;
    return resolver;
}

ResourceSnapshot skinSnapshot(std::string view = kEmptyFloaterView, std::string style = {}) {
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {strings: {reload.message: Live}}}\n");
    snapshot.add("skin.css", std::move(style));
    snapshot.add("view.html", std::move(view));
    return snapshot;
}

ResourceSnapshot importedStyleSnapshot(std::string entrypoint, std::map<std::string, std::string> modules) {
    ResourceSnapshot snapshot = skinSnapshot(kEmptyFloaterView, entrypoint);
    snapshot.setLayers("skin.css", {ResourceLayer{"test/skin.css", std::move(entrypoint), "skin.css", std::move(modules)}});
    return snapshot;
}

ResourceSnapshot conflictingEventSnapshot() {
    return skinSnapshot(
        R"XML(<floater><head><title>events</title></head><body><button onClick="shared()"/><input type="checkbox" switch="true" onChange="shared()"/></body></floater>)XML");
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
        std::string constructorMessage;
    };

    struct Controller final : DocumentController {
        Controller(System& system, Document& document, ControllerState& state) : DocumentController(system, document), mState(state) {
            mState.constructorMessage = system.resolveText("reload.message");
            handler("shared", &Controller::shared);
        }

        void onReloadSucceeded() override { ++mState.commits; }

        ControllerState& mState;

    private:
        void shared() {}
    };

    using Host = TestFloaterHost;

    void SetUp() override {
        const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot());
        ASSERT_TRUE(prepared.ok());
        ASSERT_TRUE(system.publish(std::move(prepared.generation)));

        ASSERT_TRUE(components.registerDefinition("component", "view.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, componentState);
        }));
        ASSERT_TRUE(components.open("component").ok());
        componentState.commits = 0;
        host.replacements = 0;
    }

    FloaterElement* installed(const std::string& definitionId = "component") const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && components.componentKeyFor(*floater) == ComponentInstanceKey{definitionId, {}}) return floater.get();
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
    components.reportReloadSucceeded();
    EXPECT_EQ(componentState.commits, 1);
    EXPECT_FALSE(update().has_value());
}

TEST_F(SkinReloadCoordinatorTest, BuildsReplacementControllersAgainstCandidateGeneration) {
    EXPECT_EQ(componentState.constructorMessage, "Live");
    snapshotSource.snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {strings: {reload.message: Candidate}}}\n");
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok());
    EXPECT_EQ(componentState.constructorMessage, "Candidate");
    EXPECT_EQ(system.resolveText("reload.message"), "Candidate");
}

TEST_F(SkinReloadCoordinatorTest, RejectsInvalidCandidateWithoutDisturbingLiveState) {
    coordinator.request();
    const auto baseline = update();
    ASSERT_TRUE(baseline.has_value());
    ASSERT_TRUE(baseline->ok());
    FloaterElement* live = installed();
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

TEST_F(SkinReloadCoordinatorTest, AcceptsControllerHandlerAcrossEventTypes) {
    snapshotSource.snapshot = conflictingEventSnapshot();
    FloaterElement* live = installed();
    coordinator.request();

    const auto rejected = update();

    ASSERT_TRUE(rejected.has_value());
    ASSERT_TRUE(rejected->ok());
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_NE(installed(), live);
    EXPECT_EQ(componentState.commits, 0);
    EXPECT_TRUE(rejected->errors.empty());
}

TEST_F(SkinReloadCoordinatorTest, AutoReloadDetectsChangesAfterWatchingIsEnabled) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot = skinSnapshot(kEmptyFloaterView, "floater { width: 420px; }");
    coordinator.setSkinAutoReload(true);

    const auto enabled = update(start);
    ASSERT_TRUE(enabled.has_value());
    ASSERT_TRUE(enabled->ok());
    EXPECT_EQ(system.generation(), 2ULL);
    components.reportReloadSucceeded();

    snapshotSource.snapshot = skinSnapshot(kEmptyFloaterView, "floater { width: 460px; }");
    EXPECT_FALSE(update(start + 250ms).has_value());
    const auto settled = update(start + 500ms);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 3ULL);
    components.reportReloadSucceeded();
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
    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"used.css\";", {{"used.css", "floater { width: 300px; }"}, {"unused.css", "floater { width: 500px; }"}});
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
        importedStyleSnapshot("@import \"used.css\";", {{"used.css", "floater { width: 300px; }"}, {"unused.css", "floater { width: 600px; }"}});
    EXPECT_FALSE(update(start + 250ms).has_value());
    EXPECT_FALSE(update(start + 500ms).has_value());

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"used.css\";", {{"used.css", "floater { width: 420px; }"}, {"unused.css", "floater { width: 600px; }"}});
    EXPECT_FALSE(update(start + 750ms).has_value());
    const auto settled = update(start + 1s);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 4ULL);
}

TEST_F(SkinReloadCoordinatorTest, RetriesARejectedCandidateAfterItsDependencyIsFixed) {
    const auto start = SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"used.css\";", {{"used.css", "floater { width: 300px; }"}, {"new.css", "floater { width: invalid; }"}});
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
        importedStyleSnapshot("@import \"new.css\";", {{"used.css", "floater { width: 300px; }"}, {"new.css", "floater { width: invalid; }"}});
    EXPECT_FALSE(update(start + 250ms).has_value());
    const auto rejected = update(start + 500ms);
    ASSERT_TRUE(rejected.has_value());
    EXPECT_FALSE(rejected->ok());
    EXPECT_EQ(system.generation(), 3ULL);

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"new.css\";", {{"used.css", "floater { width: 300px; }"}, {"new.css", "floater { width: 440px; }"}});
    EXPECT_FALSE(update(start + 750ms).has_value());
    const auto recovered = update(start + 1s);

    ASSERT_TRUE(recovered.has_value());
    ASSERT_TRUE(recovered->ok());
    EXPECT_EQ(system.generation(), 4ULL);
}

TEST_F(SkinReloadCoordinatorTest, ReplacesEveryOpenComponentInOneGeneration) {
    ControllerState secondComponentState;
    ASSERT_TRUE(components.registerDefinition("second", "view.html", [&secondComponentState](System& system, Document& document) {
        return std::make_unique<Controller>(system, document, secondComponentState);
    }));
    ASSERT_TRUE(components.open("second").ok());
    secondComponentState.commits = 0;
    FloaterElement* firstLive = installed();
    FloaterElement* secondLive = installed("second");
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok());
    EXPECT_NE(installed(), firstLive);
    EXPECT_NE(installed("second"), secondLive);
    components.reportReloadSucceeded();
    EXPECT_EQ(componentState.commits, 1);
    EXPECT_EQ(secondComponentState.commits, 1);
    EXPECT_EQ(system.generation(), 2ULL);
}

TEST_F(SkinReloadCoordinatorTest, ReplacesEveryComponentWhenHandlersCoverMultipleEventTypes) {
    ControllerState rejectedComponentState;
    ASSERT_TRUE(components.registerDefinition("second", "view.html", [&rejectedComponentState](System& system, Document& document) {
        return std::make_unique<Controller>(system, document, rejectedComponentState);
    }));
    ASSERT_TRUE(components.open("second").ok());
    componentState.commits = 0;
    rejectedComponentState.commits = 0;
    FloaterElement* firstLive = installed();
    FloaterElement* secondLive = installed("second");
    snapshotSource.snapshot = conflictingEventSnapshot();
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok());
    EXPECT_NE(installed(), firstLive);
    EXPECT_NE(installed("second"), secondLive);
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_EQ(componentState.commits, 0);
    EXPECT_EQ(rejectedComponentState.commits, 0);
}

TEST_F(SkinReloadCoordinatorTest, PreservesLiveStateWhenHostRejectsReplacement) {
    FloaterElement* live = installed();
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

TEST_F(SkinReloadCoordinatorTest, PreservesEveryComponentWhenHostRejectsMultiRootReplacement) {
    ControllerState secondComponentState;
    ASSERT_TRUE(components.registerDefinition("second", "view.html", [&secondComponentState](System& system, Document& document) {
        return std::make_unique<Controller>(system, document, secondComponentState);
    }));
    ASSERT_TRUE(components.open("second").ok());
    componentState.commits = 0;
    secondComponentState.commits = 0;
    FloaterElement* firstLive = installed();
    FloaterElement* secondLive = installed("second");
    host.failCommit = true;
    coordinator.request();

    const auto result = update();

    ASSERT_TRUE(result.has_value());
    EXPECT_FALSE(result->ok());
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(installed(), firstLive);
    EXPECT_EQ(installed("second"), secondLive);
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

    snapshotSource.snapshot = skinSnapshot(kEmptyFloaterView, "floater { width: 420px; }");
    EXPECT_FALSE(update(start + 40ms).has_value());
    EXPECT_FALSE(update(start + 119ms).has_value());
    const auto settled = update(start + 159ms);

    ASSERT_TRUE(settled.has_value());
    ASSERT_TRUE(settled->ok());
    EXPECT_EQ(system.generation(), 3ULL);
}
