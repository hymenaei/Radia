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
#include <map>
#include <optional>
#include "../test/lltut.h"
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
const char* noEventArguments(const rdui::EventCall& call, rdui::WidgetEventKind) {
    return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch";
}

const char* currentEventArgument(const rdui::EventCall& call, rdui::WidgetEventKind) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<rdui::CurrentEventArgument>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

template<typename T> const char* singleEventArgument(const rdui::EventCall& call, rdui::WidgetEventKind) {
    if (call.arguments().size() != 1) return "binding.event.arity_mismatch";
    return std::holds_alternative<T>(call.arguments().front()) ? nullptr : "binding.event.argument_type_mismatch";
}

void bindEvent(rdui::Binder& binder, std::string settingName, std::optional<rdui::WidgetEventKind> kind,
               rdui::EventHandlerRegistration::Invoke invoke, rdui::EventHandlerRegistration::ArgumentError argumentError) {
    binder.event(rdui::detail::makeEventRegistration(std::move(settingName), kind, std::move(invoke), std::move(argumentError)));
}

template<typename Callback> void bindEvent(rdui::Binder& binder, std::string settingName, Callback callback) {
    bindEvent(
        binder, std::move(settingName), std::nullopt,
        [callback = std::move(callback)](const rdui::WidgetEvent&, const rdui::EventCall&) mutable { callback(); }, noEventArguments);
}

template<typename T> class TestValueBinding final : public rdui::ValueBinding<T> {
public:
    explicit TestValueBinding(T value) : mState{value, value, std::nullopt} {}

    rdui::ValueState<T> state() const override { return mState; }
    void write(T value) override {
        mState.value = std::move(value);
        notify();
    }
    rdui::ValueBindingSubscription observe(typename rdui::ValueBinding<T>::Observer observer) override {
        const std::size_t id = mNextObserver++;
        mObservers.emplace(id, std::move(observer));
        return rdui::ValueBindingSubscription([this, id] { mObservers.erase(id); });
    }

    void publish(rdui::ValueState<T> state) {
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

    rdui::ValueState<T> mState;
    std::map<std::size_t, typename rdui::ValueBinding<T>::Observer> mObservers;
    std::size_t mNextObserver = 1;
};

class TestSettingResolver final : public rdui::SettingResolver {
public:
    template<typename T> void add(std::string settingName, std::shared_ptr<TestValueBinding<T>> binding) {
        mBindings.emplace(std::move(settingName), Entry{std::move(binding), typeid(T)});
    }

    rdui::SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override {
        const auto found = mBindings.find(std::string(settingName));
        if (found == mBindings.end()) return {rdui::SettingResolution::ResolutionStatus::Missing, {}};
        if (found->second.type != requestedType) return {rdui::SettingResolution::ResolutionStatus::TypeMismatch, {}};
        return {rdui::SettingResolution::ResolutionStatus::Found, found->second.binding};
    }

    void clear() { mBindings.clear(); }

private:
    struct Entry {
        std::shared_ptr<rdui::ValueBindingBase> binding;
        std::type_index type{typeid(void)};
    };

    std::map<std::string, Entry> mBindings;
};

class MisreportingSettingResolver final : public rdui::SettingResolver {
public:
    MisreportingSettingResolver() : binding(std::make_shared<TestValueBinding<std::string>>("wrong type")) {}

    rdui::SettingResolution resolve(std::string_view, std::type_index) override {
        return {rdui::SettingResolution::ResolutionStatus::Found, binding};
    }

    std::shared_ptr<TestValueBinding<std::string>> binding;
};

template<typename WidgetT> rdui::WidgetRef<WidgetT> lookupWidget(rdui::Widget& root, std::string_view id) {
    return rdui::WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(rdui::detail::findWidgetInScope(root, id)));
}

struct TestBindingResult : rdui::DiagnosticResult {
    bool ok() const { return !hasErrors(); }
    rdui::Binding binding;
};

TestBindingResult finishBinding(rdui::Binder& binder) {
    rdui::PreparedBindingResult prepared = binder.prepare();
    TestBindingResult result;
    result.warnings = std::move(prepared.warnings);
    result.errors = std::move(prepared.errors);
    if (prepared.ok()) result.binding = prepared.binding.commit();
    return result;
}
} // namespace

namespace tut {
struct binderData {};
using binderTest = test_group<binderData>;
using binderObject = binderTest::object;
binderTest binderTestCase("binder");

template<> template<> void binderObject::test<1>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setId("save").setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("save"));
    root.addChild(std::move(button));

    int activations = 0;
    rdui::WidgetRef<rdui::Button> save;
    rdui::Binder binder(root);
    save = lookupWidget<rdui::Button>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    TestBindingResult result = finishBinding(binder);
    ensure("Event binding transaction commits", result.ok());
    ensure("typed lookup resolves", !!save);
    save->activate();
    ensure_equals("handler attached", activations, 1);
}

