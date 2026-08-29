/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include "types.h"

namespace radia::ui {
enum class PaintTargetKind : std::uint8_t { Direct, Offscreen };

enum class AAIntent : std::uint8_t { None, Coverage };

struct PaintTarget {
    Rect bounds;
    Vec2 pixelOrigin;
    float scale = 1.f;
    PaintTargetKind kind = PaintTargetKind::Direct;
    bool opaque = false;
    AAIntent shapeAA = AAIntent::Coverage;
    AAIntent textAA = AAIntent::Coverage;
    AAIntent clipAA = AAIntent::None;
    const NativeAppearance* nativeAppearance = nullptr;
};
} // namespace radia::ui
