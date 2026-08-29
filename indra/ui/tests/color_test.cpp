/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <string>
#include "style/color.h"

namespace {
using radia::ui::Color;
using radia::ui::parseColor;
using ::testing::Message;

void expectColor(const std::string& source, const Color& expected) {
    constexpr float kColorComponentTolerance = 1.0e-4f;
    SCOPED_TRACE(Message() << "color notation: " << source);

    const auto parsed = parseColor(source);
    ASSERT_TRUE(parsed.has_value());

    EXPECT_NEAR(parsed->r, expected.r, kColorComponentTolerance);
    EXPECT_NEAR(parsed->g, expected.g, kColorComponentTolerance);
    EXPECT_NEAR(parsed->b, expected.b, kColorComponentTolerance);
    EXPECT_NEAR(parsed->a, expected.a, kColorComponentTolerance);
}
} // namespace

TEST(ColorTest, ParsesHexColorsWithOptionalAlpha) {
    expectColor("#f80", {1.f, 8.f / 15.f, 0.f, 1.f});
    expectColor("#f808", {1.f, 8.f / 15.f, 0.f, 8.f / 15.f});
    expectColor("#ff8800", {1.f, 136.f / 255.f, 0.f, 1.f});
    expectColor("#ff880080", {1.f, 136.f / 255.f, 0.f, 128.f / 255.f});
}

TEST(ColorTest, ParsesRgbColorsAcrossCommaAndSpaceSyntax) {
    expectColor("rgb(255, 128, 0)", {1.f, 128.f / 255.f, 0.f, 1.f});
    expectColor("RGB(100%, 50%, 0%, 25%)", {1.f, .5f, 0.f, .25f});
    expectColor("rgb(255 128 0 / 50%)", {1.f, 128.f / 255.f, 0.f, .5f});
}

TEST(ColorTest, ClampsRgbComponentsAndAlphaToTheirValidRange) {
    expectColor("rgb(300 -5 0 / 2)", {1.f, 0.f, 0.f, 1.f});
}

TEST(ColorTest, ParsesHslColorsAcrossHueUnits) {
    expectColor("hsl(120 100% 50%)", {0.f, 1.f, 0.f, 1.f});
    expectColor("hsl(.5turn 100% 50% / 25%)", {0.f, 1.f, 1.f, .25f});
    expectColor("hsl(3.14159265rad, 100%, 50%, .5)", {0.f, 1.f, 1.f, .5f});
    expectColor("hsl(-120deg 100% 50%)", {0.f, 0.f, 1.f, 1.f});
    expectColor("hsl(200grad 100% 50%)", {0.f, 1.f, 1.f, 1.f});
}

TEST(ColorTest, ParsesHwbAndNormalizesWhitenessAndBlackness) {
    expectColor("hwb(0 0% 0%)", {1.f, 0.f, 0.f, 1.f});
    expectColor("hwb(120 60% 60% / 50%)", {.5f, .5f, .5f, .5f});
}

TEST(ColorTest, ConvertsLabAndLchToSrgb) {
    expectColor("lab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    expectColor("lab(0 0 0 / .25)", {0.f, 0.f, 0.f, .25f});
    expectColor("lab(54.29054295% 80.80492033 69.89098846)", {1.f, 0.f, 0.f, 1.f});
    expectColor("lch(100% 0 270)", {1.f, 1.f, 1.f, 1.f});
    expectColor("lch(54.29054295% 106.83719118 40.85766886)", {1.f, 0.f, 0.f, 1.f});
}

TEST(ColorTest, ConvertsOklabAndOklchToSrgb) {
    expectColor("oklab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    expectColor("oklab(0 0 0)", {0.f, 0.f, 0.f, 1.f});
    expectColor("oklab(.62795536 .22486306 .12584630)", {1.f, 0.f, 0.f, 1.f});
    expectColor("oklch(1 0 0deg / 20%)", {1.f, 1.f, 1.f, .2f});
    expectColor("oklch(62.795536% .25768331 29.23388519)", {1.f, 0.f, 0.f, 1.f});
}

TEST(ColorTest, ParsesTransparentKeywordIgnoringCaseAndWhitespace) {
    expectColor("transparent", {0.f, 0.f, 0.f, 0.f});
    expectColor("  TRANSPARENT  ", {0.f, 0.f, 0.f, 0.f});
}

TEST(ColorTest, RejectsUnsupportedAndMalformedSyntax) {
    for (const char* source : {"##ff880080", "rgba(255 128 0 / 50%)", "hsla(120 100% 50% / .5)", "#ggg", "rgb(1, 2, 3 / .5)", "hsl(0 1 1)",
                               "color(1 2 3)", "lab(50%, 0, 0)"}) {
        SCOPED_TRACE(Message() << "unsupported color notation: " << source);
        EXPECT_FALSE(parseColor(source).has_value());
    }
}
