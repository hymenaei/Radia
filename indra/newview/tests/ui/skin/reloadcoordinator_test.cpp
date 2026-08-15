/**
 * @file reloadcoordinator_test.cpp
 * @brief Tests debounced skin reload requests, diagnostics, and generation commits.
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
#include <map>
#include <memory>
#include <utility>
#include <vector>
#include "../test/lltut.h"
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

namespace tut {
namespace {

radia::ui::SettingResolver& missingSettingResolver() {
    class MissingSettingResolver final : public radia::ui::SettingResolver {
    public:
        radia::ui::SettingResolution resolve(std::string_view, std::type_index) override {
            return {radia::ui::SettingResolution::ResolutionStatus::Missing, {}};
        }
    };
    static MissingSettingResolver resolver;
    return resolver;
}

radia::ui::ResourceSnapshot skinSnapshot(std::string view = "<floater/>", std::string style = {}) {
    radia::ui::ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n");
    snapshot.add("skin.radia", std::move(style));
    snapshot.add("view.xml", std::move(view));
    return snapshot;
}

radia::ui::ResourceSnapshot importedStyleSnapshot(std::string entrypoint, std::map<std::string, std::string> modules) {
    radia::ui::ResourceSnapshot snapshot = skinSnapshot("<floater/>", entrypoint);
    snapshot.setLayers("skin.radia", {radia::ui::ResourceLayer{"test/skin.radia", std::move(entrypoint), "skin.radia", std::move(modules)}});
    return snapshot;
}

radia::ui::ResourceSnapshot conflictingEventSnapshot() {
    return skinSnapshot(R"XML(<floater><button onClick="shared()"/><switch onChange="shared()"/></floater>)XML");
}
} // namespace

struct reloadCoordinatorData {
    struct SnapshotSource final : radia::viewer::ui::SkinSnapshotSource {
        radia::viewer::ui::SkinSnapshotResult capture() const override {
            ++captures;
            radia::viewer::ui::SkinSnapshotResult result;
            result.snapshot = snapshot;
            if (rejectCapture) result.error("skin.test.rejected", "Test manifest rejected.");
            return result;
        }

        radia::ui::ResourceSnapshot snapshot = skinSnapshot();
        bool rejectCapture = false;
        mutable int captures = 0;
    };

    struct ControllerState {
        int commits = 0;
    };

    struct Controller final : radia::viewer::ui::ComponentController {
        Controller(radia::ui::System& system, ControllerState& state) : radia::viewer::ui::ComponentController(system), mState(state) {
            event("shared", &Controller::shared);
        }

        void postBuild() override { ++mState.commits; }

        ControllerState& mState;

    private:
        void shared() {}
    };

    using Host = radia::viewer::ui::test::TestFloaterHost;

    SnapshotSource snapshotSource;
    radia::ui::System system;
    Host host;
    ControllerState componentState;
    radia::viewer::ui::ComponentManager components{system, host, missingSettingResolver()};
    radia::viewer::ui::SkinReloadCoordinator coordinator{system, snapshotSource};

    reloadCoordinatorData() {
        radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(skinSnapshot());
        if (prepared.ok()) system.publish(std::move(prepared.generation));
        components.registerDefinition("component", "view.xml",
                                      [this](radia::ui::System& system) { return std::make_unique<Controller>(system, componentState); });
        components.open("component");
        componentState.commits = 0;
        host.replacements = 0;
    }

    radia::ui::Floater* installed(const std::string& definitionId = "component") const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && components.componentKeyFor(*floater) == radia::viewer::ui::ComponentKey{definitionId, {}}) return floater.get();
        return nullptr;
    }

    std::optional<radia::viewer::ui::SkinReloadResult> update(radia::viewer::ui::SkinReloadCoordinator::TimePoint now = {}) {
        return coordinator.update(now, components);
    }
};
using reloadCoordinatorTest = test_group<reloadCoordinatorData>;
using reloadCoordinatorObject = reloadCoordinatorTest::object;
reloadCoordinatorTest reloadCoordinatorTestCase("UISkinReloadCoordinator");

template<> template<> void reloadCoordinatorObject::test<1>() {
    set_test_name("requested reload publishes and installs one complete transaction");
    ensure("idle update has no transaction", !update().has_value());

    coordinator.request();
    const auto result = update();

    ensure("reload produced a result", result.has_value());
    ensure("reload committed", result->ok());
    ensure_equals("candidate generation published", system.generation(), 2ULL);
    ensure_equals("result reports generation", result->generationNumber, 2ULL);
    ensure("replacement installed", installed() != nullptr);
    ensure_equals("controller committed once", componentState.commits, 1);
    ensure("request consumed", !update().has_value());
}

template<> template<> void reloadCoordinatorObject::test<2>() {
    set_test_name("invalid candidate preserves the live generation and component");
    coordinator.request();
    ensure("baseline committed", update()->ok());
    radia::ui::Floater* live = installed();
    const int commits = componentState.commits;

    snapshotSource.snapshot = skinSnapshot();
    snapshotSource.snapshot.add("localization.yaml", "defaultLocale: [");
    coordinator.request();
    const auto rejected = update();

    ensure("rejection produced a result", rejected.has_value());
    ensure("invalid candidate rejected", !rejected->ok());
    ensure("diagnostics returned", !rejected->errors.empty());
    ensure_equals("live generation preserved", system.generation(), 2ULL);
    ensure("live Floater preserved", installed() == live);
    ensure_equals("controller commit not reached", componentState.commits, commits);
}

template<> template<> void reloadCoordinatorObject::test<3>() {
    set_test_name("controller preparation rejection prevents publication and installation");
    snapshotSource.snapshot = conflictingEventSnapshot();
    radia::ui::Floater* live = installed();
    coordinator.request();
    const auto rejected = update();

    ensure("controller rejection returned", rejected.has_value());
    ensure("controller rejection failed", !rejected->ok());
    ensure_equals("candidate was not published", system.generation(), 1ULL);
    ensure("live component was preserved", installed() == live);
    ensure_equals("controller not committed", componentState.commits, 0);
    ensure_equals("Event diagnostic retained", rejected->errors.front().code, std::string("binding.event.kind_mismatch"));
}

template<> template<> void reloadCoordinatorObject::test<4>() {
    set_test_name("authoring changes debounce into one coherent snapshot reload");
    using namespace std::chrono_literals;
    const auto start = radia::viewer::ui::SkinReloadCoordinator::TimePoint{} + 1s;
    coordinator.setSkinAutoReload(true);
    ensure("first poll establishes baseline", !update(start).has_value());

    snapshotSource.snapshot = skinSnapshot("<floater/>", "floater { width: 420px; }");
    ensure("changed snapshot waits to settle", !update(start + 250ms).has_value());
    const auto settled = update(start + 500ms);

    ensure("settled snapshot committed", settled && settled->ok());
    ensure_equals("one candidate generation published", system.generation(), 2ULL);
    ensure_equals("one controller committed", componentState.commits, 1);
    ensure("acknowledged snapshot does not repeat", !update(start + 750ms).has_value());
}

template<> template<> void reloadCoordinatorObject::test<5>() {
    set_test_name("snapshot resolution diagnostics reject before generation preparation");
    snapshotSource.rejectCapture = true;
    coordinator.request();

    const auto rejected = update();

    ensure("resolution rejection returned", rejected.has_value());
    ensure("resolution rejection failed", !rejected->ok());
    ensure_equals("diagnostic retained", rejected->errors.front().code, std::string("skin.test.rejected"));
    ensure_equals("candidate generation not published", system.generation(), 1ULL);
    ensure_equals("controller not committed", componentState.commits, 0);
}

template<> template<> void reloadCoordinatorObject::test<6>() {
    set_test_name("authoring reload invalidates only imported stylesheet modules");
    using namespace std::chrono_literals;
    const auto start = radia::viewer::ui::SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 300px; }"}, {"unused.radia", "floater { width: 500px; }"}});
    coordinator.request();
    ensure("dependency baseline publishes", update()->ok());
    coordinator.setSkinAutoReload(true);
    ensure("first poll establishes authoring baseline", !update(start).has_value());

    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 300px; }"}, {"unused.radia", "floater { width: 600px; }"}});
    ensure("unused module edit is ignored", !update(start + 250ms).has_value());
    ensure("unused module remains ignored", !update(start + 500ms).has_value());

    snapshotSource.snapshot = importedStyleSnapshot("@import \"used.radia\";",
                                                    {{"used.radia", "floater { width: 420px; }"}, {"unused.radia", "floater { width: 600px; }"}});
    ensure("imported edit waits to settle", !update(start + 750ms).has_value());
    const auto settled = update(start + 1s);

    ensure("imported module edit reloads", settled && settled->ok());
    ensure_equals("only dependency edit publishes again", system.generation(), 3ULL);
}

template<> template<> void reloadCoordinatorObject::test<7>() {
    set_test_name("rejected candidate retries when its new dependency changes");
    using namespace std::chrono_literals;
    const auto start = radia::viewer::ui::SkinReloadCoordinator::TimePoint{} + 1s;
    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"used.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: invalid; }"}});
    coordinator.request();
    ensure("live dependency baseline publishes", update()->ok());
    coordinator.setSkinAutoReload(true);
    ensure("first poll establishes authoring baseline", !update(start).has_value());

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"new.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: invalid; }"}});
    ensure("new import waits to settle", !update(start + 250ms).has_value());
    const auto rejected = update(start + 500ms);
    ensure("invalid new dependency is rejected", rejected && !rejected->ok());
    ensure_equals("rejection preserves live generation", system.generation(), 2ULL);

    snapshotSource.snapshot =
        importedStyleSnapshot("@import \"new.radia\";", {{"used.radia", "floater { width: 300px; }"}, {"new.radia", "floater { width: 440px; }"}});
    ensure("dependency fix waits to settle", !update(start + 750ms).has_value());
    const auto recovered = update(start + 1s);

    ensure("fixed rejected dependency retries", recovered && recovered->ok());
    ensure_equals("fixed candidate publishes", system.generation(), 3ULL);
}

template<> template<> void reloadCoordinatorObject::test<8>() {
    set_test_name("one generation atomically replaces every open component");
    ControllerState secondComponentState;
    ensure("second definition registered", components.registerDefinition("second", "view.xml", [&secondComponentState](radia::ui::System& system) {
        return std::make_unique<reloadCoordinatorData::Controller>(system, secondComponentState);
    }));
    ensure("second component opened", components.open("second").ok());
    secondComponentState.commits = 0;
    radia::ui::Floater* firstLive = installed();
    radia::ui::Floater* secondLive = installed("second");
    coordinator.request();

    const auto result = coordinator.update({}, components);

    ensure("multi-component reload committed", result && result->ok());
    ensure("first component replaced", installed() != firstLive);
    ensure("second component replaced", installed("second") != secondLive);
    ensure_equals("first controller committed", componentState.commits, 1);
    ensure_equals("second controller committed", secondComponentState.commits, 1);
    ensure_equals("one candidate generation published", system.generation(), 2ULL);
}

template<> template<> void reloadCoordinatorObject::test<9>() {
    set_test_name("one rejected controller preparation rolls back every open component");
    ControllerState rejectedComponentState;
    ensure("second definition registered", components.registerDefinition("second", "view.xml", [&rejectedComponentState](radia::ui::System& system) {
        return std::make_unique<reloadCoordinatorData::Controller>(system, rejectedComponentState);
    }));
    ensure("second component opened", components.open("second").ok());
    componentState.commits = 0;
    rejectedComponentState.commits = 0;
    radia::ui::Floater* firstLive = installed();
    radia::ui::Floater* secondLive = installed("second");
    snapshotSource.snapshot = conflictingEventSnapshot();
    coordinator.request();

    const auto result = coordinator.update({}, components);

    ensure("transaction rejected", result && !result->ok());
    ensure("first component preserved", installed() == firstLive);
    ensure("second component preserved", installed("second") == secondLive);
    ensure_equals("live generation preserved", system.generation(), 1ULL);
    ensure_equals("first controller not committed", componentState.commits, 0);
    ensure_equals("rejected controller not committed", rejectedComponentState.commits, 0);
}

template<> template<> void reloadCoordinatorObject::test<10>() {
    set_test_name("host rejection preserves the live generation");
    radia::ui::Floater* live = installed();
    host.rejectReplacements = true;
    coordinator.request();

    const auto result = coordinator.update({}, components);

    ensure("host rejection returned", result && !result->ok());
    ensure_equals("host diagnostic retained", result->errors.front().code, std::string("floater.host.replace_failed"));
    ensure_equals("generation was not published", system.generation(), 1ULL);
    ensure("live component preserved", installed() == live);
    ensure_equals("controller not committed", componentState.commits, 0);
}

template<> template<> void reloadCoordinatorObject::test<11>() {
    set_test_name("host publication failure rolls back every swapped root");
    ControllerState secondComponentState;
    ensure("second definition registered", components.registerDefinition("second", "view.xml", [&secondComponentState](radia::ui::System& system) {
        return std::make_unique<reloadCoordinatorData::Controller>(system, secondComponentState);
    }));
    ensure("second component opened", components.open("second").ok());
    componentState.commits = 0;
    secondComponentState.commits = 0;
    radia::ui::Floater* firstLive = installed();
    radia::ui::Floater* secondLive = installed("second");
    host.failAfterFirst = true;
    coordinator.request();

    const auto result = coordinator.update({}, components);

    ensure("host publication failure rejected", result && !result->ok());
    ensure_equals("generation remains live", system.generation(), 1ULL);
    ensure("first root rolled back", installed() == firstLive);
    ensure("second root rolled back", installed("second") == secondLive);
    ensure("host rollback preserved its invariant", !host.rollbackInvariantViolated);
    ensure_equals("first controller not committed", componentState.commits, 0);
    ensure_equals("second controller not committed", secondComponentState.commits, 0);
    ensure_equals("host swaps rolled back", host.replacements, 0);
}

template<> template<> void reloadCoordinatorObject::test<12>() {
    set_test_name("automatic reload uses configured scan and settle intervals");
    using namespace std::chrono_literals;
    const auto start = radia::viewer::ui::SkinReloadCoordinator::TimePoint{} + 1s;
    ensure("custom timing is accepted", coordinator.setAutoReloadTiming({40ms, 80ms}));
    coordinator.setSkinAutoReload(true);
    ensure("first poll establishes baseline", !update(start).has_value());

    snapshotSource.snapshot = skinSnapshot("<floater/>", "floater { width: 420px; }");
    ensure("changed snapshot waits for the configured scan interval", !update(start + 40ms).has_value());
    ensure("changed snapshot waits for the configured settle interval", !update(start + 119ms).has_value());
    const auto settled = update(start + 159ms);

    ensure("configured intervals trigger one reload", settled && settled->ok());
}
} // namespace tut
