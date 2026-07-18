#include "linden_common.h"
#include "../test/lltut.h"

#include "../rduiskinreloadcoordinator.h"
#include "../rduifloaterdocumentmanager.h"
#include "../rduiskinresolver.h"

#include "rdfloater.h"
#include "rduiskincompiler.h"
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

        struct ControllerState
        {
            bool rejectBinding = false;
            int prepares = 0;
            int commits = 0;
            rdui::Binding binding;
        };

        struct Controller final : rdui::viewer::FloaterController
        {
            explicit Controller(ControllerState& state) : mState(state) {}

            std::string resourceId() const override { return "view.xml"; }

            rdui::PreparedBindingResult prepareBindings(rdui::Floater& floater) override
            {
                ++mState.prepares;
                if (mState.rejectBinding)
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
                mState.binding = prepared.commit();
                ++mState.commits;
            }

            ControllerState& mState;
        };

        struct Host final : rdui::viewer::FloaterDocumentManager::Host
        {
            rdui::Floater* mount(const rdui::viewer::FloaterInstanceId& identity,
                                 std::unique_ptr<rdui::Floater> floater) override
            {
                rdui::Floater* result = floater.get();
                mounted.insert_or_assign(identity.value(), std::move(floater));
                return result;
            }

            rdui::Floater* replace(const rdui::viewer::FloaterInstanceId& identity,
                                   rdui::Floater& current,
                                   std::unique_ptr<rdui::Floater> replacement) override
            {
                const auto found = mounted.find(identity.value());
                if (found == mounted.end() || found->second.get() != &current) return nullptr;
                rdui::Floater* result = replacement.get();
                found->second = std::move(replacement);
                ++replacements;
                return result;
            }

            void show(rdui::Floater&) override {}

            std::map<std::string, std::unique_ptr<rdui::Floater>> mounted;
            int replacements = 0;
        };

        SnapshotSource snapshots;
        rdui::System system;
        Host host;
        ControllerState document;
        rdui::viewer::FloaterDocumentManager documents{system, host};
        rdui::viewer::SkinReloadCoordinator coordinator{system, snapshots};

        skin_reload_coordinator()
        {
            rdui::SkinGenerationPrepareResult prepared =
                rdui::SkinCompiler().prepare(skinSnapshot());
            if (prepared.ok()) system.publish(std::move(prepared.generation));
            documents.registerDefinition("document", [this](rdui::System&)
            {
                return std::make_unique<Controller>(document);
            });
            documents.open("document");
            document.prepares = 0;
            document.commits = 0;
            host.replacements = 0;
        }

        rdui::Floater* installed(const std::string& identity = "document") const
        {
            const auto found = host.mounted.find(identity);
            return found == host.mounted.end() ? nullptr : found->second.get();
        }

        std::optional<rdui::viewer::SkinReloadResult> update(
            rdui::viewer::SkinReloadCoordinator::TimePoint now = {})
        {
            return coordinator.update(now, documents);
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
        ensure_equals("candidate generation published", system.generation(), 2ULL);
        ensure_equals("result reports generation", result->generation, 2ULL);
        ensure("replacement installed", installed() != nullptr);
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
        rdui::Floater* live = installed();
        const int prepares = document.prepares;
        const int commits = document.commits;

        snapshots.snapshot = skinSnapshot();
        snapshots.snapshot.add("localization.xml", "<localizations>");
        coordinator.request();
        const auto rejected = update();

        ensure("rejection produced a result", rejected.has_value());
        ensure("invalid candidate rejected", !rejected->ok());
        ensure("diagnostics returned", !rejected->errors.empty());
        ensure_equals("live generation preserved", system.generation(), 2ULL);
        ensure("live Floater preserved", installed() == live);
        ensure_equals("binding preparation not reached", document.prepares, prepares);
        ensure_equals("binding commit not reached", document.commits, commits);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<3>()
    {
        set_test_name("binding rejection prevents publication and installation");
        document.rejectBinding = true;
        rdui::Floater* live = installed();
        coordinator.request();
        const auto rejected = update();

        ensure("binding rejection returned", rejected.has_value());
        ensure("binding rejection failed", !rejected->ok());
        ensure_equals("candidate was not published", system.generation(), 1ULL);
        ensure("live document was preserved", installed() == live);
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
        ensure_equals("one candidate generation published", system.generation(), 2ULL);
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
        ensure_equals("candidate generation not published", system.generation(), 1ULL);
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
        ensure_equals("only dependency edit publishes again", system.generation(), 3ULL);
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
        ensure_equals("rejection preserves live generation", system.generation(), 2ULL);

        snapshots.snapshot = importedStyleSnapshot(
            "@import \"new.radia\";",
            {{"used.radia", "floater { width: 300px; }"},
             {"new.radia", "floater { width: 440px; }"}});
        ensure("dependency fix waits to settle", !update(start + 750ms).has_value());
        const auto recovered = update(start + 1s);

        ensure("fixed rejected dependency retries", recovered && recovered->ok());
        ensure_equals("fixed candidate publishes", system.generation(), 3ULL);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<8>()
    {
        set_test_name("one generation atomically replaces every open document");
        ControllerState second_document;
        ensure("second definition registered", documents.registerDefinition(
            "second", [&second_document](rdui::System&)
            {
                return std::make_unique<skin_reload_coordinator::Controller>(second_document);
            }));
        ensure("second document opened", documents.open("second").ok());
        second_document.prepares = 0;
        second_document.commits = 0;
        rdui::Floater* first_live = installed();
        rdui::Floater* second_live = installed("second");
        coordinator.request();

        const auto result = coordinator.update({}, documents);

        ensure("multi-document reload committed", result && result->ok());
        ensure("first document replaced", installed() != first_live);
        ensure("second document replaced", installed("second") != second_live);
        ensure_equals("first binding committed", document.commits, 1);
        ensure_equals("second binding committed", second_document.commits, 1);
        ensure_equals("one candidate generation published", system.generation(), 2ULL);
    }

    template<> template<>
    void skin_reload_coordinator_object::test<9>()
    {
        set_test_name("one rejected binding rolls back every open document");
        ControllerState rejected_document;
        rejected_document.rejectBinding = true;
        ensure("second definition registered", documents.registerDefinition(
            "second", [&rejected_document](rdui::System&)
            {
                return std::make_unique<skin_reload_coordinator::Controller>(rejected_document);
            }));
        // Open succeeds before binding rejection is enabled for reload.
        rejected_document.rejectBinding = false;
        ensure("second document opened", documents.open("second").ok());
        rejected_document.rejectBinding = true;
        document.prepares = 0;
        document.commits = 0;
        rejected_document.prepares = 0;
        rejected_document.commits = 0;
        rdui::Floater* first_live = installed();
        rdui::Floater* second_live = installed("second");
        coordinator.request();

        const auto result = coordinator.update({}, documents);

        ensure("transaction rejected", result && !result->ok());
        ensure("first document preserved", installed() == first_live);
        ensure("second document preserved", installed("second") == second_live);
        ensure_equals("live generation preserved", system.generation(), 1ULL);
        ensure_equals("no binding committed", document.commits, 0);
        ensure_equals("rejected binding not committed", rejected_document.commits, 0);
    }
}
