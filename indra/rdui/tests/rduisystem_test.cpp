#include "linden_common.h"
#include "../test/lltut.h"
#include "rdlabel.h"
#include "rdtext.h"
#include "rduiskincompiler.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"
#include <utility>

namespace tut
{
    namespace
    {
        rdui::ResourceSnapshot skinSnapshot(
            std::string localization = "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"/></localizations>",
            std::string style = {})
        {
            if (localization.empty())
                localization = "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"/></localizations>";
            rdui::ResourceSnapshot snapshot;
            snapshot.add("localization.xml", std::move(localization));
            snapshot.add("skin.radia", std::move(style));
            return snapshot;
        }

        float resolvedLabelWidth(const rdui::System& system)
        {
            std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
            surface->setViewport(200.f, 100.f);
            auto label = std::make_unique<rdui::Label>();
            rdui::Label* label_ptr = label.get();
            surface->mount(std::move(label));
            surface->updateLayout();
            return label_ptr->rect().w;
        }
    }

    class LocaleProbe final : public rdui::Widget
    {
        public:
            LocaleProbe() : Widget("locale-probe") {}
            int notifications() const { return mNotifications; }

        private:
            void onLocaleChanged(const rdui::System&) override { ++mNotifications; }
            int mNotifications = 0;
    };

    struct rduisystem_data {};
    typedef test_group<rduisystem_data> rduisystem_test;
    typedef rduisystem_test::object rduisystem_object;
    rduisystem_test rduisystem_testcase("rduisystem");

    template<> template<>
    void rduisystem_object::test<1>()
    {
        rdui::ResourceSnapshot snapshot = skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">Ready</string></localization></localizations>",
            "label { width: 40px; }");
        snapshot.add("view.xml", "<text id=\"message\">message</text>");
        snapshot.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>");

        const rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("complete generation prepares", prepared.ok());

        rdui::System system;
        system.publish(prepared.generation);
        ensure_equals("publication advances generation", system.generation(), 1ULL);
        ensure_equals("localization published", system.resolveText("message"), "Ready");
        ensure_equals("stylesheet published", resolvedLabelWidth(system), 40.f);
        ensure("compiled icon published", system.hasIcon("search"));

