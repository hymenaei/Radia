/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <optional>
#include <utility>

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

inline std::optional<NativePointerInput> takePointerMoveForFrame(std::optional<NativePointerInput>& pending, bool uiVisible, bool pointerInWindow,
                                                                 bool pointerCaptured, std::uint32_t modifiers, float dx, float dy) noexcept {
    std::optional<NativePointerInput> sample = std::exchange(pending, std::nullopt);
    if (!sample || !uiVisible || (!pointerInWindow && !pointerCaptured)) return std::nullopt;
    sample->modifiers = modifiers;
    sample->dx = dx;
    sample->dy = dy;
    return sample;
}

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