template<> template<> void binderObject::test<2>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setId("save").setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("save"));
    rdui::Button* source = button.get();
    root.addChild(std::move(button));
    auto label = std::make_unique<rdui::Label>();
    label->setId("status");
    root.addChild(std::move(label));

    int activations = 0;
    rdui::WidgetRef<rdui::Button> save;
    rdui::WidgetRef<rdui::Button> wrongType;
    rdui::WidgetRef<rdui::Label> missing;
    rdui::Binder binder(root);
    save = lookupWidget<rdui::Button>(root, "save");
    bindEvent(binder, "save", [&] { ++activations; });
    wrongType = lookupWidget<rdui::Button>(root, "status");
    missing = lookupWidget<rdui::Label>(root, "missing");
    const TestBindingResult result = finishBinding(binder);
    ensure("Event transaction remains valid", result.ok());
    ensure("typed lookup distinguishes wrong and missing Widgets", save && !wrongType && !missing);
    source->activate();
    ensure_equals("handler attaches to the resolved source", activations, 1);
}

template<> template<> void binderObject::test<3>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setId("temporary");
    root.addChild(std::move(button));
    rdui::WidgetRef<rdui::Button> reference;
    rdui::Binder binder(root);
    reference = lookupWidget<rdui::Button>(root, "temporary");
    ensure("reference resolves", finishBinding(binder).ok() && reference);
    root.clearChildren();
    ensure("destroyed widget invalidates reference", !reference);
}

template<> template<> void binderObject::test<4>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* source = button.get();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("optional"));
    root.addChild(std::move(button));

    source->activate();
    int activations = 0;
    rdui::Binder binder(root);
    bindEvent(binder, "optional", [&] { ++activations; });
    TestBindingResult result = finishBinding(binder);
    ensure("declared Event Handler binds", result.ok() && result.binding);
    source->activate();
    ensure_equals("bound Event Handler runs", activations, 1);
    result.binding = rdui::Binding{};
    source->activate();
    ensure_equals("destroyed binding detaches handler", activations, 1);
}

template<> template<> void binderObject::test<5>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("shared"));
    root.addChild(std::move(button));
    auto control = std::make_unique<rdui::Switch>();
    control->setEventCall(rdui::WidgetEventKind::Change, rdui::EventCall("shared"));
    root.addChild(std::move(control));

    rdui::Binder binder(root);
    const TestBindingResult result = finishBinding(binder);
    ensure("one Handler settingName cannot span Event kinds", !result.ok());
    ensure_equals("kind conflict diagnostic", result.errors.front().code, "binding.event.kind_mismatch");
}

template<> template<> void binderObject::test<6>() {
    rdui::Panel root;
    auto control = std::make_unique<rdui::Switch>();
    rdui::Switch* source = control.get();
    control->setEventCall(rdui::WidgetEventKind::Change, rdui::EventCall("changed", {rdui::CurrentEventArgument{}}));
    root.addChild(std::move(control));

    int changes = 0;
    rdui::Binder binder(root);
    bindEvent(
        binder, "changed", rdui::WidgetEventKind::Change,
        [&](const rdui::WidgetEvent& event, const rdui::EventCall&) {
            ensure("change context contains completed value", static_cast<const rdui::ChangeEvent&>(event).checked);
            ++changes;
        },
        currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ensure("change binding commits", result.ok());
    source->setChecked(false);
    ensure_equals("programmatic setter stays silent", changes, 0);
    source->activate();
    ensure_equals("user activation emits change", changes, 1);
}

template<> template<> void binderObject::test<7>() {
    rdui::Panel root;
    auto left = std::make_unique<rdui::Panel>();
    left->setId("left");
    rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*left);
    auto leftItem = std::make_unique<rdui::Label>();
    leftItem->setId("item");
    left->addChild(std::move(leftItem));
    root.addChild(std::move(left));

    auto right = std::make_unique<rdui::Panel>();
    right->setId("right");
    rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*right);
    auto rightItem = std::make_unique<rdui::Label>();
    rightItem->setId("item");
    right->addChild(std::move(rightItem));
    root.addChild(std::move(right));

    rdui::WidgetRef<rdui::Panel> leftScope;
    rdui::WidgetRef<rdui::Panel> rightScope;
    rdui::Binder parent(root);
    leftScope = lookupWidget<rdui::Panel>(root, "left");
    rightScope = lookupWidget<rdui::Panel>(root, "right");
    ensure("parent resolves resource instances", finishBinding(parent).ok() && leftScope && rightScope);

    rdui::WidgetRef<rdui::Label> leftBound;
    rdui::WidgetRef<rdui::Label> rightBound;
    rdui::Binder leftBinder(*leftScope);
    leftBound = lookupWidget<rdui::Label>(*leftScope, "item");
    rdui::Binder rightBinder(*rightScope);
    rightBound = lookupWidget<rdui::Label>(*rightScope, "item");
    ensure("left local id binds", finishBinding(leftBinder).ok() && leftBound);
    ensure("right duplicate local id binds independently", finishBinding(rightBinder).ok() && rightBound);
    ensure("resource instances remain distinct", leftBound.get() != rightBound.get());
}

