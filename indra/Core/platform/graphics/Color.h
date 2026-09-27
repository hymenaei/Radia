/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

namespace radia::ui {
struct Color {
    float r = 1.f;
    float g = 1.f;
    float b = 1.f;
    float a = 1.f;

    Color() = default;
    Color(float red, float green, float blue, float alpha = 1.f) : r(red), g(green), b(blue), a(alpha) {}

    Color withAlpha(float alpha) const { return Color(r, g, b, alpha); }

    friend bool operator==(const Color&, const Color&) = default;
};
} // namespace radia::ui
