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
#include "llcontrol.h"
#include "surface/surface.h"
#include "widgets/floater.h"

namespace tut {
struct detachedFloaterManagerData {
    struct PresentationState {
        bool openSucceeds = true;
        bool opened = false;
        bool visible = true;
        bool closeRequested = false;
        bool minimizeRequested = false;
        bool dragEnded = false;
        bool resizeEnded = false;
        bool resizeStarted = false;
        bool replacementPrepared = false;
        bool replacementSucceeds = true;
        int ticks = 0;
        AuxiliaryWindowRect native{100, 200, 320, 240};
        radia::ui::Vec2 logicalSize{320.f, 240.f};
        radia::ui::Vec2 authoredSize{320.f, 240.f};
        radia::ui::Vec2 headerCenter{120.f, 220.f};
    };

    class FakePresentation final : public radia::viewer::ui::DetachedFloaterPresentation {
    public:
        FakePresentation(std::unique_ptr<radia::ui::Floater> floater, PresentationState& state) : mFloater(std::move(floater)), mState(state) {}

        std::optional<radia::viewer::ui::DetachedFloaterPresentationUpdate> open(
            const radia::viewer::ui::DetachedFloaterPresentationOpenRequest& request) override {
            mState.opened = true;
            mState.native = request.rect;
            if (request.logicalSize) mState.logicalSize = *request.logicalSize;
            if (!mState.openSucceeds) return std::nullopt;
            return radia::viewer::ui::DetachedFloaterPresentationUpdate{
                false, false, false, false, mState.native, mState.logicalSize, mState.headerCenter};
        }
        bool beginResize() override {
            mState.resizeStarted = true;
            return true;
        }
        void applyResize(const radia::ui::Rect& logicalRect) override { mState.logicalSize = {logicalRect.w, logicalRect.h}; }
        radia::viewer::ui::DetachedFloaterPresentationUpdate update() override {
            ++mState.ticks;
            return {mState.closeRequested,
                    mState.minimizeRequested,
                    std::exchange(mState.dragEnded, false),
                    std::exchange(mState.resizeEnded, false),
                    mState.native,
                    mState.logicalSize,
                    mState.headerCenter};
        }
        void setVisible(bool visible) override { mState.visible = visible; }
        std::optional<radia::ui::Rect> prepareReplacement(radia::ui::Floater& replacement) override {
            mState.replacementPrepared = true;
            if (!mState.replacementSucceeds) return std::nullopt;
            return radia::ui::Rect{0.f, 0.f, mState.authoredSize.x, mState.authoredSize.y};
        }
        std::unique_ptr<radia::ui::Floater> releaseFloater() override { return std::move(mFloater); }
        std::unique_ptr<radia::ui::Floater> replaceFloater(std::unique_ptr<radia::ui::Floater> replacement,
                                                      const std::optional<radia::ui::Vec2>& logicalSize) override {
            std::unique_ptr<radia::ui::Floater> retired = std::move(mFloater);
            mFloater = std::move(replacement);
            if (logicalSize) mState.logicalSize = *logicalSize;
            return retired;
        }

    private:
        std::unique_ptr<radia::ui::Floater> mFloater;
        PresentationState& mState;
    };

    struct FakeEnvironment final : radia::viewer::ui::DetachedFloaterManager::DetachedFloaterEnvironment {
        AuxiliaryWindowRect mainRectToNative(const radia::ui::Rect& rect) const override {
            return {static_cast<S32>(rect.x), static_cast<S32>(rect.y), static_cast<S32>(rect.w), static_cast<S32>(rect.h)};
        }
        float nativeScaleMultiplier() const override { return 1.f; }
        radia::ui::Vec2 nativeBottomLeftInMain(const AuxiliaryWindowRect&) const override { return {15.f, 25.f}; }
        bool nativePointInsideMain(const radia::ui::Vec2&) const override { return pointInside; }
        bool placementVisible(const AuxiliaryWindowRect&) const override { return visiblePlacement; }
        std::optional<AuxiliaryScreenPoint> releasePointerForDetach(const radia::ui::Vec2& position) override {
            releasedPointer = true;
            releasedAt = position;
            return AuxiliaryScreenPoint{77, 88};
        }

        bool pointInside = false;
        bool visiblePlacement = true;
        bool releasedPointer = false;
        radia::ui::Vec2 releasedAt;
    };

