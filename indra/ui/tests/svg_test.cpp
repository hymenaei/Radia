/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cmath>
#include <cstddef>
#include <gtest/gtest.h>
#include "render/svg.h"
#include "render/tessellator.h"

namespace {
using radia::ui::compileSvgIcon;
using radia::ui::Mesh;
using radia::ui::Path;
using radia::ui::Rect;
using radia::ui::StrokeCap;
using radia::ui::SvgCompileResult;
using radia::ui::SvgIcon;
using radia::ui::tessellateStroke;
using radia::ui::transformSvgPath;
using radia::ui::Vertex;
using ::testing::Message;
} // namespace

TEST(SvgTest, CompilesPathsAndPreservesIconPresentationMetadata) {
    constexpr char kCrossIconSvg[] = "<svg viewBox=\"0 0 24 24\" stroke-width=\"2\" stroke-linecap=\"round\">"
                                     "<path d=\"M18 6 6 18\"/><path d=\"m6 6 12 12\"/></svg>";

    const SvgCompileResult compiled = compileSvgIcon(kCrossIconSvg);
    ASSERT_TRUE(compiled.ok());
    ASSERT_TRUE(compiled.icon.has_value());
    const SvgIcon& icon = *compiled.icon;

    EXPECT_EQ(icon.paths.size(), std::size_t(2));
    EXPECT_EQ(icon.viewBox.w, 24.f);
    EXPECT_EQ(icon.strokeWidth, 2.f);
    EXPECT_EQ(icon.strokeCap, StrokeCap::Round);
}

TEST(SvgTest, CompilesPathAndCircleShapesIntoIndependentContours) {
    constexpr char kPathAndCircleSvg[] = "<svg viewBox=\"0 0 24 24\"><path d=\"m21 21-4.34-4.34\">"
                                         "</path><circle cx=\"11\" cy=\"11\" r=\"8\"/></svg>";

    const SvgCompileResult compiled = compileSvgIcon(kPathAndCircleSvg);
    ASSERT_TRUE(compiled.ok());
    ASSERT_TRUE(compiled.icon.has_value());
    const SvgIcon& icon = *compiled.icon;
    ASSERT_EQ(icon.paths.size(), std::size_t(2));
    EXPECT_EQ(icon.strokeCap, StrokeCap::Butt);

    const auto circleContours = icon.paths[1].flatten();
    ASSERT_EQ(circleContours.size(), std::size_t(1));
    const auto& circle = circleContours.front();
    ASSERT_GE(circle.size(), std::size_t{2});
    EXPECT_FLOAT_EQ(circle.front().x, circle.back().x);
    EXPECT_FLOAT_EQ(circle.front().y, circle.back().y);
    for (std::size_t index = 0; index < circle.size(); ++index) {
        SCOPED_TRACE(Message() << "circle point index: " << index);
        const auto& point = circle[index];
        EXPECT_NEAR(std::hypot(point.x - 11.f, point.y - 11.f), 8.f, 0.001f);
    }
}

