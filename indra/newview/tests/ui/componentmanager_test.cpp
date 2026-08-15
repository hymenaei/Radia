/**
 * @file componentmanager_test.cpp
 * @brief Tests component registration, opening, binding, and skin replacement behavior.
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
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "../test/lltut.h"
#include "binding/settingresolver.h"
#include "binding/valuebinding.h"
#include "componentcontroller.h"
#include "componentcontrollerregistration.h"
#include "componentmanager.h"
#include "skin/compiler.h"
#include "system.h"
#include "test_floater_host.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/switch.h"
#include "widgets/text.h"

namespace tut {
namespace {

radia::ui::Widget* findWidget(radia::ui::Widget& root, const std::string& id) {
    if (root.id() == id) return &root;
    for (const auto& child : root.children())
        if (radia::ui::Widget* found = findWidget(*child, id)) return found;
    return nullptr;
}

class TestBooleanBinding final : public radia::ui::ValueBinding<bool> {
public:
    explicit TestBooleanBinding(bool value) : mState{value, value, std::nullopt} {}

    radia::ui::ValueState<bool> state() const override { return mState; }
    void write(bool value) override {
        mState.value = value;
        notify();
    }
    radia::ui::ValueBindingSubscription observe(Observer observer) override {
        const std::size_t id = mNextObserver++;
        mObservers.emplace(id, std::move(observer));
        return radia::ui::ValueBindingSubscription([this, id] { mObservers.erase(id); });
    }

private:
    void notify() {
        const auto observers = mObservers;
        for (const auto& [id, observer] : observers)
            if (mObservers.find(id) != mObservers.end()) observer(mState);
    }

    radia::ui::ValueState<bool> mState;
    std::map<std::size_t, Observer> mObservers;
    std::size_t mNextObserver = 1;
};

class TestSettingResolver final : public radia::ui::SettingResolver {
public:
    TestSettingResolver() : binding(std::make_shared<TestBooleanBinding>(true)) {}

    radia::ui::SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override {
        if (settingName != "test-enabled") return {radia::ui::SettingResolution::ResolutionStatus::Missing, {}};
        if (requestedType != typeid(bool)) return {radia::ui::SettingResolution::ResolutionStatus::TypeMismatch, {}};
        return {radia::ui::SettingResolution::ResolutionStatus::Found, binding};
    }

    std::shared_ptr<TestBooleanBinding> binding;
};
} // namespace

struct componentManagerData {
    struct ControllerState {
        int postBuild = 0;
        int opened = 0;
        int closed = 0;
        int widgetsAvailable = 0;
        int succeeded = 0;
        int failed = 0;
        int presses = 0;
        bool multipleArguments = false;
        bool statusAvailable = false;
        bool changeObserved = false;
        bool doubleClickObserved = false;
        bool mouseDownObserved = false;
        bool mouseUpObserved = false;
        bool mouseMoveObserved = false;
        bool longClickObserved = false;
        bool contextMenuObserved = false;
    };

    class Controller final : public radia::viewer::ui::ComponentController {
    public:
        Controller(radia::ui::System& system, ControllerState& state) : radia::viewer::ui::ComponentController(system), mState(state) {
            event("press", &Controller::press);
            event("inspect", &Controller::inspect);
            event("changed", &Controller::changed);
            event("doubleClick", &Controller::doubleClick);
            event("mouseDown", &Controller::mouseDown);
            event("mouseUp", &Controller::mouseUp);
            event("mouseMove", &Controller::mouseMove);
            event("longClick", &Controller::longClick);
            event("contextMenu", &Controller::contextMenu);
        }

        void postBuild() override {
            ++mState.postBuild;
            mState.statusAvailable = static_cast<bool>(mStatus);
            if (mState.statusAvailable) ++mState.widgetsAvailable;
        }
        void onOpen() override { ++mState.opened; }
        void onClose() override { ++mState.closed; }
        void onReloadSucceeded() override { ++mState.succeeded; }
        void onReloadFailed(const radia::ui::DiagnosticResult&) override { ++mState.failed; }

    private:
        void press() {
            ++mState.presses;
            mStatus.setContent(localize("controller.ready"));
        }

        void inspect(int index, std::string destination, bool enabled, radia::viewer::ui::Widget source, const radia::viewer::ui::ClickEvent& event) {
            mState.multipleArguments =
                index == 4 && destination == "settings" && enabled && source.id() == "inspect" && event.source().id() == source.id();
        }

        void changed(const radia::viewer::ui::ChangeEvent& event) { mState.changeObserved = event.checked; }
        void doubleClick(const radia::viewer::ui::MouseWidgetEvent&) { mState.doubleClickObserved = true; }
        void mouseDown(const radia::viewer::ui::MouseWidgetEvent&) { mState.mouseDownObserved = true; }
        void mouseUp(const radia::viewer::ui::MouseWidgetEvent&) { mState.mouseUpObserved = true; }
        void mouseMove(const radia::viewer::ui::MouseWidgetEvent&) { mState.mouseMoveObserved = true; }
        void longClick(const radia::viewer::ui::LongClickEvent&) { mState.longClickObserved = true; }
        void contextMenu(const radia::viewer::ui::MouseWidgetEvent&) { mState.contextMenuObserved = true; }

        ControllerState& mState;
        radia::viewer::ui::Widget& mStatus = getWidgetById("status");
    };

    class ForeignController final : public radia::viewer::ui::ComponentController {
    public:
        explicit ForeignController(radia::ui::System& system) : radia::viewer::ui::ComponentController(system) {}
        void wrongTarget() {}
    };

    class MismatchedController final : public radia::viewer::ui::ComponentController {
    public:
        explicit MismatchedController(radia::ui::System& system) : radia::viewer::ui::ComponentController(system) {
            event("wrongTarget", &ForeignController::wrongTarget);
        }
    };

    using Host = radia::viewer::ui::test::TestFloaterHost;

    componentManagerData() : manager(system, host, resolver) {
        radia::ui::SkinGenerationPrepareResult prepared = prepareGeneration();
        if (prepared.ok()) system.publish(std::move(prepared.generation));
    }

    static radia::ui::SkinGenerationPrepareResult prepareGeneration(bool includeStatus = true) {
        radia::ui::ResourceSnapshot resources;
        resources.add("localization.yaml", "defaultLocale: en\nlocales: {en: {name: English, strings: {controller.ready: Ready}}}\n");
        resources.add("skin.radia", "");
        const std::string kStatus = includeStatus ? "<text id=\"status\"/>" : "";
        const std::string kView = "<floater>"
            + kStatus
            + "<button id=\"press\" onClick=\"press()\"/><button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"/><button id=\"events\" onDoubleClick=\"doubleClick(event)\" onMouseDown=\"mouseDown(event)\" onMouseUp=\"mouseUp(event)\" onMouseMove=\"mouseMove(event)\" onLongClick=\"longClick(event)\" onContextMenu=\"contextMenu(event)\" longClickDelay=\"250ms\"/><switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/><switch id=\"setting\" setting=\"test-enabled\"/></floater>";
        resources.add("one.xml", kView);
        resources.add("two.xml", kView);
        resources.add("empty.xml", "<floater/>");
        const std::string kPanel = "<panel>"
            + kStatus
            + "<button id=\"press\" onClick=\"press()\"/><switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/></panel>";
        resources.add("panel.xml", kPanel);
        return radia::ui::SkinCompiler().prepare(std::move(resources));
    }

    bool registerOne(const std::string& definition = "one") {
        return manager.registerDefinition(definition, "one.xml",
                                          [this](radia::ui::System& system) { return std::make_unique<Controller>(system, controllerState); });
    }

    bool registerEmpty() {
        return manager.registerDefinition("empty", "empty.xml", [this](radia::ui::System& system) {
            class EmptyController final : public radia::viewer::ui::ComponentController {
            public:
                explicit EmptyController(radia::ui::System& system) : radia::viewer::ui::ComponentController(system) {}
            };
            return std::make_unique<EmptyController>(system);
        });
    }

    bool registerPanel() {
        return manager.registerDefinition("panel", "panel.xml",
                                          [this](radia::ui::System& system) { return std::make_unique<Controller>(system, controllerState); });
    }

    bool registerMismatched() {
        return manager.registerDefinition("mismatched", "empty.xml",
                                          [this](radia::ui::System& system) { return std::make_unique<MismatchedController>(system); });
    }

    std::vector<radia::ui::Floater*> liveFloaters() const {
        std::vector<radia::ui::Floater*> result;
        for (const auto& [root, floater] : host.mounted)
            if (floater && !floater->closed()) result.push_back(floater.get());
        return result;
    }

    std::optional<radia::viewer::ui::ComponentKey> keyFor(const radia::ui::Floater& floater) const { return manager.componentKeyFor(floater); }

    radia::ui::Floater* mountedFor(const radia::viewer::ui::ComponentKey& key) const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && manager.componentKeyFor(*floater) == key) return floater.get();
        return nullptr;
    }

    radia::ui::System system;
    Host host;
    TestSettingResolver resolver;
    ControllerState controllerState;
    radia::viewer::ui::ComponentManager manager;
};
using componentManagerTest = test_group<componentManagerData>;
using componentManagerObject = componentManagerTest::object;
componentManagerTest componentManagerTestCase("UIComponentManager");

template<> template<> void componentManagerObject::test<1>() {
    set_test_name("registering a definition does not open a floater");
    ensure("definition registered", registerOne());
    ensure("no live instances", liveFloaters().empty());
    ensure_equals("host remains empty", host.mounted.size(), std::size_t(0));
}

template<> template<> void componentManagerObject::test<2>() {
    set_test_name("one definition opens independently keyed instances");
    ensure("definition registered", registerOne("profile"));

    const auto first = manager.open("profile", "alice");
    const auto second = manager.open("profile", "bob");

    ensure("first instance opened", first.ok());
    ensure("second instance opened", second.ok());
    ensure("instances are distinct", first.floater != second.floater);
    ensure("first stable identity mounted", manager.componentKeyFor(*first.floater) == radia::viewer::ui::ComponentKey{"profile", "alice"});
    ensure("second stable identity mounted", manager.componentKeyFor(*second.floater) == radia::viewer::ui::ComponentKey{"profile", "bob"});
    ensure_equals("two live instances", liveFloaters().size(), std::size_t(2));
}

template<> template<> void componentManagerObject::test<3>() {
    set_test_name("opening the same singleton shows its existing instance");
    ensure("definition registered", registerOne());
    const auto first = manager.open("one");
    const auto second = manager.open("one");

    ensure("singleton opened", first.ok() && second.ok());
    ensure("same instance returned", first.floater == second.floater);
    ensure_equals("host mounted once", host.mounted.size(), std::size_t(1));
    ensure_equals("existing instance shown", host.presentations, 2);
}

template<> template<> void componentManagerObject::test<4>() {
    set_test_name("reload replacement retains component identity and controller lifecycle");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("instance opened", opened.ok());
    ensure_equals("post-build ran after initial commit", controllerState.postBuild, 1);
    ensure_equals("open hook ran after initial commit", controllerState.opened, 1);
    ensure_equals("controller Widget handle resolved", controllerState.widgetsAvailable, 1);
    auto* press = dynamic_cast<radia::ui::Button*>(findWidget(*opened.floater, "press"));
    auto* inspect = dynamic_cast<radia::ui::Button*>(findWidget(*opened.floater, "inspect"));
    auto* status = dynamic_cast<radia::ui::Text*>(findWidget(*opened.floater, "status"));
    ensure("zero-argument Event source exists", press != nullptr);
    ensure("multi-argument Event source exists", inspect != nullptr);
    auto* events = dynamic_cast<radia::ui::Button*>(findWidget(*opened.floater, "events"));
    auto* changed = dynamic_cast<radia::ui::Switch*>(findWidget(*opened.floater, "changed"));
    ensure("typed Event source exists", events != nullptr);
    ensure("typed Change source exists", changed != nullptr);
    ensure("double-click Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::DoubleClick));
    ensure("mouse-down Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::MouseDown));
    ensure("mouse-up Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::MouseUp));
    ensure("mouse-move Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::MouseMove));
    ensure("long-click Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::LongClick));
    ensure("context-menu Event Call is bound", events && events->eventCall(radia::ui::WidgetEventKind::ContextMenu));
    press->activate();
    inspect->activate();
    changed->activate();
    ensure_equals("zero-argument Event dispatched", controllerState.presses, 1);
    ensure("multi-argument Event dispatched", controllerState.multipleArguments);
    ensure("typed Change Event dispatched", controllerState.changeObserved);
    ensure("localized Widget exists", status != nullptr);
    ensure_equals("controller localization applied", status->text(), std::string("Ready"));
    opened.floater->close();
    ensure_equals("close hook ran", controllerState.closed, 1);
    ensure("closed instance reopened", manager.open("one").ok());
    ensure_equals("reopen hook ran", controllerState.opened, 2);
    radia::ui::Floater* original = opened.floater;
    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());
    ensure("replacement not installed before commit", liveFloaters().front() == original);
    ensure("replacement committed", prepared.replacement.commit());
    radia::ui::Floater* replacement = liveFloaters().front();
    manager.idle();
    manager.reportReloadSucceeded();
    radia::ui::DiagnosticResult diagnostics;
    manager.reportReloadFailed(diagnostics);

    ensure("replacement installed", replacement != original);
    ensure_equals("post-build ran for replacement", controllerState.postBuild, 2);
    ensure_equals("replacement Widget handle resolved", controllerState.widgetsAvailable, 2);
    ensure_equals("host replaced once", host.replacements, 1);
    ensure_equals("component key retained", keyFor(*replacement)->definitionId, std::string("one"));
    ensure_equals("success reported", controllerState.succeeded, 1);
    ensure_equals("failure reported", controllerState.failed, 1);
}

template<> template<> void componentManagerObject::test<5>() {
    set_test_name("open component snapshot preserves definition and instance key");
    ensure("definition registered", registerOne("profile"));
    const auto first = manager.open("profile", "alice");
    const auto second = manager.open("profile", "bob");
    ensure("instances opened", first.ok() && second.ok());

    auto openComponents = [&] {
        std::vector<radia::viewer::ui::ComponentKey> result;
        manager.forEachOpen([&](const radia::viewer::ui::ComponentKey& key, radia::ui::Floater&) { result.push_back(key); });
        return result;
    };

    second.floater->close();
    const std::vector<radia::viewer::ui::ComponentKey> closed = openComponents();
    ensure_equals("only open instance retained", closed.size(), std::size_t(1));
    ensure_equals("definition retained", closed.front().definitionId, std::string("profile"));
    ensure_equals("instance key retained", closed.front().instanceKey, std::string("alice"));

    ensure("closed instance reopened", manager.open("profile", "bob").ok());
    ensure_equals("reopened instance retained", openComponents().size(), std::size_t(2));
}

template<> template<> void componentManagerObject::test<6>() {
    set_test_name("missing optional controller Widget warns and becomes unavailable after reload");
    ensure("definition registered", registerOne());
    ensure("instance opened", manager.open("one").ok());
    ensure_equals("initial Widget available", controllerState.widgetsAvailable, 1);

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration(false);
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement with missing Widget remains valid", prepared.ok());
    ensure("missing Widget warning returned", !prepared.warnings.empty());
    ensure_equals("missing Widget diagnostic is stable", prepared.warnings.front().code, std::string("controller.widget.missing"));
    ensure("replacement committed", prepared.replacement.commit());
    ensure("missing Widget is explicitly unavailable", !controllerState.statusAvailable);
    ensure_equals("missing Widget is unavailable after commit", controllerState.widgetsAvailable, 1);

    auto* press = dynamic_cast<radia::ui::Button*>(findWidget(*liveFloaters().front(), "press"));
    ensure("remaining Event source exists", press != nullptr);
    press->activate();
    ensure_equals("Event remains live with missing optional Widget", controllerState.presses, 1);
}

template<> template<> void componentManagerObject::test<7>() {
    set_test_name("a host rejection leaves the current component and controller untouched");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("instance opened", opened.ok());
    radia::ui::Floater* original = opened.floater;

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());

    host.rejectReplacements = true;
    ensure("host rejection is reported", !prepared.replacement.commit());
    ensure("current component remains installed", liveFloaters().front() == original);
    ensure_equals("host did not partially replace", host.replacements, 0);
    ensure_equals("replacement controller was not committed", controllerState.postBuild, 1);
}

template<> template<> void componentManagerObject::test<8>() {
    set_test_name("closed Floaters are evicted before replacement");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("instance opened", opened.ok());
    radia::ui::WidgetRef<radia::ui::Floater> evictedRoot(opened.floater);
    opened.floater->close();
    ensure("instance is closed before reload", opened.floater->closed());
    const int openedCount = controllerState.opened;

    manager.idle();
    ensure("closed instance is removed from live Floaters", liveFloaters().empty());
    ensure("closed root is unmounted", host.mounted.empty());
    ensure("closed root is omitted from live Floaters", liveFloaters().empty());
    ensure("evicted root lifetime ended", !evictedRoot);

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());
    ensure("replacement committed", prepared.replacement.commit());
    ensure_equals("reload does not invoke onOpen for an evicted Floater", controllerState.opened, openedCount);

    const auto reopened = manager.open("one");
    ensure("evicted Floater reopens", reopened.ok());
    ensure("reopen creates a fresh controller generation", controllerState.postBuild == 2);
    ensure_equals("fresh root runs onOpen", controllerState.opened, openedCount + 1);
    ensure_equals("fresh root runs postBuild", controllerState.postBuild, 2);
}

template<> template<> void componentManagerObject::test<9>() {
    set_test_name("prepared replacement cannot outlive its component manager");
    Host temporaryHost;
    std::optional<radia::viewer::ui::ComponentManager::PreparedReplacement> orphan;
    {
        radia::viewer::ui::ComponentManager temporary(system, temporaryHost, resolver);
        ensure("definition registered", temporary.registerDefinition("one", "one.xml", [this](radia::ui::System& system) {
            return std::make_unique<componentManagerData::Controller>(system, controllerState);
        }));
        ensure("instance opened", temporary.open("one").ok());
        radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
        ensure("candidate generation prepared", generation.ok());
        auto prepared = temporary.prepareReplacement(*generation.generation, "en");
        ensure("replacement prepared", prepared.ok());
        orphan.emplace(std::move(prepared.replacement));
    }
    ensure("stale replacement is rejected", !orphan->commit());
}

template<> template<> void componentManagerObject::test<10>() {
    set_test_name("a failed host publication preserves the live generation");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("instance opened", opened.ok());
    radia::ui::Floater* original = opened.floater;

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());

    host.failCommit = true;
    ensure("failed publication is reported", !prepared.replacement.commit());
    ensure("live component remains installed", liveFloaters().front() == original);
    ensure_equals("failed publication did not replace a root", host.replacements, 0);
    ensure_equals("failed publication did not commit the controller", controllerState.postBuild, 1);
}

template<> template<> void componentManagerObject::test<11>() {
    set_test_name("a failed multi-root publication rolls back earlier roots");
    ensure("first definition registered", registerOne("one"));
    ensure("second definition registered", registerOne("two"));
    const auto first = manager.open("one", "first");
    const auto second = manager.open("two", "second");
    ensure("both instances opened", first.ok() && second.ok());
    radia::ui::Floater* originalFirst = first.floater;
    radia::ui::Floater* originalSecond = second.floater;

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("multi-root replacement prepared", prepared.ok());

    host.failAfterFirst = true;
    ensure("failed multi-root replacement publication is reported", !prepared.replacement.commit());
    ensure("first root was rolled back", mountedFor({"one", "first"}) == originalFirst);
    ensure("second root remains installed", mountedFor({"two", "second"}) == originalSecond);
    ensure("host rollback preserved its invariant", !host.rollbackInvariantViolated);
    ensure_equals("no replacement remains counted after rollback", host.replacements, 0);
    ensure_equals("controllers were not committed after rollback", controllerState.postBuild, 2);
}

template<> template<> void componentManagerObject::test<12>() {
    set_test_name("a controller with no Widgets or Event Handlers still mounts");
    ensure("definition registered", registerEmpty());
    const auto opened = manager.open("empty");
    ensure("empty controller opened", opened.ok());
    ensure("empty root mounted", opened.floater != nullptr && manager.componentKeyFor(*opened.floater) == radia::viewer::ui::ComponentKey{"empty", {}});
}

template<> template<> void componentManagerObject::test<13>() {
    set_test_name("a member Event Handler from another Controller type is rejected during preparation");
    ensure("definition registered", registerMismatched());
    const auto opened = manager.open("mismatched");
    ensure("mismatched controller is rejected", !opened.ok());
    ensure("invalid registration diagnostic is reported",
           !opened.errors.empty() && opened.errors.front().code == "binding.event.registration_invalid");
    ensure("invalid controller was not mounted", host.mounted.empty());
}

template<> template<> void componentManagerObject::test<14>() {
    set_test_name("a Panel root is rejected by the Floater host contract");
    ensure("Panel definition registered", registerPanel());
    const auto opened = manager.open("panel");
    ensure("Panel component is rejected", !opened.ok());
    ensure("Panel rejection identifies the root contract", !opened.errors.empty() && opened.errors.front().code == "component.root.type_mismatch");
    ensure("Panel root is not mounted", host.mounted.empty());
}

template<> template<> void componentManagerObject::test<15>() {
    set_test_name("setting bindings are resolved and committed with a Component mount");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("instance opened", opened.ok());
    auto* setting = dynamic_cast<radia::ui::Switch*>(findWidget(*opened.floater, "setting"));
    ensure("setting control exists", setting != nullptr);
    ensure("resolver state commits during mount", setting && setting->checked());
    resolver.binding->write(false);
    ensure("external setting update reaches mounted control", !setting->checked());
    setting->activate();
    ensure("user activation writes through the resolver", resolver.binding->state().value);
}

template<> template<> void componentManagerObject::test<16>() {
    set_test_name("clearing an account session closes and unmounts components");
    ensure("definition registered", registerOne());
    ensure("component opened", manager.open("one").ok());
    ensure("component manager clears", manager.clearInstances());
    ensure("no live components remain", liveFloaters().empty());
    ensure("host roots are unmounted", host.mounted.empty());
    ensure_equals("controller close hook ran", controllerState.closed, 1);
}

template<> template<> void componentManagerObject::test<17>() {
    set_test_name("prepared replacement is rejected after account reset");
    ensure("definition registered", registerOne());
    ensure("component opened", manager.open("one").ok());

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());
    ensure("component manager clears", manager.clearInstances());
    ensure("stale replacement is rejected", !prepared.replacement.commit());
}

template<> template<> void componentManagerObject::test<18>() {
    set_test_name("prepared replacement is rejected after idle eviction");
    ensure("definition registered", registerOne());
    const auto opened = manager.open("one");
    ensure("component opened", opened.ok());

    radia::ui::SkinGenerationPrepareResult generation = prepareGeneration();
    ensure("candidate generation prepared", generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ensure("replacement prepared", prepared.ok());
    opened.floater->close();
    manager.idle();
    ensure("stale replacement is rejected", !prepared.replacement.commit());
}

template<> template<> void componentManagerObject::test<19>() {
    set_test_name("failed clear preparation leaves every component and controller untouched");
    ensure("first definition registered", registerOne("one"));
    ensure("second definition registered", registerOne("two"));
    const auto first = manager.open("one");
    const auto second = manager.open("two");
    ensure("both components opened", first.ok() && second.ok());

    host.rejectClears = true;
    ensure("clear failure is reported", !manager.clearInstances());
    ensure_equals("both host roots remain mounted", host.mounted.size(), std::size_t(2));
    ensure("first root remains mapped", manager.componentKeyFor(*first.floater) == radia::viewer::ui::ComponentKey{"one", {}});
    ensure("second root remains mapped", manager.componentKeyFor(*second.floater) == radia::viewer::ui::ComponentKey{"two", {}});
    ensure("first root remains open", !first.floater->closed());
    ensure("second root remains open", !second.floater->closed());
    ensure_equals("clear preparation does not close controllers", controllerState.closed, 0);
}
} // namespace tut
