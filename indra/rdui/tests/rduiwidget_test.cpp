#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rduirecordingpaintcontext.h"
#include "rduiskincompiler.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"

namespace tut
{
    struct rduiwidget_data {};
    typedef test_group<rduiwidget_data> rduiwidget_test;
    typedef rduiwidget_test::object rduiwidget_object;
    rduiwidget_test rduiwidget_testcase("rduiwidget");

    template<> template<>
    void rduiwidget_object::test<1>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        rdui::WidgetRef<rdui::Button> reference(button.get());
        root.addChild(std::move(button));
        ensure("mounted widget reference remains valid", static_cast<bool>(reference));
        root.clearChildren();
        ensure("destroyed widget expires its reference", !reference);
    }

    template<> template<>
    void rduiwidget_object::test<2>()
    {
        rdui::Button button;
        int activations = 0;
        button.setOnActivate([&](rdui::Widget&) { ++activations; });
        button.activate();
        button.setDisabled(true).activate();
        ensure_equals("disabled button does not activate", activations, 1);
        ensure("button is focusable", button.focusable());
    }

    template<> template<>
    void rduiwidget_object::test<3>()
    {
        rdui::Panel root;
        auto first = std::make_unique<rdui::Button>();
        first->setId("first");
        root.addChild(std::move(first));
        auto leading = std::make_unique<rdui::Button>();
        leading->setId("leading");
        root.prependChild(std::move(leading));
        ensure_equals("prepend changes order", root.children().front()->id(), "leading");
        ensure("parent assigned", root.children().front()->parent() == &root);
        root.clearChildren();
        ensure("clear removes children", root.children().empty());
    }

    template<> template<>
    void rduiwidget_object::test<4>()
    {
        rdui::RecordingPaintContext recording;
        rdui::Style style;

        rdui::Label label("hello");
        label.setRect({1.f, 2.f, 30.f, 10.f});
        label.paint(recording, style, 1.f);
        ensure_equals("label emits box primitive", recording.count(rdui::PaintCommandKind::Box), 1U);
        ensure_equals("label emits text primitive", recording.count(rdui::PaintCommandKind::Text), 1U);
        const rdui::PaintCommand* text = recording.last(rdui::PaintCommandKind::Text);
        ensure("text primitive recorded", text != nullptr);
        ensure_equals("text primitive carries content", text->value, std::string("hello"));
        ensure_equals("text primitive carries arranged width", text->rect.w, 30.f);

        rdui::Icon icon("search");
        icon.setRect({4.f, 5.f, 16.f, 16.f});
        icon.paint(recording, style, 2.f);
        ensure_equals("icon emits box primitive", recording.count(rdui::PaintCommandKind::Box), 2U);
        ensure_equals("icon emits icon primitive", recording.count(rdui::PaintCommandKind::Icon), 1U);
        const rdui::PaintCommand* icon_command = recording.last(rdui::PaintCommandKind::Icon);
        ensure("icon primitive recorded", icon_command != nullptr);
        ensure_equals("icon primitive carries resource name", icon_command->value, std::string("search"));
        ensure_equals("icon primitive carries device scale", icon_command->scale, 2.f);
    }

    template<> template<>
    void rduiwidget_object::test<5>()
    {
        rdui::System system;
        rdui::ResourceSnapshot resources;
        resources.add("localization.xml",
            "<localizations default=\"en\">"
            "<localization id=\"en\" lang=\"English\" direction=\"ltr\"/>"
            "<localization id=\"ar\" lang=\"العربية\" direction=\"rtl\"/>"
            "</localizations>");
        resources.add("skin.radia", "panel { opacity: .5; effect: background-blur(3px), layer-blur(to right, 0px 4px); }"
                                    "label { opacity: .5; text-align: start; } icon { size: 16px; }");
        resources.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M2 2 L22 22\"/></svg>");
        rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(resources));
        ensure("paint resources compile", prepared.ok());
        system.publish(prepared.generation);
        ensure("RTL paint locale selected", system.setLocale("ar"));

        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        surface->setViewport(100.f, 100.f);
        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({0.f, 0.f, 100.f, 100.f});
        auto label = std::make_unique<rdui::Label>("hello");
        label->setRect({0.f, 20.f, 30.f, 10.f});
        panel->addChild(std::move(label));
        auto icon = std::make_unique<rdui::Icon>("search");
        icon->setRect({0.f, 0.f, 16.f, 16.f});
        panel->addChild(std::move(icon));
        surface->root().addChild(std::move(panel));

        rdui::RecordingPaintContext recording;
        surface->paint(recording, 2.f);
        ensure_equals("Surface begins one paint frame", recording.count(rdui::PaintCommandKind::BeginFrame), 1U);
        ensure_equals("Surface ends one paint frame", recording.count(rdui::PaintCommandKind::EndFrame), 1U);
        ensure_equals("Surface begins one composed effect scope", recording.count(rdui::PaintCommandKind::BeginEffects), 1U);
        ensure_equals("Surface ends one composed effect scope", recording.count(rdui::PaintCommandKind::EndEffects), 1U);
        ensure_equals("Surface traverses text widget", recording.count(rdui::PaintCommandKind::Text), 1U);
        ensure_equals("Surface emits compiled icon identifier", recording.count(rdui::PaintCommandKind::Icon), 1U);
        const rdui::PaintCommand* text_command = recording.last(rdui::PaintCommandKind::Text);
        const rdui::PaintCommand* icon_command = recording.last(rdui::PaintCommandKind::Icon);
        const rdui::PaintCommand* effect_command = recording.last(rdui::PaintCommandKind::BeginEffects);
        ensure("Surface records text, icon, and effect primitives", text_command && icon_command && effect_command);
        ensure_equals("effect scope preserves device scale", effect_command->scale, 2.f);
        ensure_equals("effect scope preserves the composed list", effect_command->style.effects.size(), 2U);
        ensure_equals("Surface preserves icon resource name", icon_command->value, std::string("search"));
        ensure_equals("ancestor opacity multiplies child opacity", text_command->style.text_color.a, .25f);
        ensure_equals("Surface resolves RTL logical text alignment", static_cast<int>(text_command->style.text_align),
                      static_cast<int>(rdui::TextAlign::Right));
        ensure_equals("Surface primitive order begins with frame", static_cast<int>(recording.commands().front().kind),
                      static_cast<int>(rdui::PaintCommandKind::BeginFrame));
        ensure_equals("Surface primitive order ends with frame", static_cast<int>(recording.commands().back().kind),
                      static_cast<int>(rdui::PaintCommandKind::EndFrame));
        ensure("text is emitted before the later icon",
               text_command < icon_command);
        ensure("successful Surface paint clears invalidation", !surface->needsPaint());
    }

}
