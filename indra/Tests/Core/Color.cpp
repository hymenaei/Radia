/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/CSSColor.h>
#include <Core/Color.h>
#include <clocale>
#include <gtest/gtest.h>
#include <string>

namespace {
using Core::Color;
using Core::CSS::consumeColor;
using Core::CSS::isColorSyntax;
using ::testing::Message;

void expectColor(const std::string& source, const Color& expected) {
    constexpr float kColorComponentTolerance = 1.0e-4f;
    SCOPED_TRACE(Message() << "color notation: " << source);

    const auto parsed = consumeColor(source);
    ASSERT_TRUE(parsed.has_value());

    EXPECT_NEAR(parsed->r, expected.r, kColorComponentTolerance);
    EXPECT_NEAR(parsed->g, expected.g, kColorComponentTolerance);
    EXPECT_NEAR(parsed->b, expected.b, kColorComponentTolerance);
    EXPECT_NEAR(parsed->a, expected.a, kColorComponentTolerance);
}
} // namespace

TEST(Color, ParsesHexColorsWithOptionalAlpha) {
    expectColor("#f80", {1.f, 8.f / 15.f, 0.f, 1.f});
    expectColor("#f808", {1.f, 8.f / 15.f, 0.f, 8.f / 15.f});
    expectColor("#ff8800", {1.f, 136.f / 255.f, 0.f, 1.f});
    expectColor("#ff880080", {1.f, 136.f / 255.f, 0.f, 128.f / 255.f});
}

TEST(Color, ParsesRgbSyntax) {
    expectColor("rgb(255, 128, 0)", {1.f, 128.f / 255.f, 0.f, 1.f});
    expectColor("RGB(100%, 50%, 0%, 25%)", {1.f, .5f, 0.f, .25f});
    expectColor("rgba(255, 128, 0, 50%)", {1.f, 128.f / 255.f, 0.f, .5f});
    expectColor("rgb(255 128 0 / 50%)", {1.f, 128.f / 255.f, 0.f, .5f});
    expectColor("rgba(255 128 0 / 50%)", {1.f, 128.f / 255.f, 0.f, .5f});
    expectColor("rgb(255 50% 0 / 50%)", {1.f, .5f, 0.f, .5f});
    expectColor("rgb(255 128 0", {1.f, 128.f / 255.f, 0.f, 1.f});
}

TEST(Color, RecognizesRgbAndHslAliases) {
    for (const char* source : {"rgb(1 2 3)", "rgba(1 2 3 / .5)", "hsl(0 0% 0%)", "hsla(0 0% 0% / .5)"})
        EXPECT_TRUE(isColorSyntax(source)) << source;
}

TEST(Color, RoundsTinyRgbComponentsToZero) { expectColor("rgb(1e-999 0 0)", {0.f, 0.f, 0.f, 1.f}); }

TEST(Color, ParsesDecimalUnderflowWithCommaNumericLocale) {
    const char* currentLocale = std::setlocale(LC_NUMERIC, nullptr);
    ASSERT_NE(currentLocale, nullptr);
    const std::string originalLocale(currentLocale);
    struct RestoreLocale {
        const std::string& value;
        ~RestoreLocale() { std::setlocale(LC_NUMERIC, value.c_str()); }
    } restore {originalLocale};

    bool usesCommaDecimal = std::string(std::localeconv()->decimal_point) == ",";
    for (const char* candidate : {"de_DE.UTF-8", "fr_FR.UTF-8", "German_Germany.1252", "French_France.1252"}) {
        if (usesCommaDecimal)
            break;
        if (std::setlocale(LC_NUMERIC, candidate))
            usesCommaDecimal = std::string(std::localeconv()->decimal_point) == ",";
    }
    if (!usesCommaDecimal)
        GTEST_SKIP() << "No comma-decimal locale is installed";

    expectColor("rgb(1.5e-999 0 0)", {0.f, 0.f, 0.f, 1.f});
}

TEST(Color, ClampsComponents) { expectColor("rgb(300 -5 0 / 2)", {1.f, 0.f, 0.f, 1.f}); }