template<> template<> void binderObject::test<8>() {
    rdui::Panel live;
    auto liveButton = std::make_unique<rdui::Button>();
    rdui::Button* liveButtonPtr = liveButton.get();
    liveButton->setId("reload").setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("reload"));
    live.addChild(std::move(liveButton));

    rdui::WidgetRef<rdui::Button> reference;
    rdui::Binder liveBinder(live);
    reference = lookupWidget<rdui::Button>(live, "reload");
    bindEvent(liveBinder, "reload", [] {});
    TestBindingResult liveBinding = finishBinding(liveBinder);
    ensure("initial binding commits", liveBinding.ok() && reference.get() == liveButtonPtr);

    rdui::Panel candidate;
    auto candidateButton = std::make_unique<rdui::Button>();
    rdui::Button* candidateButtonPtr = candidateButton.get();
    candidateButton->setId("reload").setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("reload"));
    candidate.addChild(std::move(candidateButton));

    rdui::Binder candidateBinder(candidate);
    auto candidateReference = lookupWidget<rdui::Button>(candidate, "reload");
    bindEvent(candidateBinder, "reload", [] {});
    rdui::PreparedBindingResult prepared = candidateBinder.prepare();
    ensure("replacement binding prepares", prepared.ok());
    ensure("preparation leaves live reference untouched", reference.get() == liveButtonPtr && candidateReference);

    rdui::Binding replacement = prepared.binding.commit();
    ensure("candidate Event binding commits independently", candidateReference.get() == candidateButtonPtr);
    ensure("prepared commit returns attached handlers", !!replacement);

    rdui::Panel removedCandidate;
    rdui::Binder removedBinder(removedCandidate);
    auto removedReference = lookupWidget<rdui::Button>(removedCandidate, "reload");
    rdui::PreparedBindingResult removed = removedBinder.prepare();
    ensure("removing a widget keeps the reference model independent", removed.ok() && !removedReference);
    ensure("removal preparation leaves the live reference untouched", reference.get() == liveButtonPtr);
    rdui::Binding removedBinding = removed.binding.commit();
    ensure("committing widget removal leaves controller-owned references unchanged", reference.get() == liveButtonPtr);
    ensure("widget removal still returns a valid binding", !!removedBinding);
}

template<> template<> void binderObject::test<9>() {
    rdui::Panel root;
    rdui::Binder binder(root);
    bindEvent(binder, "missing", [] {});
    rdui::PreparedBindingResult result = binder.prepare();
    ensure("Controller Event Handler may be absent from this layout", result.ok());
    ensure("optional Event Handler still produces a valid binding", !!result.binding.commit());
}

template<> template<> void binderObject::test<10>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("unhandled"));
    root.addChild(std::move(button));

    rdui::Binder binder(root);
    const TestBindingResult result = finishBinding(binder);
    ensure("unhandled Layout Resource call does not reject binding", result.ok());
    ensure_equals("unhandled call reports one warning", result.warnings.size(), 1U);
    ensure_equals("unhandled call diagnostic", result.warnings.front().code, "binding.event.unhandled");
}

