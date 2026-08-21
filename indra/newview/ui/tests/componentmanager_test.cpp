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

namespace {
using radia::ui::Button;
using radia::ui::DiagnosticResult;
using radia::ui::Floater;
using radia::ui::ResourceSnapshot;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Switch;
using radia::ui::System;
using radia::ui::Text;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetRef;
using radia::viewer::ui::ChangeEvent;
using radia::viewer::ui::ClickEvent;
using radia::viewer::ui::ComponentController;
using radia::viewer::ui::ComponentKey;
using radia::viewer::ui::ComponentManager;
using radia::viewer::ui::LongClickEvent;
using radia::viewer::ui::MouseWidgetEvent;
using radia::viewer::ui::test::TestFloaterHost;
using ControllerWidget = radia::viewer::ui::Widget;
using CoreWidget = radia::ui::Widget;

CoreWidget* findWidget(CoreWidget& root, std::string_view id) {
    if (root.id() == id) return &root;
    for (const auto& child : root.children())
        if (CoreWidget* found = findWidget(*child, id)) return found;
    return nullptr;
}

class TestBooleanBinding final : public ValueBinding<bool> {
public:
    explicit TestBooleanBinding(bool value) : mState{value, value, std::nullopt} {}

    ValueState<bool> state() const override { return mState; }

    void write(bool value) override {
        mState.value = value;
        notify();
    }

    ValueBindingSubscription observe(Observer observer) override {
        const std::size_t id = mNextObserver++;
        mObservers.emplace(id, std::move(observer));
        return ValueBindingSubscription([this, id] { mObservers.erase(id); });
    }

private:
    void notify() {
        const auto observers = mObservers;
        for (const auto& [id, observer] : observers)
            if (mObservers.find(id) != mObservers.end()) observer(mState);
    }

    ValueState<bool> mState;
    std::map<std::size_t, Observer> mObservers;
    std::size_t mNextObserver = 1;
};

class TestSettingResolver final : public SettingResolver {
public:
    TestSettingResolver() : binding(std::make_shared<TestBooleanBinding>(true)) {}

    SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override {
        if (settingName != "test-enabled") return {SettingResolution::ResolutionStatus::Missing, {}};
        if (requestedType != typeid(bool)) return {SettingResolution::ResolutionStatus::TypeMismatch, {}};
        return {SettingResolution::ResolutionStatus::Found, binding};
    }

    std::shared_ptr<TestBooleanBinding> binding;
};

class ComponentManagerTest : public ::testing::Test {
protected:
    struct ControllerState {
        int postBuildCount = 0;
        int openCount = 0;
        int closeCount = 0;
        int availableWidgetCount = 0;
        int reloadSuccessCount = 0;
        int reloadFailureCount = 0;
        int pressCount = 0;
        bool inspectArgumentsValid = false;
        bool statusAvailable = false;
        bool changeObserved = false;
        bool doubleClickObserved = false;
        bool mouseDownObserved = false;
        bool mouseUpObserved = false;
        bool mouseMoveObserved = false;
        bool longClickObserved = false;
        bool contextMenuObserved = false;
    };

    class Controller final : public ComponentController {
    public:
        Controller(System& system, ControllerState& state) : ComponentController(system), mState(state) {
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
            ++mState.postBuildCount;
            mState.statusAvailable = static_cast<bool>(mStatus);
            if (mState.statusAvailable) ++mState.availableWidgetCount;
        }

        void onOpen() override { ++mState.openCount; }
        void onClose() override { ++mState.closeCount; }
        void onReloadSucceeded() override { ++mState.reloadSuccessCount; }
        void onReloadFailed(const DiagnosticResult&) override { ++mState.reloadFailureCount; }

    private:
        void press() {
            ++mState.pressCount;
            mStatus.setContent(localize("controller.ready"));
        }

        void inspect(int index, std::string destination, bool enabled, ControllerWidget source, const ClickEvent& event) {
            mState.inspectArgumentsValid =
                index == 4 && destination == "settings" && enabled && source.id() == "inspect" && event.source().id() == source.id();
        }

        void changed(const ChangeEvent& event) { mState.changeObserved = event.checked; }
        void doubleClick(const MouseWidgetEvent&) { mState.doubleClickObserved = true; }
        void mouseDown(const MouseWidgetEvent&) { mState.mouseDownObserved = true; }
        void mouseUp(const MouseWidgetEvent&) { mState.mouseUpObserved = true; }
        void mouseMove(const MouseWidgetEvent&) { mState.mouseMoveObserved = true; }
        void longClick(const LongClickEvent&) { mState.longClickObserved = true; }
        void contextMenu(const MouseWidgetEvent&) { mState.contextMenuObserved = true; }

