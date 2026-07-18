#include "linden_common.h"
#include "../test/lltut.h"

#include "../rduifloaterplacementstore.h"

#include <optional>
#include <variant>

namespace tut
{
    struct floater_placement_store
    {
        struct MemoryPersistence final : rdui::viewer::FloaterPlacementStore::Persistence
        {
            LLSD read() const override { return value; }

            void write(const LLSD& placements) override
            {
                value = placements;
                ++writes;
            }

            LLSD value = LLSD::emptyMap();
            int writes = 0;
        };
    };

    using floater_placement_store_group = test_group<floater_placement_store>;
    using floater_placement_store_object = floater_placement_store_group::object;
    floater_placement_store_group floater_placement_store_tests("RduiFloaterPlacementStore");

    template<> template<>
    void floater_placement_store_object::test<1>()
    {
        set_test_name("startup saves cannot overwrite placement before restore");
        MemoryPersistence persistence;
        persistence.value["demo"]["detached"] = false;
        persistence.value["demo"]["x"] = 31.f;
        persistence.value["demo"]["y"] = 42.f;

        const rdui::viewer::FloaterInstanceId identity("demo");
        rdui::viewer::FloaterPlacementStore store(persistence);
        store.save(identity, rdui::viewer::AttachedFloaterPlacement{0.f, 0.f, std::nullopt});

        ensure_equals("startup placement was not written", persistence.writes, 0);
        const auto placement = store.restore(identity);
        ensure("saved placement restored", placement.has_value());
        const auto* attached = std::get_if<rdui::viewer::AttachedFloaterPlacement>(&*placement);
        ensure("attached placement restored", attached != nullptr);
        ensure_equals("saved x preserved", attached->x, 31.f);
        ensure_equals("saved y preserved", attached->y, 42.f);
    }

    template<> template<>
    void floater_placement_store_object::test<2>()
    {
        set_test_name("attached placement round trips and preserves other floaters");
        MemoryPersistence persistence;
        persistence.value["other"]["x"] = 9;
        const rdui::viewer::FloaterInstanceId identity("demo");
        rdui::viewer::FloaterPlacementStore store(persistence);
        store.restore(identity);
        store.save(identity, rdui::viewer::AttachedFloaterPlacement{
            12.5f, 18.f, rdui::viewer::FloaterPlacementSize{320.f, 240.f}});

        ensure_equals("one write", persistence.writes, 1);
        ensure_equals("other placement preserved", persistence.value["other"]["x"].asInteger(), 9);
        ensure("attached marker stored", !persistence.value["demo"]["detached"].asBoolean());

        rdui::viewer::FloaterPlacementStore restored_store(persistence);
        const auto restored = restored_store.restore(identity);
        const auto* attached = std::get_if<rdui::viewer::AttachedFloaterPlacement>(&*restored);
        ensure("attached placement decoded", attached != nullptr);
        ensure_equals("x round trips", attached->x, 12.5f);
        ensure_equals("y round trips", attached->y, 18.f);
        ensure("size round trips", attached->size.has_value());
        ensure_equals("width round trips", attached->size->width, 320.f);
        ensure_equals("height round trips", attached->size->height, 240.f);
    }

    template<> template<>
    void floater_placement_store_object::test<3>()
    {
        set_test_name("detached placement round trips with optional logical size");
        MemoryPersistence persistence;
        const rdui::viewer::FloaterInstanceId identity("demo");
        rdui::viewer::FloaterPlacementStore store(persistence);
        store.restore(identity);
        store.save(identity, rdui::viewer::DetachedFloaterPlacement{
            100, 200, 640, 480, "monitor-2",
            rdui::viewer::FloaterPlacementSize{400.f, 300.f}});

        rdui::viewer::FloaterPlacementStore restored_store(persistence);
        const auto restored = restored_store.restore(identity);
        const auto* detached = std::get_if<rdui::viewer::DetachedFloaterPlacement>(&*restored);
        ensure("detached placement decoded", detached != nullptr);
        ensure_equals("native x round trips", detached->x, 100);
        ensure_equals("native y round trips", detached->y, 200);
        ensure_equals("native width round trips", detached->width, 640);
        ensure_equals("native height round trips", detached->height, 480);
        ensure_equals("monitor round trips", detached->monitor, std::string("monitor-2"));
        ensure("logical size round trips", detached->logicalSize.has_value());
        ensure_equals("logical width round trips", detached->logicalSize->width, 400.f);
        ensure_equals("logical height round trips", detached->logicalSize->height, 300.f);
    }

    template<> template<>
    void floater_placement_store_object::test<4>()
    {
        set_test_name("non-resizable attached placement discards stale size");
        MemoryPersistence persistence;
        persistence.value["demo"]["width"] = 800.f;
        persistence.value["demo"]["height"] = 600.f;
        const rdui::viewer::FloaterInstanceId identity("demo");
        rdui::viewer::FloaterPlacementStore store(persistence);
        store.restore(identity);
        store.save(identity, rdui::viewer::AttachedFloaterPlacement{4.f, 5.f, std::nullopt});

        ensure("stale width removed", !persistence.value["demo"].has("width"));
        ensure("stale height removed", !persistence.value["demo"].has("height"));
    }

    template<> template<>
    void floater_placement_store_object::test<5>()
    {
        set_test_name("restore gates are independent for each floater identity");
        MemoryPersistence persistence;
        const rdui::viewer::FloaterInstanceId first("first");
        const rdui::viewer::FloaterInstanceId second("second");
        rdui::viewer::FloaterPlacementStore store(persistence);

        store.restore(first);
        store.save(first, rdui::viewer::AttachedFloaterPlacement{10.f, 20.f, std::nullopt});
        store.save(second, rdui::viewer::AttachedFloaterPlacement{30.f, 40.f, std::nullopt});

        ensure("restored identity may save", persistence.value["first"].isMap());
        ensure("unrestored identity remains gated", !persistence.value["second"].isMap());
        store.restore(second);
        store.save(second, rdui::viewer::AttachedFloaterPlacement{30.f, 40.f, std::nullopt});
        ensure("second identity opens independently", persistence.value["second"].isMap());
    }

}
