/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <vector>
#include "path.h"

namespace radia::ui {
struct Vertex {
    Vec2 position;
    Color color;
};
struct Mesh {
    std::vector<Vertex> vertices;
    bool empty() const { return vertices.empty(); }
};

Mesh tessellateStroke(const Path& path, const Color& color, float width, float fringeWidth, StrokeCap cap = StrokeCap::Butt);
} // namespace radia::ui
