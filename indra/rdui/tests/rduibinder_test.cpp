#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdfield.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rduibinder.h"
#include "rduilayoutresourcecompiler.h"
#include "rduiskincompiler.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"
#include "rduiviewcontract.h"
#include "rduivaluebinding.h"
#include <map>

namespace
{
    template<typename T>
    class TestValueBinding final : public rdui::ValueBinding<T>
    {
        public:
            explicit TestValueBinding(T value) : mState{value, value, std::nullopt} {}

            rdui::ValueState<T> state() const override { return mState; }
            void write(T value) override
            {
                mState.value = std::move(value);
                notify();
            }
            rdui::ValueBindingSubscription observe(
                typename rdui::ValueBinding<T>::Observer observer) override
            {
                const std::size_t id = mNextObserver++;
                mObservers.emplace(id, std::move(observer));
                return rdui::ValueBindingSubscription([this, id] { mObservers.erase(id); });
            }

            void publish(rdui::ValueState<T> state)
            {
                mState = std::move(state);
                notify();
            }

            std::size_t observerCount() const { return mObservers.size(); }

        private:
            void notify()
            {
                const auto observers = mObservers;
                for (const auto& [id, observer] : observers)
                    if (mObservers.find(id) != mObservers.end()) observer(mState);
            }

            rdui::ValueState<T> mState;
            std::map<std::size_t, typename rdui::ValueBinding<T>::Observer> mObservers;
            std::size_t mNextObserver = 1;
    };
}

namespace tut
{
    struct rduibinder_data {};
    typedef test_group<rduibinder_data> rduibinder_test;
    typedef rduibinder_test::object rduibinder_object;
    rduibinder_test rduibinder_testcase("rduibinder");

    template<> template<>
    void rduibinder_object::test<1>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        button->setId("save").setAction(rdui::ActionEventKind::Click, "save");
        root.addChild(std::move(button));

