/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/FloaterResize.h>
#include <array>
#include <gtest/gtest.h>

namespace {
using Core::detail::FloaterAuthoredGeometry;
using Core::detail::FloaterResizeConstraints;
using Core::detail::preserveUserResizeOnReload;
using Core::detail::resizeCursor;
using Core::detail::resizedRect;
using Core::detail::ResizeEdges;
using Core::detail::resizeEdgesAt;
using Core::Layout::Rect;
using Core::Layout::Vec2;
using Core::Style::CursorStyle;
using ::testing::Message;
} // namespace

TEST(FloaterResize, ClassifiesResizeEdges) {
    const Rect bounds {20.f, 30.f, 100.f, 80.f};
    struct RegionCase {
        const char* name;
        Vec2 point;
        ResizeEdges expected;
    };
    const std::array cases {
        RegionCase {"left edge", {21.f, 70.f}, ResizeEdges::Left},
        RegionCase {"right edge", {119.f, 70.f}, ResizeEdges::Right},
        RegionCase {"bottom edge", {70.f, 31.f}, ResizeEdges::Bottom},
        RegionCase {"top edge", {70.f, 109.f}, ResizeEdges::Top},
        RegionCase {"left bottom corner", {22.f, 32.f}, ResizeEdges::Left | ResizeEdges::Bottom},
        RegionCase {"left top corner", {22.f, 108.f}, ResizeEdges::Left | ResizeEdges::Top},
        RegionCase {"right bottom corner", {118.f, 32.f}, ResizeEdges::Right | ResizeEdges::Bottom},
        RegionCase {"right top corner", {118.f, 108.f}, ResizeEdges::Right | ResizeEdges::Top},
        RegionCase {"interior", {70.f, 70.f}, ResizeEdges::NoEdges},
        RegionCase {"outside", {19.f, 70.f}, ResizeEdges::NoEdges},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "resize region: " << test.name);
        EXPECT_EQ(resizeEdgesAt(bounds, test.point), test.expected);
    }
}

TEST(FloaterResize, RespectsResizeConstraints) {
    const Rect initial {20.f, 30.f, 100.f, 80.f};
    const FloaterResizeConstraints constraints {{40.f, 35.f}, Rect {0.f, 0.f, 200.f, 160.f}};

    const Rect resized = resizedRect(initial, {20.f, 70.f}, {115.f, 70.f}, ResizeEdges::Left, constraints);

    EXPECT_FLOAT_EQ(resized.right(), initial.right());
    EXPECT_FLOAT_EQ(resized.w, constraints.minimum.x);
    EXPECT_FLOAT_EQ(resized.h, initial.h);
}

TEST(FloaterResize, ClampsCornerResizeToSurfaceBounds) {
    const Rect initial {20.f, 30.f, 100.f, 80.f};
    const Rect surface {0.f, 0.f, 200.f, 160.f};
    const FloaterResizeConstraints constraints {{40.f, 35.f}, surface};

    const Rect resized = resizedRect(initial, {120.f, 110.f}, {250.f, 250.f}, ResizeEdges::Right | ResizeEdges::Top, constraints);

    EXPECT_FLOAT_EQ(resized.right(), surface.right());
    EXPECT_FLOAT_EQ(resized.top(), surface.top());
    EXPECT_FLOAT_EQ(resized.left(), initial.left());
    EXPECT_FLOAT_EQ(resized.bottom(), initial.bottom());
}

TEST(FloaterResize, MapsResizeEdgesToCursorStyles) {
    struct CursorCase {
        const char* name;
        ResizeEdges edges;
        CursorStyle expected;
    };
    const std::array cases {
        CursorCase {"left bottom diagonal", ResizeEdges::Left | ResizeEdges::Bottom, Core::Style::CursorStyle::NortheastSouthwestResize},
        CursorCase {"right top diagonal", ResizeEdges::Right | ResizeEdges::Top, Core::Style::CursorStyle::NortheastSouthwestResize},
        CursorCase {"left top diagonal", ResizeEdges::Left | ResizeEdges::Top, Core::Style::CursorStyle::NorthwestSoutheastResize},
        CursorCase {"right bottom diagonal", ResizeEdges::Right | ResizeEdges::Bottom, Core::Style::CursorStyle::NorthwestSoutheastResize},
        CursorCase {"left horizontal", ResizeEdges::Left, Core::Style::CursorStyle::EastWestResize},
        CursorCase {"right horizontal", ResizeEdges::Right, Core::Style::CursorStyle::EastWestResize},
        CursorCase {"bottom vertical", ResizeEdges::Bottom, Core::Style::CursorStyle::NorthSouthResize},
        CursorCase {"top vertical", ResizeEdges::Top, Core::Style::CursorStyle::NorthSouthResize},
        CursorCase {"no edges", ResizeEdges::NoEdges, Core::Style::CursorStyle::Auto},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "cursor case: " << test.name);
        EXPECT_EQ(resizeCursor(test.edges), test.expected);
    }
}

TEST(FloaterResize, PreservesEquivalentResize) {
    const FloaterAuthoredGeometry current {{300.f, 240.f}, {280.f, 200.f}};
    struct ReloadCase {
        const char* name;
        bool currentResizable;
        bool replacementResizable;
        FloaterAuthoredGeometry replacement;
        bool expected;
    };
    const std::array cases {
        ReloadCase {"unchanged geometry", true, true, current, true},
        ReloadCase {"outer width changed", true, true, {{320.f, 240.f}, {280.f, 200.f}}, false},
        ReloadCase {"outer height changed", true, true, {{300.f, 260.f}, {280.f, 200.f}}, false},
        ReloadCase {"content width changed", true, true, {{300.f, 240.f}, {282.f, 200.f}}, false},
        ReloadCase {"content height changed", true, true, {{300.f, 240.f}, {280.f, 182.f}}, false},
        ReloadCase {"difference within tolerance", true, true, {{300.49f, 240.f}, {280.f, 200.f}}, true},
        ReloadCase {"difference at tolerance", true, true, {{300.5f, 240.f}, {280.f, 200.f}}, false},
        ReloadCase {"replacement is not resizable", true, false, current, false},
        ReloadCase {"current is not resizable", false, true, current, false},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "reload case: " << test.name);
        EXPECT_EQ(preserveUserResizeOnReload(test.currentResizable, test.replacementResizable, current, test.replacement), test.expected);
    }
}
