/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstdint>
#include <functional>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <typeindex>
#include <utility>
#include <vector>
#include "binding/binder.h"
#include "binding/settingresolver.h"
#include "binding/valuebinding.h"
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "elements/elementinternal.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "event.h"
#include "layout/resourcecompiler.h"
#include "text/metrics.h"

namespace {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::ButtonElement;
using radia::ui::CurrentEventArgument;
using radia::ui::DiagnosticResult;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::Event;
using radia::ui::EventArgument;
using radia::ui::EventCall;
using radia::ui::EventHandlerRegistration;
using radia::ui::InputElement;
using radia::ui::kChangeEvent;
using radia::ui::kClickEvent;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutResourceCompiler;
using radia::ui::PanelElement;
using radia::ui::PreparedBindingResult;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingBase;
using radia::ui::ValueBindingRef;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueState;
using radia::ui::ValueValidation;
using radia::ui::Visibility;
using radia::ui::detail::ElementCompilerAccess;
using radia::ui::detail::findElementInScope;
using radia::ui::detail::makeEventRegistration;

const char* noEventArguments(const EventCall& call) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

const char* currentEventArgument(const EventCall& call) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<CurrentEventArgument>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

template<typename T> const char* singleEventArgument(const EventCall& call) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<T>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

void bindEvent(Binder& binder, std::string settingName, EventHandlerRegistration::Invoke invoke,
               EventHandlerRegistration::ArgumentError argumentError) {
    binder.event(makeEventRegistration(std::move(settingName), std::move(invoke), std::move(argumentError)));
}

template<typename Callback> void bindEvent(Binder& binder, std::string settingName, Callback callback) {
    bindEvent(binder, std::move(settingName), [callback = std::move(callback)](Event&, const EventCall&) mutable { callback(); }, noEventArguments);
}

template<typename T> class TestValueBinding final : public ValueBinding<T> {
public:
    explicit TestValueBinding(T value) : mState{value, value, std::nullopt} {}

    ValueState<T> state() const override { return mState; }
    void write(T value) override {
        mState.value = std::move(value);
        notify();
    }
    ValueBindingSubscription observe(typename ValueBinding<T>::Observer observer) override {
        const std::size_t id = mNextObserver++;
        mObservers.emplace(id, std::move(observer));
        return ValueBindingSubscription([this, id] { mObservers.erase(id); });
    }

    void publish(ValueState<T> state) {
        mState = std::move(state);
        notify();
    }

    std::size_t observerCount() const { return mObservers.size(); }

private:
    void notify() {
        const auto observers = mObservers;
        for (const auto& [id, observer] : observers)
            if (mObservers.find(id) != mObservers.end()) observer(mState);
    }

    ValueState<T> mState;
    std::map<std::size_t, typename ValueBinding<T>::Observer> mObservers;
    std::size_t mNextObserver = 1;
};

class TestSettingResolver final : public SettingResolver {
public:
    template<typename T> void add(std::string settingName, std::shared_ptr<TestValueBinding<T>> binding) {
        mBindings.emplace(std::move(settingName), Entry{std::move(binding), typeid(T)});
    }

    SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override {
        const auto found = mBindings.find(std::string(settingName));
        if (found == mBindings.end()) return {SettingResolution::ResolutionStatus::Missing, {}};
        if (found->second.type != requestedType) return {SettingResolution::ResolutionStatus::TypeMismatch, {}};
        return {SettingResolution::ResolutionStatus::Found, found->second.binding};
    }

    void clear() { mBindings.clear(); }

private:
    struct Entry {
        std::shared_ptr<ValueBindingBase> binding;
        std::type_index type{typeid(void)};
    };

    std::map<std::string, Entry> mBindings;
};

class MisreportingSettingResolver final : public SettingResolver {
public:
    MisreportingSettingResolver() : binding(std::make_shared<TestValueBinding<std::string>>("wrong type")) {}

    SettingResolution resolve(std::string_view, std::type_index) override { return {SettingResolution::ResolutionStatus::Found, binding}; }

