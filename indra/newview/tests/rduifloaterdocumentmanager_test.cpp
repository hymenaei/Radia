#include "linden_common.h"
#include "../test/lltut.h"

#include "../rduifloaterdocumentmanager.h"

#include "rdfloater.h"
#include "rduiskincompiler.h"
#include "rduisystem.h"

#include <map>
#include <memory>
#include <string>

namespace tut
{
    struct floater_document_manager
    {
        struct ControllerState
        {
            int idle = 0;
            int succeeded = 0;
            int failed = 0;
        };

        class Controller final : public rdui::viewer::ReloadableFloater
        {
            public:
                Controller(std::string resource, ControllerState& state)
                    : mResource(std::move(resource)), mState(state) {}

                std::string reloadResourceId() const override { return mResource; }
                rdui::PreparedBindingResult prepareBindings(rdui::Floater& floater) override
                {
                    return rdui::Binder(floater).prepare();
                }
                void commitBindings(rdui::PreparedBinding&& prepared) override
                {
                    mBinding = prepared.commit();
                }
                void idle() override { ++mState.idle; }
                void reportReloadSucceeded() override { ++mState.succeeded; }
                void reportReloadFailed(const rdui::DiagnosticResult&) override { ++mState.failed; }

            private:
                std::string mResource;
                ControllerState& mState;
                rdui::Binding mBinding;
        };

        class Host final : public rdui::viewer::FloaterDocumentManager::Host
        {
            public:
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

                void show(rdui::Floater&) override { ++shows; }

                std::map<std::string, std::unique_ptr<rdui::Floater>> mounted;
                int replacements = 0;
                int shows = 0;
        };

        floater_document_manager() : manager(system, host)
        {
            rdui::ResourceSnapshot resources;
            resources.add("localization.xml",
                          "<localizations default=\"en\">"
                          "<localization id=\"en\" lang=\"English\" direction=\"ltr\"/>"
                          "</localizations>");
            resources.add("skin.radia", "");
            resources.add("one.xml", "<floater/>");
            resources.add("two.xml", "<floater/>");
            rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(resources));
            if (prepared.ok()) system.publish(std::move(prepared.generation));
        }

        bool registerOne(const std::string& definition = "one")
        {
            return manager.registerDefinition(definition, [this](rdui::System&)
            {
                return std::make_unique<Controller>("one.xml", controllerState);
            });
        }

        rdui::System system;
        Host host;
        ControllerState controllerState;
        rdui::viewer::FloaterDocumentManager manager;
    };

    using floater_document_manager_group = test_group<floater_document_manager>;
    using floater_document_manager_object = floater_document_manager_group::object;
    floater_document_manager_group floater_document_manager_tests("RduiFloaterDocumentManager");

    template<> template<>
    void floater_document_manager_object::test<1>()
    {
        set_test_name("registering a definition does not open a floater");
        ensure("definition registered", registerOne());
        ensure_equals("no live instances", manager.size(), std::size_t(0));
        ensure_equals("host remains empty", host.mounted.size(), std::size_t(0));
    }

    template<> template<>
    void floater_document_manager_object::test<2>()
    {
        set_test_name("one definition opens independently keyed instances");
        ensure("definition registered", registerOne("profile"));

        const auto first = manager.open("profile", "alice");
        const auto second = manager.open("profile", "bob");

        ensure("first instance opened", first.ok());
        ensure("second instance opened", second.ok());
        ensure("instances are distinct", first.floater != second.floater);
        ensure("first stable identity mounted", host.mounted.contains("profile/alice"));
        ensure("second stable identity mounted", host.mounted.contains("profile/bob"));
        ensure_equals("two live instances", manager.size(), std::size_t(2));
    }

    template<> template<>
    void floater_document_manager_object::test<3>()
    {
        set_test_name("opening the same singleton shows its existing instance");
        ensure("definition registered", registerOne());
        const auto first = manager.open("one");
        const auto second = manager.open("one");

        ensure("singleton opened", first.ok() && second.ok());
        ensure("same instance returned", first.floater == second.floater);
        ensure_equals("host mounted once", host.mounted.size(), std::size_t(1));
        ensure_equals("existing instance shown", host.shows, 1);
    }

    template<> template<>
    void floater_document_manager_object::test<4>()
    {
        set_test_name("reload replacement retains document identity and controller lifecycle");
        ensure("definition registered", registerOne());
        const auto opened = manager.open("one");
        ensure("instance opened", opened.ok());
        rdui::Floater* original = opened.floater;
        std::vector<rdui::viewer::FloaterReloadTarget> targets = manager.reloadTargets();
        ensure_equals("one reload target", targets.size(), std::size_t(1));

        targets.front().install(std::make_unique<rdui::Floater>());
        rdui::Floater* replacement = manager.floaters().front();
        manager.idle();
        manager.reportReloadSucceeded();
        rdui::DiagnosticResult diagnostics;
        manager.reportReloadFailed(diagnostics);

        ensure("replacement installed", replacement != original);
        ensure_equals("host replaced once", host.replacements, 1);
        ensure_equals("identity retained", manager.identity(*replacement)->value(), std::string("one"));
        ensure_equals("controller idled", controllerState.idle, 1);
        ensure_equals("success reported", controllerState.succeeded, 1);
        ensure_equals("failure reported", controllerState.failed, 1);
    }

    template<> template<>
    void floater_document_manager_object::test<5>()
    {
        set_test_name("open document snapshot preserves definition and instance key");
        ensure("definition registered", registerOne("profile"));
        const auto first = manager.open("profile", "alice");
        const auto second = manager.open("profile", "bob");
        ensure("instances opened", first.ok() && second.ok());

        second.floater->close();
        const std::vector<rdui::viewer::FloaterDocumentId> closed = manager.openDocuments();
        ensure_equals("only open instance retained", closed.size(), std::size_t(1));
        ensure_equals("definition retained", closed.front().definitionId, std::string("profile"));
        ensure_equals("instance key retained", closed.front().instanceKey, std::string("alice"));

        ensure("closed instance reopened", manager.open("profile", "bob").ok());
        ensure_equals("reopened instance retained", manager.openDocuments().size(), std::size_t(2));
    }
}
