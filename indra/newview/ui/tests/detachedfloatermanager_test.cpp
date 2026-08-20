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
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <utility>
#include "auxiliarywindow.h"
#include "componentpersistence.h"
#include "detachedfloatermanager.h"
#include "llcontrol.h"
#include "llsd.h"
#include "surface/surface.h"
#include "widgets/floater.h"

namespace {
using radia::ui::Floater;
using radia::ui::Rect;
using radia::ui::Surface;
using radia::ui::Vec2;
using radia::viewer::ui::ComponentKey;
using radia::viewer::ui::ComponentOpenState;
using radia::viewer::ui::ComponentPersistence;
using radia::viewer::ui::DetachedFloaterManager;
using radia::viewer::ui::DetachedFloaterPlacement;
using radia::viewer::ui::DetachedFloaterPresentation;
using radia::viewer::ui::DetachedFloaterPresentationOpenRequest;
using radia::viewer::ui::DetachedFloaterPresentationResult;
using radia::viewer::ui::DetachedFloaterPresentationUpdate;
using radia::viewer::ui::FloaterLogicalSize;
using radia::viewer::ui::FloaterPlacement;

class DetachedFloaterManagerTest : public ::testing::Test {
protected:
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
        Vec2 logicalSize{320.f, 240.f};
        Vec2 authoredSize{320.f, 240.f};
        Vec2 headerCenter{120.f, 220.f};
    };

    class FakePresentation final : public DetachedFloaterPresentation {
    public:
        FakePresentation(std::unique_ptr<Floater> floater, PresentationState& state) : mFloater(std::move(floater)), mState(state) {}

        std::optional<DetachedFloaterPresentationUpdate> open(const DetachedFloaterPresentationOpenRequest& request) override {
            mState.opened = true;
            mState.native = request.rect;
            if (request.logicalSize) mState.logicalSize = *request.logicalSize;
            if (!mState.openSucceeds) return std::nullopt;
            return DetachedFloaterPresentationUpdate{false, false, false, false, mState.native, mState.logicalSize, mState.headerCenter};
        }

        bool beginResize() override {
            mState.resizeStarted = true;
            return true;
        }

        void applyResize(const Rect& logicalRect) override { mState.logicalSize = {logicalRect.w, logicalRect.h}; }

        DetachedFloaterPresentationUpdate update() override {
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

        std::optional<Rect> prepareReplacement(Floater&) override {
            mState.replacementPrepared = true;
            if (!mState.replacementSucceeds) return std::nullopt;
            return Rect{0.f, 0.f, mState.authoredSize.x, mState.authoredSize.y};
        }

        std::unique_ptr<Floater> releaseFloater() override { return std::move(mFloater); }

        std::unique_ptr<Floater> replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize) override {
            std::unique_ptr<Floater> retired = std::move(mFloater);
            mFloater = std::move(replacement);
            if (logicalSize) mState.logicalSize = *logicalSize;
            return retired;
        }

    private:
        std::unique_ptr<Floater> mFloater;
        PresentationState& mState;
    };

    struct FakeEnvironment final : DetachedFloaterManager::DetachedFloaterEnvironment {
        AuxiliaryWindowRect mainRectToNative(const Rect& rect) const override {
            return {static_cast<S32>(rect.x), static_cast<S32>(rect.y), static_cast<S32>(rect.w), static_cast<S32>(rect.h)};
        }

        float nativeScaleMultiplier() const override { return 1.f; }
        Vec2 nativeBottomLeftInMain(const AuxiliaryWindowRect&) const override { return {15.f, 25.f}; }
        bool nativePointInsideMain(const Vec2&) const override { return pointInside; }
        bool placementVisible(const AuxiliaryWindowRect&) const override { return visiblePlacement; }

        std::optional<AuxiliaryScreenPoint> releasePointerForDetach(const Vec2& position) override {
            releasedPointer = true;
            releasedAt = position;
            return AuxiliaryScreenPoint{77, 88};
        }

        bool pointInside = false;
        bool visiblePlacement = true;
        bool releasedPointer = false;
        Vec2 releasedAt;
    };