    std::shared_ptr<TestValueBinding<std::string>> binding;
};

template<typename ElementT> ElementRef<ElementT> lookupElement(Element& root, std::string_view id) {
    return ElementRef<ElementT>(dynamic_cast<ElementT*>(findElementInScope(root, id)));
}

struct TestBindingResult : DiagnosticResult {
    bool ok() const { return !hasErrors() && static_cast<bool>(binding); }
    Binding binding;
};

TestBindingResult finishBinding(Binder& binder) {
    PreparedBindingResult prepared = binder.prepare();
    TestBindingResult result;
    result.warnings = std::move(prepared.warnings);
    result.errors = std::move(prepared.errors);
    if (prepared.ok()) result.binding = prepared.binding.commit();
    return result;
}
} // namespace

TEST(BinderTest, CommitsEventBindingAndResolvesTypedElement) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    button->setId("save").setEventCall(kClickEvent, EventCall("save"));
    root.append(std::move(button));

    int activations = 0;
    ElementRef<ButtonElement> save;
    Binder binder(root);
    save = lookupElement<ButtonElement>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_NE(save.get(), nullptr);
    save->activate();
    EXPECT_EQ(activations, 1);
}

TEST(BinderTest, DistinguishesTypedAndMissingElementLookups) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    button->setId("save").setEventCall(kClickEvent, EventCall("save"));
    ButtonElement* source = button.get();
    root.append(std::move(button));
    auto label = std::make_unique<radia::ui::LabelElement>();
    label->setId("status");
    root.append(std::move(label));

    int activations = 0;
    ElementRef<ButtonElement> save;
    ElementRef<ButtonElement> wrongType;
    ElementRef<radia::ui::LabelElement> missing;
    Binder binder(root);
    save = lookupElement<ButtonElement>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    wrongType = lookupElement<ButtonElement>(root, "status");
    missing = lookupElement<radia::ui::LabelElement>(root, "missing");
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_NE(save.get(), nullptr);
    EXPECT_EQ(wrongType.get(), nullptr);
    EXPECT_EQ(missing.get(), nullptr);
    source->activate();
    EXPECT_EQ(activations, 1);
}

TEST(BinderTest, InvalidatesElementReferenceAfterElementRemoval) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    button->setId("temporary");
    root.append(std::move(button));
    ElementRef<ButtonElement> reference;
    Binder binder(root);
    reference = lookupElement<ButtonElement>(root, "temporary");
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_NE(reference.get(), nullptr);
    root.replaceChildren();
    EXPECT_EQ(reference.get(), nullptr);
}

TEST(BinderTest, DetachesEventHandlerWhenBindingIsDestroyed) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ButtonElement* source = button.get();
    button->setEventCall(kClickEvent, EventCall("optional"));
    root.append(std::move(button));

    source->activate();
    int activations = 0;
    Binder binder(root);
    bindEvent(binder, "optional", [&] { ++activations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    source->activate();
    EXPECT_EQ(activations, 1);
    result.binding = Binding{};
    source->activate();
    EXPECT_EQ(activations, 1);
}

TEST(BinderTest, AllowsOneHandlerAcrossMultipleEventTypes) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ButtonElement* buttonTarget = button.get();
    button->setEventCall(kClickEvent, EventCall("shared"));
    root.append(std::move(button));
    auto control = std::make_unique<InputElement>();
    InputElement* controlTarget = control.get();
    control->type("checkbox").switchMode(true);
    control->setEventCall(kChangeEvent, EventCall("shared"));
    root.append(std::move(control));

    std::vector<std::string> eventTypes;
    Binder binder(root);
    bindEvent(binder, "shared", [&](Event& event, const EventCall&) { eventTypes.emplace_back(event.type()); }, noEventArguments);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    buttonTarget->activate();
    ASSERT_EQ(eventTypes.size(), std::size_t{1});
    EXPECT_EQ(eventTypes.front(), "click");
    controlTarget->activate();
    ASSERT_EQ(eventTypes.size(), std::size_t{2});
    EXPECT_EQ(eventTypes.back(), "change");
}

