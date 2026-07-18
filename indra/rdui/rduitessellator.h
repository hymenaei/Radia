#ifndef LL_RDUI_TESSELLATOR_H
#define LL_RDUI_TESSELLATOR_H

#include "rduipath.h"
#include <vector>

namespace rdui
{
    struct Vertex { Vec2 position; Color color; };
    struct Mesh
    {
        std::vector<Vertex> triangles;
        bool empty() const { return triangles.empty(); }
    };

    Mesh tessellateStroke(const Path& path, const Color& color, float width, float fringe_width, StrokeCap cap = StrokeCap::Butt);
}

#endif // LL_RDUI_TESSELLATOR_H
