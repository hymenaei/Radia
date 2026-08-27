/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include "surface/scrollgeometry.h"

namespace {
using radia::ui::hitTestScrollbar;
using radia::ui::LayoutDirection;
using radia::ui::makeScrollGeometry;
using radia::ui::Rect;
using radia::ui::ScrollbarAxis;
using radia::ui::ScrollbarMode;
using radia::ui::ScrollbarPart;
using radia::ui::ScrollGeometryInput;
using radia::ui::scrollOffsetForThumbPosition;
using radia::ui::Vec2;

void expectRect(const Rect& actual, const Rect& expected) {
    EXPECT_FLOAT_EQ(actual.x, expected.x);
    EXPECT_FLOAT_EQ(actual.y, expected.y);
    EXPECT_FLOAT_EQ(actual.w, expected.w);
    EXPECT_FLOAT_EQ(actual.h, expected.h);
}
} // namespace

TEST(ScrollGeometryTest, ComputesClassicTracksArrowsThumbsAndCorner) {
    ScrollGeometryInput input;
    input.scrollport = {10.f, 20.f, 100.f, 80.f};
    input.horizontal = {10.f, 200.f, 100.f, true};
    input.vertical = {0.f, 160.f, 80.f, true};
    input.thickness = 12.f;
    input.arrowLength = 8.f;
    input.minimumThumbLength = 20.f;
    input.thumbPadding = 0.f;

    const auto geometry = makeScrollGeometry(input);
    ASSERT_TRUE(geometry.horizontal.visible);
    ASSERT_TRUE(geometry.vertical.visible);
    EXPECT_EQ(geometry.horizontal.axis, ScrollbarAxis::Horizontal);
    EXPECT_EQ(geometry.vertical.axis, ScrollbarAxis::Vertical);
    expectRect(geometry.vertical.bounds, {110.f, 20.f, 12.f, 80.f});
    expectRect(geometry.horizontal.bounds, {10.f, 8.f, 100.f, 12.f});
    expectRect(geometry.vertical.startArrow, {110.f, 92.f, 12.f, 8.f});
    expectRect(geometry.vertical.endArrow, {110.f, 20.f, 12.f, 8.f});
    expectRect(geometry.horizontal.startArrow, {10.f, 8.f, 8.f, 12.f});
    expectRect(geometry.horizontal.endArrow, {102.f, 8.f, 8.f, 12.f});
    expectRect(geometry.corner, {110.f, 8.f, 12.f, 12.f});
    EXPECT_TRUE(geometry.hasCorner);
    EXPECT_FLOAT_EQ(geometry.vertical.thumb.h, 32.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumbTravel, 32.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumb.top(), geometry.vertical.track.top());
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.vertical, geometry.vertical.track.top() - geometry.vertical.thumb.h, 0.f), 0.f);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.vertical, geometry.vertical.track.bottom(), 0.f), geometry.vertical.maxScrollOffset);

    EXPECT_EQ(hitTestScrollbar(geometry, {115.f, 24.f}).part, ScrollbarPart::EndArrow);
    EXPECT_EQ(hitTestScrollbar(geometry, {115.f, 45.f}).part, ScrollbarPart::Track);
    EXPECT_EQ(hitTestScrollbar(geometry, {115.f, 75.f}).part, ScrollbarPart::Thumb);
    EXPECT_EQ(hitTestScrollbar(geometry, {115.f, 95.f}).part, ScrollbarPart::StartArrow);
    EXPECT_EQ(hitTestScrollbar(geometry, {115.f, 10.f}).part, ScrollbarPart::Corner);
}

TEST(ScrollGeometryTest, ComputesOverlayBarsWithoutArrowsAndGivesCornerPrecedence) {
    ScrollGeometryInput input;
    input.scrollport = {0.f, 0.f, 100.f, 100.f};
    input.horizontal = {0.f, 200.f, 100.f, true};
    input.vertical = {0.f, 200.f, 100.f, true};
    input.mode = ScrollbarMode::Overlay;
    input.thickness = 10.f;
    input.arrowLength = 15.f;

    const auto geometry = makeScrollGeometry(input);
    expectRect(geometry.vertical.bounds, {90.f, 0.f, 10.f, 100.f});
    expectRect(geometry.horizontal.bounds, {0.f, 0.f, 100.f, 10.f});
    EXPECT_TRUE(geometry.vertical.startArrow.empty());
    EXPECT_TRUE(geometry.vertical.endArrow.empty());
    EXPECT_TRUE(geometry.horizontal.startArrow.empty());
    EXPECT_TRUE(geometry.horizontal.endArrow.empty());
    expectRect(geometry.corner, {90.f, 0.f, 10.f, 10.f});

    const auto corner = hitTestScrollbar(geometry, {95.f, 5.f});
    EXPECT_EQ(corner.axis, ScrollbarAxis::NoneValue);
    EXPECT_EQ(corner.part, ScrollbarPart::Corner);
    EXPECT_EQ(hitTestScrollbar(geometry, {95.f, 25.f}).part, ScrollbarPart::Track);
    EXPECT_EQ(hitTestScrollbar(geometry, {95.f, 80.f}).part, ScrollbarPart::Thumb);
}

