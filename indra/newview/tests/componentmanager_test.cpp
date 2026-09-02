/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
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
#include "componentmanager.h"
#include "controllerregistration.h"
#include "documentcontroller.h"
#include "dom/elementinternal.h"
#include "eventcall.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/input.h"
#include "skin/compiler.h"
#include "system.h"
#include "test_floater_host.h"
#include "text/metrics.h"

namespace {
using radia::ui::authoredEventCall;
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::Event;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLInputElement;
using radia::ui::ResourceSnapshot;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;
using radia::viewer::ui::ComponentInstanceKey;
using radia::viewer::ui::ComponentManager;
using radia::viewer::ui::DocumentController;
using radia::viewer::ui::test::TestFloaterHost;
using ::testing::Test;

Element* findElement(Element& root, std::string_view id) {
    if (root.id() == id) return &root;
    for (const auto& child : root.children())
        if (Element* found = findElement(*child, id)) return found;
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

class ComponentManagerTest : public Test {
protected:
    struct ControllerState {
        int committedCount = 0;
        int openCount = 0;
        int closeCount = 0;
        int availableElementCount = 0;
        int reloadSuccessCount = 0;
        int reloadFailureCount = 0;
        int pressCount = 0;
        bool inspectArgumentsValid = false;
        bool statusAvailable = false;
        bool changeObserved = false;
        bool doubleClickObserved = false;
        bool pointerDownObserved = false;
        bool pointerUpObserved = false;
        bool pointerMoveObserved = false;
        bool contextMenuObserved = false;
        bool bubbledCurrentTargetValid = false;
        std::optional<Element*> previousStatus;
        std::optional<Element*> activeStatus;
    };

    class Controller final : public DocumentController {
    public:
        Controller(System& system, Document& document, ControllerState& state)
            : DocumentController(system, document), mState(state), mStatus(getElementById("status")) {
            if (mState.activeStatus.has_value()) mState.previousStatus = mState.activeStatus;
            mState.activeStatus = mStatus;
            mState.statusAvailable = mStatus != nullptr;
            if (mStatus) ++mState.availableElementCount;
            handler("press", &Controller::press);
            handler("inspect", &Controller::inspect);
            handler("changed", &Controller::changed);
            handler("doubleClick", &Controller::doubleClick);
            handler("pointerDown", &Controller::pointerDown);
            handler("pointerUp", &Controller::pointerUp);
            handler("pointerMove", &Controller::pointerMove);
            handler("contextMenu", &Controller::contextMenu);
            handler("observeCurrentTarget", &Controller::observeCurrentTarget);
        }

        void onOpen() override {
            ++mState.openCount;
            if (!mOpened) {
                mOpened = true;
                ++mState.committedCount;
            }
        }
        void onClose() override { ++mState.closeCount; }
        void onReloadSucceeded() override {
            ++mState.reloadSuccessCount;
            ++mState.committedCount;
        }
        void onReloadFailed(const DiagnosticResult&) override { ++mState.reloadFailureCount; }

    private:
        void press() {
            ++mState.pressCount;
            if (mStatus) mStatus->innerHTML(t("controller.ready"));
        }

        void inspect(int index, std::string destination, bool enabled, Element* source, const Event& event) {
            mState.inspectArgumentsValid = index == 4
                && destination == "settings"
                && enabled
                && source
                && source->id() == "inspect"
                && event.target()
                && event.target()->id() == source->id();
        }

        void changed(const Event& event) { mState.changeObserved = event.checked(); }
        void doubleClick(const Event&) { mState.doubleClickObserved = true; }
        void pointerDown(const Event&) { mState.pointerDownObserved = true; }
        void pointerUp(const Event&) { mState.pointerUpObserved = true; }
        void pointerMove(const Event&) { mState.pointerMoveObserved = true; }
        void contextMenu(const Event&) { mState.contextMenuObserved = true; }
        void observeCurrentTarget(Element* source, const Event& event) {
            mState.bubbledCurrentTargetValid = source && source == event.currentTarget() && event.target() != source;
        }

        ControllerState& mState;
        Element* mStatus = nullptr;
        bool mOpened = false;
    };

    class ForeignController final : public DocumentController {
    public:
        ForeignController(System& system, Document& document) : DocumentController(system, document) {}
        void wrongTarget() {}
    };

    class MismatchedController final : public DocumentController {
    public:
        MismatchedController(System& system, Document& document) : DocumentController(system, document) {
            handler("wrongTarget", &ForeignController::wrongTarget);
        }
    };

    using Host = TestFloaterHost;

    void SetUp() override {
        const SkinGenerationPrepareResult prepared = prepareGeneration();
        ASSERT_TRUE(prepared.ok());
        ASSERT_TRUE(system.publish(std::move(prepared.generation)));
        surface = system.createSurface(fixedTextMetrics());
        host.surface = surface.get();
    }

    static SkinGenerationPrepareResult prepareGeneration(bool includeStatus = true) {
        constexpr char kLocalization[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {controller.ready: Ready}}}\n";
        constexpr char kViewWithStatus[] = "<floater>"
                                           "<head><title>controller</title><close></close></head>"
                                           "<body onClick=\"observeCurrentTarget(this, event)\">"
                                           "<p id=\"status\"></p>"
                                           "<button id=\"press\" onClick=\"press()\"></button>"
                                           "<button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"></button>"
                                           "<button id=\"events\" onDoubleClick=\"doubleClick(event)\" onPointerDown=\"pointerDown(event)\" "
                                           "onPointerUp=\"pointerUp(event)\" onPointerMove=\"pointerMove(event)\" "
                                           "onContextMenu=\"contextMenu(event)\"></button>"
                                           "<input type=\"checkbox\" switch=\"true\" id=\"changed\" checked=\"false\" onChange=\"changed(event)\">"
                                           "<input type=\"checkbox\" switch=\"true\" id=\"setting\" setting=\"test-enabled\">"
                                           "</body>"
                                           "</floater>";
        constexpr char kViewWithoutStatus[] = "<floater>"
                                              "<head><title>controller</title><close></close></head>"
                                              "<body>"
                                              "<button id=\"press\" onClick=\"press()\"></button>"
                                              "<button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"></button>"
                                              "<button id=\"events\" onDoubleClick=\"doubleClick(event)\" onPointerDown=\"pointerDown(event)\" "
                                              "onPointerUp=\"pointerUp(event)\" onPointerMove=\"pointerMove(event)\" "
                                              "onContextMenu=\"contextMenu(event)\"></button>"
                                              "<input type=\"checkbox\" switch=\"true\" id=\"changed\" checked=\"false\" onChange=\"changed(event)\">"
                                              "<input type=\"checkbox\" switch=\"true\" id=\"setting\" setting=\"test-enabled\">"
                                              "</body>"
                                              "</floater>";
        constexpr char kPanelWithStatus[] = "<panel>"
                                            "<p id=\"status\"></p>"
                                            "<button id=\"press\" onClick=\"press()\"></button>"
                                            "<input type=\"checkbox\" switch=\"true\" id=\"changed\" checked=\"false\" onChange=\"changed(event)\">"
                                            "</panel>";
        constexpr char kPanelWithoutStatus[] = "<panel>"
                                               "<button id=\"press\" onClick=\"press()\"></button>"
                                               "<input type=\"checkbox\" switch=\"true\" id=\"changed\" "
                                               "checked=\"false\" onChange=\"changed(event)\">"
                                               "</panel>";
        constexpr char kDuplicateControllerView[] = "<floater>"
                                                    "<head><title>duplicate</title><close></close></head>"
                                                    "<body><p id=\"status\"></p><p id=\"status\"></p></body>"
                                                    "</floater>";

        ResourceSnapshot resources;
        resources.add("localization.yaml", kLocalization);
        resources.add("skin.css", "");
        resources.add("one.html", includeStatus ? kViewWithStatus : kViewWithoutStatus);
        resources.add("two.html", includeStatus ? kViewWithStatus : kViewWithoutStatus);
        resources.add("empty.html", "<floater><head><title>empty</title><close></close></head><body></body></floater>");
        resources.add("panel.html", includeStatus ? kPanelWithStatus : kPanelWithoutStatus);
        resources.add("duplicate.html", kDuplicateControllerView);
        return SkinCompiler().prepare(std::move(resources));
    }

    bool registerOne(std::string definitionId = "one") {
        return manager.registerDefinition(std::move(definitionId), "one.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, controllerState);
        });
    }