        ControllerState& mState;
        ControllerWidget& mStatus = getWidgetById("status");
    };

    class ForeignController final : public ComponentController {
    public:
        explicit ForeignController(System& system) : ComponentController(system) {}
        void wrongTarget() {}
    };

    class MismatchedController final : public ComponentController {
    public:
        explicit MismatchedController(System& system) : ComponentController(system) { event("wrongTarget", &ForeignController::wrongTarget); }
    };

    using Host = TestFloaterHost;

    void SetUp() override {
        const SkinGenerationPrepareResult prepared = prepareGeneration();
        ASSERT_TRUE(prepared.ok());
        system.publish(std::move(prepared.generation));
    }

    static SkinGenerationPrepareResult prepareGeneration(bool includeStatus = true) {
        constexpr char kLocalization[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {controller.ready: Ready}}}\n";
        constexpr char kViewWithStatus[] = "<floater>"
                                           "<p id=\"status\"/>"
                                           "<button id=\"press\" onClick=\"press()\"/>"
                                           "<button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"/>"
                                           "<button id=\"events\" onDoubleClick=\"doubleClick(event)\" onMouseDown=\"mouseDown(event)\" "
                                           "onMouseUp=\"mouseUp(event)\" onMouseMove=\"mouseMove(event)\" onLongClick=\"longClick(event)\" "
                                           "onContextMenu=\"contextMenu(event)\" longClickDelay=\"250ms\"/>"
                                           "<switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/>"
                                           "<switch id=\"setting\" setting=\"test-enabled\"/>"
                                           "</floater>";
        constexpr char kViewWithoutStatus[] = "<floater>"
                                              "<button id=\"press\" onClick=\"press()\"/>"
                                              "<button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"/>"
                                              "<button id=\"events\" onDoubleClick=\"doubleClick(event)\" onMouseDown=\"mouseDown(event)\" "
                                              "onMouseUp=\"mouseUp(event)\" onMouseMove=\"mouseMove(event)\" onLongClick=\"longClick(event)\" "
                                              "onContextMenu=\"contextMenu(event)\" longClickDelay=\"250ms\"/>"
                                              "<switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/>"
                                              "<switch id=\"setting\" setting=\"test-enabled\"/>"
                                              "</floater>";
        constexpr char kPanelWithStatus[] = "<panel>"
                                            "<p id=\"status\"/>"
                                            "<button id=\"press\" onClick=\"press()\"/>"
                                            "<switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/>"
                                            "</panel>";
        constexpr char kPanelWithoutStatus[] = "<panel>"
                                               "<button id=\"press\" onClick=\"press()\"/>"
                                               "<switch id=\"changed\" checked=\"false\" onChange=\"changed(event)\"/>"
                                               "</panel>";

        ResourceSnapshot resources;
        resources.add("localization.yaml", kLocalization);
        resources.add("skin.radia", "");
        resources.add("one.xml", includeStatus ? kViewWithStatus : kViewWithoutStatus);
        resources.add("two.xml", includeStatus ? kViewWithStatus : kViewWithoutStatus);
        resources.add("empty.xml", "<floater/>");
        resources.add("panel.xml", includeStatus ? kPanelWithStatus : kPanelWithoutStatus);
        return SkinCompiler().prepare(std::move(resources));
    }

    bool registerOne(std::string definitionId = "one") {
        return manager.registerDefinition(std::move(definitionId), "one.xml",
                                          [this](System& system) { return std::make_unique<Controller>(system, controllerState); });
    }

    bool registerEmpty() {
        return manager.registerDefinition("empty", "empty.xml", [](System& system) {
            class EmptyController final : public ComponentController {
            public:
                explicit EmptyController(System& system) : ComponentController(system) {}
            };
            return std::make_unique<EmptyController>(system);
        });
    }

    bool registerPanel() {
        return manager.registerDefinition("panel", "panel.xml",
                                          [this](System& system) { return std::make_unique<Controller>(system, controllerState); });
    }

    bool registerMismatched() {
        return manager.registerDefinition("mismatched", "empty.xml", [](System& system) { return std::make_unique<MismatchedController>(system); });
    }

    std::vector<Floater*> liveFloaters() const {
        std::vector<Floater*> result;
        for (const auto& [root, floater] : host.mounted)
            if (floater && !floater->closed()) result.push_back(floater.get());
        return result;
    }

