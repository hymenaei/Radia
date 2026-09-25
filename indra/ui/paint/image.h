/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <vector>

namespace radia::ui {
struct RasterImage {
    std::uint32_t width = 0;
    std::uint32_t height = 0;
    std::vector<std::uint8_t> rgba;

    bool empty() const { return width == 0 || height == 0 || rgba.empty(); }
};
} // namespace radia::ui