    bool registerEmpty() {
        return manager.registerDefinition("empty", "empty.html", [](System& system, Document& document) {
            class EmptyController final : public DocumentController {
            public:
                EmptyController(System& system, Document& document) : DocumentController(system, document) {}
            };
            return std::make_unique<EmptyController>(system, document);
        });
    }

    bool registerPanel() {
        return manager.registerDefinition("panel", "panel.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, controllerState);
        });
    }

    bool registerMismatched() {
        return manager.registerDefinition(
            "mismatched", "empty.html", [](System& system, Document& document) { return std::make_unique<MismatchedController>(system, document); });
    }

    bool registerDuplicate() {
        return manager.registerDefinition("duplicate", "duplicate.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, controllerState);
        });
    }

    std::vector<HTMLFloaterElement*> liveFloaters() const {
        std::vector<HTMLFloaterElement*> result;
        for (const auto& [root, floater] : host.mounted)
            if (floater && !floater->closed()) result.push_back(floater.get());
        return result;
    }

    std::optional<ComponentInstanceKey> keyFor(const HTMLFloaterElement& floater) const { return manager.componentKeyFor(floater); }

    HTMLFloaterElement* mountedFor(const ComponentInstanceKey& key) const {
        for (const auto& [root, floater] : host.mounted)
            if (floater && manager.componentKeyFor(*floater) == key) return floater.get();
        return nullptr;
    }

    System system;
    std::unique_ptr<Surface> surface;
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
    const ComponentInstanceKey firstKey{"profile", "alice"};
    const ComponentInstanceKey secondKey{"profile", "bob"};
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

    EXPECT_EQ(controllerState.committedCount, 1);
    EXPECT_EQ(controllerState.openCount, 1);
    EXPECT_EQ(controllerState.availableElementCount, 1);

    auto* press = dynamic_cast<HTMLButtonElement*>(findElement(*opened.floater, "press"));
    auto* inspect = dynamic_cast<HTMLButtonElement*>(findElement(*opened.floater, "inspect"));
    auto* events = dynamic_cast<HTMLButtonElement*>(findElement(*opened.floater, "events"));
    auto* changed = dynamic_cast<HTMLInputElement*>(findElement(*opened.floater, "changed"));
    auto* status = findElement(*opened.floater, "status");
    ASSERT_NE(press, nullptr);
    ASSERT_NE(inspect, nullptr);
    ASSERT_NE(events, nullptr);
    ASSERT_NE(changed, nullptr);
    ASSERT_NE(status, nullptr);

    EXPECT_TRUE(authoredEventCall(*events, "dblclick"));
    EXPECT_TRUE(authoredEventCall(*events, "pointerdown"));
    EXPECT_TRUE(authoredEventCall(*events, "pointerup"));
    EXPECT_TRUE(authoredEventCall(*events, "pointermove"));
    EXPECT_TRUE(authoredEventCall(*events, "contextmenu"));

    press->activate();
    inspect->activate();
    changed->activate();

    EXPECT_EQ(controllerState.pressCount, 1);
    EXPECT_TRUE(controllerState.inspectArgumentsValid);
    EXPECT_TRUE(controllerState.bubbledCurrentTargetValid);
    EXPECT_TRUE(controllerState.changeObserved);
    EXPECT_EQ(status->textContent(), "Ready");
}