TEST(BinderTest, BindsChangeEventsWithCurrentState) {
    PanelElement root;
    auto control = std::make_unique<InputElement>();
    control->type("checkbox").switchMode(true);
    InputElement* source = control.get();
    control->setEventCall(kChangeEvent, EventCall("changed", {CurrentEventArgument{}}));
    root.append(std::move(control));

    int changes = 0;
    Binder binder(root);
    bindEvent(
        binder, "changed",
        [&](Event& event, const EventCall&) {
            EXPECT_TRUE(event.checked());
            ++changes;
        },
        currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    source->checked(false);
    EXPECT_EQ(changes, 0);
    source->activate();
    EXPECT_EQ(changes, 1);
}

TEST(BinderTest, ResolvesIdsWithinIndependentResourceScopes) {
    PanelElement root;
    auto left = std::make_unique<PanelElement>();
    left->setId("left");
    radia::ui::detail::ElementCompilerAccess::setIdScopeRoot(*left);
    auto leftItem = std::make_unique<radia::ui::LabelElement>();
    leftItem->setId("item");
    left->append(std::move(leftItem));
    root.append(std::move(left));

    auto right = std::make_unique<PanelElement>();
    right->setId("right");
    radia::ui::detail::ElementCompilerAccess::setIdScopeRoot(*right);
    auto rightItem = std::make_unique<radia::ui::LabelElement>();
    rightItem->setId("item");
    right->append(std::move(rightItem));
    root.append(std::move(right));

    ElementRef<PanelElement> leftScope;
    ElementRef<PanelElement> rightScope;
    Binder parent(root);
    leftScope = lookupElement<PanelElement>(root, "left");
    rightScope = lookupElement<PanelElement>(root, "right");
    const TestBindingResult parentResult = finishBinding(parent);
    ASSERT_TRUE(parentResult.ok());
    ASSERT_NE(leftScope.get(), nullptr);
    ASSERT_NE(rightScope.get(), nullptr);

    ElementRef<radia::ui::LabelElement> leftBound;
    ElementRef<radia::ui::LabelElement> rightBound;
    Binder leftBinder(*leftScope);
    leftBound = lookupElement<radia::ui::LabelElement>(*leftScope, "item");
    Binder rightBinder(*rightScope);
    rightBound = lookupElement<radia::ui::LabelElement>(*rightScope, "item");
    const TestBindingResult leftResult = finishBinding(leftBinder);
    ASSERT_TRUE(leftResult.ok());
    ASSERT_NE(leftBound.get(), nullptr);
    const TestBindingResult rightResult = finishBinding(rightBinder);
    ASSERT_TRUE(rightResult.ok());
    ASSERT_NE(rightBound.get(), nullptr);
    EXPECT_NE(leftBound.get(), rightBound.get());
}

TEST(BinderTest, PreparesReplacementWithoutMutatingLiveBinding) {
    PanelElement live;
    auto liveButton = std::make_unique<ButtonElement>();
    ButtonElement* liveButtonPtr = liveButton.get();
    liveButton->setId("reload").setEventCall(kClickEvent, EventCall("reload"));
    live.append(std::move(liveButton));

    ElementRef<ButtonElement> reference;
    Binder liveBinder(live);
    reference = lookupElement<ButtonElement>(live, "reload");
    bindEvent(liveBinder, "reload", [] {});
    TestBindingResult liveBinding = finishBinding(liveBinder);
    ASSERT_TRUE(liveBinding.ok());
    EXPECT_EQ(reference.get(), liveButtonPtr);

    PanelElement candidate;
    auto candidateButton = std::make_unique<ButtonElement>();
    ButtonElement* candidateButtonPtr = candidateButton.get();
    candidateButton->setId("reload").setEventCall(kClickEvent, EventCall("reload"));
    candidate.append(std::move(candidateButton));

    Binder candidateBinder(candidate);
    auto candidateReference = lookupElement<ButtonElement>(candidate, "reload");
    bindEvent(candidateBinder, "reload", [] {});
    PreparedBindingResult prepared = candidateBinder.prepare();
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(reference.get(), liveButtonPtr);
    ASSERT_NE(candidateReference.get(), nullptr);

    Binding replacement = prepared.binding.commit();
    EXPECT_EQ(candidateReference.get(), candidateButtonPtr);
    EXPECT_TRUE(static_cast<bool>(replacement));

    PanelElement removedCandidate;
    Binder removedBinder(removedCandidate);
    auto removedReference = lookupElement<ButtonElement>(removedCandidate, "reload");
    PreparedBindingResult removed = removedBinder.prepare();
    ASSERT_TRUE(removed.ok());
    EXPECT_EQ(removedReference.get(), nullptr);
    EXPECT_EQ(reference.get(), liveButtonPtr);
    Binding removedBinding = removed.binding.commit();
    EXPECT_EQ(reference.get(), liveButtonPtr);
    EXPECT_TRUE(static_cast<bool>(removedBinding));
}

TEST(BinderTest, AllowsUnmatchedOptionalEventHandler) {
    PanelElement root;
    Binder binder(root);
    bindEvent(binder, "missing", [] {});
    PreparedBindingResult result = binder.prepare();
    ASSERT_TRUE(result.ok());
    Binding binding = result.binding.commit();
    EXPECT_TRUE(static_cast<bool>(binding));
}

TEST(BinderTest, WarnsForUnhandledLayoutEvent) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    button->setEventCall(kClickEvent, EventCall("unhandled"));
    root.append(std::move(button));

    Binder binder(root);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t{1});
    EXPECT_EQ(result.warnings.front().code, "binding.event.unhandled");
}

