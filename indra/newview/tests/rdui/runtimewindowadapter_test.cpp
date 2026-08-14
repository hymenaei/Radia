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
#include "runtimewindowadapter.h"

namespace tut {

struct runtimeWindowAdapterData {
    struct AuxiliaryWindows final : AuxiliaryWindowFactory {
        std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect&, const std::string&, AuxiliaryWindowClient&) override { return {}; }

        bool placementVisible(const AuxiliaryWindowRect& rect) const override {
            lastPlacement = rect;
            return visible;
        }

        mutable AuxiliaryWindowRect lastPlacement;
        bool visible = true;
    } auxiliaryWindows;

    LLWindow* window = nullptr;
    int clears = 0;
    rdui::viewer::RuntimeWindowAdapter adapter{window, auxiliaryWindows, [] { return rdui::Vec2{2.f, 2.f}; }, [] { return std::pair{800, 600}; },
                                               [this] { ++clears; }};
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
    const rdui::Vec2 scale{2.f, 2.f};
    const rdui::Vec2 native = rdui::viewer::detail::scaleLogicalPoint({10.f, 15.f}, scale);
    ensure_equals("logical x is scaled once", native.x, 20.f);
    ensure_equals("logical y is scaled once", native.y, 30.f);
    const rdui::Vec2 logical = rdui::viewer::detail::unscaleNativePoint(native, scale);
    ensure_equals("native x converts back to logical coordinates", logical.x, 10.f);
    ensure_equals("native y converts back to logical coordinates", logical.y, 15.f);
}
} // namespace tut