    detachedFloaterManagerData()
        : settings("detached-floater-manager"), persistence(settings, settings),
          manager(
              surface,
              [this](std::unique_ptr<radia::ui::Floater> floater) {
                  if (shouldFailFactory) return radia::viewer::ui::DetachedFloaterPresentationResult::failure(std::move(floater));
                  return radia::viewer::ui::DetachedFloaterPresentationResult::success(
                      std::make_unique<FakePresentation>(std::move(floater), presentation));
              },
              environment,
              [this](const radia::viewer::ui::ComponentKey& componentKey, radia::viewer::ui::FloaterPlacement placement,
                     radia::viewer::ui::ComponentOpenState state) { persistence.savePlacement(componentKey, std::move(placement), state); }) {
        settings.declareLLSD("UILayout", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        settings.declareLLSD("UIWorkspace", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        surface.setViewport(800.f, 600.f);
        persistence.restorePlacement(identity);
    }

    radia::ui::Floater& mountFloater() {
        auto floater = std::make_unique<radia::ui::Floater>();
        floater->setCanResize(true).setRect({30.f, 40.f, 320.f, 240.f});
        return surface.mountFloater(std::move(floater));
    }

    LLControlGroup settings;
    radia::viewer::ui::ComponentPersistence persistence;
    radia::viewer::ui::ComponentKey identity{"demo", {}};
    radia::ui::Surface surface;
    PresentationState presentation;
    FakeEnvironment environment;
    bool shouldFailFactory = false;
    radia::viewer::ui::DetachedFloaterManager manager;
};
using detachedFloaterManagerTest = test_group<detachedFloaterManagerData>;
using detachedFloaterManagerObject = detachedFloaterManagerTest::object;
detachedFloaterManagerTest detachedFloaterManagerTestCase("UIDetachedFloaterManager");

template<> template<> void detachedFloaterManagerObject::test<1>() {
    set_test_name("detach transfers ownership and persists native placement");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {12.f, 8.f});
    manager.processPendingDetachment();

    ensure("presentation opened", presentation.opened);
    ensure("floater is detached", manager.isDetached(floater));
    ensure("main pointer capture released", environment.releasedPointer);
    ensure_equals("drag handoff x", environment.releasedAt.x, 102.f);
    ensure_equals("detached marker persisted in user-wide layout", settings.getLLSD("UILayout")["demo"]["detached"].asBoolean(), true);
    ensure("workspace keeps the open marker", settings.getLLSD("UIWorkspace").has("demo"));
    ensure("workspace does not duplicate detached state", !settings.getLLSD("UIWorkspace")["demo"].has("detached"));
}

template<> template<> void detachedFloaterManagerObject::test<2>() {
    set_test_name("drag ending over main surface reattaches floater");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    environment.pointInside = true;
    presentation.dragEnded = true;
    manager.update();

    ensure("floater returned to attached surface", !manager.isDetached(floater));
    ensure_equals("reattached x converted from native geometry", floater.rect().x, 15.f);
    ensure_equals("reattached y converted from native geometry", floater.rect().y, 25.f);
    ensure("attached marker persisted in user-wide layout", !settings.getLLSD("UILayout")["demo"].has("detached"));
}

template<> template<> void detachedFloaterManagerObject::test<3>() {
    set_test_name("invisible saved placement leaves floater attached");
    radia::ui::Floater& floater = mountFloater();
    environment.visiblePlacement = false;
    const bool restored = manager.restoreDetachedPlacement(identity, floater, {100, 200, radia::viewer::ui::FloaterLogicalSize{320.f, 240.f}, false});

    ensure("invalid detached placement rejected", !restored);
    ensure("presentation was not opened", !presentation.opened);
    ensure("floater remains attached", !manager.isDetached(floater));
}

template<> template<> void detachedFloaterManagerObject::test<4>() {
    set_test_name("reload replacement stays in detached presentation");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    auto replacement = std::make_unique<radia::ui::Floater>();
    replacement->setCanResize(true).setRect({0.f, 0.f, 400.f, 300.f});
    const auto mountedResult = manager.replace(floater, std::move(replacement), radia::ui::Vec2{400.f, 300.f});
    radia::ui::Floater* mounted = mountedResult.installed();

    ensure("replacement mounted", mounted != nullptr);
    ensure("replacement remains detached", manager.isDetached(*mounted));
    ensure_equals("logical replacement width preserved", manager.logicalSize(*mounted).x, 400.f);
}

template<> template<> void detachedFloaterManagerObject::test<5>() {
    set_test_name("failed native open restores attached ownership and geometry");
    radia::ui::Floater& floater = mountFloater();
    presentation.openSucceeds = false;
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    ensure("failed presentation does not remain detached", !manager.isDetached(floater));
    ensure_equals("attached x restored", floater.rect().x, 30.f);
    ensure_equals("attached y restored", floater.rect().y, 40.f);
    ensure_equals("attached width restored", floater.rect().w, 320.f);
    ensure_equals("attached height restored", floater.rect().h, 240.f);
}

template<> template<> void detachedFloaterManagerObject::test<6>() {
    set_test_name("detached placement persists compact size arrays");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    presentation.logicalSize = {411.5f, 277.25f};
    presentation.resizeEnded = true;
    manager.update();

    ensure("detached position persists as an array", settings.getLLSD("UILayout")["demo"]["position"].isArray());
    ensure("detached size persists as an array", settings.getLLSD("UILayout")["demo"]["size"].isArray());
    ensure_equals("logical width is persisted in size", settings.getLLSD("UILayout")["demo"]["size"][0].asReal(), 411.5);
    ensure_equals("logical height is persisted in size", settings.getLLSD("UILayout")["demo"]["size"][1].asReal(), 277.25);
}

template<> template<> void detachedFloaterManagerObject::test<7>() {
    set_test_name("detached persistence keeps global layout and omits monitor identity");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    ensure("detached placement omits monitor identity", !settings.getLLSD("UILayout")["demo"].has("monitor"));

    auto replacement = std::make_unique<radia::ui::Floater>();
    replacement->setCanResize(false).setRect({0.f, 0.f, 292.f, 300.f});
    const auto mountedResult = manager.replace(floater, std::move(replacement), radia::ui::Vec2{292.f, 300.f});
    radia::ui::Floater* mounted = mountedResult.installed();

    ensure("replacement remains detached", mounted && manager.isDetached(*mounted));
}

template<> template<> void detachedFloaterManagerObject::test<8>() {
    set_test_name("logical resize geometry converts from one canonical baseline");
    const AuxiliaryWindowRect native = auxiliaryWindowRectForLogicalResize({100, 200, 300, 240}, {-10.5f, 5.25f, 220.5f, 150.25f}, 1.5f);

    ensure_equals("left edge converts at current scale", native.x, 84);
    ensure_equals("top edge converts from bottom-left logical coordinates", native.y, 207);
    ensure_equals("width rounds only at native presentation", native.width, 331);
    ensure_equals("height rounds only at native presentation", native.height, 225);
}

template<> template<> void detachedFloaterManagerObject::test<9>() {
    set_test_name("manager owns native resize requests and logical geometry");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    ensure("manager begins presentation resize", manager.beginResize(floater));
    manager.applyResize(floater, {-10.f, 0.f, 410.5f, 280.25f});
    ensure("presentation received begin request", presentation.resizeStarted);
    ensure_equals("presentation receives canonical logical width", presentation.logicalSize.x, 410.5f);
    ensure_equals("presentation receives canonical logical height", presentation.logicalSize.y, 280.25f);
}

template<> template<> void detachedFloaterManagerObject::test<10>() {
    set_test_name("detached presentations retain independent floater identities");
    const radia::viewer::ui::ComponentKey secondIdentity{"second", {}};
    persistence.restorePlacement(secondIdentity);
    radia::ui::Floater& first = mountFloater();
    radia::ui::Floater& second = mountFloater();

    manager.requestDetach(identity, first, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    manager.requestDetach(secondIdentity, second, {190.f, 210.f}, {});
    manager.processPendingDetachment();

    ensure("first floater remains detached", manager.isDetached(first));
    ensure("second floater remains detached", manager.isDetached(second));
    ensure_equals("first placement uses first identity", settings.getLLSD("UILayout")["demo"]["position"][0].asReal(), 90.0);
    ensure_equals("second placement uses second identity", settings.getLLSD("UILayout")["second"]["position"][0].asReal(), 190.0);
}

template<> template<> void detachedFloaterManagerObject::test<11>() {
    set_test_name("manager update pumps each detached presentation");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    ensure_equals("presentation has not ticked during detach", presentation.ticks, 0);
    manager.update();

    ensure_equals("one manager update produces one presentation tick", presentation.ticks, 1);
    ensure("ordinary presentation tick keeps floater detached", manager.isDetached(floater));
}

template<> template<> void detachedFloaterManagerObject::test<12>() {
    set_test_name("manager visibility propagates to detached presentations");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    ensure("detached presentation starts visible", presentation.visible);
    manager.setVisible(false);
    ensure("hiding manager hides detached presentation", !presentation.visible);
    manager.setVisible(true);
    ensure("showing manager shows detached presentation", presentation.visible);
}

template<> template<> void detachedFloaterManagerObject::test<13>() {
    set_test_name("native close callback reattaches without preserving minimization");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.closeRequested = true;

    manager.update();

    ensure("closed presentation is no longer detached", !manager.isDetached(floater));
    ensure("close transition clears minimized state", !floater.minimized());
    ensure("close transition clears the global detached marker", !settings.getLLSD("UILayout")["demo"].has("detached"));
}

template<> template<> void detachedFloaterManagerObject::test<14>() {
    set_test_name("native minimize callback reattaches and preserves minimization");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.minimizeRequested = true;

    manager.update();

    ensure("minimized presentation is no longer detached", !manager.isDetached(floater));
    ensure("minimize transition preserves minimized state", floater.minimized());
    ensure("minimize transition persists minimized state", settings.getLLSD("UIWorkspace")["demo"]["minimized"].asBoolean());
    ensure("minimize transition clears the global detached marker", !settings.getLLSD("UILayout")["demo"].has("detached"));
}
template<> template<> void detachedFloaterManagerObject::test<15>() {
    set_test_name("failed presentation creation returns the transferred Floater to the Surface");
    radia::ui::Floater& floater = mountFloater();
    shouldFailFactory = true;

    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    ensure("failed presentation does not remain detached", !manager.isDetached(floater));
    ensure("failed presentation preserves Surface ownership", surface.ownsFloater(floater));
}

template<> template<> void detachedFloaterManagerObject::test<16>() {
    set_test_name("replacement geometry is prepared by the detached presentation");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    presentation.authoredSize = {411.f, 277.f};
    auto replacement = std::make_unique<radia::ui::Floater>();
    const std::optional<radia::ui::Rect> authoredRect = manager.prepareReplacement(floater, *replacement);

    ensure("detached presentation prepared replacement Rect", presentation.replacementPrepared && authoredRect.has_value());
    ensure_equals("detached authored width is returned", authoredRect ? authoredRect->w : 0.f, 411.f);
    ensure_equals("detached authored height is returned", authoredRect ? authoredRect->h : 0.f, 277.f);
}

template<> template<> void detachedFloaterManagerObject::test<17>() {
    set_test_name("failed detached replacement preparation is propagated");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    presentation.replacementSucceeds = false;
    auto replacement = std::make_unique<radia::ui::Floater>();
    ensure("detached preparation failure is returned", !manager.prepareReplacement(floater, *replacement));
    ensure("detached presentation was still consulted", presentation.replacementPrepared);
}

template<> template<> void detachedFloaterManagerObject::test<18>() {
    set_test_name("teardown reattachment preserves the detached placement");
    radia::ui::Floater& floater = mountFloater();
    manager.requestDetach(identity, floater, {90.f, 110.f}, {});
    manager.processPendingDetachment();

    const LLSD detachedLayout = settings.getLLSD("UILayout")["demo"];
    manager.reattachAll(radia::viewer::ui::DetachedFloaterManager::ReattachMode::PreservePlacement);

    ensure("floater returns to the attached Surface", surface.ownsFloater(floater));
    ensure("presentation is no longer detached", !manager.isDetached(floater));
    ensure_equals("detached x remains persisted", settings.getLLSD("UILayout")["demo"]["position"][0].asReal(),
                  detachedLayout["position"][0].asReal());
    ensure_equals("detached y remains persisted", settings.getLLSD("UILayout")["demo"]["position"][1].asReal(),
                  detachedLayout["position"][1].asReal());
    ensure("detached marker remains persisted", settings.getLLSD("UILayout")["demo"]["detached"].asBoolean());
}
} // namespace tut