    DetachedFloaterManagerTest()
        : settings("detached-floater-manager"), persistence(settings, settings),
          manager(
              surface,
              [this](std::unique_ptr<Floater> floater) {
                  if (shouldFailFactory) return DetachedFloaterPresentationResult::failure(std::move(floater));
                  return DetachedFloaterPresentationResult::success(std::make_unique<FakePresentation>(std::move(floater), presentation));
              },
              environment,
              [this](const ComponentKey& componentKey, FloaterPlacement placement, ComponentOpenState state) {
                  persistence.savePlacement(componentKey, std::move(placement), state);
              }) {
        settings.declareLLSD("UILayout", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        settings.declareLLSD("UIWorkspace", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        surface.setViewport(800.f, 600.f);
        persistence.restorePlacement(identity);
    }

    Floater& mountFloater() {
        auto floater = std::make_unique<Floater>();
        floater->setCanResize(true).setRect({30.f, 40.f, 320.f, 240.f});
        return surface.mountFloater(std::move(floater));
    }

    void detachFloater(Floater& floater, const Vec2& desired = {90.f, 110.f}, const Vec2& dragOffset = {}) {
        manager.requestDetach(identity, floater, desired, dragOffset);
        manager.processPendingDetachment();
    }

    LLControlGroup settings;
    ComponentPersistence persistence;
    ComponentKey identity{"demo", {}};
    Surface surface;
    PresentationState presentation;
    FakeEnvironment environment;
    bool shouldFailFactory = false;
    DetachedFloaterManager manager;
};

TEST_F(DetachedFloaterManagerTest, DetachTransfersOwnershipAndPersistsPlacement) {
    Floater& floater = mountFloater();
    detachFloater(floater, {90.f, 110.f}, {12.f, 8.f});

    EXPECT_TRUE(presentation.opened);
    EXPECT_TRUE(manager.isDetached(floater));
    EXPECT_TRUE(environment.releasedPointer);
    EXPECT_FLOAT_EQ(environment.releasedAt.x, 102.f);
    EXPECT_TRUE(settings.getLLSD("UILayout")["demo"]["detached"].asBoolean());
    EXPECT_TRUE(settings.getLLSD("UIWorkspace").has("demo"));
    EXPECT_FALSE(settings.getLLSD("UIWorkspace")["demo"].has("detached"));
}

TEST_F(DetachedFloaterManagerTest, DragEndingInsideMainWindowReattachesFloater) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    environment.pointInside = true;
    presentation.dragEnded = true;

    manager.update();

    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_FLOAT_EQ(floater.rect().x, 15.f);
    EXPECT_FLOAT_EQ(floater.rect().y, 25.f);
    EXPECT_FALSE(settings.getLLSD("UILayout")["demo"].has("detached"));
}

TEST_F(DetachedFloaterManagerTest, RejectsInvisibleSavedPlacement) {
    Floater& floater = mountFloater();
    environment.visiblePlacement = false;
    const DetachedFloaterPlacement placement{100, 200, FloaterLogicalSize{320.f, 240.f}, false};

    EXPECT_FALSE(manager.restoreDetachedPlacement(identity, floater, placement));
    EXPECT_FALSE(presentation.opened);
    EXPECT_FALSE(manager.isDetached(floater));
}

TEST_F(DetachedFloaterManagerTest, ReplacesDetachedFloaterInPlace) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    auto replacement = std::make_unique<Floater>();
    replacement->setCanResize(true).setRect({0.f, 0.f, 400.f, 300.f});

    const auto result = manager.replace(floater, std::move(replacement), Vec2{400.f, 300.f});
    Floater* mounted = result.installed();

    ASSERT_NE(mounted, nullptr);
    EXPECT_TRUE(manager.isDetached(*mounted));
    EXPECT_FLOAT_EQ(manager.logicalSize(*mounted).x, 400.f);
}

TEST_F(DetachedFloaterManagerTest, RestoresFloaterWhenPresentationOpenFails) {
    Floater& floater = mountFloater();
    presentation.openSucceeds = false;
    detachFloater(floater);

    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_FLOAT_EQ(floater.rect().x, 30.f);
    EXPECT_FLOAT_EQ(floater.rect().y, 40.f);
    EXPECT_FLOAT_EQ(floater.rect().w, 320.f);
    EXPECT_FLOAT_EQ(floater.rect().h, 240.f);
}

TEST_F(DetachedFloaterManagerTest, PersistsLogicalSizeAfterDetachedResize) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    presentation.logicalSize = {411.5f, 277.25f};
    presentation.resizeEnded = true;

    manager.update();

    const LLSD layout = settings.getLLSD("UILayout")["demo"];
    ASSERT_TRUE(layout["position"].isArray());
    ASSERT_TRUE(layout["size"].isArray());
    EXPECT_DOUBLE_EQ(layout["size"][0].asReal(), 411.5);
    EXPECT_DOUBLE_EQ(layout["size"][1].asReal(), 277.25);
}

TEST_F(DetachedFloaterManagerTest, PersistsDetachedPlacementWithoutMonitorIdentity) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    EXPECT_FALSE(settings.getLLSD("UILayout")["demo"].has("monitor"));

    auto replacement = std::make_unique<Floater>();
    replacement->setCanResize(false).setRect({0.f, 0.f, 292.f, 300.f});
    const auto result = manager.replace(floater, std::move(replacement), Vec2{292.f, 300.f});
    Floater* mounted = result.installed();

    ASSERT_NE(mounted, nullptr);
    EXPECT_TRUE(manager.isDetached(*mounted));
}

TEST_F(DetachedFloaterManagerTest, ConvertsLogicalResizeToNativeCoordinates) {
    const AuxiliaryWindowRect native = auxiliaryWindowRectForLogicalResize({100, 200, 300, 240}, {-10.5f, 5.25f, 220.5f, 150.25f}, 1.5f);

    EXPECT_EQ(native.x, 84);
    EXPECT_EQ(native.y, 207);
    EXPECT_EQ(native.width, 331);
    EXPECT_EQ(native.height, 225);
}