TEST_F(ComponentManagerTest, ReplacesOpenComponentWithoutChangingItsIdentity) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    HTMLFloaterElement* original = opened.floater;

    opened.floater->close();
    ASSERT_TRUE(manager.open("one").ok());
    EXPECT_EQ(controllerState.openCount, 2);

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_EQ(liveFloaters().size(), std::size_t{1});
    EXPECT_EQ(liveFloaters().front(), original);

    ASSERT_TRUE(prepared.replacement.commit());
    ASSERT_EQ(liveFloaters().size(), std::size_t{1});
    HTMLFloaterElement* replacement = liveFloaters().front();
    manager.idle();
    manager.reportReloadSucceeded();
    const DiagnosticResult diagnostics;
    manager.reportReloadFailed(diagnostics);

    EXPECT_NE(replacement, original);
    EXPECT_EQ(controllerState.committedCount, 2);
    EXPECT_EQ(controllerState.availableElementCount, 2);
    ASSERT_TRUE(controllerState.previousStatus.has_value());
    ASSERT_TRUE(controllerState.activeStatus.has_value());
    EXPECT_NE(*controllerState.previousStatus, *controllerState.activeStatus);
    EXPECT_TRUE(*controllerState.activeStatus);
    EXPECT_EQ(host.replacements, 1);
    const ComponentInstanceKey replacementKey{"one", {}};
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
        std::vector<ComponentInstanceKey> result;
        manager.forEachOpen([&](const ComponentInstanceKey& key, HTMLFloaterElement&) { result.push_back(key); });
        return result;
    };

    second.floater->close();
    const std::vector<ComponentInstanceKey> openAfterClose = openComponents();
    ASSERT_EQ(openAfterClose.size(), std::size_t{1});
    const ComponentInstanceKey remainingKey{"profile", "alice"};
    EXPECT_EQ(openAfterClose.front(), remainingKey);

    ASSERT_TRUE(manager.open("profile", "bob").ok());
    EXPECT_EQ(openComponents().size(), std::size_t{2});
}