    std::optional<ComponentKey> keyFor(const Floater& floater) const { return manager.componentKeyFor(floater); }

    Floater* mountedFor(const ComponentKey& key) const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && manager.componentKeyFor(*floater) == key) return floater.get();
        return nullptr;
    }

    System system;
    Host host;
    TestSettingResolver resolver;
    ControllerState controllerState;
    ComponentManager manager{system, host, resolver};
};
} // namespace

TEST_F(ComponentManagerTest, RegistersDefinitionWithoutOpeningIt) {
    ASSERT_TRUE(registerOne());
    EXPECT_TRUE(liveFloaters().empty());
    EXPECT_TRUE(host.mounted.empty());
}

TEST_F(ComponentManagerTest, OpensIndependentInstancesForDifferentKeys) {
    ASSERT_TRUE(registerOne("profile"));

    const auto first = manager.open("profile", "alice");
    const auto second = manager.open("profile", "bob");

    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());
    ASSERT_NE(first.floater, second.floater);
    const ComponentKey firstKey{"profile", "alice"};
    const ComponentKey secondKey{"profile", "bob"};
    EXPECT_EQ(manager.componentKeyFor(*first.floater), firstKey);
    EXPECT_EQ(manager.componentKeyFor(*second.floater), secondKey);
    EXPECT_EQ(liveFloaters().size(), std::size_t{2});
}

TEST_F(ComponentManagerTest, PresentsExistingInstanceForARepeatedSingletonOpen) {
    ASSERT_TRUE(registerOne());

    const auto first = manager.open("one");
    const auto second = manager.open("one");

    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());
    EXPECT_EQ(first.floater, second.floater);
    EXPECT_EQ(host.mounted.size(), std::size_t{1});
    EXPECT_EQ(host.presentations, 2);
}

TEST_F(ComponentManagerTest, OpensComponentAndDispatchesTypedEvents) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());

    EXPECT_EQ(controllerState.postBuildCount, 1);
    EXPECT_EQ(controllerState.openCount, 1);
    EXPECT_EQ(controllerState.availableWidgetCount, 1);

    auto* press = dynamic_cast<Button*>(findWidget(*opened.floater, "press"));
    auto* inspect = dynamic_cast<Button*>(findWidget(*opened.floater, "inspect"));
    auto* events = dynamic_cast<Button*>(findWidget(*opened.floater, "events"));
    auto* changed = dynamic_cast<Switch*>(findWidget(*opened.floater, "changed"));
    auto* status = dynamic_cast<Text*>(findWidget(*opened.floater, "status"));
    ASSERT_NE(press, nullptr);
    ASSERT_NE(inspect, nullptr);
    ASSERT_NE(events, nullptr);
    ASSERT_NE(changed, nullptr);
    ASSERT_NE(status, nullptr);

    EXPECT_TRUE(events->eventCall(WidgetEventKind::DoubleClick));
    EXPECT_TRUE(events->eventCall(WidgetEventKind::MouseDown));
    EXPECT_TRUE(events->eventCall(WidgetEventKind::MouseUp));
    EXPECT_TRUE(events->eventCall(WidgetEventKind::MouseMove));
    EXPECT_TRUE(events->eventCall(WidgetEventKind::LongClick));
    EXPECT_TRUE(events->eventCall(WidgetEventKind::ContextMenu));

    press->activate();
    inspect->activate();
    changed->activate();

    EXPECT_EQ(controllerState.pressCount, 1);
    EXPECT_TRUE(controllerState.inspectArgumentsValid);
    EXPECT_TRUE(controllerState.changeObserved);
    EXPECT_EQ(status->text(), "Ready");
}

TEST_F(ComponentManagerTest, ReplacesOpenComponentWithoutChangingItsIdentity) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    Floater* original = opened.floater;

    opened.floater->close();
    ASSERT_TRUE(manager.open("one").ok());
    EXPECT_EQ(controllerState.openCount, 2);

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_EQ(liveFloaters().size(), std::size_t{1});
    EXPECT_EQ(liveFloaters().front(), original);

    ASSERT_TRUE(prepared.replacement.commit());
    ASSERT_EQ(liveFloaters().size(), std::size_t{1});
    Floater* replacement = liveFloaters().front();
    manager.idle();
    manager.reportReloadSucceeded();
    const DiagnosticResult diagnostics;
    manager.reportReloadFailed(diagnostics);

    EXPECT_NE(replacement, original);
    EXPECT_EQ(controllerState.postBuildCount, 2);
    EXPECT_EQ(controllerState.availableWidgetCount, 2);
    EXPECT_EQ(host.replacements, 1);
    const ComponentKey replacementKey{"one", {}};
    EXPECT_EQ(keyFor(*replacement), replacementKey);
    EXPECT_EQ(controllerState.reloadSuccessCount, 1);
    EXPECT_EQ(controllerState.reloadFailureCount, 1);
}