template<> template<> void binderObject::test<11>() {
    rdui::Panel liveRoot;
    auto live = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver liveResolver;
    liveResolver.add("demo-enabled", live);
    rdui::ValueBindingRef<bool> reference;
    rdui::Binder liveBinder(liveRoot, &liveResolver);
    liveBinder.requireValueBinding({"demo-enabled"}, reference);
    TestBindingResult liveResult = finishBinding(liveBinder);
    ensure("typed value binding commits", liveResult.ok() && liveResult.binding);
    ensure("committed reference points at the live provider", reference.get() == live.get());

    rdui::Panel candidateRoot;
    auto candidate = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver candidateResolver;
    candidateResolver.add("demo-enabled", candidate);
    rdui::Binder candidateBinder(candidateRoot, &candidateResolver);
    candidateBinder.requireValueBinding({"demo-enabled"}, reference);
    rdui::PreparedBindingResult prepared = candidateBinder.prepare();
    ensure("replacement value binding prepares", prepared.ok());
    ensure("preparation does not replace the live provider", reference.get() == live.get());

    {
        rdui::Panel abandonedRoot;
        auto abandoned = std::make_shared<TestValueBinding<bool>>(true);
        TestSettingResolver abandonedResolver;
        abandonedResolver.add("demo-enabled", abandoned);
        rdui::Binder abandonedBinder(abandonedRoot, &abandonedResolver);
        abandonedBinder.requireValueBinding({"demo-enabled"}, reference);
        rdui::PreparedBindingResult abandonedResult = abandonedBinder.prepare();
        ensure("a second replacement value binding prepares", abandonedResult.ok());
    }
    ensure("discarding a prepared transaction preserves the live provider", reference.get() == live.get());

    rdui::Binding replacement = prepared.binding.commit();
    ensure("commit switches to the candidate provider", replacement && reference.get() == candidate.get());
    ensure("candidate state is available through the typed reference", reference->state().value);
}

template<> template<> void binderObject::test<12>() {
    rdui::Panel root;
    rdui::ValueBindingRef<bool> reference;
    TestSettingResolver resolver;
    rdui::Binder binder(root, &resolver);
    binder.requireValueBinding({"missing-value"}, reference);
    rdui::PreparedBindingResult result = binder.prepare();
    ensure("missing setting rejects the transaction", !result.ok() && !reference);
    ensure_equals("missing setting diagnostic", result.errors.front().code, std::string("binding.setting.missing"));
}

template<> template<> void binderObject::test<13>() {
    rdui::Panel root;
    auto setting = std::make_shared<TestValueBinding<std::string>>("enabled");
    TestSettingResolver resolver;
    resolver.add("demo-enabled", setting);
    rdui::ValueBindingRef<bool> reference;
    rdui::Binder binder(root, &resolver);
    binder.requireValueBinding({"demo-enabled"}, reference);
    const rdui::PreparedBindingResult result = binder.prepare();
    ensure("setting type mismatch rejects the transaction", !result.ok() && !reference);
    ensure_equals("setting type mismatch diagnostic", result.errors.front().code, std::string("binding.setting.type_mismatch"));
}

template<> template<> void binderObject::test<14>() {
    rdui::Panel root;
    auto setting = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", setting);
    rdui::ValueBindingRef<bool> first;
    rdui::ValueBindingRef<bool> second;
    rdui::Binder binder(root, &resolver);
    binder.requireValueBinding({"demo-enabled"}, first);
    binder.requireValueBinding({"demo-enabled"}, second);
    rdui::PreparedBindingResult result = binder.prepare();
    ensure("repeated setting requirements prepare", result.ok());
    rdui::Binding binding = result.binding.commit();
    ensure("repeated setting requirements resolve independently", binding && first.get() == setting.get() && second.get() == setting.get());
}