        int activations = 0;
        rdui::WidgetRef<rdui::Button> save;
        rdui::Binder binder(root);
        binder.bind("save", save);
        binder.onClick("save", [&] { ++activations; });
        rdui::BindingResult result = binder.finish();
        ensure("valid binding transaction commits", result.ok());
        ensure("typed ref committed", !!save);
        save->activate();
        ensure_equals("handler attached", activations, 1);
    }

    template<> template<>
    void rduibinder_object::test<2>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        button->setId("save").setAction(rdui::ActionEventKind::Click, "save");
        rdui::Button* source = button.get();
        root.addChild(std::move(button));
        auto label = std::make_unique<rdui::Label>();
        label->setId("status");
        root.addChild(std::move(label));

        int activations = 0;
        rdui::WidgetRef<rdui::Button> save;
        rdui::WidgetRef<rdui::Button> wrong_type;
        rdui::WidgetRef<rdui::Label> missing;
        rdui::Binder binder(root);
        binder.bind("save", save);
        binder.onClick("save", [&] { ++activations; });
        binder.bind("status", wrong_type);
        binder.bind("missing", missing);
        const rdui::BindingResult result = binder.finish();
        ensure("invalid transaction fails", !result.ok());
        ensure_equals("only the existing wrong-typed widget is an error", result.errors.size(), 1U);
        ensure("no refs commit", !save && !wrong_type && !missing);
        source->activate();
        ensure_equals("no handler attaches", activations, 0);
    }

    template<> template<>
    void rduibinder_object::test<3>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        button->setId("temporary");
        root.addChild(std::move(button));
        rdui::WidgetRef<rdui::Button> reference;
        rdui::Binder binder(root);
        binder.bind("temporary", reference);
        ensure("reference binds", binder.finish().ok() && reference);
        root.clearChildren();
        ensure("destroyed widget invalidates reference", !reference);
    }

    template<> template<>
    void rduibinder_object::test<4>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        rdui::Button* source = button.get();
        button->setAction(rdui::ActionEventKind::Click, "optional");
        root.addChild(std::move(button));

        source->activate();
        int activations = 0;
        rdui::Binder binder(root);
        binder.onClick("optional", [&] { ++activations; });
        rdui::BindingResult result = binder.finish();
        ensure("declared action binds", result.ok() && result.binding);
        source->activate();
        ensure_equals("bound action runs", activations, 1);
        result.binding.reset();
        source->activate();
        ensure_equals("destroyed binding detaches handler", activations, 1);
    }

    template<> template<>
    void rduibinder_object::test<5>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        button->setAction(rdui::ActionEventKind::Click, "shared");
        root.addChild(std::move(button));
        auto control = std::make_unique<rdui::Switch>();
        control->setAction(rdui::ActionEventKind::Change, "shared");
        root.addChild(std::move(control));

        rdui::Binder binder(root);
        const rdui::BindingResult result = binder.finish();
        ensure("one action name cannot span event kinds", !result.ok());
        ensure_equals("kind conflict diagnostic", result.errors.front().code, "binding.action.kind_mismatch");
    }

    template<> template<>
    void rduibinder_object::test<6>()
    {
        rdui::Panel root;
        auto control = std::make_unique<rdui::Switch>();
        rdui::Switch* source = control.get();
        control->setAction(rdui::ActionEventKind::Change, "changed");
        root.addChild(std::move(control));

        int changes = 0;
        rdui::Binder binder(root);
        binder.onChange("changed", [&](const rdui::ChangeActionEvent& event)
        {
            ensure("change context contains completed value", event.checked);
            ++changes;
        });
        rdui::BindingResult result = binder.finish();
        ensure("change binding commits", result.ok());
        source->setChecked(false);
        ensure_equals("programmatic setter stays silent", changes, 0);
        source->activate();
        ensure_equals("user activation emits change", changes, 1);
    }

    template<> template<>
    void rduibinder_object::test<7>()
    {
        rdui::Panel root;
        auto left = std::make_unique<rdui::Panel>();
        left->setId("left");
        rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*left);
        auto left_item = std::make_unique<rdui::Label>();
        left_item->setId("item");
        left->addChild(std::move(left_item));
        root.addChild(std::move(left));

        auto right = std::make_unique<rdui::Panel>();
        right->setId("right");
        rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*right);
        auto right_item = std::make_unique<rdui::Label>();
        right_item->setId("item");
        right->addChild(std::move(right_item));
        root.addChild(std::move(right));

        rdui::WidgetRef<rdui::Panel> left_scope;
        rdui::WidgetRef<rdui::Panel> right_scope;
        rdui::Binder parent(root);
        parent.bind("left", left_scope);
        parent.bind("right", right_scope);
        ensure("parent binds resource instances", parent.finish().ok() && left_scope && right_scope);

        rdui::WidgetRef<rdui::Label> left_bound;
        rdui::WidgetRef<rdui::Label> right_bound;
        rdui::Binder left_binder(*left_scope);
        left_binder.bind("item", left_bound);
        rdui::Binder right_binder(*right_scope);
        right_binder.bind("item", right_bound);
        ensure("left local id binds", left_binder.finish().ok() && left_bound);
        ensure("right duplicate local id binds independently", right_binder.finish().ok() && right_bound);
        ensure("resource instances remain distinct", left_bound.get() != right_bound.get());
    }

    template<> template<>
    void rduibinder_object::test<8>()
    {
        rdui::Panel root;
        auto included = std::make_unique<rdui::Panel>();
        included->setId("profile");
        rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*included);
        auto save = std::make_unique<rdui::Button>();
        rdui::Button* source = save.get();
        save->setId("save").setAction(rdui::ActionEventKind::Click, "save");
        included->addChild(std::move(save));
        root.addChild(std::move(included));

        int activations = 0;
        rdui::WidgetRef<rdui::Button> save_ref;
        rdui::WidgetRef<rdui::Label> missing;
        rdui::Binder binder(root);
        binder.scope("profile", [&](rdui::Binder& profile)
        {
            profile.bind("save", save_ref);
            profile.onClick("save", [&] { ++activations; });
            profile.bind("missing", missing);
        });

        rdui::BindingResult result = binder.finish();
        ensure("missing nested widget is an optional binding", result.ok() && result.binding);
        ensure("present nested ref commits while missing ref remains empty", save_ref && !missing);
        source->activate();
        ensure_equals("nested handler still attaches", activations, 1);
    }

    template<> template<>
    void rduibinder_object::test<9>()
    {
        rdui::Panel root;
        auto included = std::make_unique<rdui::Panel>();
        included->setId("profile");
        rdui::detail::WidgetCompilerAccess::setIdScopeRoot(*included);
        auto save = std::make_unique<rdui::Button>();
        rdui::Button* source = save.get();
        save->setId("save").setAction(rdui::ActionEventKind::Click, "save");
        included->addChild(std::move(save));
        root.addChild(std::move(included));

        int activations = 0;
        rdui::WidgetRef<rdui::Button> save_ref;
        rdui::Binder binder(root);
        binder.scope("profile", [&](rdui::Binder& profile)
        {
            profile.bind("save", save_ref);
            profile.onClick("save", [&] { ++activations; });
        });

        rdui::BindingResult result = binder.finish();
        ensure("nested transaction commits", result.ok() && result.binding && save_ref);
        source->activate();
        ensure_equals("nested action binds inside local scope", activations, 1);
    }

    template<> template<>
    void rduibinder_object::test<10>()
    {
        rdui::Panel live;
        auto live_button = std::make_unique<rdui::Button>();
        rdui::Button* live_button_ptr = live_button.get();
        live_button->setId("reload").setAction(rdui::ActionEventKind::Click, "reload");
        live.addChild(std::move(live_button));

        rdui::WidgetRef<rdui::Button> reference;
        rdui::Binder live_binder(live);
        live_binder.bind("reload", reference);
        live_binder.onClick("reload", [] {});
        rdui::BindingResult live_binding = live_binder.finish();
        ensure("initial binding commits", live_binding.ok() && reference.get() == live_button_ptr);

        rdui::Panel candidate;
        auto candidate_button = std::make_unique<rdui::Button>();
        rdui::Button* candidate_button_ptr = candidate_button.get();
        candidate_button->setId("reload").setAction(rdui::ActionEventKind::Click, "reload");
        candidate.addChild(std::move(candidate_button));

        rdui::Binder candidate_binder(candidate);
        candidate_binder.bind("reload", reference);
        candidate_binder.onClick("reload", [] {});
        rdui::PreparedBindingResult prepared = candidate_binder.prepare();
        ensure("replacement binding prepares", prepared.ok());
        ensure("preparation leaves live reference untouched", reference.get() == live_button_ptr);

        rdui::Binding replacement = prepared.binding.commit();
        ensure("commit switches reference to candidate", reference.get() == candidate_button_ptr);
        ensure("prepared commit returns attached handlers", !!replacement);

        rdui::Panel removed_candidate;
        rdui::Binder removed_binder(removed_candidate);
        removed_binder.bind("reload", reference);
        rdui::PreparedBindingResult removed = removed_binder.prepare();
        ensure("removing a bound widget prepares successfully", removed.ok());
        ensure("removal preparation leaves the live reference untouched",
               reference.get() == candidate_button_ptr);
        rdui::Binding removed_binding = removed.binding.commit();
        ensure("committing widget removal clears its optional reference", !reference);
        ensure("widget removal still returns a valid binding", !!removed_binding);
    }

    template<> template<>
    void rduibinder_object::test<11>()
    {
        rdui::Panel root;
        rdui::Binder binder(root);
        binder.onClick("missing", [] {});
        rdui::PreparedBindingResult result = binder.prepare();
        ensure("controller action may be absent from this layout", result.ok());
        ensure("optional action still produces a valid binding", !!result.binding.commit());
    }

    template<> template<>
    void rduibinder_object::test<12>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        button->setAction(rdui::ActionEventKind::Click, "unhandled");
        root.addChild(std::move(button));

        rdui::Binder binder(root);
        const rdui::BindingResult result = binder.finish();
        ensure("unhandled layout action does not reject binding", result.ok());
        ensure_equals("unhandled action reports one warning", result.warnings.size(), 1U);
        ensure_equals("unhandled action diagnostic", result.warnings.front().code, "binding.action.unhandled");
    }

    template<> template<>
    void rduibinder_object::test<13>()
    {
        rdui::Panel live_root;
        auto live = std::make_shared<TestValueBinding<bool>>(false);
        rdui::ValueBindingRef<bool> reference;
        rdui::Binder live_binder(live_root);
        live_binder.provideValue("demo-enabled", live);
        live_binder.requireValue("demo-enabled", reference);
        rdui::BindingResult live_result = live_binder.finish();
        ensure("typed value binding commits", live_result.ok() && live_result.binding);
        ensure("committed reference points at the live provider", reference.get() == live.get());

        rdui::Panel candidate_root;
        auto candidate = std::make_shared<TestValueBinding<bool>>(true);
        rdui::Binder candidate_binder(candidate_root);
        candidate_binder.provideValue("demo-enabled", candidate);
        candidate_binder.requireValue("demo-enabled", reference);
        rdui::PreparedBindingResult prepared = candidate_binder.prepare();
        ensure("replacement value binding prepares", prepared.ok());
        ensure("preparation does not replace the live provider", reference.get() == live.get());

        {
            rdui::Panel abandoned_root;
            auto abandoned = std::make_shared<TestValueBinding<bool>>(true);
            rdui::Binder abandoned_binder(abandoned_root);
            abandoned_binder.provideValue("demo-enabled", abandoned);
            abandoned_binder.requireValue("demo-enabled", reference);
            rdui::PreparedBindingResult abandoned_result = abandoned_binder.prepare();
            ensure("a second replacement value binding prepares", abandoned_result.ok());
        }
        ensure("discarding a prepared transaction preserves the live provider",
               reference.get() == live.get());

        rdui::Binding replacement = prepared.binding.commit();
        ensure("commit switches to the candidate provider", replacement && reference.get() == candidate.get());
        ensure("candidate state is available through the typed reference", reference->state().value);
    }

    template<> template<>
    void rduibinder_object::test<14>()
    {
        rdui::Panel root;
        rdui::ValueBindingRef<bool> reference;
        rdui::Binder binder(root);
        binder.requireValue("missing-value", reference);
        const rdui::PreparedBindingResult result = binder.prepare();
        ensure("missing value provider rejects the transaction", !result.ok() && !reference);
        ensure_equals("missing provider diagnostic", result.errors.front().code,
                      std::string("binding.value.missing"));
    }

    template<> template<>
    void rduibinder_object::test<15>()
    {
        rdui::Panel root;
        auto provider = std::make_shared<TestValueBinding<std::string>>("enabled");
        rdui::ValueBindingRef<bool> reference;
        rdui::Binder binder(root);
        binder.provideValue("demo-enabled", provider);
        binder.requireValue("demo-enabled", reference);
        const rdui::PreparedBindingResult result = binder.prepare();
        ensure("value type mismatch rejects the transaction", !result.ok() && !reference);
        ensure_equals("value type mismatch diagnostic", result.errors.front().code,
                      std::string("binding.value.type_mismatch"));
    }

    template<> template<>
    void rduibinder_object::test<16>()
    {
        rdui::Panel root;
        auto first = std::make_shared<TestValueBinding<bool>>(false);
        auto second = std::make_shared<TestValueBinding<bool>>(true);
        rdui::Binder binder(root);
        binder.provideValue("demo-enabled", first);
        binder.provideValue("demo-enabled", second);
        const rdui::PreparedBindingResult result = binder.prepare();
        ensure("duplicate value providers reject the transaction", !result.ok());
        ensure_equals("duplicate provider diagnostic", result.errors.front().code,
                      std::string("binding.value.duplicate"));
    }

    template<> template<>
    void rduibinder_object::test<17>()
    {
        rdui::ViewBuildResult view = rdui::LayoutResourceCompiler().createFromString(
            "<field><label for=\"demo-switch\">Enabled</label>"
            "<switch id=\"demo-switch\" bind=\"demo-enabled\" onChange=\"demo-changed\"/><br/>"
            "<hint>Persistent hint</hint><br/><error>Fallback error</error></field>", "bound-field.xml");
        auto* field = view.rootAs<rdui::Field>();
        auto* control = field ? dynamic_cast<rdui::Switch*>(field->control()) : nullptr;
        ensure("bound standalone Field compiles", view.ok() && field && control);

        auto provider = std::make_shared<TestValueBinding<bool>>(false);
        int changes = 0;
        rdui::Binder binder(*field);
        binder.provideValue("demo-enabled", provider);
        binder.onChange("demo-changed", [&](const rdui::ChangeActionEvent& event)
        {
            ensure("bound Change action observes the provider value", !event.checked);
            ++changes;
        });
        rdui::BindingResult result = binder.finish();
        ensure("Switch bind resolves automatically through Binder", result.ok() && result.binding);
        ensure_equals("committed Switch owns one provider subscription", provider->observerCount(), 1U);

        provider->publish({true, false, rdui::ValueValidation::invalid(
            rdui::TextSource::text("Dynamic error"))});
        ensure("external provider state updates Switch", control->checked());
        ensure("Field reflects dirty Value State", field->dirty());
        ensure("Field reflects invalid Value State", field->invalid());
        ensure_equals("dynamic validation text overrides authored Error", field->error()->text(),
                      "Dynamic error");
        ensure_equals("invalid Field reveals Error", static_cast<int>(field->error()->visibility()),
                      static_cast<int>(rdui::Visibility::Visible));

        control->activate();
        ensure("Switch activation writes through the provider", !provider->state().value && !control->checked());
        ensure_equals("bound Switch emits Change after writing the provider", changes, 1);

        provider->publish({false, false, rdui::ValueValidation::valid()});
        ensure("valid provider state clears Field invalid state", !field->invalid());
        ensure_equals("valid Field collapses Error", static_cast<int>(field->error()->visibility()),
                      static_cast<int>(rdui::Visibility::Collapsed));

        result.binding.reset();
        ensure_equals("reset Binding disconnects automatic Value Control observation",
                      provider->observerCount(), 0U);
        provider->publish({true, false, rdui::ValueValidation::valid()});
        ensure("disconnected provider no longer updates Switch", !control->checked());

        rdui::ViewBuildResult missing_view = rdui::LayoutResourceCompiler().createFromString(
            "<switch bind=\"missing-value\"/>", "missing-value.xml");
        rdui::Binder missing_binder(*missing_view.root);
        rdui::BindingResult missing = missing_binder.finish();
        ensure("authored bind requires a matching controller provider", !missing.ok());
        ensure_equals("automatic missing provider diagnostic is stable", missing.errors.front().code,
                      std::string("binding.value.missing"));

        rdui::ViewBuildResult typed_view = rdui::LayoutResourceCompiler().createFromString(
            "<switch bind=\"typed-value\"/>", "typed-value.xml");
        auto wrong_type = std::make_shared<TestValueBinding<std::string>>("false");
        rdui::Binder typed_binder(*typed_view.root);
        typed_binder.provideValue("typed-value", wrong_type);
        rdui::BindingResult typed = typed_binder.finish();
        ensure("authored bind enforces the Value Control type", !typed.ok());
        ensure_equals("automatic type mismatch diagnostic is stable", typed.errors.front().code,
                      std::string("binding.value.type_mismatch"));

        rdui::ViewBuildResult lifetime_view = rdui::LayoutResourceCompiler().createFromString(
            "<switch bind=\"retained-value\"/>", "retained-value.xml");
        auto retained_provider = std::make_shared<TestValueBinding<bool>>(false);
        std::weak_ptr<TestValueBinding<bool>> provider_lifetime = retained_provider;
        rdui::Binder lifetime_binder(*lifetime_view.root);
        lifetime_binder.provideValue("retained-value", retained_provider);
        rdui::BindingResult lifetime_binding = lifetime_binder.finish();
        ensure("provider lifetime binding commits", lifetime_binding.ok());
        retained_provider.reset();
        lifetime_view.root.reset();
        ensure("Binding retains provider until its subscription disconnects", !provider_lifetime.expired());
        lifetime_binding.binding.reset();
        ensure("provider is released after Binding disconnects", provider_lifetime.expired());
    }

    template<> template<>
    void rduibinder_object::test<18>()
    {
        rdui::ViewBuildResult view = rdui::LayoutResourceCompiler().createFromString(
            "<switch bind=\"replaceable-value\"/>", "replaceable-value.xml");
        auto* control = view.rootAs<rdui::Switch>();
        auto first_provider = std::make_shared<TestValueBinding<bool>>(false);
        rdui::Binder first_binder(*control);
        first_binder.provideValue("replaceable-value", first_provider);
        rdui::BindingResult first_result = first_binder.finish();
        ensure("first live binding commits", first_result.ok());

        auto second_provider = std::make_shared<TestValueBinding<bool>>(true);
        rdui::Binder second_binder(*control);
        second_binder.provideValue("replaceable-value", second_provider);
        rdui::BindingResult second_result = second_binder.finish();
        ensure("replacement binding commits on the same Value Control", second_result.ok() && control->checked());

        first_result.binding = std::move(second_result.binding);
        control->activate();
        ensure("old Binding teardown preserves the replacement provider",
               !second_provider->state().value && !control->checked());
        ensure("replaced provider is no longer written", !first_provider->state().value);
    }

    template<> template<>
    void rduibinder_object::test<19>()
    {
        rdui::ResourceSnapshot snapshot;
        snapshot.add("localization.yaml", R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      field.label: Enabled
      field.fallback: 'Fallback EN <b>bold EN</b> <kbd binding="toggle-demo"/>'
      field.dynamic: Dynamic EN
  pt:
    name: Portuguese
    strings:
      field.label: Ativado
      field.fallback: 'Fallback PT <b>bold PT</b> <kbd binding="toggle-demo"/>'
      field.dynamic: Dynamic PT
)YAML");
        snapshot.add("skin.radia", "");
        snapshot.add("field.xml",
            "<field><label for=\"demo-switch\">field.label</label>"
            "<switch id=\"demo-switch\" bind=\"demo-enabled\"/><br/>"
            "<error>field.fallback</error></field>");

        const rdui::SkinGenerationPrepareResult prepared =
            rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("localized Field fixture prepares", prepared.ok());

        rdui::KeybindingPresentation presentation{{"F"}};
        rdui::System system;
        system.setKeybindingResolver([&presentation](const std::string& binding)
        {
            return binding == "toggle-demo" ? presentation : rdui::KeybindingPresentation{};
        });
        system.publish(prepared.generation);
        rdui::ViewBuildResult view = system.createView("field.xml");
        auto* field = view.rootAs<rdui::Field>();
        ensure("localized Field View builds", view.ok() && field && field->error());

        const rdui::TextSource stale_dynamic =
            system.localized("field.dynamic");
        auto provider = std::make_shared<TestValueBinding<bool>>(false);
        rdui::Binder binder(*field);
        binder.provideValue("demo-enabled", provider);
        rdui::BindingResult binding = binder.finish();
        ensure("localized Field binding commits", binding.ok());

        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        surface->mount(std::move(view.root));
        provider->publish({false, false, rdui::ValueValidation::invalid()});
        ensure_equals("authored Field error resolves after mounting", field->error()->text(),
                      "Fallback EN bold EN F");
        const auto& english_nodes = field->error()->content().nodes();
        ensure_equals("English rich error preserves its four inline nodes",
                      english_nodes.size(), std::size_t(4));
        ensure_equals("English rich error preserves bold structure",
                      static_cast<int>(english_nodes[1].kind()),
                      static_cast<int>(rdui::InlineContentKind::B));
        ensure_equals("English rich error preserves bold text",
                      english_nodes[1].children().front().value(),
                      std::string("bold EN"));
        ensure_equals("English rich error resolves keybinding presentation",
                      english_nodes[3].keybindingPresentation().keys.front(),
                      std::string("F"));

        ensure("Portuguese locale selected", system.setLocale("pt"));
        ensure_equals("visible authored Field error refreshes with locale", field->error()->text(),
                      "Fallback PT bold PT F");
        const auto& portuguese_nodes = field->error()->content().nodes();
        ensure_equals("Portuguese rich error preserves its four inline nodes",
                      portuguese_nodes.size(), std::size_t(4));
        ensure_equals("Portuguese rich error preserves bold structure",
                      static_cast<int>(portuguese_nodes[1].kind()),
                      static_cast<int>(rdui::InlineContentKind::B));
        ensure_equals("Portuguese rich error refreshes bold text",
                      portuguese_nodes[1].children().front().value(),
                      std::string("bold PT"));
        ensure_equals("Portuguese rich error retains keybinding presentation",
                      portuguese_nodes[3].keybindingPresentation().keys.front(),
                      std::string("F"));

        provider->publish({false, false, rdui::ValueValidation::invalid(stale_dynamic)});
        ensure_equals("late dynamic Field error resolves against the current locale",
                      field->error()->text(), "Dynamic PT");
        provider->publish({false, false, rdui::ValueValidation::valid()});
        provider->publish({false, false, rdui::ValueValidation::invalid()});
        ensure_equals("restored authored Field error retains the current locale",
                      field->error()->text(), "Fallback PT bold PT F");

        presentation = {{"Ctrl", "F"}};
        system.refreshKeybindings();
        provider->publish({false, false, rdui::ValueValidation::valid()});
        provider->publish({false, false, rdui::ValueValidation::invalid()});
        ensure_equals("restored authored Field error retains current keybindings",
                      field->error()->text(), "Fallback PT bold PT Ctrl F");
        ensure_equals("keybinding refresh preserves localized bold structure",
                      static_cast<int>(
                          field->error()->content().nodes()[1].kind()),
                      static_cast<int>(rdui::InlineContentKind::B));
    }
}
