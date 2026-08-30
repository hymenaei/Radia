/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <gtest/gtest.h>
#include "path.h"

namespace {
using radia::ui::compileSvgPathData;
using radia::ui::Path;
using radia::ui::PathCompileResult;
using radia::ui::PathVerb;
using radia::ui::Vec2;
using ::testing::Message;
}

TEST(PathTest, CompilesMixedAbsoluteAndRelativeCommandsIntoClosedContour) {
    const PathCompileResult compiled = compileSvgPathData("M 0 0 L 10 0 h 5 v 10 l -5 0 Z");
    ASSERT_TRUE(compiled.ok()) << "valid path compiles";
    const auto& commands = compiled.path->commands();
    ASSERT_EQ(commands.size(), std::size_t{6}) << "path command count";
    EXPECT_EQ(commands[0].verb, PathVerb::MoveTo);
    EXPECT_FLOAT_EQ(commands[0].p0.x, 0.f);
    EXPECT_FLOAT_EQ(commands[0].p0.y, 0.f);
    EXPECT_EQ(commands[1].verb, PathVerb::LineTo);
    EXPECT_FLOAT_EQ(commands[1].p0.x, 10.f);
    EXPECT_FLOAT_EQ(commands[1].p0.y, 0.f);
    EXPECT_EQ(commands[2].verb, PathVerb::LineTo);
    EXPECT_FLOAT_EQ(commands[2].p0.x, 15.f);
    EXPECT_FLOAT_EQ(commands[2].p0.y, 0.f);
    EXPECT_EQ(commands[3].verb, PathVerb::LineTo);
    EXPECT_FLOAT_EQ(commands[3].p0.x, 15.f);
    EXPECT_FLOAT_EQ(commands[3].p0.y, 10.f);
    EXPECT_EQ(commands[4].verb, PathVerb::LineTo);
    EXPECT_FLOAT_EQ(commands[4].p0.x, 10.f);
    EXPECT_FLOAT_EQ(commands[4].p0.y, 10.f);
    EXPECT_EQ(commands[5].verb, PathVerb::Close);

    const auto contours = compiled.path->flatten();
    ASSERT_EQ(contours.size(), std::size_t{1}) << "one closed contour";
    ASSERT_GE(contours.front().size(), std::size_t{2}) << "closed contour has endpoints";
    EXPECT_EQ(contours.front().front().x, contours.front().back().x) << "closed contour repeats start";
    EXPECT_EQ(contours.front().front().y, contours.front().back().y) << "closed contour repeats start";
}

TEST(PathTest, RefinesCubicFlatteningAsToleranceTightens) {
    Path curve;
    curve.moveTo(0.f, 0.f).cubicTo(0.f, 20.f, 20.f, 20.f, 20.f, 0.f);

    const auto adaptiveContours = curve.flatten(0.25f);
    const auto coarseContours = curve.flatten(4.f);
    const auto fineContours = curve.flatten(0.1f);
    ASSERT_EQ(adaptiveContours.size(), std::size_t{1}) << "adaptive flatten emits one contour";
    ASSERT_EQ(coarseContours.size(), std::size_t{1}) << "coarse flatten emits one contour";
    ASSERT_EQ(fineContours.size(), std::size_t{1}) << "fine flatten emits one contour";
    EXPECT_GT(adaptiveContours.front().size(), std::size_t{4}) << "adaptive flatten emits curve segments";
    EXPECT_LT(coarseContours.front().size(), fineContours.front().size()) << "looser tolerance emits fewer points";

    const auto& adaptiveCurve = adaptiveContours.front();
    EXPECT_FLOAT_EQ(adaptiveCurve.front().x, 0.f) << "curve keeps its start point";
    EXPECT_FLOAT_EQ(adaptiveCurve.front().y, 0.f) << "curve keeps its start point";
    EXPECT_FLOAT_EQ(adaptiveCurve.back().x, 20.f) << "curve keeps its end point";
    EXPECT_FLOAT_EQ(adaptiveCurve.back().y, 0.f) << "curve keeps its end point";

    float maximumY = adaptiveCurve.front().y;
    for (const Vec2& point : adaptiveCurve) maximumY = std::max(maximumY, point.y);
    EXPECT_NEAR(maximumY, 15.f, .25f) << "flattened curve follows its expected apex";
}

TEST(PathTest, CircleFlattensToClosedContourAtExpectedRadius) {
    const auto contours = Path::circle({11.f, 11.f}, 8.f).flatten();
    ASSERT_EQ(contours.size(), std::size_t{1}) << "circle produces one contour";
    ASSERT_GE(contours.front().size(), std::size_t{2}) << "circle contour has endpoints";

    const auto& contour = contours.front();
    EXPECT_FLOAT_EQ(contour.front().x, contour.back().x) << "circle is closed on x";
    EXPECT_FLOAT_EQ(contour.front().y, contour.back().y) << "circle is closed on y";
    for (std::size_t index = 0; index < contour.size(); ++index) {
        SCOPED_TRACE(Message() << "circle point index: " << index);
        const Vec2& point = contour[index];
        EXPECT_NEAR(std::hypot(point.x - 11.f, point.y - 11.f), 8.f, .001f) << "circle keeps its radius";
    }
}

TEST(PathTest, RejectsMalformedPathWithSourceLocation) {
    const PathCompileResult compiled = compileSvgPathData("M0 0 L10", "broken.svg", 7);
    EXPECT_FALSE(compiled.ok()) << "malformed path rejected";
    EXPECT_FALSE(compiled.path.has_value()) << "partial path never exposed";
    ASSERT_FALSE(compiled.errors.empty()) << "malformed path reports a diagnostic";

    const auto& diagnostic = compiled.errors.front();
    EXPECT_EQ(diagnostic.code, "svg.path.arguments_invalid") << "malformed path diagnostic";
    EXPECT_EQ(diagnostic.source, "broken.svg") << "diagnostic source retained";
    EXPECT_EQ(diagnostic.line, std::size_t{7}) << "diagnostic line retained";
}

TEST(PathTest, RejectsEmptyPathData) {
    const PathCompileResult compiled = compileSvgPathData("  ", "empty.svg", 3);
    EXPECT_FALSE(compiled.ok()) << "empty path rejected";
    EXPECT_FALSE(compiled.path.has_value()) << "empty path never exposed";
    ASSERT_FALSE(compiled.errors.empty()) << "empty path reports a diagnostic";

    const auto& diagnostic = compiled.errors.front();
    EXPECT_EQ(diagnostic.code, "svg.path.empty") << "empty path diagnostic";
    EXPECT_EQ(diagnostic.source, "empty.svg") << "diagnostic source retained";
    EXPECT_EQ(diagnostic.line, std::size_t{3}) << "diagnostic line retained";
}

TEST(PathTest, RejectsUnsupportedCommandWithDiagnostic) {
    const PathCompileResult compiled = compileSvgPathData("M0 0 A4 4 0 0 1 8 8");
    EXPECT_FALSE(compiled.ok()) << "unsupported command rejected";
    ASSERT_FALSE(compiled.errors.empty()) << "unsupported command reports a diagnostic";
    EXPECT_EQ(compiled.errors.front().code, "svg.path.command_unsupported") << "unsupported command diagnostic";
}
