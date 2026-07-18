#include "linden_common.h"
#include "../test/lltut.h"

#include "../rduiskinreloadcoordinator.h"
#include "../rduiskinresolver.h"

#include "rdfloater.h"
#include "rduisystem.h"

#include <chrono>
#include <map>
#include <memory>
#include <utility>

namespace tut
{
    namespace
    {
        rdui::ResourceSnapshot skinSnapshot(std::string view = "<floater/>",
                                            std::string style = {})
        {
            rdui::ResourceSnapshot snapshot;
            snapshot.add("localization.xml",
                         "<localizations default=\"en\">"
                         "<localization id=\"en\" lang=\"English\" direction=\"ltr\"/>"
                         "</localizations>");
            snapshot.add("skin.radia", std::move(style));
            snapshot.add("view.xml", std::move(view));
            return snapshot;
        }

        rdui::ResourceSnapshot importedStyleSnapshot(
            std::string entrypoint, std::map<std::string, std::string> modules)
        {
            rdui::ResourceSnapshot snapshot = skinSnapshot("<floater/>", entrypoint);
            snapshot.setLayers("skin.radia", {rdui::ResourceLayer{
                "test/skin.radia", std::move(entrypoint), "skin.radia", std::move(modules)}});
            return snapshot;
        }
    }

    struct skin_reload_coordinator
    {
        struct SnapshotSource final : rdui::viewer::SkinSnapshotSource
        {
            rdui::viewer::SkinSnapshotResult capture() const override
            {
                ++captures;
                rdui::viewer::SkinSnapshotResult result;
                result.snapshot = snapshot;
                if (rejectCapture)
                    result.error("skin.test.rejected", "Test manifest rejected.");
                return result;
            }

            rdui::ResourceSnapshot snapshot = skinSnapshot();
            bool rejectCapture = false;
            mutable int captures = 0;
        };

        struct Document final : rdui::viewer::ReloadableFloater
        {
            std::string reloadResourceId() const override { return "view.xml"; }

            rdui::PreparedBindingResult prepareBindings(rdui::Floater& floater) override
            {
                ++prepares;
                if (rejectBinding)
                {
                    rdui::PreparedBindingResult result;
                    result.error("binding.test.rejected", "Test binding rejected.");
                    return result;
                }
                rdui::Binder binder(floater);
                return binder.prepare();
            }

            void commitBindings(rdui::PreparedBinding&& prepared) override
            {
                binding = prepared.commit();
                ++commits;
            }

            bool rejectBinding = false;
            int prepares = 0;
            int commits = 0;
            rdui::Binding binding;
        };

        SnapshotSource snapshots;
        rdui::System system;
        Document document;
        rdui::viewer::SkinReloadCoordinator coordinator{system, snapshots};
        std::unique_ptr<rdui::Floater> installed;

        std::optional<rdui::viewer::SkinReloadResult> update(
            rdui::viewer::SkinReloadCoordinator::TimePoint now = {})
        {
            std::vector<rdui::viewer::FloaterReloadTarget> targets;
            targets.push_back({&document, [this](std::unique_ptr<rdui::Floater> floater)
                {
                    installed = std::move(floater);
                }});
            return coordinator.update(now, targets);
        }
    };

    using skin_reload_coordinator_group = test_group<skin_reload_coordinator>;
    using skin_reload_coordinator_object = skin_reload_coordinator_group::object;
    skin_reload_coordinator_group skin_reload_coordinator_tests("RduiSkinReloadCoordinator");

    template<> template<>
    void skin_reload_coordinator_object::test<1>()
    {
        set_test_name("requested reload publishes and installs one complete transaction");
        ensure("idle update has no transaction", !update().has_value());

        coordinator.request();
        const auto result = update();

        ensure("reload produced a result", result.has_value());
        ensure("reload committed", result->ok());
        ensure_equals("generation published once", system.generation(), 1ULL);
        ensure_equals("result reports generation", result->generation, 1ULL);
        ensure("replacement installed", installed != nullptr);
        ensure_equals("binding prepared once", document.prepares, 1);
        ensure_equals("binding committed once", document.commits, 1);
        ensure("request consumed", !update().has_value());
    }

