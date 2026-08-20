/**
 * @file runtimewindowadapter_test.cpp
 * @brief Tests the viewer-window seam used by the UI runtime.
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
#include <string>
#include <utility>
#include "llwindowheadless.h"
#include "runtimewindowadapter.h"

namespace {
using radia::ui::Vec2;
using radia::viewer::ui::RuntimeWindowAdapter;
using radia::viewer::ui::detail::scaleLogicalPoint;
using radia::viewer::ui::detail::unscaleNativePoint;

class RuntimeWindowAdapterTest : public ::testing::Test {
protected:
    struct AuxiliaryWindows final : AuxiliaryWindowFactory {
        std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect&, const std::string&, AuxiliaryWindowClient&) override { return {}; }

        bool placementVisible(const AuxiliaryWindowRect& rect) const override {
            lastPlacement = rect;
            if (!visible) return false;
            if (!bounded) return true;
            return rect.x >= workArea.x
                && rect.y >= workArea.y
                && rect.x + rect.width <= workArea.x + workArea.width
                && rect.y + rect.height <= workArea.y + workArea.height;
        }

        mutable AuxiliaryWindowRect lastPlacement;
        AuxiliaryWindowRect workArea{0, 0, 816, 638};
        bool visible = true;
        bool bounded = false;
    } auxiliaryWindows;

    class ClientInsetWindow final : public LLWindowHeadless {
    public:
        using LLWindowHeadless::LLWindowHeadless;

        bool convertCoords(LLCoordGL from, LLCoordScreen* to) override {
            if (!to) return false;
            to->mX = 8 + from.mX;
            to->mY = 30 + 600 - from.mY - 1;
            return true;
        }

        bool convertCoords(LLCoordScreen from, LLCoordGL* to) override {
            if (!to) return false;
            to->mX = from.mX - 8;
            to->mY = 30 + 600 - from.mY - 1;
            return true;
        }
    };

    LLWindow* window = nullptr;
    int clears = 0;
    RuntimeWindowAdapter adapter{window, auxiliaryWindows, [] { return Vec2{2.f, 2.f}; }, [] { return std::pair{800, 600}; }, [this] { ++clears; }};
};

TEST_F(RuntimeWindowAdapterTest, HandlesMissingMainWindow) {
    EXPECT_EQ(adapter.mainRectToNative({1.f, 2.f, 3.f, 4.f}).width, 0);
    EXPECT_FLOAT_EQ(adapter.nativeScaleMultiplier(), 1.f);
    EXPECT_FALSE(adapter.nativePointInsideMain({10.f, 10.f}));
    EXPECT_FALSE(adapter.releasePointerForDetach({10.f, 10.f}));
    EXPECT_EQ(clears, 1);

    adapter.setMouseClipping(true);
}

TEST_F(RuntimeWindowAdapterTest, DelegatesPlacementVisibility) {
    const AuxiliaryWindowRect placement{10, 20, 300, 200};

    EXPECT_TRUE(adapter.placementVisible(placement));
    EXPECT_EQ(auxiliaryWindows.lastPlacement.x, placement.x);
    EXPECT_EQ(auxiliaryWindows.lastPlacement.width, placement.width);
}

TEST_F(RuntimeWindowAdapterTest, ScalesAndUnscalesLogicalPoints) {
    const Vec2 scale{2.f, 2.f};
    const Vec2 native = scaleLogicalPoint({10.f, 15.f}, scale);
    EXPECT_FLOAT_EQ(native.x, 20.f);
    EXPECT_FLOAT_EQ(native.y, 30.f);

    const Vec2 logical = unscaleNativePoint(native, scale);
    EXPECT_FLOAT_EQ(logical.x, 10.f);
    EXPECT_FLOAT_EQ(logical.y, 15.f);
}

TEST_F(RuntimeWindowAdapterTest, ChecksLeftEdgeDisplaySpace) {
    auxiliaryWindows.visible = false;
    EXPECT_FALSE(adapter.hasDisplaySpaceBeyondEdge({0.f, 300.f}, {-1.f, 0.f}));
    EXPECT_EQ(auxiliaryWindows.lastPlacement.x, -1);
    EXPECT_EQ(auxiliaryWindows.lastPlacement.width, 1);

    auxiliaryWindows.visible = true;
    EXPECT_TRUE(adapter.hasDisplaySpaceBeyondEdge({0.f, 300.f}, {-1.f, 0.f}));
}

TEST_F(RuntimeWindowAdapterTest, ConvertsCoordinatesAndReleasesPointerForARealWindow) {
    ClientInsetWindow client{nullptr, "", "", 0, 0, 816, 638, 0, false, false, false, false, false};
    LLWindow* mainWindow = &client;
    RuntimeWindowAdapter nativeAdapter(
        mainWindow, auxiliaryWindows, [] { return Vec2{2.f, 2.f}; }, [] { return std::pair{800, 600}; }, [this] { ++clears; });

    const AuxiliaryWindowRect native = nativeAdapter.mainRectToNative({10.f, 20.f, 100.f, 50.f});
    EXPECT_EQ(native.x, 28);
    EXPECT_EQ(native.y, 489);
    EXPECT_EQ(native.width, 200);
    EXPECT_EQ(native.height, 100);
    EXPECT_FLOAT_EQ(nativeAdapter.nativeScaleMultiplier(), 2.f);

    const Vec2 logicalBottomLeft = nativeAdapter.nativeBottomLeftInMain(native);
    EXPECT_FLOAT_EQ(logicalBottomLeft.x, 10.f);
    EXPECT_FLOAT_EQ(logicalBottomLeft.y, 20.f);
    EXPECT_TRUE(nativeAdapter.nativePointInsideMain({100.f, 500.f}));
    EXPECT_FALSE(nativeAdapter.nativePointInsideMain({0.f, 500.f}));

    const std::optional<AuxiliaryScreenPoint> screenPoint = nativeAdapter.releasePointerForDetach({10.f, 20.f});
    ASSERT_TRUE(screenPoint.has_value());
    EXPECT_EQ(screenPoint->x, 28);
    EXPECT_EQ(screenPoint->y, 589);
    EXPECT_EQ(clears, 1);
    nativeAdapter.setMouseClipping(true);
}

TEST_F(RuntimeWindowAdapterTest, UsesOuterNativeWindowBoundsForVerticalEdgeProbes) {
    ClientInsetWindow client{nullptr, "", "", 0, 0, 816, 638, 0, false, false, false, false, false};
    LLWindow* mainWindow = &client;
    const RuntimeWindowAdapter nativeAdapter(mainWindow, auxiliaryWindows, [] { return Vec2{1.f, 1.f}; }, [] { return std::pair{800, 600}; }, {});

    auxiliaryWindows.visible = true;
    auxiliaryWindows.bounded = true;
    EXPECT_FALSE(nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 599.f}, {0.f, 1.f}));
    EXPECT_EQ(auxiliaryWindows.lastPlacement.y, -1);

    EXPECT_FALSE(nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 0.f}, {0.f, -1.f}));
    EXPECT_EQ(auxiliaryWindows.lastPlacement.y, 638);

    auxiliaryWindows.workArea.height = 1000;
    EXPECT_TRUE(nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 0.f}, {0.f, -1.f}));
}
} // namespace