TEST_F(ComponentManagerTest, ReportsOpenComponentKeysAfterClosingAndReopeningAnInstance) {
    ASSERT_TRUE(registerOne("profile"));
    const auto first = manager.open("profile", "alice");
    const auto second = manager.open("profile", "bob");
    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());

    const auto openComponents = [this] {
        std::vector<ComponentKey> result;
        manager.forEachOpen([&](const ComponentKey& key, Floater&) { result.push_back(key); });
        return result;
    };

    second.floater->close();
    const std::vector<ComponentKey> openAfterClose = openComponents();
    ASSERT_EQ(openAfterClose.size(), std::size_t{1});
    const ComponentKey remainingKey{"profile", "alice"};
    EXPECT_EQ(openAfterClose.front(), remainingKey);

    ASSERT_TRUE(manager.open("profile", "bob").ok());
    EXPECT_EQ(openComponents().size(), std::size_t{2});
}

TEST_F(ComponentManagerTest, KeepsRemainingBindingsLiveWhenAnOptionalWidgetDisappears) {
    ASSERT_TRUE(registerOne());
    ASSERT_TRUE(manager.open("one").ok());
    EXPECT_EQ(controllerState.availableWidgetCount, 1);

    const SkinGenerationPrepareResult generation = prepareGeneration(false);
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_FALSE(prepared.warnings.empty());
    EXPECT_EQ(prepared.warnings.front().code, "controller.widget.missing");
    ASSERT_TRUE(prepared.replacement.commit());

    EXPECT_FALSE(controllerState.statusAvailable);
    EXPECT_EQ(controllerState.availableWidgetCount, 1);
    auto* press = dynamic_cast<Button*>(findWidget(*liveFloaters().front(), "press"));
    ASSERT_NE(press, nullptr);
    press->activate();
    EXPECT_EQ(controllerState.pressCount, 1);
}

TEST_F(ComponentManagerTest, LeavesCurrentComponentUntouchedWhenTheHostRejectsReplacement) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    Floater* original = opened.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.rejectReplacements = true;
    EXPECT_FALSE(prepared.replacement.commit());
    EXPECT_EQ(liveFloaters().front(), original);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.postBuildCount, 1);
}

TEST_F(ComponentManagerTest, EvictsClosedFloatersBeforeReplacement) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    WidgetRef<Floater> evictedRoot(opened.floater);
    opened.floater->close();
    ASSERT_TRUE(opened.floater->closed());
    const int openCount = controllerState.openCount;

    manager.idle();
    EXPECT_TRUE(liveFloaters().empty());
    EXPECT_TRUE(host.mounted.empty());
    EXPECT_FALSE(evictedRoot);

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(prepared.replacement.commit());
    EXPECT_EQ(controllerState.openCount, openCount);

    const auto reopened = manager.open("one");
    ASSERT_TRUE(reopened.ok());
    EXPECT_EQ(controllerState.postBuildCount, 2);
    EXPECT_EQ(controllerState.openCount, openCount + 1);
}

TEST_F(ComponentManagerTest, RejectsPreparedReplacementAfterItsManagerIsDestroyed) {
    Host temporaryHost;
    std::optional<ComponentManager::PreparedReplacement> orphan;
    {
        ComponentManager temporary(system, temporaryHost, resolver);
        ASSERT_TRUE(
            temporary.registerDefinition("one", "one.xml", [this](System& system) { return std::make_unique<Controller>(system, controllerState); }));
        ASSERT_TRUE(temporary.open("one").ok());
        const SkinGenerationPrepareResult generation = prepareGeneration();
        ASSERT_TRUE(generation.ok());
        auto prepared = temporary.prepareReplacement(*generation.generation, "en");
        ASSERT_TRUE(prepared.ok());
        orphan.emplace(std::move(prepared.replacement));
    }

    ASSERT_TRUE(orphan.has_value());
    EXPECT_FALSE(orphan->commit());
}

TEST_F(ComponentManagerTest, PreservesLiveGenerationWhenHostPublicationFails) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    Floater* original = opened.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.failCommit = true;
    EXPECT_FALSE(prepared.replacement.commit());
    EXPECT_EQ(liveFloaters().front(), original);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.postBuildCount, 1);
}