template<> template<> void binderObject::test<15>() {
    const char* kFieldLayout =
        "<field><label for=\"demoSwitch\">Enabled</label><switch id=\"demoSwitch\" setting=\"demo-enabled\" onChange=\"demoChanged()\"/><br/><hint>Persistent hint</hint><br/><error>Fallback error</error></field>";
    rdui::LayoutBuildResult buildResult = rdui::LayoutResourceCompiler().buildWidgetTreeFromString(kFieldLayout, "bound-field.xml");
    auto* field = buildResult.rootAs<rdui::Field>();
    auto* control = field ? dynamic_cast<rdui::Switch*>(field->valueControl()) : nullptr;
    ensure("bound standalone Field compiles", buildResult.ok() && field && control);

    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);
    int changes = 0;
    rdui::Binder binder(*field, &resolver);
    bindEvent(
        binder, "demoChanged", rdui::WidgetEventKind::Change,
        [&](const rdui::WidgetEvent& event, const rdui::EventCall&) {
            ensure("bound Change Event observes the provider value", !static_cast<const rdui::ChangeEvent&>(event).checked);
            ++changes;
        },
        noEventArguments);
    TestBindingResult result = finishBinding(binder);
    ensure("Switch setting resolves through Binder", result.ok() && result.binding);
    ensure_equals("committed Switch owns one provider subscription", provider->observerCount(), 1U);

    provider->publish({true, false, rdui::ValueValidation::invalid(rdui::TextSource::text("Dynamic error"))});
    ensure("external provider state updates Switch", control->checked());
    ensure("Field reflects dirty Value State", field->dirty());
    ensure("Field reflects invalid Value State", field->invalid());
    ensure_equals("dynamic validation text overrides authored Error", field->error()->text(), "Dynamic error");
    ensure_equals("invalid Field reveals Error", static_cast<int>(field->error()->visibility()), static_cast<int>(rdui::Visibility::Visible));

    int valueStatePublications = 0;
    rdui::ValueBindingSubscription valueStateSubscription =
        control->observeValueControlState([&](const rdui::ValueControlState&) { ++valueStatePublications; });
    control->activate();
    ensure("Switch activation writes through the provider", !provider->state().value && !control->checked());
    ensure_equals("synchronous bound activation publishes one value state", valueStatePublications, 1);
    ensure_equals("bound Switch emits Change after writing the provider", changes, 1);
    valueStateSubscription.reset();

    provider->publish({false, false, rdui::ValueValidation::valid()});
    ensure("valid provider state clears Field invalid state", !field->invalid());
    ensure_equals("valid Field collapses Error", static_cast<int>(field->error()->visibility()), static_cast<int>(rdui::Visibility::Collapsed));

    result.binding = rdui::Binding{};
    ensure_equals("reset Binding disconnects automatic Value Control observation", provider->observerCount(), 0U);
    provider->publish({true, false, rdui::ValueValidation::valid()});
    ensure("disconnected provider no longer updates Switch", !control->checked());

    rdui::LayoutBuildResult missingBuildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"missing-value\"/>", "missing-value.xml");
    TestSettingResolver missingResolver;
    rdui::Binder missingBinder(*missingBuildResult.root, &missingResolver);
    TestBindingResult missing = finishBinding(missingBinder);
    ensure("authored setting requires a matching setting", !missing.ok());
    ensure_equals("automatic missing setting diagnostic is stable", missing.errors.front().code, std::string("binding.setting.missing"));

    rdui::LayoutBuildResult typedBuildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"typed-value\"/>", "typed-value.xml");
    auto wrongType = std::make_shared<TestValueBinding<std::string>>("false");
    TestSettingResolver typedResolver;
    typedResolver.add("typed-value", wrongType);
    rdui::Binder typedBinder(*typedBuildResult.root, &typedResolver);
    TestBindingResult typed = finishBinding(typedBinder);
    ensure("authored setting enforces the Value Control type", !typed.ok());
    ensure_equals("automatic type mismatch diagnostic is stable", typed.errors.front().code, std::string("binding.setting.type_mismatch"));

    rdui::LayoutBuildResult lifetimeBuildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"retained-value\"/>", "retained-value.xml");
    auto retainedProvider = std::make_shared<TestValueBinding<bool>>(false);
    std::weak_ptr<TestValueBinding<bool>> providerLifetime = retainedProvider;
    TestSettingResolver lifetimeResolver;
    lifetimeResolver.add("retained-value", retainedProvider);
    rdui::Binder lifetimeBinder(*lifetimeBuildResult.root, &lifetimeResolver);
    TestBindingResult lifetimeBinding = finishBinding(lifetimeBinder);
    ensure("provider lifetime binding commits", lifetimeBinding.ok());
    lifetimeResolver.clear();
    retainedProvider.reset();
    lifetimeBuildResult.root.reset();
    ensure("Binding retains provider until its subscription disconnects", !providerLifetime.expired());
    lifetimeBinding.binding = rdui::Binding{};
    ensure("provider is released after Binding disconnects", providerLifetime.expired());
}

template<> template<> void binderObject::test<16>() {
    rdui::LayoutBuildResult buildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"replaceable-value\"/>", "replaceable-value.xml");
    auto* control = buildResult.rootAs<rdui::Switch>();
    auto firstProvider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver firstResolver;
    firstResolver.add("replaceable-value", firstProvider);
    rdui::Binder firstBinder(*control, &firstResolver);
    TestBindingResult firstResult = finishBinding(firstBinder);
    ensure("first live binding commits", firstResult.ok());

    auto secondProvider = std::make_shared<TestValueBinding<bool>>(true);
    TestSettingResolver secondResolver;
    secondResolver.add("replaceable-value", secondProvider);
    rdui::Binder secondBinder(*control, &secondResolver);
    TestBindingResult secondResult = finishBinding(secondBinder);
    ensure("replacement binding commits on the same Value Control", secondResult.ok() && control->checked());

    firstResult.binding = std::move(secondResult.binding);
    control->activate();
    ensure("old Binding teardown preserves the replacement provider", !secondProvider->state().value && !control->checked());
    ensure("replaced provider is no longer written", !firstProvider->state().value);
}