TEST(BinderTest, PreservesLiveValueBindingUntilReplacementCommits) {
    PanelElement liveRoot;
    auto live = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver liveResolver;
    liveResolver.add("demo-enabled", live);
    ValueBindingRef<bool> reference;
    Binder liveBinder(liveRoot, &liveResolver);
    liveBinder.requireValueBinding({"demo-enabled"}, reference);
    TestBindingResult liveResult = finishBinding(liveBinder);
    ASSERT_TRUE(liveResult.ok());
    EXPECT_EQ(reference.get(), live.get());

    PanelElement candidateRoot;
    auto candidate = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver candidateResolver;
    candidateResolver.add("demo-enabled", candidate);
    Binder candidateBinder(candidateRoot, &candidateResolver);
    candidateBinder.requireValueBinding({"demo-enabled"}, reference);
    PreparedBindingResult prepared = candidateBinder.prepare();
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(reference.get(), live.get());

    {
        PanelElement abandonedRoot;
        auto abandoned = std::make_shared<TestValueBinding<bool>>(true);
        TestSettingResolver abandonedResolver;
        abandonedResolver.add("demo-enabled", abandoned);
        Binder abandonedBinder(abandonedRoot, &abandonedResolver);
        abandonedBinder.requireValueBinding({"demo-enabled"}, reference);
        PreparedBindingResult abandonedResult = abandonedBinder.prepare();
        EXPECT_TRUE(abandonedResult.ok());
    }
    EXPECT_EQ(reference.get(), live.get());

    Binding replacement = prepared.binding.commit();
    ASSERT_TRUE(static_cast<bool>(replacement));
    EXPECT_EQ(reference.get(), candidate.get());
    ASSERT_NE(reference.get(), nullptr);
    EXPECT_TRUE(reference->state().value);
}

TEST(BinderTest, RejectsMissingSetting) {
    PanelElement root;
    ValueBindingRef<bool> reference;
    TestSettingResolver resolver;
    Binder binder(root, &resolver);
    binder.requireValueBinding({"missing-value"}, reference);
    PreparedBindingResult result = binder.prepare();
    ASSERT_FALSE(result.ok());
    EXPECT_EQ(reference.get(), nullptr);
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.missing");
}

TEST(BinderTest, RejectsSettingTypeMismatch) {
    PanelElement root;
    auto setting = std::make_shared<TestValueBinding<std::string>>("enabled");
    TestSettingResolver resolver;
    resolver.add("demo-enabled", setting);
    ValueBindingRef<bool> reference;
    Binder binder(root, &resolver);
    binder.requireValueBinding({"demo-enabled"}, reference);
    const PreparedBindingResult result = binder.prepare();
    ASSERT_FALSE(result.ok());
    EXPECT_EQ(reference.get(), nullptr);
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}

TEST(BinderTest, ResolvesRepeatedValueRequirementsIndependently) {
    PanelElement root;
    auto setting = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", setting);
    ValueBindingRef<bool> first;
    ValueBindingRef<bool> second;
    Binder binder(root, &resolver);
    binder.requireValueBinding({"demo-enabled"}, first);
    binder.requireValueBinding({"demo-enabled"}, second);
    PreparedBindingResult result = binder.prepare();
    ASSERT_TRUE(result.ok());
    Binding binding = result.binding.commit();
    ASSERT_TRUE(static_cast<bool>(binding));
    EXPECT_EQ(first.get(), setting.get());
    EXPECT_EQ(second.get(), setting.get());
}