    template<> template<>
    void skin_reload_coordinator_object::test<2>()
    {
        set_test_name("invalid candidate preserves the live generation and document");
        coordinator.request();
        ensure("baseline committed", update()->ok());
        rdui::Floater* live = installed.get();
        const int prepares = document.prepares;
        const int commits = document.commits;

        snapshots.snapshot = skinSnapshot();
        snapshots.snapshot.add("localization.xml", "<localizations>");
        coordinator.request();
        const auto rejected = update();

        ensure("rejection produced a result", rejected.has_value());
        ensure("invalid candidate rejected", !rejected->ok());
        ensure("diagnostics returned", !rejected->errors.empty());
        ensure_equals("live generation preserved", system.generation(), 1ULL);
        ensure("live Floater preserved", installed.get() == live);
        ensure_equals("binding preparation not reached", document.prepares, prepares);
        ensure_equals("binding commit not reached", document.commits, commits);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<3>()
    {
        set_test_name("binding rejection prevents publication and installation");
        document.rejectBinding = true;
        coordinator.request();
        const auto rejected = update();

        ensure("binding rejection returned", rejected.has_value());
        ensure("binding rejection failed", !rejected->ok());
        ensure_equals("candidate was not published", system.generation(), 0ULL);
        ensure("candidate was not installed", installed == nullptr);
        ensure_equals("binding prepared", document.prepares, 1);
        ensure_equals("binding not committed", document.commits, 0);
        ensure_equals("binding diagnostic retained", rejected->errors.front().code,
                      std::string("binding.test.rejected"));
    }

    template<> template<>
    void skin_reload_coordinator_object::test<4>()
    {
        set_test_name("authoring changes debounce into one coherent snapshot reload");
        using namespace std::chrono_literals;
        const auto start = rdui::viewer::SkinReloadCoordinator::TimePoint{} + 1s;
        coordinator.setAuthoringEnabled(true);
        ensure("first poll establishes baseline", !update(start).has_value());

        snapshots.snapshot = skinSnapshot("<floater/>", "floater { width: 420px; }");
        ensure("changed snapshot waits to settle", !update(start + 250ms).has_value());
        const auto settled = update(start + 500ms);

        ensure("settled snapshot committed", settled && settled->ok());
        ensure_equals("one generation published", system.generation(), 1ULL);
        ensure_equals("one binding committed", document.commits, 1);
        ensure("acknowledged snapshot does not repeat", !update(start + 750ms).has_value());
    }

    template<> template<>
    void skin_reload_coordinator_object::test<5>()
    {
        set_test_name("snapshot resolution diagnostics reject before generation preparation");
        snapshots.rejectCapture = true;
        coordinator.request();

        const auto rejected = update();

        ensure("resolution rejection returned", rejected.has_value());
        ensure("resolution rejection failed", !rejected->ok());
        ensure_equals("diagnostic retained", rejected->errors.front().code, std::string("skin.test.rejected"));
        ensure_equals("generation not published", system.generation(), 0ULL);
        ensure_equals("binding not prepared", document.prepares, 0);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<6>()
    {
        set_test_name("authoring reload invalidates only imported stylesheet modules");
        using namespace std::chrono_literals;
        const auto start = rdui::viewer::SkinReloadCoordinator::TimePoint{} + 1s;
        snapshots.snapshot = importedStyleSnapshot(
            "@import \"used.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"unused.radia", "floater { width: 500px; }"}});
        coordinator.request();
        ensure("dependency baseline publishes", update()->ok());
        coordinator.setAuthoringEnabled(true);
        ensure("first poll establishes authoring baseline", !update(start).has_value());

        snapshots.snapshot = importedStyleSnapshot(
            "@import \"used.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"unused.radia", "floater { width: 600px; }"}});
        ensure("unused module edit is ignored", !update(start + 250ms).has_value());
        ensure("unused module remains ignored", !update(start + 500ms).has_value());

        snapshots.snapshot = importedStyleSnapshot(
            "@import \"used.radia\";",
            {{"used.radia", "floater { width: 420px; }"},
             {"unused.radia", "floater { width: 600px; }"}});
        ensure("imported edit waits to settle", !update(start + 750ms).has_value());
        const auto settled = update(start + 1s);

        ensure("imported module edit reloads", settled && settled->ok());
        ensure_equals("only dependency edit publishes again", system.generation(), 2ULL);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<7>()
    {
        set_test_name("rejected candidate retries when its new dependency changes");
        using namespace std::chrono_literals;
        const auto start = rdui::viewer::SkinReloadCoordinator::TimePoint{} + 1s;
        snapshots.snapshot = importedStyleSnapshot(
            "@import \"used.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"new.radia", "floater { width: invalid; }"}});
        coordinator.request();
        ensure("live dependency baseline publishes", update()->ok());
        coordinator.setAuthoringEnabled(true);
        ensure("first poll establishes authoring baseline", !update(start).has_value());

        snapshots.snapshot = importedStyleSnapshot(
            "@import \"new.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"new.radia", "floater { width: invalid; }"}});
        ensure("new import waits to settle", !update(start + 250ms).has_value());
        const auto rejected = update(start + 500ms);
        ensure("invalid new dependency is rejected", rejected && !rejected->ok());
        ensure_equals("rejection preserves live generation", system.generation(), 1ULL);

        snapshots.snapshot = importedStyleSnapshot(
            "@import \"new.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"new.radia", "floater { width: 440px; }"}});
        ensure("dependency fix waits to settle", !update(start + 750ms).has_value());
        const auto recovered = update(start + 1s);

        ensure("fixed rejected dependency retries", recovered && recovered->ok());
        ensure_equals("fixed candidate publishes", system.generation(), 2ULL);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<8>()
    {
        set_test_name("one generation atomically replaces every open document");
        Document second_document;
        std::unique_ptr<rdui::Floater> second_installed;
        std::vector<rdui::viewer::FloaterReloadTarget> targets{
            {&document, [this](std::unique_ptr<rdui::Floater> floater)
                { installed = std::move(floater); }},
            {&second_document, [&second_installed](std::unique_ptr<rdui::Floater> floater)
                { second_installed = std::move(floater); }},
        };
        coordinator.request();

        const auto result = coordinator.update({}, targets);

        ensure("multi-document reload committed", result && result->ok());
        ensure("first document installed", installed != nullptr);
        ensure("second document installed", second_installed != nullptr);
        ensure_equals("first binding committed", document.commits, 1);
        ensure_equals("second binding committed", second_document.commits, 1);
        ensure_equals("one generation published", system.generation(), 1ULL);
    }
}