TEST_F(DetachedFloaterManagerTest, RoutesResizeRequestsToPresentation) {
    Floater& floater = mountFloater();
    detachFloater(floater);

    EXPECT_TRUE(manager.beginResize(floater));
    manager.applyResize(floater, {-10.f, 0.f, 410.5f, 280.25f});

    EXPECT_TRUE(presentation.resizeStarted);
    EXPECT_FLOAT_EQ(presentation.logicalSize.x, 410.5f);
    EXPECT_FLOAT_EQ(presentation.logicalSize.y, 280.25f);
}

TEST_F(DetachedFloaterManagerTest, TracksMultipleDetachedFloatersByIdentity) {
    const ComponentKey secondIdentity{"second", {}};
    persistence.restorePlacement(secondIdentity);
    Floater& first = mountFloater();
    Floater& second = mountFloater();

    detachFloater(first);
    manager.requestDetach(secondIdentity, second, {190.f, 210.f}, {});
    manager.processPendingDetachment();

    EXPECT_TRUE(manager.isDetached(first));
    EXPECT_TRUE(manager.isDetached(second));
    EXPECT_DOUBLE_EQ(settings.getLLSD("UILayout")["demo"]["position"][0].asReal(), 90.0);
    EXPECT_DOUBLE_EQ(settings.getLLSD("UILayout")["second"]["position"][0].asReal(), 190.0);
}

TEST_F(DetachedFloaterManagerTest, UpdatesEachDetachedPresentation) {
    Floater& floater = mountFloater();
    detachFloater(floater);

    EXPECT_EQ(presentation.ticks, 0);
    manager.update();

    EXPECT_EQ(presentation.ticks, 1);
    EXPECT_TRUE(manager.isDetached(floater));
}

TEST_F(DetachedFloaterManagerTest, PropagatesVisibilityToDetachedPresentations) {
    Floater& floater = mountFloater();
    detachFloater(floater);

    EXPECT_TRUE(presentation.visible);
    manager.setVisible(false);
    EXPECT_FALSE(presentation.visible);
    manager.setVisible(true);
    EXPECT_TRUE(presentation.visible);
}

TEST_F(DetachedFloaterManagerTest, CloseReattachesAndClearsMinimization) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.closeRequested = true;

    manager.update();

    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_FALSE(floater.minimized());
    EXPECT_FALSE(settings.getLLSD("UILayout")["demo"].has("detached"));
}

TEST_F(DetachedFloaterManagerTest, MinimizeReattachesAndPreservesMinimization) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    floater.setCanMinimize(true);
    floater.setMinimized(true);
    presentation.minimizeRequested = true;

    manager.update();

    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_TRUE(floater.minimized());
    EXPECT_TRUE(settings.getLLSD("UIWorkspace")["demo"]["minimized"].asBoolean());
    EXPECT_FALSE(settings.getLLSD("UILayout")["demo"].has("detached"));
}

TEST_F(DetachedFloaterManagerTest, ReturnsFloaterWhenPresentationCreationFails) {
    Floater& floater = mountFloater();
    shouldFailFactory = true;
    detachFloater(floater);

    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_TRUE(surface.ownsFloater(floater));
}

TEST_F(DetachedFloaterManagerTest, UsesPresentationGeometryForReplacement) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    presentation.authoredSize = {411.f, 277.f};
    auto replacement = std::make_unique<Floater>();

    const std::optional<Rect> authoredRect = manager.prepareReplacement(floater, *replacement);

    ASSERT_TRUE(authoredRect.has_value());
    EXPECT_TRUE(presentation.replacementPrepared);
    EXPECT_FLOAT_EQ(authoredRect->w, 411.f);
    EXPECT_FLOAT_EQ(authoredRect->h, 277.f);
}

TEST_F(DetachedFloaterManagerTest, PropagatesReplacementPreparationFailure) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    presentation.replacementSucceeds = false;
    auto replacement = std::make_unique<Floater>();

    EXPECT_FALSE(manager.prepareReplacement(floater, *replacement));
    EXPECT_TRUE(presentation.replacementPrepared);
}

TEST_F(DetachedFloaterManagerTest, ReattachAllPreservesDetachedPlacement) {
    Floater& floater = mountFloater();
    detachFloater(floater);
    const LLSD detachedLayout = settings.getLLSD("UILayout")["demo"];

    manager.reattachAll(DetachedFloaterManager::ReattachMode::PreservePlacement);

    EXPECT_TRUE(surface.ownsFloater(floater));
    EXPECT_FALSE(manager.isDetached(floater));
    EXPECT_DOUBLE_EQ(settings.getLLSD("UILayout")["demo"]["position"][0].asReal(), detachedLayout["position"][0].asReal());
    EXPECT_DOUBLE_EQ(settings.getLLSD("UILayout")["demo"]["position"][1].asReal(), detachedLayout["position"][1].asReal());
    EXPECT_TRUE(settings.getLLSD("UILayout")["demo"]["detached"].asBoolean());
}
} // namespace
