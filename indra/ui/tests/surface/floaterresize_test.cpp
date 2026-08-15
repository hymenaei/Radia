/**
 * @file floaterresize_test.cpp
 * @brief Tests floater resize edge detection and constrained geometry.
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
#include <array>
#include <utility>
#include "../test/lltut.h"
#include "surface/floaterresize.h"

namespace {
using radia::ui::CursorStyle;
using radia::ui::Rect;
using radia::ui::Vec2;
using radia::ui::detail::FloaterResizeConstraints;
using radia::ui::detail::preserveUserResizeOnReload;
using radia::ui::detail::resizeCursor;
using radia::ui::detail::resizedRect;
using radia::ui::detail::ResizeEdges;
using radia::ui::detail::resizeEdgesAt;
} // namespace

namespace tut {
struct floaterResizeData {};
using floaterResizeTest = test_group<floaterResizeData>;
using floaterResizeObject = floaterResizeTest::object;
floaterResizeTest floaterResizeTestCase("floaterresize");

template<> template<> void floaterResizeObject::test<1>() {
    const Rect bounds{20.f, 30.f, 100.f, 80.f};
    const std::array<std::pair<Vec2, ResizeEdges>, 8> regions{{
        {{21.f, 70.f}, ResizeEdges::Left},
        {{119.f, 70.f}, ResizeEdges::Right},
        {{70.f, 31.f}, ResizeEdges::Bottom},
        {{70.f, 109.f}, ResizeEdges::Top},
        {{22.f, 32.f}, ResizeEdges::Left | ResizeEdges::Bottom},
        {{22.f, 108.f}, ResizeEdges::Left | ResizeEdges::Top},
        {{118.f, 32.f}, ResizeEdges::Right | ResizeEdges::Bottom},
        {{118.f, 108.f}, ResizeEdges::Right | ResizeEdges::Top},
    }};
    for (const auto& [point, expected] : regions) ensure("all edge and corner regions classify", resizeEdgesAt(bounds, point) == expected);
    ensure("interior is not resizeable", resizeEdgesAt(bounds, {70.f, 70.f}) == ResizeEdges::NoEdges);
    ensure("outside is not resizeable", resizeEdgesAt(bounds, {19.f, 70.f}) == ResizeEdges::NoEdges);
}

template<> template<> void floaterResizeObject::test<2>() {
    const Rect initial{20.f, 30.f, 100.f, 80.f};
    const FloaterResizeConstraints constraints{{40.f, 35.f}, Rect{0.f, 0.f, 200.f, 160.f}};

    const Rect left = resizedRect(initial, {20.f, 70.f}, {115.f, 70.f}, ResizeEdges::Left, constraints);
    ensure_equals("left resize holds right edge", left.right(), initial.right());
    ensure_equals("left resize respects minimum", left.w, 40.f);

    const Rect corner = resizedRect(initial, {120.f, 110.f}, {250.f, 250.f}, ResizeEdges::Right | ResizeEdges::Top, constraints);
    ensure_equals("right resize respects Surface", corner.right(), 200.f);
    ensure_equals("top resize respects Surface", corner.top(), 160.f);
    ensure_equals("corner holds left edge", corner.left(), initial.left());
    ensure_equals("corner holds bottom edge", corner.bottom(), initial.bottom());
}

template<> template<> void floaterResizeObject::test<3>() {
    ensure("left-bottom cursor", resizeCursor(ResizeEdges::Left | ResizeEdges::Bottom) == CursorStyle::NortheastSouthwestResize);
    ensure("left-top cursor", resizeCursor(ResizeEdges::Left | ResizeEdges::Top) == CursorStyle::NorthwestSoutheastResize);
    ensure("horizontal cursor", resizeCursor(ResizeEdges::Right) == CursorStyle::EastWestResize);
    ensure("vertical cursor", resizeCursor(ResizeEdges::Top) == CursorStyle::NorthSouthResize);
}

template<> template<> void floaterResizeObject::test<4>() {
    ensure("user resize survives a reload with unchanged authored size",
           preserveUserResizeOnReload(true, true, {{300.f, 240.f}, {280.f, 200.f}}, {{300.f, 240.f}, {280.f, 200.f}}));
    ensure("authored width change discards the user resize",
           !preserveUserResizeOnReload(true, true, {{300.f, 240.f}, {280.f, 200.f}}, {{320.f, 240.f}, {280.f, 200.f}}));
    ensure("authored height change discards the user resize",
           !preserveUserResizeOnReload(true, true, {{300.f, 240.f}, {280.f, 200.f}}, {{300.f, 260.f}, {280.f, 200.f}}));
    ensure("content height change discards the user resize even when an outer minimum masks it",
           !preserveUserResizeOnReload(true, true, {{300.f, 240.f}, {280.f, 200.f}}, {{300.f, 240.f}, {280.f, 182.f}}));
    ensure("resizability changes discard the user resize",
           !preserveUserResizeOnReload(true, false, {{300.f, 240.f}, {280.f, 200.f}}, {{300.f, 240.f}, {280.f, 200.f}}));
}
} // namespace tut