TEST(BinderTest, ReplacesValueBindingOnTheSameControl) {
    constexpr char kReplaceableValueLayout[] = "<input type=\"checkbox\" switch=\"true\" setting=\"replaceable-value\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kReplaceableValueLayout, "replaceable-value.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* control = buildResult.rootAs<InputElement>();
    ASSERT_NE(control, nullptr);
    auto firstProvider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver firstResolver;
    firstResolver.add("replaceable-value", firstProvider);
    Binder firstBinder(*control, &firstResolver);
    TestBindingResult firstResult = finishBinding(firstBinder);
    ASSERT_TRUE(firstResult.ok());
    Binding activeBinding = std::move(firstResult.binding);
    ASSERT_TRUE(static_cast<bool>(activeBinding));
    EXPECT_FALSE(control->checked());

    auto secondProvider = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver secondResolver;
    secondResolver.add("replaceable-value", secondProvider);
    Binder secondBinder(*control, &secondResolver);
    TestBindingResult replacementResult = finishBinding(secondBinder);
    ASSERT_TRUE(replacementResult.ok());
    EXPECT_TRUE(static_cast<bool>(replacementResult.binding));
    EXPECT_TRUE(control->checked());

    // Releasing the old binding detaches the first provider before the new
    // binding becomes the owner of the control's subscription.
    activeBinding = std::move(replacementResult.binding);
    ASSERT_TRUE(static_cast<bool>(activeBinding));
    control->activate();
    EXPECT_FALSE(secondProvider->state().value);
    EXPECT_FALSE(control->checked());
    EXPECT_FALSE(firstProvider->state().value);
}

TEST(BinderTest, WarnsWhenEventArgumentsDoNotMatchHandler) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ButtonElement* target = button.get();
    button->setEventCall(kClickEvent, EventCall("inspect", {EventArgument(std::int64_t(4))}));
    root.append(std::move(button));

    int invocations = 0;
    Binder binder(root);
    bindEvent(binder, "inspect", [&] { ++invocations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t{1});
    EXPECT_EQ(result.warnings.front().code, "binding.event.arity_mismatch");
    target->activate();
    EXPECT_EQ(invocations, 0);
}

TEST(BinderTest, DispatchesGenericEventHandler) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ButtonElement* target = button.get();
    button->setEventCall(kClickEvent, EventCall("press"));
    root.append(std::move(button));

    int invocations = 0;
    Binder binder(root);
    bindEvent(binder, "press", [&] { ++invocations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.warnings.size(), std::size_t{0});
    target->activate();
    EXPECT_EQ(invocations, 1);
}

TEST(BinderTest, DispatchesTypedEventArguments) {
    PanelElement root;
    auto select = std::make_unique<ButtonElement>();
    ButtonElement* selectTarget = select.get();
    select->setEventCall(kClickEvent, EventCall("select", {EventArgument(std::int64_t(4))}));
    root.append(std::move(select));
    auto open = std::make_unique<ButtonElement>();
    ButtonElement* openTarget = open.get();
    open->setEventCall(kClickEvent, EventCall("open", {EventArgument(std::string("settings"))}));
    root.append(std::move(open));
    auto enabled = std::make_unique<ButtonElement>();
    ButtonElement* enabledTarget = enabled.get();
    enabled->setEventCall(kClickEvent, EventCall("updateAdvanced", {EventArgument(true)}));
    root.append(std::move(enabled));
    auto inspect = std::make_unique<ButtonElement>();
    ButtonElement* inspectTarget = inspect.get();
    inspect->setEventCall(kClickEvent, EventCall("inspectEventSource", {EventArgument(CurrentEventArgument{})}));
    root.append(std::move(inspect));

    int selected = 0;
    std::string destination;
    bool advanced = false;
    Element* source = nullptr;
    Binder binder(root);
    bindEvent(
        binder, "select", [&](Event&, const EventCall& call) { selected = static_cast<int>(std::get<std::int64_t>(call.arguments().front())); },
        singleEventArgument<std::int64_t>);
    bindEvent(
        binder, "open", [&](Event&, const EventCall& call) { destination = std::get<std::string>(call.arguments().front()); },
        singleEventArgument<std::string>);
    bindEvent(
        binder, "updateAdvanced", [&](Event&, const EventCall& call) { advanced = std::get<bool>(call.arguments().front()); },
        singleEventArgument<bool>);
    bindEvent(binder, "inspectEventSource", [&](Event& event, const EventCall&) { source = &event.target(); }, currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.warnings.size(), std::size_t{0});
    selectTarget->activate();
    openTarget->activate();
    enabledTarget->activate();
    inspectTarget->activate();
    EXPECT_EQ(selected, 4);
    EXPECT_EQ(destination, "settings");
    EXPECT_TRUE(advanced);
    EXPECT_EQ(source, inspectTarget);
}