TEST(SvgTest, TransformsAndTessellatesPathsWithinTheTargetBounds) {
    constexpr char kTransformSvg[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M20 10h-6V4\"/></svg>";
    const Rect target{10.f, 20.f, 16.f, 16.f};

    const SvgCompileResult compiled = compileSvgIcon(kTransformSvg);
    ASSERT_TRUE(compiled.ok());
    ASSERT_TRUE(compiled.icon.has_value());
    const SvgIcon& icon = *compiled.icon;
    const Path transformed = transformSvgPath(icon.paths.front(), icon.viewBox, target);
    const Mesh mesh = tessellateStroke(transformed, {1.f, 1.f, 1.f, 1.f}, 2.f, 1.f);

    ASSERT_FALSE(mesh.empty());
    const auto contours = transformed.flatten();
    ASSERT_EQ(contours.size(), std::size_t(1));
    ASSERT_EQ(contours.front().size(), std::size_t(3));
    EXPECT_NEAR(contours.front()[0].x, 23.333333f, 0.001f);
    EXPECT_NEAR(contours.front()[0].y, 29.333333f, 0.001f);
    EXPECT_NEAR(contours.front()[1].x, 19.333333f, 0.001f);
    EXPECT_NEAR(contours.front()[1].y, 29.333333f, 0.001f);
    EXPECT_NEAR(contours.front()[2].x, 19.333333f, 0.001f);
    EXPECT_NEAR(contours.front()[2].y, 33.333333f, 0.001f);

    constexpr float kFringeWidth = 1.f;
    for (const Vertex& vertex : mesh.vertices) {
        EXPECT_GE(vertex.position.x, target.x - kFringeWidth);
        EXPECT_LE(vertex.position.x, target.x + target.w + kFringeWidth);
        EXPECT_GE(vertex.position.y, target.y - kFringeWidth);
        EXPECT_LE(vertex.position.y, target.y + target.h + kFringeWidth);
    }
}

TEST(SvgTest, RejectsMalformedPathWithoutExposingPartialIcon) {
    constexpr char kMalformedSvg[] = "<svg viewBox=\"0 0 24 24\">\n<path d=\"M0 0 L10\"/>\n</svg>";

    const SvgCompileResult compiled = compileSvgIcon(kMalformedSvg, "icons/broken.svg");
    ASSERT_FALSE(compiled.ok());
    EXPECT_FALSE(compiled.icon.has_value());
    ASSERT_FALSE(compiled.errors.empty());
    EXPECT_EQ(compiled.errors.front().code, "svg.path.arguments_invalid");
    EXPECT_EQ(compiled.errors.front().source, "icons/broken.svg");
    EXPECT_GT(compiled.errors.front().line, std::size_t(0));
}

TEST(SvgTest, RejectsUnsupportedElementsWithoutExposingAnIcon) {
    constexpr char kUnsupportedElementSvg[] = "<svg viewBox=\"0 0 24 24\">"
                                              "<rect x=\"0\" y=\"0\" width=\"4\" height=\"4\"/></svg>";

    const SvgCompileResult rejected = compileSvgIcon(kUnsupportedElementSvg);
    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.icon.has_value());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "svg.element.unsupported");
}

TEST(SvgTest, RejectsUnsupportedAttributesWithoutExposingAnIcon) {
    constexpr char kUnsupportedAttributeSvg[] = "<svg viewBox=\"0 0 24 24\">"
                                                "<path d=\"M0 0 L4 4\" opacity=\"0.5\"/></svg>";

    const SvgCompileResult rejected = compileSvgIcon(kUnsupportedAttributeSvg);
    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.icon.has_value());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "svg.attribute.unsupported");
}

TEST(SvgTest, RejectsInvalidSvgBoundariesWithSpecificDiagnostics) {
    struct InvalidSvgCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };
    const InvalidSvgCase cases[] = {
        {"empty source", "", "svg.empty"},
        {"missing viewBox", "<svg><path d=\"M0 0 L4 4\"/></svg>", "svg.view_box.missing"},
        {"invalid circle",
         "<svg viewBox=\"0 0 24 24\"><circle cx=\"1\" cy=\"2\" "
         "r=\"0\"/></svg>",
         "svg.circle.invalid"},
        {"malformed XML", "<svg viewBox=\"0 0 24 24\"><path></svg>", "svg.xml.invalid"},
        {"text content", "<svg viewBox=\"0 0 24 24\">text<path d=\"M0 0 L4 4\"/></svg>", "svg.text.unsupported"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid SVG case: " << test.name);
        const SvgCompileResult rejected = compileSvgIcon(test.source, "invalid.svg");
        ASSERT_FALSE(rejected.ok());
        EXPECT_FALSE(rejected.icon.has_value());
        ASSERT_FALSE(rejected.errors.empty());
        EXPECT_EQ(rejected.errors.front().code, test.diagnostic);
        EXPECT_EQ(rejected.errors.front().source, "invalid.svg");
    }
}
