/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include "NativeAppearance.h"

namespace Core::Layout {
struct IntrinsicSizeConstraints {
    std::optional<float> width;
    std::optional<float> height;
    std::optional<NativeLayoutMetrics> nativeMetrics;

    IntrinsicSizeConstraints() = default;
    IntrinsicSizeConstraints(std::optional<float> constrainedWidth, std::optional<float> constrainedHeight,
        std::optional<NativeLayoutMetrics> metrics = std::nullopt)
        : width(constrainedWidth)
        , height(constrainedHeight)
        , nativeMetrics(metrics) {}
};
} // namespace Core::Layout