TEST(Color, ParsesHslColorsAcrossHueUnits) {
    expectColor("hsl(120 100% 50%)", {0.f, 1.f, 0.f, 1.f});
    expectColor("hsl(120DEG 100% 50%)", {0.f, 1.f, 0.f, 1.f});
    expectColor("hsl(.5turn 100% 50% / 25%)", {0.f, 1.f, 1.f, .25f});
    expectColor("hsla(120, 100%, 50%, 50%)", {0.f, 1.f, 0.f, .5f});
    expectColor("hsla(120 100% 50% / .5)", {0.f, 1.f, 0.f, .5f});
    expectColor("hsl(1e38turn 100% 50%)", {1.f, 0.f, 0.f, 1.f});
    expectColor("hsl(3.14159265rad, 100%, 50%, .5)", {0.f, 1.f, 1.f, .5f});
    expectColor("hsl(-120deg 100% 50%)", {0.f, 0.f, 1.f, 1.f});
    expectColor("hsl(200grad 100% 50%)", {0.f, 1.f, 1.f, 1.f});
}

TEST(Color, ParsesHwb) {
    expectColor("hwb(0 0% 0%)", {1.f, 0.f, 0.f, 1.f});
    expectColor("hwb(120 60% 60% / 50%)", {.5f, .5f, .5f, .5f});
}

TEST(Color, ConvertsLabColors) {
    expectColor("lab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    expectColor("lab(0 0 0 / .25)", {0.f, 0.f, 0.f, .25f});
    expectColor("lab(54.29054295% 80.80492033 69.89098846)", {1.f, 0.f, 0.f, 1.f});
    expectColor("lch(100% 0 270)", {1.f, 1.f, 1.f, 1.f});
    expectColor("lch(54.29054295% 106.83719118 40.85766886)", {1.f, 0.f, 0.f, 1.f});
}

TEST(Color, ConvertsOklabColors) {
    expectColor("oklab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    expectColor("oklab(0 0 0)", {0.f, 0.f, 0.f, 1.f});
    expectColor("oklab(.62795536 .22486306 .12584630)", {1.f, 0.f, 0.f, 1.f});
    expectColor("oklch(1 0 0deg / 20%)", {1.f, 1.f, 1.f, .2f});
    expectColor("oklch(62.795536% .25768331 29.23388519)", {1.f, 0.f, 0.f, 1.f});
}

TEST(Color, ParsesTransparentKeyword) {
    expectColor("transparent", {0.f, 0.f, 0.f, 0.f});
    expectColor("  TRANSPARENT  ", {0.f, 0.f, 0.f, 0.f});
}

TEST(Color, ParsesNamedColors) {
    expectColor("rebeccapurple", {102.f / 255.f, 51.f / 255.f, 153.f / 255.f, 1.f});
    expectColor("GREY", {128.f / 255.f, 128.f / 255.f, 128.f / 255.f, 1.f});
    expectColor("aqua", {0.f, 1.f, 1.f, 1.f});
}

TEST(Color, RejectsInvalidSyntax) {
    for (const char* source : {"##ff880080", "rgb(255, 50%, 0)", "rgba(255, 50%, 0, .5)", "#ggg", "rgb(1, 2, 3 / .5)", "rgb(1, 2, 3,)",
             "hsl(1, 2%, 3%,)", "hsl(0 1 1)", "color(1 2 3)", "lab(50%, 0, 0)"}) {
        SCOPED_TRACE(Message() << "unsupported color notation: " << source);
        EXPECT_FALSE(consumeColor(source).has_value());
    }
}

TEST(Color, RejectsHexadecimalFloatNotation) {
    for (const char* source : {"rgb(0x1p3 0 0)", "rgb(1 2 3 / 0x1p3)", "hsl(0x1p3 100% 50%)", "lab(0x1p3 0 0)"}) {
        SCOPED_TRACE(Message() << "color notation: " << source);
        EXPECT_FALSE(consumeColor(source).has_value());
    }
}
