/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <map>
#include <sstream>
#include <string>

namespace {
using ::testing::Message;

TEST(PaintProtocolTest, MatchesPaintProtocolWithShaderConstants) {
    const std::filesystem::path uiSourceRoot = std::filesystem::path(__FILE__).parent_path().parent_path();
    const std::filesystem::path newviewSourceRoot = uiSourceRoot.parent_path() / "newview";
    std::ifstream vertexFile(newviewSourceRoot / "app_settings/shaders/class1/interface/uiV.glsl");
    std::ifstream fragmentFile(newviewSourceRoot / "app_settings/shaders/class1/interface/uiF.glsl");
    std::ifstream paintProtocolFile(uiSourceRoot / "render/paintprotocol.def");
    ASSERT_TRUE(vertexFile.good());
    ASSERT_TRUE(fragmentFile.good());
    ASSERT_TRUE(paintProtocolFile.good());

    std::ostringstream vertex;
    std::ostringstream fragment;
    std::ostringstream paintProtocol;
    vertex << vertexFile.rdbuf();
    fragment << fragmentFile.rdbuf();
    paintProtocol << paintProtocolFile.rdbuf();
    const std::string vertexSource = vertex.str();
    const std::string fragmentSource = fragment.str();
    const std::string paintProtocolSource = paintProtocol.str();

    const auto contains = [](const std::string& source, const char* text) { return source.find(text) != std::string::npos; };
    const auto trimToken = [](std::string token) {
        const std::size_t first = token.find_first_not_of(" \t\r\n");
        const std::size_t last = token.find_last_not_of(" \t\r\n");
        return first == std::string::npos ? std::string() : token.substr(first, last - first + 1);
    };
    const auto parseDefinition = [&](const char* macro) {
        std::map<std::string, int> entries;
        const std::string marker = std::string(macro) + "(";
        std::size_t position = 0;
        while ((position = paintProtocolSource.find(marker, position)) != std::string::npos) {
            const std::size_t nameStart = position + marker.size();
            const std::size_t firstComma = paintProtocolSource.find(',', nameStart);
            const std::size_t close = paintProtocolSource.find(')', firstComma + 1);
            if (firstComma == std::string::npos || close == std::string::npos) break;
            entries.emplace(trimToken(paintProtocolSource.substr(nameStart, firstComma - nameStart)),
                            std::stoi(trimToken(paintProtocolSource.substr(firstComma + 1, close - firstComma - 1))));
            position = close + 1;
        }
        return entries;
    };
    const auto parseShaderConstants = [&](const char* prefix) {
        std::map<std::string, int> entries;
        const std::string marker = std::string("const int ") + prefix;
        std::size_t position = 0;
        while ((position = fragmentSource.find(marker, position)) != std::string::npos) {
            const std::size_t nameStart = position + std::string("const int ").size();
            const std::size_t equals = fragmentSource.find('=', nameStart);
            const std::size_t semicolon = fragmentSource.find(';', equals);
            if (equals == std::string::npos || semicolon == std::string::npos) break;
            const std::string name = trimToken(fragmentSource.substr(nameStart, equals - nameStart));
            entries.emplace(name.substr(std::string(prefix).size()), std::stoi(trimToken(fragmentSource.substr(equals + 1, semicolon - equals - 1))));
            position = semicolon + 1;
        }
        return entries;
    };
    const auto expectProtocol = [&](const char* macro, const char* shaderPrefix) {
        const auto definitions = parseDefinition(macro);
        const auto constants = parseShaderConstants(shaderPrefix);
        EXPECT_EQ(definitions.size(), constants.size()) << macro;
        for (const auto& [name, value] : definitions) {
            SCOPED_TRACE(Message() << "paint protocol entry: " << name);
            const auto found = constants.find(name);
            ASSERT_NE(found, constants.end());
            EXPECT_EQ(found->second, value);
        }
    };

    expectProtocol("PAINT_OP_ENTRY", "kPaintOp");
    expectProtocol("GRADIENT_OP_ENTRY", "kGradient");
    expectProtocol("OUTLINE_OP_ENTRY", "kOutline");

    EXPECT_TRUE(contains(vertexSource, "#ifdef PAINT_SHADER"));
    EXPECT_TRUE(contains(fragmentSource, "#ifdef PAINT_SHADER"));
    EXPECT_TRUE(contains(vertexSource, "shapeCoord = texcoord0"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpDirect = 0")
                && contains(fragmentSource, "kPaintOpFill = 1")
                && contains(fragmentSource, "paintOp == kPaintOpDirect"));
    EXPECT_TRUE(contains(paintProtocolSource, "PAINT_OP_ENTRY(Direct, 0)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Fill, 1)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Border, 2)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Gradient, 3)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(OuterShadow, 4)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(InsetShadow, 5)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(GradientBorder, 6)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Blur, 7)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Composite, 8)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Arrow, 9)"));
    EXPECT_TRUE(contains(fragmentSource, "uniform int paintOp;")
                && contains(fragmentSource, "uniform vec4 shapeRect;")
                && contains(fragmentSource, "uniform vec4 shapeRadiusX;")
                && contains(fragmentSource, "uniform vec4 shapeRadiusY;")
                && contains(fragmentSource, "uniform vec4 innerRadiusX;")
                && contains(fragmentSource, "uniform vec4 innerRadiusY;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRect;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRadiusX;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRadiusY;")
                && contains(fragmentSource, "uniform int scrollbarClipEnabled;")
                && contains(fragmentSource, "uniform vec4 clipCoverageRect;")
                && contains(fragmentSource, "uniform int clipCoverageEnabled;")
                && contains(fragmentSource, "uniform vec2 effectTextureSize;")
                && contains(fragmentSource, "uniform int gradientKind;")
                && contains(fragmentSource, "uniform int outlineStyle;"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpBorder = 2")
                && contains(fragmentSource, "paintOp == kPaintOpBorder")
                && contains(fragmentSource, "fwidth"));
    EXPECT_TRUE(contains(fragmentSource, "roundedTriangleCornerDistance")
                && contains(fragmentSource, "roundedTriangleCoverage(shapeCoord, a, b, c, arrowRadius)"));
    EXPECT_TRUE(contains(fragmentSource, "scrollbarClipEnabled != 0") && contains(fragmentSource, "coverageFromDistance(clipDistance)"));
    EXPECT_TRUE(contains(fragmentSource, "applyClipCoverage")
                && contains(fragmentSource, "gl_FragCoord.xy")
                && contains(fragmentSource, "clipCoverageEnabled == 0"));
    EXPECT_TRUE(contains(fragmentSource, "kGradientLinear = 0")
                && contains(fragmentSource, "kGradientRadial = 1")
                && contains(fragmentSource, "kGradientConic = 2")
                && contains(fragmentSource, "kOutlineSolid = 0")
                && contains(fragmentSource, "kOutlineDashed = 1"));
    EXPECT_TRUE(contains(fragmentSource, "topBorderGap"));
    EXPECT_TRUE(contains(fragmentSource, "gradientKind") && contains(fragmentSource, "atan(delta.x, delta.y)"));
    EXPECT_TRUE(contains(fragmentSource, "gradientRepeating")
                && contains(fragmentSource, "underlyingGradientIntegral")
                && contains(fragmentSource, "cycles * repeatingTotal"));
    EXPECT_TRUE(contains(fragmentSource, "gradientPixelWidth")
                && contains(fragmentSource, "filteredGradientColor")
                && contains(fragmentSource, "gradientIntervalIntegral")
                && contains(fragmentSource, "dFdx(delta)"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpGradientBorder = 6")
                && contains(fragmentSource, "paintOp == kPaintOpGradientBorder")
                && contains(fragmentSource, "borderWidths"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpBlur = 7")
                && contains(fragmentSource, "kPaintOpComposite = 8")
                && contains(fragmentSource, "paintOp == kPaintOpBlur")
                && contains(fragmentSource, "paintOp == kPaintOpComposite")
                && contains(fragmentSource, "blurredEffectColor")
                && contains(fragmentSource, "maxSamplesPerSide")
                && contains(fragmentSource, "totalWeight"));
    EXPECT_TRUE(contains(fragmentSource, "return vec4(color.rgb, mask);") && !contains(fragmentSource, "color.a * mask"));
}
} // namespace