        rdui::ViewBuildResult view = system.createView("view.xml");
        ensure("System creates localized View", view.ok());
        ensure_equals("View uses published localization", view.rootAs<rdui::Text>()->text(), "Ready");

        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        ensure("System creates Surface", surface != nullptr);
        surface->setViewport(100.f, 100.f);
        auto styled = std::make_unique<rdui::Label>();
        rdui::Label* styled_ptr = styled.get();
        surface->mount(std::move(styled));
        surface->updateLayout();
        ensure_equals("Surface shares published stylesheet", styled_ptr->rect().w, 40.f);
    }

    template<> template<>
    void rduisystem_object::test<2>()
    {
        rdui::System system;
        rdui::SkinGenerationPrepareResult live = rdui::SkinCompiler().prepare(skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">Live</string></localization></localizations>",
            "label { width: 40px; }"));
        ensure("live generation prepares", live.ok());
        system.publish(live.generation);

        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(skinSnapshot(
            "<localizations>", "label { flow: sideways; width: 90px; }"));
        ensure("invalid candidate rejected", !rejected.ok());
        ensure_equals("failed preparation preserves generation", system.generation(), 1ULL);
        ensure_equals("failed preparation preserves localization", system.resolveText("message"), "Live");
        ensure_equals("failed preparation preserves stylesheet", resolvedLabelWidth(system), 40.f);
    }

    template<> template<>
    void rduisystem_object::test<3>()
    {
        rdui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
        snapshot.add("resources/icons/search.svg", "");
        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("empty icon rejects complete generation", !rejected.ok());
        ensure("rejected generation is not exposed", rejected.generation == nullptr);
    }

    template<> template<>
    void rduisystem_object::test<4>()
    {
        rdui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
        snapshot.add("known.xml", "<icon source=\"actions/search\"/>");
        snapshot.add("missing.xml", "<icon source=\"actions/missing\"/>");
        snapshot.add("resources/icons/actions/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>");

        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("unknown icon in any Layout Resource rejects generation", !rejected.ok());
        ensure("invalid complete generation is not exposed", rejected.generation == nullptr);
    }

    template<> template<>
    void rduisystem_object::test<5>()
    {
        rdui::ResourceSnapshot missing_localization;
        missing_localization.add("skin.radia", "label { width: 40px; }");
        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(std::move(missing_localization));
        ensure("missing required resource rejects candidate", !rejected.ok());
        ensure_equals("diagnostic identifies missing resource", rejected.errors.front().source, "localization.xml");
    }

    template<> template<>
    void rduisystem_object::test<6>()
    {
        rdui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
        snapshot.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10\"/></svg>");
        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("malformed SVG rejects complete generation", !rejected.ok());
        ensure_equals("icon diagnostic identifies resource", rejected.errors.front().source, "resources/icons/search.svg");
    }

    template<> template<>
    void rduisystem_object::test<7>()
    {
        const std::string multilingual =
            "<localizations default=\"en\">"
            "<localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">Ready</string></localization>"
            "<localization id=\"pt\" lang=\"Português\" direction=\"ltr\"><string id=\"message\">Pronto</string></localization>"
            "<localization id=\"ar\" lang=\"العربية\" direction=\"rtl\"><string id=\"message\">جاهز</string></localization>"
            "</localizations>";
        rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(skinSnapshot(multilingual));
        ensure("multilingual generation prepares", prepared.ok());

        rdui::System system;
        system.publish(prepared.generation);
        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        auto localized = std::make_unique<rdui::Label>();
        rdui::Label* localized_ptr = localized.get();
        localized->setText(system.localized("message"));
        surface->root().addChild(std::move(localized));
        auto probe = std::make_unique<LocaleProbe>();
        LocaleProbe* probe_ptr = probe.get();
        surface->root().addChild(std::move(probe));

        ensure("Portuguese locale selected", system.setLocale("pt"));
        ensure_equals("localized Widget updates reactively", localized_ptr->text(), "Pronto");
        ensure_equals("Surface delivers locale change", probe_ptr->notifications(), 2);

        rdui::SkinGenerationPrepareResult compatible = rdui::SkinCompiler().prepare(skinSnapshot(multilingual));
        system.publish(compatible.generation);
        ensure_equals("selected locale survives publication", system.activeLocale(), "pt");

        rdui::SkinGenerationPrepareResult fallback = rdui::SkinCompiler().prepare(skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">Ready again</string></localization></localizations>"));
        system.publish(fallback.generation);
        ensure_equals("removed locale falls back to default", system.activeLocale(), "en");
        ensure_equals("fallback refreshes localized Widget", localized_ptr->text(), "Ready again");
    }

    template<> template<>
    void rduisystem_object::test<8>()
    {
        rdui::System system;
        rdui::SkinGenerationPrepareResult live = rdui::SkinCompiler().prepare(skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">Old</string></localization></localizations>",
            "label { width: 40px; }"));
        system.publish(live.generation);
        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        surface->setViewport(200.f, 100.f);
        auto styled = std::make_unique<rdui::Label>();
        rdui::Label* styled_ptr = styled.get();
        surface->mount(std::move(styled));
        surface->updateLayout();
        ensure_equals("existing Surface starts with live stylesheet", styled_ptr->rect().w, 40.f);

        rdui::ResourceSnapshot snapshot = skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"message\">New</string></localization></localizations>",
            "label { width: 90px; }");
        snapshot.add("view.xml", "<text>message</text>");
        rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("candidate generation prepares", prepared.ok());
        ensure_equals("preparation does not advance live generation", system.generation(), 1ULL);
        ensure_equals("preparation preserves live localization", system.resolveText("message"), "Old");
        rdui::ViewBuildResult candidate_view = prepared.generation->createView("view.xml", system.activeLocale());
        ensure("candidate View builds against candidate generation", candidate_view.ok());
        ensure_equals("candidate View uses candidate localization", candidate_view.rootAs<rdui::Text>()->text(), "New");

        bool document_commit = false;
        system.publish(prepared.generation, [&document_commit] { document_commit = true; });
        ensure("document commit participates in publication", document_commit);
        ensure_equals("publication advances generation once", system.generation(), 2ULL);
        ensure_equals("publication commits candidate localization", system.resolveText("message"), "New");
        surface->updateLayout();
        ensure_equals("existing Surface observes published stylesheet", styled_ptr->rect().w, 90.f);
        ensure("live View creation uses published snapshot", system.createView("view.xml").ok());
    }

    template<> template<>
    void rduisystem_object::test<9>()
    {
        rdui::System system;
        rdui::SkinGenerationPrepareResult live = rdui::SkinCompiler().prepare(skinSnapshot({}, "label { width: 40px; }"));
        system.publish(live.generation);

        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(skinSnapshot(
            "<localizations>", "label { width: 90px; }"));
        ensure("invalid prepared generation rejects", !rejected.ok());
        ensure_equals("rejected preparation preserves generation", system.generation(), 1ULL);
        ensure_equals("rejected preparation preserves stylesheet", resolvedLabelWidth(system), 40.f);
    }

    template<> template<>
    void rduisystem_object::test<10>()
    {
        rdui::ResourceSnapshot invalid = skinSnapshot({}, "label { width: 90px; }");
        invalid.add("valid.xml", "<text>Ready</text>");
        invalid.add("unused.xml", "<unsupported/>");

        const rdui::SkinGenerationPrepareResult rejected = rdui::SkinCompiler().prepare(std::move(invalid));
        ensure("invalid unmounted Layout Resource rejects complete generation", !rejected.ok());
        ensure("rejected complete generation is not exposed", rejected.generation == nullptr);
    }

    template<> template<>
    void rduisystem_object::test<11>()
    {
        rdui::ResourceSnapshot snapshot = skinSnapshot(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\">"
            "<string id=\"fly.label\">Fly</string></localization></localizations>");
        snapshot.add("view.xml", "<text>fly.label <kbd binding=\"toggle-fly\"/></text>");
        rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(snapshot));
        ensure("Kbd presentation fixture prepares", prepared.ok());

        rdui::KeybindingPresentation presentation{{"F"}};
        rdui::System system;
        system.setKeybindingResolver([&presentation](const std::string& binding)
        {
            return binding == "toggle-fly" ? presentation : rdui::KeybindingPresentation{};
        });
        system.publish(prepared.generation);
        rdui::ViewBuildResult view = system.createView("view.xml");
        auto* text = view.rootAs<rdui::Text>();
        ensure("Kbd View builds", view.ok() && text);

        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        surface->setViewport(200.f, 100.f);
        surface->mount(std::move(view.root));
        surface->updateLayout();
        ensure_equals("Kbd resolves through the System presentation seam", text->text(), "Fly F");
        const float initial_width = text->desiredSize().x;

        presentation = {{"Ctrl", "F"}};
        system.refreshKeybindings();
        surface->updateLayout();
        ensure_equals("Kbd refreshes after a user keybinding change", text->text(), "Fly Ctrl F");
        ensure("changed Kbd presentation invalidates intrinsic measurement",
               text->desiredSize().x > initial_width);
    }
}
