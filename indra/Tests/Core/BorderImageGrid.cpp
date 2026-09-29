/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include <Core/BorderImageGrid.h>
#include <Core/ComputedStyle.h>
#include <Core/LayoutGeometry.h>
#include <cmath>
#include <gtest/gtest.h>

namespace CoreTests {
using Core::Layout::BorderImageGridPiece;
using Core::Layout::borderImageTilePlan;
using Core::Layout::Rect;
using Core::Layout::resolveBorderImageArea;
using Core::Layout::resolveBorderImageGrid;
using Core::Style::BorderImage;
using Core::Style::BorderImageRepeatMode;
using Core::Style::BorderImageValueUnit;
using Core::Style::BorderImageWidthValue;
using Core::Style::ComputedStyle;
TEST(BorderImageGrid, ResolvesSlicesOutsetsAndWidths) {
    BorderImage borderImage;
    borderImage.slice.edges = {{10.f, false}, {20.f, false}, {30.f, false}, {40.f, false}};
    borderImage.slice.fill = true;
    borderImage.width = Core::Style::ComputedStyle::initialBorderImageWidth();
    borderImage.outset.edges = {{1.f, false}, {2.f, false}, {3.f, false}, {4.f, false}};

    const auto grid = resolveBorderImageGrid(borderImage.slice, borderImage.width, borderImage.outset, borderImage.repeat,
        {10.f, 20.f, 120.f, 60.f}, 100.f, 80.f, {4.f, 6.f, 8.f, 10.f});
    ASSERT_TRUE(grid);
    const auto area = resolveBorderImageArea({10.f, 20.f, 120.f, 60.f}, borderImage.outset, {4.f, 6.f, 8.f, 10.f});
    ASSERT_TRUE(area);
    EXPECT_FLOAT_EQ(area->x, 6.f);
    EXPECT_FLOAT_EQ(area->y, 17.f);
    EXPECT_FLOAT_EQ(area->w, 126.f);
    EXPECT_FLOAT_EQ(area->h, 64.f);
    EXPECT_FALSE(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Center)].source.empty());
    const Rect& topLeftSource = grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::TopLeft)].source;
    EXPECT_FLOAT_EQ(topLeftSource.x, 0.f);
    EXPECT_FLOAT_EQ(topLeftSource.y, 70.f);
    EXPECT_FLOAT_EQ(topLeftSource.w, 40.f);
    EXPECT_FLOAT_EQ(topLeftSource.h, 10.f);
    const Rect& topLeftDestination = grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::TopLeft)].destination;
    EXPECT_FLOAT_EQ(topLeftDestination.x, 6.f);
    EXPECT_FLOAT_EQ(topLeftDestination.y, 77.f);
    EXPECT_FLOAT_EQ(topLeftDestination.w, 10.f);
    EXPECT_FLOAT_EQ(topLeftDestination.h, 4.f);
    const Rect& centerSource = grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Center)].source;
    EXPECT_FLOAT_EQ(centerSource.x, 40.f);
    EXPECT_FLOAT_EQ(centerSource.y, 30.f);
    EXPECT_FLOAT_EQ(centerSource.w, 40.f);
    EXPECT_FLOAT_EQ(centerSource.h, 40.f);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Top)].tileSize.x, 16.f);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Center)].tileSize.y, 10.f);
}

TEST(BorderImageGrid, ScalesOverlappingWidthsTogether) {
    BorderImage borderImage;
    const BorderImageWidthValue width {70.f, BorderImageValueUnit::Length};
    borderImage.width.edges = {width, width, width, width};
    borderImage.slice.edges = {{10.f, false}, {10.f, false}, {10.f, false}, {10.f, false}};

    const auto grid = resolveBorderImageGrid(borderImage.slice, borderImage.width, borderImage.outset, borderImage.repeat,
        {0.f, 0.f, 100.f, 40.f}, 100.f, 100.f, {1.f, 1.f, 1.f, 1.f});
    ASSERT_TRUE(grid);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Left)].destination.w, 20.f);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Top)].destination.h, 20.f);
}

TEST(BorderImageGrid, PlansRepeatModes) {
    const auto stretch = borderImageTilePlan(BorderImageRepeatMode::Stretch, 5.f, 100.f, 30.f);
    ASSERT_TRUE(stretch);
    EXPECT_FLOAT_EQ(stretch->first, 5.f);
    EXPECT_FLOAT_EQ(stretch->size, 100.f);
    EXPECT_EQ(stretch->count, 1u);

    const auto round = borderImageTilePlan(BorderImageRepeatMode::Round, 0.f, 100.f, 30.f);
    ASSERT_TRUE(round);
    EXPECT_EQ(round->count, 3u);
    EXPECT_FLOAT_EQ(round->size, 100.f / 3.f);

    const auto space = borderImageTilePlan(BorderImageRepeatMode::Space, 0.f, 100.f, 30.f);
    ASSERT_TRUE(space);
    EXPECT_EQ(space->count, 3u);
    EXPECT_FLOAT_EQ(space->gap, 2.5f);
    EXPECT_FLOAT_EQ(space->position(0), 2.5f);
    EXPECT_FLOAT_EQ(space->position(2), 67.5f);

    const auto repeat = borderImageTilePlan(BorderImageRepeatMode::Repeat, 0.f, 96.f, 16.f);
    ASSERT_TRUE(repeat);
    EXPECT_EQ(repeat->count, 7u);
    EXPECT_FLOAT_EQ(repeat->position(0), -8.f);
    EXPECT_FLOAT_EQ(repeat->position(6), 88.f);
}

TEST(BorderImageGrid, ResolvesPercentWidthsAndEmptySlices) {
    BorderImage borderImage;
    borderImage.slice.edges = {{60.f, false}, {60.f, false}, {60.f, false}, {60.f, false}};
    borderImage.width.edges = {{10.f, BorderImageValueUnit::Percentage}, {10.f, BorderImageValueUnit::Percentage},
        {10.f, BorderImageValueUnit::Percentage}, {10.f, BorderImageValueUnit::Percentage}};

    const auto grid = resolveBorderImageGrid(borderImage.slice, borderImage.width, borderImage.outset, borderImage.repeat,
        {0.f, 0.f, 100.f, 40.f}, 100.f, 100.f, {1.f, 1.f, 1.f, 1.f});
    ASSERT_TRUE(grid);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Left)].destination.w, 10.f);
    EXPECT_FLOAT_EQ(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Top)].destination.h, 4.f);
    EXPECT_TRUE(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Top)].source.empty());
    EXPECT_TRUE(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::Center)].source.empty());
    EXPECT_FALSE(grid->pieces[static_cast<std::size_t>(BorderImageGridPiece::TopLeft)].source.empty());
}
} // namespace CoreTests