template<> template<> void binderObject::test<17>() {
    rdui::ResourceSnapshot snapshot;
    const char* kLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {field.label: Enabled, field.fallback: 'Fallback EN <b>bold EN</b> <kbd shortcut=\"toggle-demo\"/>', field.dynamic: Dynamic EN}}, pt: {name: Portuguese, strings: {field.label: Ativado, field.fallback: 'Fallback PT <b>bold PT</b> <kbd shortcut=\"toggle-demo\"/>', field.dynamic: Dynamic PT}}}\n";
    const char* kFieldLayout =
        "<field><label for=\"demoSwitch\">field.label</label><switch id=\"demoSwitch\" setting=\"demo-enabled\"/><br/><error>field.fallback</error></field>";
    snapshot.add("localization.yaml", kLocalization);
    snapshot.add("skin.radia", "");
    snapshot.add("field.xml", kFieldLayout);

    const rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(snapshot));
    ensure("localized Field fixture prepares", prepared.ok());

    rdui::KeybindingPresentation presentation{{"F"}};
    rdui::System system;
    system.setKeybindingResolver(
        [&presentation](const std::string& binding) { return binding == "toggle-demo" ? presentation : rdui::KeybindingPresentation{}; });
    system.publish(prepared.generation);
    rdui::LayoutBuildResult buildResult = system.buildWidgetTree("field.xml");
    auto* field = buildResult.rootAs<rdui::Field>();
    ensure("localized Field View builds", buildResult.ok() && field && field->error());

    const rdui::TextSource staleDynamic = system.localize("field.dynamic");
    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);
    rdui::Binder binder(*field, &resolver);
    TestBindingResult binding = finishBinding(binder);
    ensure("localized Field binding commits", binding.ok());

    std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
    surface->mount(std::move(buildResult.root));
    provider->publish({false, false, rdui::ValueValidation::invalid()});
    ensure_equals("authored Field error resolves after mounting", field->error()->text(), "Fallback EN bold EN F");
    const auto& englishNodes = field->error()->content().nodes();
    ensure_equals("English rich error preserves its four inline nodes", englishNodes.size(), std::size_t(4));
    ensure_equals("English rich error preserves bold structure", static_cast<int>(englishNodes[1].kind()),
                  static_cast<int>(rdui::InlineContentKind::B));
    ensure_equals("English rich error preserves bold text", englishNodes[1].children().front().value(), std::string("bold EN"));
    ensure_equals("English rich error resolves keybinding presentation", englishNodes[3].keybindingPresentation().keys.front(), std::string("F"));

    ensure("Portuguese locale selected", system.setLocale("pt"));
    ensure_equals("visible authored Field error refreshes with locale", field->error()->text(), "Fallback PT bold PT F");
    const auto& portugueseNodes = field->error()->content().nodes();
    ensure_equals("Portuguese rich error preserves its four inline nodes", portugueseNodes.size(), std::size_t(4));
    ensure_equals("Portuguese rich error preserves bold structure", static_cast<int>(portugueseNodes[1].kind()),
                  static_cast<int>(rdui::InlineContentKind::B));
    ensure_equals("Portuguese rich error refreshes bold text", portugueseNodes[1].children().front().value(), std::string("bold PT"));
    ensure_equals("Portuguese rich error retains keybinding presentation", portugueseNodes[3].keybindingPresentation().keys.front(),
                  std::string("F"));

    provider->publish({false, false, rdui::ValueValidation::invalid(staleDynamic)});
    ensure_equals("late dynamic Field error resolves against the current locale", field->error()->text(), "Dynamic PT");
    provider->publish({false, false, rdui::ValueValidation::valid()});
    provider->publish({false, false, rdui::ValueValidation::invalid()});
    ensure_equals("restored authored Field error retains the current locale", field->error()->text(), "Fallback PT bold PT F");

    presentation = {{"Ctrl", "F"}};
    system.refreshKeybindings();
    provider->publish({false, false, rdui::ValueValidation::valid()});
    provider->publish({false, false, rdui::ValueValidation::invalid()});
    ensure_equals("restored authored Field error retains current keybindings", field->error()->text(), "Fallback PT bold PT Ctrl F");
    ensure_equals("keybinding refresh preserves localized bold structure", static_cast<int>(field->error()->content().nodes()[1].kind()),
                  static_cast<int>(rdui::InlineContentKind::B));
}

