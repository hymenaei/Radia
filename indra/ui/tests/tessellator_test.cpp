/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <gtest/gtest.h>
#include <limits>
#include "render/tessellator.h"

namespace {
using radia::ui::Color;
using radia::ui::Mesh;
using radia::ui::Path;
using radia::ui::StrokeCap;
using radia::ui::tessellateStroke;
} // namespace

namespace {
Path line(float x0 = 0.f, float x1 = 10.f) {
    Path path;
    return path.moveTo(x0, 0.f).lineTo(x1, 0.f);
}

float minX(const Mesh& mesh) {
    float result = std::numeric_limits<float>::max();
    for (const auto& vertex : mesh.vertices) result = std::min(result, vertex.position.x);
    return result;
}

float maxX(const Mesh& mesh) {
    float result = std::numeric_limits<float>::lowest();
    for (const auto& vertex : mesh.vertices) result = std::max(result, vertex.position.x);
    return result;
}

float minY(const Mesh& mesh) {
    float result = std::numeric_limits<float>::max();
    for (const auto& vertex : mesh.vertices) result = std::min(result, vertex.position.y);
    return result;
}

float maxY(const Mesh& mesh) {
    float result = std::numeric_limits<float>::lowest();
    for (const auto& vertex : mesh.vertices) result = std::max(result, vertex.position.y);
    return result;
}

float opaqueYSpan(const Mesh& mesh) {
    float minimum = std::numeric_limits<float>::max();
    float maximum = std::numeric_limits<float>::lowest();
    for (const auto& vertex : mesh.vertices) {
        if (vertex.color.a < .99f) continue;
        minimum = std::min(minimum, vertex.position.y);
        maximum = std::max(maximum, vertex.position.y);
    }
    return maximum - minimum;
}
} // namespace

TEST(TessellatorTest, ScalesOpaqueStrokeWidth) {
    const Color white;
    const Mesh one = tessellateStroke(line(), white, 1.f, 0.f);
    const Mesh two = tessellateStroke(line(), white, 2.f, 0.f);
    ASSERT_FALSE(one.empty());
    ASSERT_FALSE(two.empty());
    EXPECT_NEAR(opaqueYSpan(one), 1.f, 0.001f);
    EXPECT_NEAR(opaqueYSpan(two), 2.f, 0.001f);
}

TEST(TessellatorTest, AddsAntiAliasedFringeAroundStroke) {
    const Mesh mesh = tessellateStroke(line(), Color(), 2.f, 1.f);
    ASSERT_FALSE(mesh.empty());
    bool transparent = false;
    bool opaque = false;
    for (const auto& vertex : mesh.vertices) {
        transparent |= vertex.color.a == 0.f;
        opaque |= vertex.color.a == 1.f;
    }
    EXPECT_TRUE(opaque);
    EXPECT_TRUE(transparent);
    EXPECT_FLOAT_EQ(minY(mesh), -1.5f);
    EXPECT_FLOAT_EQ(maxY(mesh), 1.5f);
}

TEST(TessellatorTest, ExpandsOpenStrokeAccordingToCapStyle) {
    const Path path = line();
    const Mesh butt = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Butt);
    const Mesh square = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Square);
    const Mesh round = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Round);
    ASSERT_FALSE(butt.empty());
    ASSERT_FALSE(square.empty());
    ASSERT_FALSE(round.empty());
    EXPECT_NEAR(minX(butt), 0.f, 0.001f);
    EXPECT_FLOAT_EQ(minX(square), -1.f);
    EXPECT_FLOAT_EQ(maxX(square), 11.f);
    EXPECT_FLOAT_EQ(minX(round), -1.f);
    EXPECT_FLOAT_EQ(maxX(round), 11.f);
}

TEST(TessellatorTest, AdaptivelyTessellatesCubicCurve) {
    Path curve;
    curve.moveTo(0.f, 0.f).cubicTo(0.f, 20.f, 20.f, 20.f, 20.f, 0.f);
    const Mesh mesh = tessellateStroke(curve, Color(), 1.f, 1.f, StrokeCap::Round);
    ASSERT_FALSE(mesh.empty());

    const auto contours = curve.flatten();
    ASSERT_EQ(contours.size(), 1U);
    ASSERT_GE(contours.front().size(), 3U);
    EXPECT_NEAR(contours.front().front().x, 0.f, 0.001f);
    EXPECT_NEAR(contours.front().front().y, 0.f, 0.001f);
    EXPECT_NEAR(contours.front().back().x, 20.f, 0.001f);
    EXPECT_NEAR(contours.front().back().y, 0.f, 0.001f);

    float curveMaximumY = std::numeric_limits<float>::lowest();
    for (const auto& point : contours.front()) curveMaximumY = std::max(curveMaximumY, point.y);
    EXPECT_NEAR(curveMaximumY, 15.f, 0.1f);
    EXPECT_NEAR(minX(mesh), -1.f, 0.1f);
    EXPECT_NEAR(maxX(mesh), 21.f, 0.1f);
    EXPECT_NEAR(maxY(mesh), 16.f, 0.1f);
}

TEST(TessellatorTest, TessellatesClosedCircleContour) {
    const Path circle = Path::circle({0.f, 0.f}, 10.f);
    const auto contours = circle.flatten();
    ASSERT_EQ(contours.size(), 1U);
    ASSERT_GE(contours.front().size(), 24U);
    EXPECT_NEAR(contours.front().front().x, contours.front().back().x, 0.001f);
    EXPECT_NEAR(contours.front().front().y, contours.front().back().y, 0.001f);

    const Mesh mesh = tessellateStroke(circle, Color(), 2.f, 1.f);
    ASSERT_FALSE(mesh.empty());
    EXPECT_NEAR(minX(mesh), -11.5f, 0.1f);
    EXPECT_NEAR(maxX(mesh), 11.5f, 0.1f);
    EXPECT_NEAR(minY(mesh), -11.5f, 0.1f);
    EXPECT_NEAR(maxY(mesh), 11.5f, 0.1f);
}
