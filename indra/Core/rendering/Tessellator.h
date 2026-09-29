/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <vector>
#include "Color.h"
#include "ComputedStyle.h"
#include "LayoutGeometry.h"
#include "Path.h"

namespace Core {
struct Vertex {
    Layout::Vec2 position;
    Color color;
};
struct Mesh {
    std::vector<Vertex> vertices;
    bool empty() const { return vertices.empty(); }
};

Mesh tessellateStroke(const Path& path, const Color& color, float width, float fringeWidth, Style::StrokeCap cap = Style::StrokeCap::Butt);
} // namespace Core