template<> template<> void binderObject::test<18>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("inspect", {rdui::EventArgument(std::int64_t(4))}));
    root.addChild(std::move(button));

    int invocations = 0;
    rdui::Binder binder(root);
    bindEvent(binder, "inspect", [&] { ++invocations; });
    TestBindingResult result = finishBinding(binder);
    ensure("parsed arguments do not reject the transitional Binder", result.ok() && result.binding);
    ensure_equals("pending typed invocation reports one warning", result.warnings.size(), 1U);
    ensure_equals("invalid zero-argument Handler signature diagnostic is stable", result.warnings.front().code,
                  std::string("binding.event.arity_mismatch"));
    target->activate();
    ensure_equals("transitional Binder never discards arguments by invoking a zero-argument Handler", invocations, 0);
}

template<> template<> void binderObject::test<19>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("press"));
    root.addChild(std::move(button));

    int invocations = 0;
    rdui::Binder binder(root);
    bindEvent(binder, "press", [&] { ++invocations; });
    TestBindingResult result = finishBinding(binder);
    ensure("generic Event Handler registration commits", result.ok() && result.binding);
    ensure("generic Event Handler registration has no warning", result.warnings.empty());
    target->activate();
    ensure_equals("generic Event Handler registration dispatches", invocations, 1);
}

template<> template<> void binderObject::test<20>() {
    rdui::Panel root;
    auto select = std::make_unique<rdui::Button>();
    rdui::Button* selectTarget = select.get();
    select->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("select", {rdui::EventArgument(std::int64_t(4))}));
    root.addChild(std::move(select));
    auto open = std::make_unique<rdui::Button>();
    rdui::Button* openTarget = open.get();
    open->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("open", {rdui::EventArgument(std::string("settings"))}));
    root.addChild(std::move(open));
    auto enabled = std::make_unique<rdui::Button>();
    rdui::Button* enabledTarget = enabled.get();
    enabled->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("updateAdvanced", {rdui::EventArgument(true)}));
    root.addChild(std::move(enabled));
    auto inspect = std::make_unique<rdui::Button>();
    rdui::Button* inspectTarget = inspect.get();
    inspect->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("inspectEventSource", {rdui::EventArgument(rdui::CurrentEventArgument{})}));
    root.addChild(std::move(inspect));

    int selected = 0;
    std::string destination;
    bool advanced = false;
    rdui::Widget* source = nullptr;
    rdui::Binder binder(root);
    bindEvent(
        binder, "select", rdui::WidgetEventKind::Click,
        [&](const rdui::WidgetEvent&, const rdui::EventCall& call) { selected = static_cast<int>(std::get<std::int64_t>(call.arguments().front())); },
        singleEventArgument<std::int64_t>);
    bindEvent(
        binder, "open", rdui::WidgetEventKind::Click,
        [&](const rdui::WidgetEvent&, const rdui::EventCall& call) { destination = std::get<std::string>(call.arguments().front()); },
        singleEventArgument<std::string>);
    bindEvent(
        binder, "updateAdvanced", rdui::WidgetEventKind::Click,
        [&](const rdui::WidgetEvent&, const rdui::EventCall& call) { advanced = std::get<bool>(call.arguments().front()); },
        singleEventArgument<bool>);
    bindEvent(
        binder, "inspectEventSource", rdui::WidgetEventKind::Click,
        [&](const rdui::WidgetEvent& event, const rdui::EventCall&) { source = &event.source; }, currentEventArgument);
    TestBindingResult result = finishBinding(binder);
    ensure("Event Handlers with arguments commit", result.ok() && result.binding);
    ensure("Event Handlers with arguments have no warnings", result.warnings.empty());
    selectTarget->activate();
    openTarget->activate();
    enabledTarget->activate();
    inspectTarget->activate();
    ensure_equals("integer argument arrives", selected, 4);
    ensure_equals("string argument arrives", destination, std::string("settings"));
    ensure("boolean argument arrives", advanced);
    ensure("Event supplies the source Widget", source == inspectTarget);
}

template<> template<> void binderObject::test<21>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("bad_action"));
    root.addChild(std::move(button));

    rdui::Binder binder(root);
    bindEvent(binder, "bad_action", [] {});
    const TestBindingResult result = finishBinding(binder);
    ensure("invalid registered Handler settingName rejects preparation", !result.ok());
    ensure_equals("invalid registered Handler settingName diagnostic", result.errors.front().code, "binding.event.name_invalid");
}

