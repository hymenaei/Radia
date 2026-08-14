/**
 * @file color_test.cpp
 * @brief Tests Radia color syntax and typed color values.
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
#include "../test/lltut.h"
#include "style/color.h"

namespace tut {
struct colorData {};
using colorTest = test_group<colorData>;
using colorObject = colorTest::object;
colorTest colorTestCase("color");

void ensureColor(const std::string& message, const std::string& value, const rdui::Color& expected) {
    const std::optional<rdui::Color> color = rdui::parseColor(value);
    ensure(message + " parses", color.has_value());
    ensure_approximately_equals((message + " red").c_str(), color->r, expected.r, 6);
    ensure_approximately_equals((message + " green").c_str(), color->g, expected.g, 6);
    ensure_approximately_equals((message + " blue").c_str(), color->b, expected.b, 6);
    ensure_approximately_equals((message + " alpha").c_str(), color->a, expected.a, 6);
}

template<> template<> void colorObject::test<1>() {
    ensureColor("short rgb", "#f80", {1.f, 8.f / 15.f, 0.f, 1.f});
    ensureColor("short rgb with alpha", "#f808", {1.f, 8.f / 15.f, 0.f, 8.f / 15.f});
    ensureColor("long rgb", "#ff8800", {1.f, 136.f / 255.f, 0.f, 1.f});
    ensureColor("long rgb with alpha", "#ff880080", {1.f, 136.f / 255.f, 0.f, 128.f / 255.f});
    ensure("double hash rejected", !rdui::parseColor("##ff880080"));
}

template<> template<> void colorObject::test<2>() {
    ensureColor("legacy rgb", "rgb(255, 128, 0)", {1.f, 128.f / 255.f, 0.f, 1.f});
    ensureColor("legacy rgb alpha", "RGB(100%, 50%, 0%, 25%)", {1.f, .5f, 0.f, .25f});
    ensureColor("modern rgb", "rgb(255 128 0 / 50%)", {1.f, 128.f / 255.f, 0.f, .5f});
    ensureColor("clamped rgb", "rgb(300 -5 0 / 2)", {1.f, 0.f, 0.f, 1.f});
    ensure("rgba function rejected", !rdui::parseColor("rgba(255 128 0 / 50%)"));
}

template<> template<> void colorObject::test<3>() {
    ensureColor("hsl degrees", "hsl(120 100% 50%)", {0.f, 1.f, 0.f, 1.f});
    ensureColor("hsl turn", "hsl(.5turn 100% 50% / 25%)", {0.f, 1.f, 1.f, .25f});
    ensureColor("hsl radians", "hsl(3.14159265rad, 100%, 50%, .5)", {0.f, 1.f, 1.f, .5f});
    ensureColor("hue wrap", "hsl(-120deg 100% 50%)", {0.f, 0.f, 1.f, 1.f});
    ensureColor("grad", "hsl(200grad 100% 50%)", {0.f, 1.f, 1.f, 1.f});
    ensure("hsla function rejected", !rdui::parseColor("hsla(120 100% 50% / .5)"));
}

template<> template<> void colorObject::test<4>() {
    ensure("bad hex rejected", !rdui::parseColor("#ggg"));
    ensure("mixed separators rejected", !rdui::parseColor("rgb(1, 2, 3 / .5)"));
    ensure("missing percent rejected", !rdui::parseColor("hsl(0 1 1)"));
    ensure("unknown function rejected", !rdui::parseColor("color(1 2 3)"));
    ensure("modern color commas rejected", !rdui::parseColor("lab(50%, 0, 0)"));
}

template<> template<> void colorObject::test<5>() {
    ensureColor("hwb red", "hwb(0 0% 0%)", {1.f, 0.f, 0.f, 1.f});
    ensureColor("hwb normalized", "hwb(120 60% 60% / 50%)", {.5f, .5f, .5f, .5f});
    ensureColor("lab white", "lab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    ensureColor("lab black", "lab(0 0 0 / .25)", {0.f, 0.f, 0.f, .25f});
    ensureColor("lab red", "lab(54.29054295% 80.80492033 69.89098846)", {1.f, 0.f, 0.f, 1.f});
    ensureColor("lch white", "lch(100% 0 270)", {1.f, 1.f, 1.f, 1.f});
    ensureColor("lch red", "lch(54.29054295% 106.83719118 40.85766886)", {1.f, 0.f, 0.f, 1.f});
    ensureColor("oklab white", "oklab(100% 0 0)", {1.f, 1.f, 1.f, 1.f});
    ensureColor("oklab black", "oklab(0 0 0)", {0.f, 0.f, 0.f, 1.f});
    ensureColor("oklab red", "oklab(.62795536 .22486306 .12584630)", {1.f, 0.f, 0.f, 1.f});
    ensureColor("oklch white", "oklch(1 0 0deg / 20%)", {1.f, 1.f, 1.f, .2f});
    ensureColor("oklch red", "oklch(62.795536% .25768331 29.23388519)", {1.f, 0.f, 0.f, 1.f});
}
} // namespace tut