TEST_F(ComponentManagerTest, KeepsRemainingBindingsLiveWhenAnOptionalElementDisappears) {
    ASSERT_TRUE(registerOne());
    ASSERT_TRUE(manager.open("one").ok());
    EXPECT_EQ(controllerState.availableElementCount, 1);

    const SkinGenerationPrepareResult generation = prepareGeneration(false);
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(prepared.replacement.commit());
    const DiagnosticResult diagnostics = prepared.replacement.takeDiagnostics();
    ASSERT_FALSE(diagnostics.warnings.empty());
    EXPECT_EQ(diagnostics.warnings.front().code, "controller.element.missing");

    EXPECT_FALSE(controllerState.statusAvailable);
    EXPECT_EQ(controllerState.availableElementCount, 1);
    auto* press = dynamic_cast<HTMLButtonElement*>(findElement(*liveFloaters().front(), "press"));
    ASSERT_NE(press, nullptr);
    press->activate();
    EXPECT_EQ(controllerState.pressCount, 1);
}

TEST_F(ComponentManagerTest, LeavesCurrentComponentUntouchedWhenTheHostRejectsReplacement) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    HTMLFloaterElement* original = opened.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.rejectReplacements = true;
    EXPECT_FALSE(prepared.replacement.commit());
    EXPECT_EQ(liveFloaters().front(), original);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.committedCount, 1);
}

TEST_F(ComponentManagerTest, EvictsClosedFloatersBeforeReplacement) {
    ASSERT_TRUE(registerOne());
    const auto opened = manager.open("one");
    ASSERT_TRUE(opened.ok());
    ElementRef<HTMLFloaterElement> evictedRoot(opened.floater);
    opened.floater->close();
    ASSERT_TRUE(opened.floater->closed());
    const int openCount = controllerState.openCount;

    manager.idle();
    EXPECT_TRUE(liveFloaters().empty());
    EXPECT_TRUE(host.mounted.empty());
    EXPECT_EQ(evictedRoot.get(), nullptr);

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(prepared.replacement.commit());
    EXPECT_EQ(controllerState.openCount, openCount);

    const auto reopened = manager.open("one");
    ASSERT_TRUE(reopened.ok());
    EXPECT_EQ(controllerState.committedCount, 2);
    EXPECT_EQ(controllerState.openCount, openCount + 1);
}

