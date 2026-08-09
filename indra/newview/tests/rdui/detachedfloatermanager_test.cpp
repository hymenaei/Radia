/**
 * @file detachedfloatermanager_test.cpp
 * @brief Tests detached Floater lifecycle, native presentation, placement, and interaction behavior.
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
#include <utility>
#include "../test/lltut.h"
#include "detachedfloatermanager.h"
#include "surface/surface.h"
#include "widgets/floater.h"

namespace tut {
struct detached_floater_manager {
    struct MemoryPersistence final : rdui::viewer::FloaterPlacementStore::Persistence {
        LLSD read() const override { return value; }
        void write(const LLSD& placements) override {
            value = placements;
            ++writes;
        }

        LLSD value = LLSD::emptyMap();
        int writes = 0;
    };

    struct PresentationState {
        bool openSucceeds = true;
        bool opened = false;
        bool visible = true;
        bool closeRequested = false;
        bool minimizeRequested = false;
        bool dragEnded = false;
        bool resizeEnded = false;
        bool resizeStarted = false;
        int ticks = 0;
        rdui::viewer::NativeRect native{100, 200, 320, 240};
        rdui::Vec2 logical{320.f, 240.f};
        rdui::Vec2 headerCenter{120.f, 220.f};
    };

    class FakePresentation final : public rdui::viewer::DetachedFloaterPresentation {
    public:
        FakePresentation(std::unique_ptr<rdui::Floater> floater, PresentationState& state) : mFloater(std::move(floater)), mState(state) {}

        bool open(const rdui::viewer::NativeRect& rect, float, const std::optional<rdui::Vec2>&, const std::optional<rdui::Vec2>& logical_size,
                  const std::optional<rdui::viewer::NativePoint>&) override {
            mState.opened = true;
            mState.native = rect;
            if (logical_size) mState.logical = *logical_size;
            return mState.openSucceeds;
        }
        bool beginResize() override {
            mState.resizeStarted = true;
            return true;
        }
        void applyResize(const rdui::Rect& logical_rect) override { mState.logical = {logical_rect.w, logical_rect.h}; }
        void tick() override { ++mState.ticks; }
        void setVisible(bool visible) override { mState.visible = visible; }
        std::unique_ptr<rdui::Floater> releaseFloater() override { return std::move(mFloater); }
        rdui::Floater& replaceFloater(std::unique_ptr<rdui::Floater> replacement, const std::optional<rdui::Vec2>& logical_size) override {
            mFloater = std::move(replacement);
            if (logical_size) mState.logical = *logical_size;
            return *mFloater;
        }
        bool closeRequested() const override { return mState.closeRequested; }
        bool minimizeRequested() const override { return mState.minimizeRequested; }
        bool takeDragEnded() override { return std::exchange(mState.dragEnded, false); }
        bool takeResizeEnded() override { return std::exchange(mState.resizeEnded, false); }
        rdui::Floater* floater() const override { return mFloater.get(); }
        rdui::viewer::NativeRect nativeRect() const override { return mState.native; }
        std::string monitorId() const override { return "test-monitor"; }
        rdui::Vec2 logicalSize() const override { return mState.logical; }
        rdui::Vec2 headerCenterScreen() const override { return mState.headerCenter; }

    private:
        std::unique_ptr<rdui::Floater> mFloater;
        PresentationState& mState;
    };

    struct FakeEnvironment final : rdui::viewer::DetachedFloaterManager::Environment {
        rdui::viewer::NativeRect mainRectToNative(const rdui::Rect& rect) const override {
            return {static_cast<S32>(rect.x), static_cast<S32>(rect.y), static_cast<S32>(rect.w), static_cast<S32>(rect.h)};
        }
        float nativeScaleMultiplier() const override { return 1.f; }
        rdui::Vec2 nativeBottomLeftInMain(const rdui::viewer::NativeRect&) const override { return {15.f, 25.f}; }
        bool nativePointInsideMain(const rdui::Vec2&) const override { return pointInside; }
        bool placementVisible(const rdui::viewer::NativeRect&, const std::string&) const override { return visiblePlacement; }
        std::optional<rdui::viewer::NativePoint> releasePointerForDetach(const rdui::Vec2& position) override {
            releasedPointer = true;
            releasedAt = position;
            return rdui::viewer::NativePoint{77, 88};
        }

        bool pointInside = false;
        bool visiblePlacement = true;
        bool releasedPointer = false;
        rdui::Vec2 releasedAt;
    };

    detached_floater_manager()
        : store(persistence),
          manager(
              surface, store,
              [this](std::unique_ptr<rdui::Floater>& floater) { return std::make_unique<FakePresentation>(std::move(floater), presentation); },
              environment) {
        surface.setViewport(800.f, 600.f);
        store.restore(identity);
    }

    rdui::Floater& mountFloater() {
        auto floater = std::make_unique<rdui::Floater>();
        floater->setCanResize(true).setRect({30.f, 40.f, 320.f, 240.f});
        return surface.mountFloater(std::move(floater));
    }

    MemoryPersistence persistence;
    rdui::viewer::FloaterInstanceId identity{"demo"};
    rdui::viewer::FloaterPlacementStore store;
    rdui::Surface surface;
    PresentationState presentation;
    FakeEnvironment environment;
    rdui::viewer::DetachedFloaterManager manager;
};

using detached_floater_manager_group = test_group<detached_floater_manager>;
using detached_floater_manager_object = detached_floater_manager_group::object;
detached_floater_manager_group detached_floater_manager_tests("RduiDetachedFloaterManager");

template<> template<> void detached_floater_manager_object::test<1>() {
    set_test_name("detach transfers ownership and persists native placement");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {12.f, 8.f});
    manager.processPendingDetach();

    ensure("presentation opened", presentation.opened);
    ensure("floater is detached", manager.contains(floater));
    ensure("main pointer capture released", environment.releasedPointer);
    ensure_equals("drag handoff x", environment.releasedAt.x, 102.f);
    ensure_equals("detached marker persisted", persistence.value["demo"]["detached"].asBoolean(), true);
}

template<> template<> void detached_floater_manager_object::test<2>() {
    set_test_name("drag ending over main surface reattaches floater");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();
    environment.pointInside = true;
    presentation.dragEnded = true;
    manager.update();

    ensure("floater returned to attached surface", !manager.contains(floater));
    ensure_equals("reattached x converted from native geometry", floater.rect().x, 15.f);
    ensure_equals("reattached y converted from native geometry", floater.rect().y, 25.f);
    ensure_equals("attached marker persisted", persistence.value["demo"]["detached"].asBoolean(), false);
}

template<> template<> void detached_floater_manager_object::test<3>() {
    set_test_name("invisible saved placement leaves floater attached");
    rdui::Floater& floater = mountFloater();
    environment.visiblePlacement = false;
    const bool restored = manager.restore(identity, floater, {100, 200, 320, 240, "missing-monitor", std::nullopt});

    ensure("invalid detached placement rejected", !restored);
    ensure("presentation was not opened", !presentation.opened);
    ensure("floater remains attached", !manager.contains(floater));
}

template<> template<> void detached_floater_manager_object::test<4>() {
    set_test_name("reload replacement stays in detached presentation");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();

    auto replacement = std::make_unique<rdui::Floater>();
    replacement->setCanResize(true).setRect({0.f, 0.f, 400.f, 300.f});
    rdui::Floater* mounted = manager.replace(floater, std::move(replacement), rdui::Vec2{400.f, 300.f});

    ensure("replacement mounted", mounted != nullptr);
    ensure("replacement remains detached", manager.contains(*mounted));
    ensure_equals("logical replacement width preserved", manager.logicalSize(*mounted).x, 400.f);
}

template<> template<> void detached_floater_manager_object::test<5>() {
    set_test_name("failed native open restores attached ownership and geometry");
    rdui::Floater& floater = mountFloater();
    presentation.openSucceeds = false;
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();

    ensure("failed presentation does not remain detached", !manager.contains(floater));
    ensure_equals("attached x restored", floater.rect().x, 30.f);
    ensure_equals("attached y restored", floater.rect().y, 40.f);
    ensure_equals("attached width restored", floater.rect().w, 320.f);
    ensure_equals("attached height restored", floater.rect().h, 240.f);
}

template<> template<> void detached_floater_manager_object::test<6>() {
    set_test_name("completed detached resize persists canonical logical size");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();
    presentation.logical = {411.5f, 277.25f};
    presentation.resizeEnded = true;
    manager.update();

    ensure_approximately_equals("logical width persists without native rounding", persistence.value["demo"]["logical_width"].asReal(), 411.5, 6);
    ensure_approximately_equals("logical height persists without native rounding", persistence.value["demo"]["logical_height"].asReal(), 277.25, 6);
}

template<> template<> void detached_floater_manager_object::test<7>() {
    set_test_name("disabling resize deletes stale detached logical dimensions");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();
    ensure("resizable placement initially has logical width", persistence.value["demo"].has("logical_width"));

    auto replacement = std::make_unique<rdui::Floater>();
    replacement->setCanResize(false).setRect({0.f, 0.f, 292.f, 300.f});
    rdui::Floater* mounted = manager.replace(floater, std::move(replacement), rdui::Vec2{292.f, 300.f});

    ensure("replacement remains detached", mounted && manager.contains(*mounted));
    ensure("stale logical width removed", !persistence.value["demo"].has("logical_width"));
    ensure("stale logical height removed", !persistence.value["demo"].has("logical_height"));
}

template<> template<> void detached_floater_manager_object::test<8>() {
    set_test_name("logical resize geometry converts from one canonical baseline");
    const rdui::viewer::NativeRect native = rdui::viewer::nativeRectForLogicalResize({100, 200, 300, 240}, {-10.5f, 5.25f, 220.5f, 150.25f}, 1.5f);

    ensure_equals("left edge converts at current scale", native.x, 84);
    ensure_equals("top edge converts from bottom-left logical coordinates", native.y, 207);
    ensure_equals("width rounds only at native presentation", native.width, 331);
    ensure_equals("height rounds only at native presentation", native.height, 225);
}

template<> template<> void detached_floater_manager_object::test<9>() {
    set_test_name("manager owns native resize requests and logical geometry");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();

    ensure("manager begins presentation resize", manager.beginResize(floater));
    manager.applyResize(floater, {-10.f, 0.f, 410.5f, 280.25f});
    ensure("presentation received begin request", presentation.resizeStarted);
    ensure_equals("presentation receives canonical logical width", presentation.logical.x, 410.5f);
    ensure_equals("presentation receives canonical logical height", presentation.logical.y, 280.25f);
}

template<> template<> void detached_floater_manager_object::test<10>() {
    set_test_name("detached presentations retain independent floater identities");
    const rdui::viewer::FloaterInstanceId second_identity("second");
    store.restore(second_identity);
    rdui::Floater& first = mountFloater();
    rdui::Floater& second = mountFloater();

    manager.requestDetach(identity, first, {90.f, 110.f}, {});
    manager.processPendingDetach();
    manager.requestDetach(second_identity, second, {190.f, 210.f}, {});
    manager.processPendingDetach();

    ensure("first floater remains detached", manager.contains(first));
    ensure("second floater remains detached", manager.contains(second));
    ensure_equals("first placement uses first identity", persistence.value["demo"]["x"].asInteger(), 90);
    ensure_equals("second placement uses second identity", persistence.value["second"]["x"].asInteger(), 190);
}

template<> template<> void detached_floater_manager_object::test<11>() {
    set_test_name("manager update pumps each detached presentation");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();

    ensure_equals("presentation has not ticked during detach", presentation.ticks, 0);
    manager.update();

    ensure_equals("one manager update produces one presentation tick", presentation.ticks, 1);
    ensure("ordinary presentation tick keeps floater detached", manager.contains(floater));
}

template<> template<> void detached_floater_manager_object::test<12>() {
    set_test_name("manager visibility propagates to detached presentations");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();

    ensure("detached presentation starts visible", presentation.visible);
    manager.setVisible(false);
    ensure("hiding manager hides detached presentation", !presentation.visible);
    manager.setVisible(true);
    ensure("showing manager shows detached presentation", presentation.visible);
}

template<> template<> void detached_floater_manager_object::test<13>() {
    set_test_name("native close callback reattaches without preserving minimization");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.closeRequested = true;

    manager.update();

    ensure("closed presentation is no longer detached", !manager.contains(floater));
    ensure("close transition clears minimized state", !floater.minimized());
    ensure_equals("close transition persists attached marker", persistence.value["demo"]["detached"].asBoolean(), false);
}

template<> template<> void detached_floater_manager_object::test<14>() {
    set_test_name("native minimize callback reattaches and preserves minimization");
    rdui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetach();
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.minimizeRequested = true;

    manager.update();

    ensure("minimized presentation is no longer detached", !manager.contains(floater));
    ensure("minimize transition preserves minimized state", floater.minimized());
    ensure_equals("minimize transition persists attached marker", persistence.value["demo"]["detached"].asBoolean(), false);
}
} // namespace tut