template<> template<> void binderObject::test<22>() {
    rdui::Panel root;
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setEventCall(rdui::WidgetEventKind::Click, rdui::EventCall("observe"));
    root.addChild(std::move(button));

    rdui::Widget* source = nullptr;
    rdui::WidgetEventKind kind = rdui::WidgetEventKind::Change;
    rdui::Binder binder(root);
    bindEvent(
        binder, "observe", std::nullopt,
        [&](const rdui::WidgetEvent& event, const rdui::EventCall&) {
            source = &event.source;
            kind = event.kind;
        },
        noEventArguments);
    const TestBindingResult result = finishBinding(binder);
    ensure("common Widget Event Handler commits", result.ok() && result.binding);
    target->activate();
    ensure("common Widget Event receives source", source == target);
    ensure("common Widget Event receives kind", kind == rdui::WidgetEventKind::Click);
}

template<> template<> void binderObject::test<23>() {
    rdui::LayoutBuildResult buildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"demo-enabled\"/>", "setting.xml");
    auto* control = buildResult.rootAs<rdui::Switch>();
    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("demo-enabled", provider);

    rdui::Binder binder(*control, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ensure("setting binding commits", buildResult.ok() && control && result.ok() && result.binding);
    ensure("setting applies initial state", !control->checked());
    provider->publish({true, false, rdui::ValueValidation::valid()});
    ensure("setting receives external updates", control->checked());
    control->activate();
    ensure("setting writes user changes", !provider->state().value);
}

template<> template<> void binderObject::test<24>() {
    rdui::LayoutBuildResult buildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"missing-setting\"/>", "missing-setting.xml");
    TestSettingResolver resolver;
    rdui::Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ensure("missing setting rejects the candidate", buildResult.ok() && !result.ok());
    ensure_equals("missing setting is an error", result.errors.size(), 1U);
    ensure_equals("missing setting diagnostic", result.errors.front().code, std::string("binding.setting.missing"));
}

template<> template<> void binderObject::test<25>() {
    rdui::LayoutBuildResult buildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"string-setting\"/>", "typed-setting.xml");
    TestSettingResolver resolver;
    resolver.add("string-setting", std::make_shared<TestValueBinding<std::string>>("not a boolean"));
    rdui::Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ensure("mismatched setting rejects the candidate", buildResult.ok() && !result.ok());
    ensure_equals("mismatched setting is an error", result.errors.size(), 1U);
    ensure_equals("mismatched setting diagnostic", result.errors.front().code, std::string("binding.setting.type_mismatch"));
}

template<> template<> void binderObject::test<26>() {
    const char* kSharedSettingLayout =
        "<panel><switch id=\"first\" setting=\"shared-enabled\"/><switch id=\"second\" setting=\"shared-enabled\"/></panel>";
    rdui::LayoutBuildResult buildResult = rdui::LayoutResourceCompiler().buildWidgetTreeFromString(kSharedSettingLayout, "shared-setting.xml");
    auto* root = buildResult.rootAs<rdui::Panel>();
    auto* firstControl = root ? dynamic_cast<rdui::Switch*>(rdui::detail::findWidgetInScope(*root, "first")) : nullptr;
    auto* secondControl = root ? dynamic_cast<rdui::Switch*>(rdui::detail::findWidgetInScope(*root, "second")) : nullptr;
    ensure("shared setting fixture compiles", buildResult.ok() && root && firstControl && secondControl);

    auto provider = std::make_shared<TestValueBinding<bool>>(false);
    TestSettingResolver resolver;
    resolver.add("shared-enabled", provider);

    rdui::Binder binder(*root, &resolver);
    TestBindingResult result = finishBinding(binder);
    ensure("two controls share one setting", result.ok() && result.binding);
    ensure_equals("shared binding subscribes both controls", provider->observerCount(), std::size_t(2));
    provider->publish({true, false, rdui::ValueValidation::valid()});
    ensure("first shared control updates", firstControl->checked());
    ensure("second shared control updates", secondControl->checked());
    firstControl->activate();
    ensure("shared control writes through one provider", !provider->state().value);
    result.binding = rdui::Binding{};
    ensure_equals("shared control teardown disconnects both", provider->observerCount(), std::size_t(0));
}

template<> template<> void binderObject::test<27>() {
    rdui::LayoutBuildResult buildResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<switch setting=\"misreported-setting\"/>", "misreported-setting.xml");
    MisreportingSettingResolver resolver;
    rdui::Binder binder(*buildResult.root, &resolver);
    const TestBindingResult result = finishBinding(binder);
    ensure("misreported binding is rejected safely", buildResult.ok() && !result.ok());
    ensure_equals("misreported binding diagnostic", result.errors.size(), 1U);
    ensure_equals("misreported binding is a type error", result.errors.front().code, std::string("binding.setting.type_mismatch"));
}
} // namespace tut