TEST_F(ComponentManagerTest, RollsBackEarlierRootsWhenMultiRootPublicationFails) {
    ASSERT_TRUE(registerOne("one"));
    ASSERT_TRUE(registerOne("two"));
    const auto first = manager.open("one", "first");
    const auto second = manager.open("two", "second");
    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());
    Floater* originalFirst = first.floater;
    Floater* originalSecond = second.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.failAfterFirst = true;
    EXPECT_FALSE(prepared.replacement.commit());
    const ComponentKey firstKey{"one", "first"};
    const ComponentKey secondKey{"two", "second"};
    EXPECT_EQ(mountedFor(firstKey), originalFirst);
    EXPECT_EQ(mountedFor(secondKey), originalSecond);
    EXPECT_FALSE(host.rollbackInvariantViolated);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.postBuildCount, 2);
}

TEST_F(ComponentManagerTest, MountsControllerWithoutWidgetsOrEventHandlers) {
    ASSERT_TRUE(registerEmpty());
    const auto opened = manager.open("empty");
    ASSERT_TRUE(opened.ok());
    const ComponentKey expectedKey{"empty", {}};
    EXPECT_EQ(manager.componentKeyFor(*opened.floater), expectedKey);
}

TEST_F(ComponentManagerTest, RejectsEventHandlerRegisteredForAnotherControllerType) {
    ASSERT_TRUE(registerMismatched());
    const auto opened = manager.open("mismatched");

    EXPECT_FALSE(opened.ok());
    ASSERT_FALSE(opened.errors.empty());
    EXPECT_EQ(opened.errors.front().code, "binding.event.registration_invalid");
    EXPECT_TRUE(host.mounted.empty());
}

TEST_F(ComponentManagerTest, RejectsPanelRootForFloaterHost) {
    ASSERT_TRUE(registerPanel());
    const auto opened = manager.open("panel");

    EXPECT_FALSE(opened.ok());
    ASSERT_FALSE(opened.errors.empty());
    EXPECT_EQ(opened.errors.front().code, "component.root.type_mismatch");
    EXPECT_TRUE(host.mounted.empty());
}

TEST_F(ComponentManagerTest, SynchronizesSettingBindingWithMountedSwitch) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    auto* setting = dynamic_cast<Switch*>(findWidget(*opened.floater, "setting"));
    ASSERT_NE(setting, nullptr);

    EXPECT_TRUE(setting->checked());
    resolver.binding->write(false);
    EXPECT_FALSE(setting->checked());
    setting->activate();
    EXPECT_TRUE(resolver.binding->state().value);
}

TEST_F(ComponentManagerTest, ClearsAndUnmountsComponentsForAnAccountReset) {
    ASSERT_TRUE(registerOne());
    ASSERT_TRUE(manager.open("one").ok());

    EXPECT_TRUE(manager.clearInstances());
    EXPECT_TRUE(liveFloaters().empty());
    EXPECT_TRUE(host.mounted.empty());
    EXPECT_EQ(controllerState.closeCount, 1);
}

TEST_F(ComponentManagerTest, RejectsPreparedReplacementAfterAccountReset) {
    ASSERT_TRUE(registerOne());
    ASSERT_TRUE(manager.open("one").ok());
    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    ASSERT_TRUE(manager.clearInstances());
    EXPECT_FALSE(prepared.replacement.commit());
}

TEST_F(ComponentManagerTest, RejectsPreparedReplacementAfterIdleEviction) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(*generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    opened.floater->close();
    manager.idle();
    EXPECT_FALSE(prepared.replacement.commit());
}

TEST_F(ComponentManagerTest, LeavesAllComponentsUntouchedWhenClearPreparationFails) {
    ASSERT_TRUE(registerOne("one"));
    ASSERT_TRUE(registerOne("two"));
    const auto first = manager.open("one");
    const auto second = manager.open("two");
    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());

    host.rejectClears = true;
    EXPECT_FALSE(manager.clearInstances());
    EXPECT_EQ(host.mounted.size(), std::size_t{2});
    const ComponentKey firstKey{"one", {}};
    const ComponentKey secondKey{"two", {}};
    EXPECT_EQ(manager.componentKeyFor(*first.floater), firstKey);
    EXPECT_EQ(manager.componentKeyFor(*second.floater), secondKey);
    EXPECT_FALSE(first.floater->closed());
    EXPECT_FALSE(second.floater->closed());
    EXPECT_EQ(controllerState.closeCount, 0);
}
