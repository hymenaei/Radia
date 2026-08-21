/**
 * @file floaters_test.cpp
 * @brief Tests Surface floater mounting, ordering, and lifecycle behavior.
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
#include <algorithm>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include "binding/binder.h"
#include "render/recordingpaintcontext.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"

namespace {
using radia::ui::Button;
using radia::ui::CursorStyle;
using radia::ui::Floater;
using radia::ui::Label;
using radia::ui::PointerButton;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::SurfaceFloaterDelegate;
using radia::ui::SurfaceLayer;
using radia::ui::Vec2;
} // namespace

namespace {
class FloaterDelegateProbe final : public SurfaceFloaterDelegate {
public:
    void floaterClosed(Surface&, Floater&) override { ++closes; }
    void floaterMinimizedChanged(Surface&, Floater&, bool) override { ++minimizeChanges; }
    void floaterMoved(Surface&, Floater&) override { ++moves; }
    void floaterMoveEnded(Surface&, Floater&) override { ++moveCompletions; }
    void floaterResized(Surface&, Floater&, bool complete) override {
        ++resizeChanges;
        if (complete) ++resizeCompletions;
    }

    int closes = 0;
    int minimizeChanges = 0;
    int moves = 0;
    int moveCompletions = 0;
    int resizeChanges = 0;
    int resizeCompletions = 0;
};

TEST(FloatersTest, ReportsFloaterVisibilityFromResolvedDisplayAndVisibility) {
    StyleSheet styleSheet;
    constexpr char kVisibility[] = ".hidden { visibility: hidden; } .none { display: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibility).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 100.f);

    auto visible = std::make_unique<Floater>();
    surface.mountFloater(std::move(visible));
    EXPECT_TRUE(surface.hasVisibleFloater());

    auto hidden = std::make_unique<Floater>();
    hidden->addClass("hidden");
    surface.mountFloater(std::move(hidden));
    auto none = std::make_unique<Floater>();
    none->addClass("none");
    surface.mountFloater(std::move(none));
    EXPECT_TRUE(surface.hasVisibleFloater());

    surface.clearLayer(SurfaceLayer::Floater);
    EXPECT_FALSE(surface.hasVisibleFloater());
}

TEST(FloatersTest, RestoresFloaterWithinViewportAfterMinimization) {
    StyleSheet styleSheet;
    constexpr char kMinimizedFloaterStyle[] = "floater { display: flex; flex-direction: column; } "
                                              "floater::header { height: 30px; display: flex; flex-direction: row; } "
                                              "floater::content { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMinimizedFloaterStyle).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setTitle("title").setCanClose(false).setCanMinimize(true);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    target->setMinimized(true);
    surface.updateLayout();
    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    EXPECT_TRUE(surface.pointerDown({dragStart, PointerButton::Left}));
    surface.pointerMove({{199.f, dragStart.y}, PointerButton::Left});
    surface.pointerUp({{199.f, dragStart.y}, PointerButton::Left});
    const float minimizedLeft = target->rect().left();

    target->setMinimized(false);
    EXPECT_EQ(target->rect().w, 100.f);
    EXPECT_EQ(target->rect().right(), 200.f);
    EXPECT_TRUE(target->rect().left() < minimizedLeft);
}

TEST(FloatersTest, TransfersFloaterBetweenSurfacesAndReportsLifecycle) {
    Surface first;
    Surface second;
    FloaterDelegateProbe firstDelegate;
    FloaterDelegateProbe secondDelegate;
    first.setFloaterDelegate(&firstDelegate);
    second.setFloaterDelegate(&secondDelegate);
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setTitle("title").setCanMinimize(true);
    floater->setRect({90.f, 90.f, 30.f, 30.f});
    first.mountFloater(std::move(floater));
    EXPECT_EQ(target->rect().right(), 100.f);
    EXPECT_EQ(target->rect().top(), 100.f);
    EXPECT_EQ(firstDelegate.moves, 1);

    std::unique_ptr<Floater> transferred = first.unmountFloater(*target);
    ASSERT_TRUE(transferred);
    EXPECT_EQ(transferred.get(), target);
    second.mountFloater(std::move(transferred));
    EXPECT_EQ(target->rect().right(), 80.f);
    EXPECT_EQ(target->rect().top(), 60.f);
    EXPECT_EQ(secondDelegate.moves, 1);

    target->setMinimized(true);
    EXPECT_EQ(secondDelegate.minimizeChanges, 1);
    EXPECT_EQ(firstDelegate.minimizeChanges, 0);
    target->setMinimized(false);
    target->close();
    EXPECT_EQ(secondDelegate.closes, 1);
    EXPECT_EQ(firstDelegate.closes, 0);
}

TEST(FloatersTest, AvoidsSyntheticMoveBeforeViewportInitialization) {
    StyleSheet styleSheet;
    constexpr char kStartupFloaterLayout[] = "floater { width: 100px; display: flex; flex-direction: column; } "
                                             "floater::header { height: 30px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStartupFloaterLayout).ok());
    Surface surface(styleSheet);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanClose(false);
    surface.mountFloater(std::move(floater));

    surface.updateLayout();
    EXPECT_EQ(target->rect().x, 0.f);
    EXPECT_EQ(target->rect().y, 0.f);
    surface.setViewport(200.f, 100.f);
    EXPECT_EQ(delegate.moves, 0);
}

TEST(FloatersTest, ResizesAttachedFloaterWithinSurfaceBounds) {
    StyleSheet styleSheet;
    constexpr char kResize[] = "floater { size: 80px 100px; min-size: 40px 50px; display: flex; flex-direction: column; } "
                               "floater::header { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kResize).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ASSERT_TRUE(prepared.has_value());
    surface.placeFloater(*target, *prepared);
    surface.updateLayout();

    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::EastWestResize);
    EXPECT_TRUE(surface.pointerDown({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}, PointerButton::Left}));
    surface.pointerMove({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    EXPECT_EQ(target->rect().right(), 200.f);
    surface.pointerUp({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    EXPECT_EQ(delegate.resizeCompletions, 1);
}

TEST(FloatersTest, HidesResizeCursorWhenResizingIsUnavailable) {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(false).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    surface.pointerMove({{119.f, 60.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
    target->setCanResize(true).setCanMinimize(true).setMinimized(true);
    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 2.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
}

TEST(FloatersTest, UsesFrozenWidthForPercentageMinimumSize) {
    StyleSheet styleSheet;
    constexpr char kPercentageMinimumLayout[] = "floater { size: 80px 100px; min-size: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageMinimumLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ASSERT_TRUE(prepared.has_value());
    surface.placeFloater(*target, *prepared);

    const Vec2 leftEdge{target->rect().left() + 1.f, target->rect().bottom() + 30.f};
    surface.pointerDown({leftEdge, PointerButton::Left});
    surface.pointerMove({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    surface.pointerUp({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    EXPECT_EQ(target->rect().w, 50.f);
}

TEST(FloatersTest, ResizesFloatersMountedInModalLayer) {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater), SurfaceLayer::Modal);

    EXPECT_TRUE(surface.pointerDown({{119.f, 60.f}, PointerButton::Left}));
    surface.pointerMove({{159.f, 60.f}, PointerButton::Left});
    surface.pointerUp({{159.f, 60.f}, PointerButton::Left});
    EXPECT_EQ(target->rect().w, 140.f);
}

TEST(FloatersTest, KeepsFixedOuterSizeWhileTrackingContentGeometry) {
    StyleSheet styleSheet;
    constexpr char kFixedOuter[] = "floater { size: 300px; display: flex; flex-direction: column; } "
                                   "floater::header { height: 20px; } floater::content { display: flex; flex-direction: column; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFixedOuter).ok());
    Surface surface(styleSheet);
    surface.setViewport(500.f, 400.f);
    auto floater = std::make_unique<Floater>();
    floater->addChild(std::make_unique<Label>("first"));

    const std::optional<Rect> firstPrepared = surface.prepareFloater(*floater);
    ASSERT_TRUE(firstPrepared.has_value());
    const Rect firstOuter = *firstPrepared;
    const Vec2 firstContent = floater->authoredContentSize();
    floater->addChild(std::make_unique<Label>("second"));
    const std::optional<Rect> secondPrepared = surface.prepareFloater(*floater);
    ASSERT_TRUE(secondPrepared.has_value());
    const Rect secondOuter = *secondPrepared;
    const Vec2 secondContent = floater->authoredContentSize();

    EXPECT_EQ(firstOuter.x, 100.f);
    EXPECT_EQ(firstOuter.y, 50.f);
    EXPECT_EQ(firstOuter.h, secondOuter.h);
    EXPECT_TRUE(secondContent.y > firstContent.y);
}

TEST(FloatersTest, ResolvesPercentageGeometryAgainstViewport) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "floater { width: 50%; height: 25%; left: 10%; bottom: 10%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    Floater floater;
    const std::optional<Rect> prepared = surface.prepareFloater(floater);
    ASSERT_TRUE(prepared.has_value());
    const Rect rect = *prepared;
    EXPECT_NEAR(rect.w, 200.f, 6);
    EXPECT_NEAR(rect.h, 75.f, 6);
    EXPECT_NEAR(rect.x, 40.f, 6);
    EXPECT_NEAR(rect.y, 30.f, 6);
}

TEST(FloatersTest, RoutesResizeThroughPointerTransparentFloater) {
    StyleSheet styleSheet;
    constexpr char kPointerTransparentLayout[] = "floater.pass-through { pointer-events: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPointerTransparentLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    auto lower = std::make_unique<Floater>();
    Floater* lowerTarget = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));
    auto upper = std::make_unique<Floater>();
    Floater* upperTarget = upper.get();
    upper->setCanResize(false).setRect({70.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(upper));
    auto upperChild = std::make_unique<Button>();
    upperChild->setRect({0.f, 0.f, 100.f, 80.f}).setPointerEvents(true);
    upperTarget->content()->addChild(std::move(upperChild));

    const Vec2 lowerEdgeUnderUpper{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    EXPECT_TRUE(surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});

    upperTarget->addClass("pass-through");
    EXPECT_TRUE(surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});
}

TEST(FloatersTest, KeepsOverflowVisibleDescendantAsPointerTarget) {
    StyleSheet styleSheet;
    constexpr char kOverflowVisiblePassThroughStyle[] = "floater.pass-through { pointer-events: none; "
                                                        "overflow: visible; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOverflowVisiblePassThroughStyle).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);

    auto lower = std::make_unique<Floater>();
    Floater* lowerTarget = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));

    auto upper = std::make_unique<Floater>();
    Floater* upperTarget = upper.get();
    upper->addClass("pass-through").setRect({70.f, 20.f, 40.f, 80.f});
    surface.mountFloater(std::move(upper));

    auto overflowChild = std::make_unique<Button>();
    overflowChild->setRect({110.f, 20.f, 40.f, 80.f}).setPointerEvents(true);
    upperTarget->content()->addChild(std::move(overflowChild));

    const Vec2 point{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    EXPECT_TRUE(surface.pointerDown({point, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
}

TEST(FloatersTest, ReplacesFloaterAndReturnsRetiredRoot) {
    Surface surface;
    surface.setViewport(240.f, 180.f);
    auto original = std::make_unique<Floater>();
    Floater* originalPointer = original.get();
    original->setRect({20.f, 25.f, 100.f, 80.f});
    surface.mountFloater(std::move(original));

    auto replacement = std::make_unique<Floater>();
    Floater* replacementPointer = replacement.get();
    replacement->setRect({30.f, 35.f, 120.f, 90.f});
    std::unique_ptr<Floater> retired = surface.replaceFloater(*originalPointer, std::move(replacement));

    ASSERT_TRUE(retired);
    EXPECT_EQ(retired.get(), originalPointer);
    EXPECT_TRUE(surface.ownsFloater(*replacementPointer));
    EXPECT_EQ(retired->parent(), nullptr);
    EXPECT_NE(replacementPointer->parent(), nullptr);
}
} // namespace