TEST(ScrollGeometryTest, InsetsThumbInsideScrollbarTrack) {
    ScrollGeometryInput input;
    input.scrollport = {0.f, 0.f, 100.f, 100.f};
    input.horizontal = {0.f, 200.f, 100.f, true};
    input.vertical = {0.f, 200.f, 100.f, true};
    input.thickness = 15.f;
    input.arrowLength = 15.f;
    input.thumbPadding = 3.f;

    const auto geometry = makeScrollGeometry(input);
    EXPECT_FLOAT_EQ(geometry.vertical.thumb.x, geometry.vertical.bounds.x + 3.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumb.w, 9.f);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumb.y, geometry.horizontal.bounds.y + 3.f);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumb.h, 9.f);
    EXPECT_TRUE(geometry.vertical.bounds.contains({geometry.vertical.thumb.x - 1.f, geometry.vertical.thumb.y + 1.f}));
    EXPECT_FALSE(geometry.vertical.thumb.contains({geometry.vertical.bounds.x + 1.f, geometry.vertical.thumb.y + 1.f}));
}

TEST(ScrollGeometryTest, SeparatesThumbFromClassicButtons) {
    ScrollGeometryInput input;
    input.scrollport = {0.f, 0.f, 100.f, 100.f};
    input.horizontal = {0.f, 200.f, 100.f, true};
    input.vertical = {0.f, 200.f, 100.f, true};
    input.thickness = 15.f;
    input.arrowLength = 15.f;
    input.minimumThumbLength = 20.f;
    input.thumbPadding = 3.f;

    const auto geometry = makeScrollGeometry(input);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumbButtonGap, 3.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumbButtonGap, 3.f);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumb.left(), geometry.horizontal.track.left() + 3.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumb.top(), geometry.vertical.track.top() - 3.f);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumbTravel, 29.f);
    EXPECT_FLOAT_EQ(geometry.vertical.thumbTravel, 29.f);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.horizontal, geometry.horizontal.thumb.left(), 0.f), 0.f);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.vertical, geometry.vertical.thumb.top(), 0.f), 0.f);

    input.horizontal.scrollOffset = 100.f;
    input.vertical.scrollOffset = 100.f;
    const auto endGeometry = makeScrollGeometry(input);
    EXPECT_FLOAT_EQ(endGeometry.horizontal.thumb.right(), endGeometry.horizontal.track.right() - 3.f);
    EXPECT_FLOAT_EQ(endGeometry.vertical.thumb.bottom(), endGeometry.vertical.track.bottom() + 3.f);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(endGeometry.horizontal, endGeometry.horizontal.thumb.right(), 0.f),
                    endGeometry.horizontal.maxScrollOffset);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(endGeometry.vertical, endGeometry.vertical.thumb.bottom(), 0.f),
                    endGeometry.vertical.maxScrollOffset);
}

TEST(ScrollGeometryTest, MirrorsClassicPlacementAndThumbMovementForRtl) {
    ScrollGeometryInput input;
    input.scrollport = {20.f, 30.f, 100.f, 80.f};
    input.horizontal = {0.f, 200.f, 100.f, true};
    input.vertical = {0.f, 160.f, 80.f, true};
    input.direction = LayoutDirection::RightToLeft;
    input.thickness = 12.f;
    input.arrowLength = 10.f;
    input.thumbPadding = 0.f;

    const auto geometry = makeScrollGeometry(input);
    expectRect(geometry.vertical.bounds, {8.f, 30.f, 12.f, 80.f});
    expectRect(geometry.horizontal.bounds, {20.f, 18.f, 100.f, 12.f});
    EXPECT_TRUE(geometry.horizontal.reversed);
    EXPECT_FLOAT_EQ(geometry.horizontal.thumb.right(), geometry.horizontal.track.right());
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.horizontal, geometry.horizontal.track.left(), 0.f), geometry.horizontal.maxScrollOffset);
    EXPECT_FLOAT_EQ(scrollOffsetForThumbPosition(geometry.horizontal, geometry.horizontal.track.right(), 0.f), 0.f);
}
