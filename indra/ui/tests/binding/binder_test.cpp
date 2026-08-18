/**
 * @file binder_test.cpp
 * @brief Tests transactional Widget, Event Handler, and Value Binding preparation.
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
#include "layout/resourcecompiler.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/widgetcontract.h"

namespace {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::Button;
using radia::ui::CurrentEventArgument;
using radia::ui::DiagnosticResult;
using radia::ui::EventArgument;
using radia::ui::EventCall;
using radia::ui::EventHandlerRegistration;
using radia::ui::Field;
using radia::ui::InlineContentKind;
using radia::ui::KeybindingPresentation;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutResourceCompiler;
using radia::ui::Panel;
using radia::ui::PreparedBindingResult;
using radia::ui::ResourceSnapshot;
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::Switch;
using radia::ui::System;
using radia::ui::TextSource;
using radia::ui::ValueBinding;
using radia::ui::ValueBindingBase;
using radia::ui::ValueBindingRef;
using radia::ui::ValueBindingSubscription;
using radia::ui::ValueControlState;
using radia::ui::ValueState;
using radia::ui::ValueValidation;
using radia::ui::Visibility;
using radia::ui::Widget;
using radia::ui::WidgetEvent;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetRef;
using radia::ui::detail::findWidgetInScope;
using radia::ui::detail::makeEventRegistration;
using radia::ui::detail::WidgetCompilerAccess;

const char* noEventArguments(const EventCall& call, WidgetEventKind) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

const char* currentEventArgument(const EventCall& call, WidgetEventKind) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<CurrentEventArgument>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

template<typename T> const char* singleEventArgument(const EventCall& call, WidgetEventKind) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<T>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

void bindEvent(Binder& binder, std::string settingName, std::optional<WidgetEventKind> kind, EventHandlerRegistration::Invoke invoke,
               EventHandlerRegistration::ArgumentError argumentError) {
    binder.event(makeEventRegistration(std::move(settingName), kind, std::move(invoke), std::move(argumentError)));
}

template<typename Callback> void bindEvent(Binder& binder, std::string settingName, Callback callback) {
    bindEvent(
        binder, std::move(settingName), std::nullopt, [callback = std::move(callback)](const WidgetEvent&, const EventCall&) mutable { callback(); },
        noEventArguments);
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

template<typename WidgetT> WidgetRef<WidgetT> lookupWidget(Widget& root, std::string_view id) {
    return WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(findWidgetInScope(root, id)));
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

TEST(BinderTest, CommitsEventBindingAndResolvesTypedWidget) {
    Panel root;
    auto button = std::make_unique<Button>();
    button->setId("save").setEventCall(WidgetEventKind::Click, EventCall("save"));
    root.addChild(std::move(button));

    int activations = 0;
    WidgetRef<Button> save;
    Binder binder(root);
    save = lookupWidget<Button>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(static_cast<bool>(save));
    save->activate();
    EXPECT_EQ(activations, 1);
}

TEST(BinderTest, DistinguishesTypedAndMissingWidgetLookups) {
    Panel root;
    auto button = std::make_unique<Button>();
    button->setId("save").setEventCall(WidgetEventKind::Click, EventCall("save"));
    Button* source = button.get();
    root.addChild(std::move(button));
    auto label = std::make_unique<radia::ui::Label>();
    label->setId("status");
    root.addChild(std::move(label));

    int activations = 0;
    WidgetRef<Button> save;
    WidgetRef<Button> wrongType;
    WidgetRef<radia::ui::Label> missing;
    Binder binder(root);
    save = lookupWidget<Button>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    wrongType = lookupWidget<Button>(root, "status");
    missing = lookupWidget<radia::ui::Label>(root, "missing");
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(static_cast<bool>(save));
    EXPECT_FALSE(static_cast<bool>(wrongType));
    EXPECT_FALSE(static_cast<bool>(missing));
    source->activate();
    EXPECT_EQ(activations, 1);
}

TEST(BinderTest, InvalidatesWidgetReferenceAfterWidgetRemoval) {
    Panel root;
    auto button = std::make_unique<Button>();
    button->setId("temporary");
    root.addChild(std::move(button));
    WidgetRef<Button> reference;
    Binder binder(root);
    reference = lookupWidget<Button>(root, "temporary");
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(static_cast<bool>(reference));
    root.clearChildren();
    EXPECT_FALSE(static_cast<bool>(reference));
}

TEST(BinderTest, DetachesEventHandlerWhenBindingIsDestroyed) {
    Panel root;
    auto button = std::make_unique<Button>();
    Button* source = button.get();
    button->setEventCall(WidgetEventKind::Click, EventCall("optional"));
    root.addChild(std::move(button));

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

TEST(BinderTest, RejectsOneHandlerAcrossMultipleEventKinds) {
    Panel root;
    auto button = std::make_unique<Button>();
    button->setEventCall(WidgetEventKind::Click, EventCall("shared"));
    root.addChild(std::move(button));
    auto control = std::make_unique<Switch>();
    control->setEventCall(WidgetEventKind::Change, EventCall("shared"));
    root.addChild(std::move(control));

    Binder binder(root);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "binding.event.kind_mismatch");
}

TEST(BinderTest, BindsChangeEventsWithCurrentState) {
    Panel root;
    auto control = std::make_unique<Switch>();
    Switch* source = control.get();
    control->setEventCall(WidgetEventKind::Change, EventCall("changed", {CurrentEventArgument{}}));
    root.addChild(std::move(control));

    int changes = 0;
    Binder binder(root);
    bindEvent(
        binder, "changed", WidgetEventKind::Change,
        [&](const WidgetEvent& event, const EventCall&) {
            EXPECT_TRUE(static_cast<const radia::ui::ChangeEvent&>(event).checked);
            ++changes;
        },
        currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    source->setChecked(false);
    EXPECT_EQ(changes, 0);
    source->activate();
    EXPECT_EQ(changes, 1);
}

TEST(BinderTest, ResolvesIdsWithinIndependentResourceScopes) {
    Panel root;
    auto left = std::make_unique<Panel>();
    left->setId("left");
    radia::ui::detail::WidgetCompilerAccess::setIdScopeRoot(*left);
    auto leftItem = std::make_unique<radia::ui::Label>();
    leftItem->setId("item");
    left->addChild(std::move(leftItem));
    root.addChild(std::move(left));

    auto right = std::make_unique<Panel>();
    right->setId("right");
    radia::ui::detail::WidgetCompilerAccess::setIdScopeRoot(*right);
    auto rightItem = std::make_unique<radia::ui::Label>();
    rightItem->setId("item");
    right->addChild(std::move(rightItem));
    root.addChild(std::move(right));

    WidgetRef<Panel> leftScope;
    WidgetRef<Panel> rightScope;
    Binder parent(root);
    leftScope = lookupWidget<Panel>(root, "left");
    rightScope = lookupWidget<Panel>(root, "right");
    const TestBindingResult parentResult = finishBinding(parent);
    ASSERT_TRUE(parentResult.ok());
    ASSERT_TRUE(static_cast<bool>(leftScope));
    ASSERT_TRUE(static_cast<bool>(rightScope));

    WidgetRef<radia::ui::Label> leftBound;
    WidgetRef<radia::ui::Label> rightBound;
    Binder leftBinder(*leftScope);
    leftBound = lookupWidget<radia::ui::Label>(*leftScope, "item");
    Binder rightBinder(*rightScope);
    rightBound = lookupWidget<radia::ui::Label>(*rightScope, "item");
    const TestBindingResult leftResult = finishBinding(leftBinder);
    ASSERT_TRUE(leftResult.ok());
    ASSERT_TRUE(static_cast<bool>(leftBound));
    const TestBindingResult rightResult = finishBinding(rightBinder);
    ASSERT_TRUE(rightResult.ok());
    ASSERT_TRUE(static_cast<bool>(rightBound));
    EXPECT_NE(leftBound.get(), rightBound.get());
}

TEST(BinderTest, PreparesReplacementWithoutMutatingLiveBinding) {
    Panel live;
    auto liveButton = std::make_unique<Button>();
    Button* liveButtonPtr = liveButton.get();
    liveButton->setId("reload").setEventCall(WidgetEventKind::Click, EventCall("reload"));
    live.addChild(std::move(liveButton));

    WidgetRef<Button> reference;
    Binder liveBinder(live);
    reference = lookupWidget<Button>(live, "reload");
    bindEvent(liveBinder, "reload", [] {});
    TestBindingResult liveBinding = finishBinding(liveBinder);
    ASSERT_TRUE(liveBinding.ok());
    EXPECT_EQ(reference.get(), liveButtonPtr);

    Panel candidate;
    auto candidateButton = std::make_unique<Button>();
    Button* candidateButtonPtr = candidateButton.get();
    candidateButton->setId("reload").setEventCall(WidgetEventKind::Click, EventCall("reload"));
    candidate.addChild(std::move(candidateButton));

    Binder candidateBinder(candidate);
    auto candidateReference = lookupWidget<Button>(candidate, "reload");
    bindEvent(candidateBinder, "reload", [] {});
    PreparedBindingResult prepared = candidateBinder.prepare();
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(reference.get(), liveButtonPtr);
    ASSERT_TRUE(static_cast<bool>(candidateReference));

    Binding replacement = prepared.binding.commit();
    EXPECT_EQ(candidateReference.get(), candidateButtonPtr);
    EXPECT_TRUE(static_cast<bool>(replacement));

    Panel removedCandidate;
    Binder removedBinder(removedCandidate);
    auto removedReference = lookupWidget<Button>(removedCandidate, "reload");
    PreparedBindingResult removed = removedBinder.prepare();
    ASSERT_TRUE(removed.ok());
    EXPECT_FALSE(static_cast<bool>(removedReference));
    EXPECT_EQ(reference.get(), liveButtonPtr);
    Binding removedBinding = removed.binding.commit();
    EXPECT_EQ(reference.get(), liveButtonPtr);
    EXPECT_TRUE(static_cast<bool>(removedBinding));
}

TEST(BinderTest, AllowsUnmatchedOptionalEventHandler) {
    Panel root;
    Binder binder(root);
    bindEvent(binder, "missing", [] {});
    PreparedBindingResult result = binder.prepare();
    ASSERT_TRUE(result.ok());
    Binding binding = result.binding.commit();
    EXPECT_TRUE(static_cast<bool>(binding));
}

TEST(BinderTest, WarnsForUnhandledLayoutEvent) {
    Panel root;
    auto button = std::make_unique<Button>();
    button->setEventCall(WidgetEventKind::Click, EventCall("unhandled"));
    root.addChild(std::move(button));

    Binder binder(root);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t{1});
    EXPECT_EQ(result.warnings.front().code, "binding.event.unhandled");
}

TEST(BinderTest, PreservesLiveValueBindingUntilReplacementCommits) {
    Panel liveRoot;
    auto live = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver liveResolver;
    liveResolver.add("demo-enabled", live);
    ValueBindingRef<bool> reference;
    Binder liveBinder(liveRoot, &liveResolver);
    liveBinder.requireValueBinding({"demo-enabled"}, reference);
    TestBindingResult liveResult = finishBinding(liveBinder);
    ASSERT_TRUE(liveResult.ok());
    EXPECT_EQ(reference.get(), live.get());

    Panel candidateRoot;
    auto candidate = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver candidateResolver;
    candidateResolver.add("demo-enabled", candidate);
    Binder candidateBinder(candidateRoot, &candidateResolver);
    candidateBinder.requireValueBinding({"demo-enabled"}, reference);
    PreparedBindingResult prepared = candidateBinder.prepare();
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(reference.get(), live.get());

    {
        Panel abandonedRoot;
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
    ASSERT_TRUE(static_cast<bool>(reference));
    EXPECT_TRUE(reference->state().value);
}

TEST(BinderTest, RejectsMissingSetting) {
    Panel root;
    ValueBindingRef<bool> reference;
    TestSettingResolver resolver;
    Binder binder(root, &resolver);
    binder.requireValueBinding({"missing-value"}, reference);
    PreparedBindingResult result = binder.prepare();
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(static_cast<bool>(reference));
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.missing");
}

TEST(BinderTest, RejectsSettingTypeMismatch) {
    Panel root;
    auto setting = std::make_shared<TestValueBinding<std::string>>("enabled");
    TestSettingResolver resolver;
    resolver.add("demo-enabled", setting);
    ValueBindingRef<bool> reference;
    Binder binder(root, &resolver);
    binder.requireValueBinding({"demo-enabled"}, reference);
    const PreparedBindingResult result = binder.prepare();
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(static_cast<bool>(reference));
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}

TEST(BinderTest, ResolvesRepeatedValueRequirementsIndependently) {
    Panel root;
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

TEST(BinderTest, BindsFieldSwitchAndPropagatesValidation) {
    constexpr char kFieldLayout[] = "<field><label for=\"demoSwitch\">Enabled</label>"
                                    "<switch id=\"demoSwitch\" setting=\"demo-enabled\" onChange=\"demoChanged()\"/>"
                                    "<br/><hint>Persistent hint</hint><br/><error>Fallback error</error></field>";
    constexpr char kMissingValueLayout[] = "<switch setting=\"missing-value\"/>";
    constexpr char kTypedValueLayout[] = "<switch setting=\"typed-value\"/>";
    constexpr char kRetainedValueLayout[] = "<switch setting=\"retained-value\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kFieldLayout, "bound-field.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* field = buildResult.rootAs<Field>();
    ASSERT_NE(field, nullptr);
    auto* control = dynamic_cast<Switch*>(field->valueControl());
    ASSERT_NE(control, nullptr);

    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);
    int changes = 0;
    Binder binder(*field, &resolver);
    bindEvent(
        binder, "demoChanged", WidgetEventKind::Change,
        [&](const WidgetEvent& event, const EventCall&) {
            EXPECT_FALSE(static_cast<const radia::ui::ChangeEvent&>(event).checked);
            ++changes;
        },
        noEventArguments);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(provider->observerCount(), std::size_t{1});

    provider->publish({true, false, ValueValidation::invalid(TextSource::text("Dynamic error"))});
    EXPECT_TRUE(control->checked());
    EXPECT_TRUE(field->dirty());
    EXPECT_TRUE(field->invalid());
    ASSERT_NE(field->error(), nullptr);
    EXPECT_EQ(field->error()->text(), "Dynamic error");
    EXPECT_EQ(field->error()->visibility(), Visibility::Visible);

    int valueStatePublications = 0;
    ValueBindingSubscription valueStateSubscription = control->observeValueControlState([&](const ValueControlState&) { ++valueStatePublications; });
    control->activate();
    EXPECT_FALSE(provider->state().value);
    EXPECT_FALSE(control->checked());
    EXPECT_EQ(valueStatePublications, 1);
    EXPECT_EQ(changes, 1);
    valueStateSubscription.reset();

    provider->publish({false, false, ValueValidation::valid()});
    EXPECT_FALSE(field->invalid());
    ASSERT_NE(field->error(), nullptr);
    EXPECT_EQ(field->error()->visibility(), Visibility::Collapsed);

    result.binding = Binding{};
    EXPECT_EQ(provider->observerCount(), std::size_t{0});
    provider->publish({true, false, ValueValidation::valid()});
    EXPECT_FALSE(control->checked());

    LayoutBuildResult missingBuildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kMissingValueLayout, "missing-value.xml");
    ASSERT_TRUE(missingBuildResult.ok());
    ASSERT_NE(missingBuildResult.root, nullptr);
    TestSettingResolver missingResolver;
    Binder missingBinder(*missingBuildResult.root, &missingResolver);
    TestBindingResult missing = finishBinding(missingBinder);
    ASSERT_FALSE(missing.ok());
    ASSERT_EQ(missing.errors.size(), std::size_t{1});
    EXPECT_EQ(missing.errors.front().code, "binding.setting.missing");

    LayoutBuildResult typedBuildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kTypedValueLayout, "typed-value.xml");
    ASSERT_TRUE(typedBuildResult.ok());
    ASSERT_NE(typedBuildResult.root, nullptr);
    auto wrongType = std::make_shared<TestValueBinding<std::string>>("false");
    TestSettingResolver typedResolver;
    typedResolver.add("typed-value", wrongType);
    Binder typedBinder(*typedBuildResult.root, &typedResolver);
    TestBindingResult typed = finishBinding(typedBinder);
    ASSERT_FALSE(typed.ok());
    ASSERT_EQ(typed.errors.size(), std::size_t{1});
    EXPECT_EQ(typed.errors.front().code, "binding.setting.type_mismatch");

    LayoutBuildResult lifetimeBuildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kRetainedValueLayout, "retained-value.xml");
    ASSERT_TRUE(lifetimeBuildResult.ok());
    ASSERT_NE(lifetimeBuildResult.root, nullptr);
    auto retainedProvider = std::make_shared<TestValueBinding<bool>>(false);
    std::weak_ptr<TestValueBinding<bool>> providerLifetime = retainedProvider;
    TestSettingResolver lifetimeResolver;
    lifetimeResolver.add("retained-value", retainedProvider);
    Binder lifetimeBinder(*lifetimeBuildResult.root, &lifetimeResolver);
    TestBindingResult lifetimeBinding = finishBinding(lifetimeBinder);
    ASSERT_TRUE(lifetimeBinding.ok());
    lifetimeResolver.clear();
    retainedProvider.reset();
    lifetimeBuildResult.root.reset();
    EXPECT_FALSE(providerLifetime.expired());
    lifetimeBinding.binding = Binding{};
    EXPECT_TRUE(providerLifetime.expired());
}

TEST(BinderTest, ReplacesValueBindingOnTheSameControl) {
    constexpr char kReplaceableValueLayout[] = "<switch setting=\"replaceable-value\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kReplaceableValueLayout, "replaceable-value.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* control = buildResult.rootAs<Switch>();
    ASSERT_NE(control, nullptr);
    auto firstProvider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver firstResolver;
    firstResolver.add("replaceable-value", firstProvider);
    Binder firstBinder(*control, &firstResolver);
    TestBindingResult firstResult = finishBinding(firstBinder);
    ASSERT_TRUE(firstResult.ok());

    auto secondProvider = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver secondResolver;
    secondResolver.add("replaceable-value", secondProvider);
    Binder secondBinder(*control, &secondResolver);
    TestBindingResult secondResult = finishBinding(secondBinder);
    ASSERT_TRUE(secondResult.ok());
    EXPECT_TRUE(control->checked());

    firstResult.binding = std::move(secondResult.binding);
    control->activate();
    EXPECT_FALSE(secondProvider->state().value);
    EXPECT_FALSE(control->checked());
    EXPECT_FALSE(firstProvider->state().value);
}

TEST(BinderTest, LocalizesFieldValidationAndRefreshesKeybindings) {
    ResourceSnapshot snapshot;
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {name: English, strings: {field.label: Enabled, "
                                     "field.fallback: 'Fallback EN <b>bold EN</b> <kbd shortcut=\"toggle-demo\"/>', "
                                     "field.dynamic: Dynamic EN}}, "
                                     "pt: {name: Portuguese, strings: {field.label: Ativado, "
                                     "field.fallback: 'Fallback PT <b>bold PT</b> <kbd shortcut=\"toggle-demo\"/>', "
                                     "field.dynamic: Dynamic PT}}}\n";
    constexpr char kFieldLayout[] = "<field><label for=\"demoSwitch\">field.label</label>"
                                    "<switch id=\"demoSwitch\" setting=\"demo-enabled\"/>"
                                    "<br/><error>field.fallback</error></field>";
    snapshot.add("localization.yaml", kLocalization);
    snapshot.add("skin.radia", "");
    snapshot.add("field.xml", kFieldLayout);

    const SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    KeybindingPresentation presentation{{"F"}};
    System system;
    system.setKeybindingResolver(
        [&presentation](const std::string& binding) { return binding == "toggle-demo" ? presentation : KeybindingPresentation{}; });
    system.publish(prepared.generation);
    LayoutBuildResult buildResult = system.buildWidgetTree("field.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* field = buildResult.rootAs<Field>();
    ASSERT_NE(field, nullptr);
    ASSERT_NE(field->error(), nullptr);

    const TextSource staleDynamic = system.localize("field.dynamic");
    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);
    Binder binder(*field, &resolver);
    TestBindingResult binding = finishBinding(binder);
    ASSERT_TRUE(binding.ok());

    std::unique_ptr<Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    surface->mount(std::move(buildResult.root));
    provider->publish({false, false, ValueValidation::invalid()});
    ASSERT_NE(field->error(), nullptr);
    EXPECT_EQ(field->error()->text(), "Fallback EN bold EN F");
    const auto& englishNodes = field->error()->content().nodes();
    ASSERT_EQ(englishNodes.size(), std::size_t{4});
    EXPECT_EQ(englishNodes[1].kind(), InlineContentKind::B);
    ASSERT_FALSE(englishNodes[1].children().empty());
    EXPECT_EQ(englishNodes[1].children().front().value(), "bold EN");
    ASSERT_FALSE(englishNodes[3].keybindingPresentation().keys.empty());
    EXPECT_EQ(englishNodes[3].keybindingPresentation().keys.front(), "F");

    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(field->error()->text(), "Fallback PT bold PT F");
    const auto& portugueseNodes = field->error()->content().nodes();
    ASSERT_EQ(portugueseNodes.size(), std::size_t{4});
    EXPECT_EQ(portugueseNodes[1].kind(), InlineContentKind::B);
    ASSERT_FALSE(portugueseNodes[1].children().empty());
    EXPECT_EQ(portugueseNodes[1].children().front().value(), "bold PT");
    ASSERT_FALSE(portugueseNodes[3].keybindingPresentation().keys.empty());
    EXPECT_EQ(portugueseNodes[3].keybindingPresentation().keys.front(), "F");

    provider->publish({false, false, ValueValidation::invalid(staleDynamic)});
    EXPECT_EQ(field->error()->text(), "Dynamic PT");
    provider->publish({false, false, ValueValidation::valid()});
    provider->publish({false, false, ValueValidation::invalid()});
    EXPECT_EQ(field->error()->text(), "Fallback PT bold PT F");

    presentation = {{"Ctrl", "F"}};
    system.refreshKeybindings();
    provider->publish({false, false, ValueValidation::valid()});
    provider->publish({false, false, ValueValidation::invalid()});
    EXPECT_EQ(field->error()->text(), "Fallback PT bold PT Ctrl F");
    const auto& refreshedNodes = field->error()->content().nodes();
    ASSERT_EQ(refreshedNodes.size(), std::size_t{4});
    EXPECT_EQ(refreshedNodes[1].kind(), InlineContentKind::B);
}

TEST(BinderTest, WarnsWhenEventArgumentsDoNotMatchHandler) {
    Panel root;
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setEventCall(WidgetEventKind::Click, EventCall("inspect", {EventArgument(std::int64_t(4))}));
    root.addChild(std::move(button));

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
    Panel root;
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setEventCall(WidgetEventKind::Click, EventCall("press"));
    root.addChild(std::move(button));

    int invocations = 0;
    Binder binder(root);
    bindEvent(binder, "press", [&] { ++invocations; });
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    target->activate();
    EXPECT_EQ(invocations, 1);
}

TEST(BinderTest, DispatchesTypedEventArguments) {
    Panel root;
    auto select = std::make_unique<Button>();
    Button* selectTarget = select.get();
    select->setEventCall(WidgetEventKind::Click, EventCall("select", {EventArgument(std::int64_t(4))}));
    root.addChild(std::move(select));
    auto open = std::make_unique<Button>();
    Button* openTarget = open.get();
    open->setEventCall(WidgetEventKind::Click, EventCall("open", {EventArgument(std::string("settings"))}));
    root.addChild(std::move(open));
    auto enabled = std::make_unique<Button>();
    Button* enabledTarget = enabled.get();
    enabled->setEventCall(WidgetEventKind::Click, EventCall("updateAdvanced", {EventArgument(true)}));
    root.addChild(std::move(enabled));
    auto inspect = std::make_unique<Button>();
    Button* inspectTarget = inspect.get();
    inspect->setEventCall(WidgetEventKind::Click, EventCall("inspectEventSource", {EventArgument(CurrentEventArgument{})}));
    root.addChild(std::move(inspect));

    int selected = 0;
    std::string destination;
    bool advanced = false;
    Widget* source = nullptr;
    Binder binder(root);
    bindEvent(
        binder, "select", WidgetEventKind::Click,
        [&](const WidgetEvent&, const EventCall& call) { selected = static_cast<int>(std::get<std::int64_t>(call.arguments().front())); },
        singleEventArgument<std::int64_t>);
    bindEvent(
        binder, "open", WidgetEventKind::Click,
        [&](const WidgetEvent&, const EventCall& call) { destination = std::get<std::string>(call.arguments().front()); },
        singleEventArgument<std::string>);
    bindEvent(
        binder, "updateAdvanced", WidgetEventKind::Click,
        [&](const WidgetEvent&, const EventCall& call) { advanced = std::get<bool>(call.arguments().front()); }, singleEventArgument<bool>);
    bindEvent(
        binder, "inspectEventSource", WidgetEventKind::Click, [&](const WidgetEvent& event, const EventCall&) { source = &event.source; },
        currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
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
    Panel root;
    auto button = std::make_unique<Button>();
    button->setEventCall(WidgetEventKind::Click, EventCall("bad_action"));
    root.addChild(std::move(button));

    Binder binder(root);
    bindEvent(binder, "bad_action", [] {});
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.event.name_invalid");
}

TEST(BinderTest, DispatchesCommonWidgetEventContext) {
    Panel root;
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    button->setEventCall(WidgetEventKind::Click, EventCall("observe"));
    root.addChild(std::move(button));

    Widget* source = nullptr;
    WidgetEventKind kind = WidgetEventKind::Change;
    Binder binder(root);
    bindEvent(
        binder, "observe", std::nullopt,
        [&](const WidgetEvent& event, const EventCall&) {
            source = &event.source;
            kind = event.kind;
        },
        noEventArguments);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_TRUE(result.ok());
    target->activate();
    EXPECT_EQ(source, target);
    EXPECT_EQ(kind, WidgetEventKind::Click);
}

TEST(BinderTest, BindsSwitchSettingAndPropagatesUpdates) {
    constexpr char kDemoSettingLayout[] = "<switch setting=\"demo-enabled\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kDemoSettingLayout, "setting.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* control = buildResult.rootAs<Switch>();
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
    constexpr char kMissingSettingLayout[] = "<switch setting=\"missing-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kMissingSettingLayout, "missing-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    ASSERT_NE(buildResult.root, nullptr);
    TestSettingResolver resolver;
    Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.missing");
}

TEST(BinderTest, RejectsMismatchedLayoutSetting) {
    constexpr char kStringSettingLayout[] = "<switch setting=\"string-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kStringSettingLayout, "typed-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    ASSERT_NE(buildResult.root, nullptr);
    TestSettingResolver resolver;
    resolver.add("string-setting", std::make_shared<TestValueBinding<std::string>>("not a boolean"));
    Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}

TEST(BinderTest, SharesOneSettingAcrossMultipleControls) {
    constexpr char kSharedSettingLayout[] = "<panel>"
                                            "<switch id=\"first\" setting=\"shared-enabled\"/>"
                                            "<switch id=\"second\" setting=\"shared-enabled\"/>"
                                            "</panel>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kSharedSettingLayout, "shared-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    auto* root = buildResult.rootAs<Panel>();
    ASSERT_NE(root, nullptr);
    auto* firstControl = dynamic_cast<Switch*>(findWidgetInScope(*root, "first"));
    auto* secondControl = dynamic_cast<Switch*>(findWidgetInScope(*root, "second"));
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
    constexpr char kMisreportedSettingLayout[] = "<switch setting=\"misreported-setting\"/>";
    LayoutBuildResult buildResult = LayoutResourceCompiler().buildWidgetTreeFromString(kMisreportedSettingLayout, "misreported-setting.xml");
    ASSERT_TRUE(buildResult.ok());
    ASSERT_NE(buildResult.root, nullptr);
    MisreportingSettingResolver resolver;
    Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t{1});
    EXPECT_EQ(result.errors.front().code, "binding.setting.type_mismatch");
}