TEST_F(ComponentManagerTest, RejectsPreparedReplacementAfterItsManagerIsDestroyed) {
    Host temporaryHost;
    std::optional<ComponentManager::PreparedReplacement> orphan;
    {
        ComponentManager temporary(system, temporaryHost, resolver);
        ASSERT_TRUE(temporary.registerDefinition("one", "one.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, controllerState);
        }));
        ASSERT_TRUE(temporary.open("one").ok());
        const SkinGenerationPrepareResult generation = prepareGeneration();
        ASSERT_TRUE(generation.ok());
        auto prepared = temporary.prepareReplacement(generation.generation, "en");
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
    HTMLFloaterElement* original = opened.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.failCommit = true;
    EXPECT_FALSE(prepared.replacement.commit());
    EXPECT_EQ(liveFloaters().front(), original);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.committedCount, 1);
}

TEST_F(ComponentManagerTest, LeavesEveryRootUntouchedWhenHostRejectsMultiRootPublication) {
    ASSERT_TRUE(registerOne("one"));
    ASSERT_TRUE(registerOne("two"));
    const auto first = manager.open("one", "first");
    const auto second = manager.open("two", "second");
    ASSERT_TRUE(first.ok());
    ASSERT_TRUE(second.ok());
    HTMLFloaterElement* originalFirst = first.floater;
    HTMLFloaterElement* originalSecond = second.floater;

    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
    ASSERT_TRUE(prepared.ok());

    host.failCommit = true;
    EXPECT_FALSE(prepared.replacement.commit());
    const ComponentInstanceKey firstKey{"one", "first"};
    const ComponentInstanceKey secondKey{"two", "second"};
    EXPECT_EQ(mountedFor(firstKey), originalFirst);
    EXPECT_EQ(mountedFor(secondKey), originalSecond);
    EXPECT_EQ(host.replacements, 0);
    EXPECT_EQ(controllerState.committedCount, 2);
}

TEST_F(ComponentManagerTest, MountsControllerWithoutElementsOrEventHandlers) {
    ASSERT_TRUE(registerEmpty());
    const auto opened = manager.open("empty");
    ASSERT_TRUE(opened.ok());
    const ComponentInstanceKey expectedKey{"empty", {}};
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

TEST_F(ComponentManagerTest, RejectsAmbiguousControllerElementId) {
    ASSERT_TRUE(registerDuplicate());
    const auto opened = manager.open("duplicate");

    EXPECT_FALSE(opened.ok());
    ASSERT_FALSE(opened.errors.empty());
    EXPECT_EQ(opened.errors.front().code, "controller.element.ambiguous");
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
    auto* setting = dynamic_cast<HTMLInputElement*>(findElement(*opened.floater, "setting"));
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

TEST_F(ComponentManagerTest, UsesHostTeardownWhenManagerIsDestroyed) {
    TestFloaterHost temporaryHost;
    {
        ComponentManager temporaryManager{system, temporaryHost, resolver};
        ASSERT_TRUE(temporaryManager.registerDefinition("one", "one.html", [this](System& system, Document& document) {
            return std::make_unique<Controller>(system, document, controllerState);
        }));
        ASSERT_TRUE(temporaryManager.open("one").ok());
    }

    EXPECT_EQ(temporaryHost.clearCalls, 1);
    EXPECT_TRUE(temporaryHost.mounted.empty());
}

TEST_F(ComponentManagerTest, RejectsPreparedReplacementAfterAccountReset) {
    ASSERT_TRUE(registerOne());
    ASSERT_TRUE(manager.open("one").ok());
    const SkinGenerationPrepareResult generation = prepareGeneration();
    ASSERT_TRUE(generation.ok());
    auto prepared = manager.prepareReplacement(generation.generation, "en");
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
    auto prepared = manager.prepareReplacement(generation.generation, "en");
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
    const ComponentInstanceKey firstKey{"one", {}};
    const ComponentInstanceKey secondKey{"two", {}};
    EXPECT_EQ(manager.componentKeyFor(*first.floater), firstKey);
    EXPECT_EQ(manager.componentKeyFor(*second.floater), secondKey);
    EXPECT_FALSE(first.floater->closed());
    EXPECT_FALSE(second.floater->closed());
    EXPECT_EQ(controllerState.closeCount, 0);
    host.rejectClears = false;
}