TEST(BinderTest, RejectsInvalidRegisteredHandlerName) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    button->setEventCall(kClickEvent, EventCall("bad_action"));
    root.append(std::move(button));

    Binder binder(root);
    bindEvent(binder, "bad_action", [] {});
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.event.name_invalid");
}

TEST(BinderTest, DispatchesCommonElementEventContext) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ButtonElement* target = button.get();
    button->setEventCall(kClickEvent, EventCall("observe"));
    root.append(std::move(button));

    Element* source = nullptr;
    std::string type;
    Binder binder(root);
    bindEvent(
        binder, "observe",
        [&](Event& event, const EventCall&) {
            source = &event.target();
            type = std::string(event.type());
        },
        noEventArguments);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    target->activate();
    EXPECT_EQ(source, target);
    EXPECT_EQ(type, "click");
}

TEST(BinderTest, BindsSwitchSettingAndPropagatesUpdates) {
    constexpr char kDemoSettingLayout[] = "<input type=\"checkbox\" switch=\"true\" setting=\"demo-enabled\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kDemoSettingLayout, "setting.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* control = buildResult.rootAs<InputElement>();
    ASSERT_NE(control, nullptr);
    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);

    Binder binder(*control, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(control->checked());
    provider->publish({true, false, ValueValidation::valid()});
    EXPECT_TRUE(control->checked());
    control->activate();
    EXPECT_FALSE(provider->state().value);
}

TEST(BinderTest, RejectsMissingLayoutSetting) {
    constexpr char kMissingSettingLayout[] = "<input type=\"checkbox\" switch=\"true\" setting=\"missing-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kMissingSettingLayout, "missing-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    TestSettingResolver resolver;
    Binder binder(*buildResult.document->documentElement(), &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.missing");
}

TEST(BinderTest, RejectsMismatchedLayoutSetting) {
    constexpr char kStringSettingLayout[] = "<input type=\"checkbox\" switch=\"true\" setting=\"string-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kStringSettingLayout, "typed-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    TestSettingResolver resolver;
    resolver.add("string-setting", std::make_shared<TestValueBinding<std::string>>("not a boolean"));
    Binder binder(*buildResult.document->documentElement(), &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}

TEST(BinderTest, SharesOneSettingAcrossMultipleControls) {
    constexpr char kSharedSettingLayout[] = "<panel>"
                                            "<input type=\"checkbox\" switch=\"true\" id=\"first\" setting=\"shared-enabled\"/>"
                                            "<input type=\"checkbox\" switch=\"true\" id=\"second\" setting=\"shared-enabled\"/>"
                                            "</panel>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kSharedSettingLayout, "shared-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* root = buildResult.rootAs<PanelElement>();
    ASSERT_NE(root, nullptr);
    auto* firstControl = dynamic_cast<InputElement*>(findElementInScope(*root, "first"));
    auto* secondControl = dynamic_cast<InputElement*>(findElementInScope(*root, "second"));
    ASSERT_NE(firstControl, nullptr);
    ASSERT_NE(secondControl, nullptr);

    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("shared-enabled", provider);

    Binder binder(*root, &resolver);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(provider->observerCount(), std::size_t{2});
    provider->publish({true, false, ValueValidation::valid()});
    EXPECT_TRUE(firstControl->checked());
    EXPECT_TRUE(secondControl->checked());
    firstControl->activate();
    EXPECT_FALSE(provider->state().value);
    result.binding = Binding{};
    EXPECT_EQ(provider->observerCount(), std::size_t{0});
}

TEST(BinderTest, RejectsMisreportedSettingTypeSafely) {
    constexpr char kMisreportedSettingLayout[] = "<input type=\"checkbox\" switch=\"true\" setting=\"misreported-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildElementTreeFromString(kMisreportedSettingLayout, "misreported-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    MisreportingSettingResolver resolver;
    Binder binder(*buildResult.document->documentElement(), &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}
