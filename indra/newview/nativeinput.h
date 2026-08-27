/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>

namespace radia::viewer::ui {
enum class NativePointerButton : std::uint8_t { NoButton, Left, Right, Middle, Auxiliary1, Auxiliary2 };

struct NativePointerInput {
    float x = 0.f;
    float y = 0.f;
    NativePointerButton button = NativePointerButton::NoButton;
    std::uint32_t modifiers = 0;
    std::uint8_t clickCount = 1;
    float dx = 0.f;
    float dy = 0.f;
};

struct NativeScrollInput {
    std::int32_t x = 0;
    std::int32_t y = 0;
    float horizontal = 0.f;
    float vertical = 0.f;
    std::uint32_t modifiers = 0;
};

struct NativeKeyInput {
    std::int32_t key = 0;
    std::uint32_t modifiers = 0;
    bool repeated = false;
};
} // namespace radia::viewer::ui
