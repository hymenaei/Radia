#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rduibinder.h"
#include "rduiviewcontract.h"

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
        binder.require("save", save);
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
        binder.require("save", save);
        binder.onClick("save", [&] { ++activations; });
        binder.require("status", wrong_type);
        binder.require("missing", missing);
        const rdui::BindingResult result = binder.finish();
        ensure("invalid transaction fails", !result.ok());
        ensure_equals("all validation errors reported", result.errors.size(), 2U);
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
        binder.require("temporary", reference);
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
        parent.require("left", left_scope);
        parent.require("right", right_scope);
        ensure("parent binds resource instances", parent.finish().ok() && left_scope && right_scope);

        rdui::WidgetRef<rdui::Label> left_bound;
        rdui::WidgetRef<rdui::Label> right_bound;
        rdui::Binder left_binder(*left_scope);
        left_binder.require("item", left_bound);
        rdui::Binder right_binder(*right_scope);
        right_binder.require("item", right_bound);
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
            profile.require("save", save_ref);
            profile.onClick("save", [&] { ++activations; });
            profile.require("missing", missing);
        });

        rdui::BindingResult failed = binder.finish();
        ensure("nested failure rejects whole transaction", !failed.ok());
        ensure("nested refs do not partially commit", !save_ref && !missing);
        source->activate();
        ensure_equals("nested handlers do not partially attach", activations, 0);
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
            profile.require("save", save_ref);
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
        live_binder.require("reload", reference);
        live_binder.onClick("reload", [] {});
        rdui::BindingResult live_binding = live_binder.finish();
        ensure("initial binding commits", live_binding.ok() && reference.get() == live_button_ptr);

        rdui::Panel candidate;
        auto candidate_button = std::make_unique<rdui::Button>();
        rdui::Button* candidate_button_ptr = candidate_button.get();
        candidate_button->setId("reload").setAction(rdui::ActionEventKind::Click, "reload");
        candidate.addChild(std::move(candidate_button));

        rdui::Binder candidate_binder(candidate);
        candidate_binder.require("reload", reference);
        candidate_binder.onClick("reload", [] {});
        rdui::PreparedBindingResult prepared = candidate_binder.prepare();
        ensure("replacement binding prepares", prepared.ok());
        ensure("preparation leaves live reference untouched", reference.get() == live_button_ptr);

        rdui::Binding replacement = prepared.binding.commit();
        ensure("commit switches reference to candidate", reference.get() == candidate_button_ptr);
        ensure("prepared commit returns attached handlers", !!replacement);
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
}
