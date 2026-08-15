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
#include "../test/lltut.h"
#include "llwindowheadless.h"
#include "runtimewindowadapter.h"

namespace tut {
using radia::ui::Vec2;
using radia::viewer::ui::RuntimeWindowAdapter;
using radia::viewer::ui::detail::scaleLogicalPoint;
using radia::viewer::ui::detail::unscaleNativePoint;

struct runtimeWindowAdapterData {
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
    };

    LLWindow* window = nullptr;
    int clears = 0;
    RuntimeWindowAdapter adapter{window, auxiliaryWindows, [] { return Vec2{2.f, 2.f}; }, [] { return std::pair{800, 600}; }, [this] { ++clears; }};
};
using runtimeWindowAdapterTest = test_group<runtimeWindowAdapterData>;
using runtimeWindowAdapterObject = runtimeWindowAdapterTest::object;
runtimeWindowAdapterTest runtimeWindowAdapterTestCase("UIRuntimeWindowAdapter");

template<> template<> void runtimeWindowAdapterObject::test<1>() {
    ensure("missing viewer window yields an empty native Rect", adapter.mainRectToNative({1.f, 2.f, 3.f, 4.f}).width == 0);
    ensure_equals("missing viewer window uses neutral scale", adapter.nativeScaleMultiplier(), 1.f);
    ensure("missing viewer window has no main-window hit area", !adapter.nativePointInsideMain({10.f, 10.f}));
    ensure("missing viewer window cannot publish a detach cursor", !adapter.releasePointerForDetach({10.f, 10.f}));
    ensure_equals("pointer release still clears runtime drag state", clears, 1);
    adapter.setMouseClipping(true);
}

template<> template<> void runtimeWindowAdapterObject::test<2>() {
    const AuxiliaryWindowRect placement{10, 20, 300, 200};
    ensure("placement visibility delegates to the native-window owner", adapter.placementVisible(placement));
    ensure_equals("delegated placement keeps x", auxiliaryWindows.lastPlacement.x, placement.x);
    ensure_equals("delegated placement keeps width", auxiliaryWindows.lastPlacement.width, placement.width);
}

template<> template<> void runtimeWindowAdapterObject::test<3>() {
    const Vec2 scale{2.f, 2.f};
    const Vec2 native = scaleLogicalPoint({10.f, 15.f}, scale);
    ensure_equals("logical x is scaled once", native.x, 20.f);
    ensure_equals("logical y is scaled once", native.y, 30.f);
    const Vec2 logical = unscaleNativePoint(native, scale);
    ensure_equals("native x converts back to logical coordinates", logical.x, 10.f);
    ensure_equals("native y converts back to logical coordinates", logical.y, 15.f);
}

template<> template<> void runtimeWindowAdapterObject::test<4>() {
    auxiliaryWindows.visible = false;
    ensure("missing native space blocks left-edge breakaway", !adapter.hasDisplaySpaceBeyondEdge({0.f, 300.f}, {-1.f, 0.f}));
    ensure_equals("left-edge probe is one native pixel outside the main window", auxiliaryWindows.lastPlacement.x, -1);
    ensure_equals("left-edge probe is one pixel wide", auxiliaryWindows.lastPlacement.width, 1);

    auxiliaryWindows.visible = true;
    ensure("native space permits left-edge breakaway", adapter.hasDisplaySpaceBeyondEdge({0.f, 300.f}, {-1.f, 0.f}));
}

template<> template<> void runtimeWindowAdapterObject::test<5>() {
    set_test_name("vertical edge probes use the outer native window bounds");
    ClientInsetWindow window{nullptr, "", "", 0, 0, 816, 638, 0, false, false, false, false, false};
    LLWindow* mainWindow = &window;
    const auto nativeAdapter =
        RuntimeWindowAdapter(mainWindow, auxiliaryWindows, [] { return Vec2{1.f, 1.f}; }, [] { return std::pair{800, 600}; }, {});

    auxiliaryWindows.visible = true;
    auxiliaryWindows.bounded = true;
    ensure("top boundary has no native space", !nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 599.f}, {0.f, 1.f}));
    ensure_equals("top probe reaches outside the outer window", auxiliaryWindows.lastPlacement.y, -1);

    ensure("bottom boundary has no native space", !nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 0.f}, {0.f, -1.f}));
    ensure_equals("bottom probe reaches outside the outer window", auxiliaryWindows.lastPlacement.y, 638);

    auxiliaryWindows.workArea.height = 1000;
    ensure("same-monitor space beyond a smaller main window permits breakaway", nativeAdapter.hasDisplaySpaceBeyondEdge({400.f, 0.f}, {0.f, -1.f}));
}
} // namespace tut
