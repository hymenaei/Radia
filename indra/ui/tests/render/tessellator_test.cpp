/**
 * @file tessellator_test.cpp
 * @brief Tests vector tessellation geometry and winding behavior.
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
#include <algorithm>
#include <cmath>
#include <limits>
#include "../test/lltut.h"
#include "render/tessellator.h"

namespace {
using radia::ui::Color;
using radia::ui::Mesh;
using radia::ui::Path;
using radia::ui::StrokeCap;
using radia::ui::tessellateStroke;
} // namespace

namespace tut {
struct tessellatorData {};
using tessellatorTest = test_group<tessellatorData>;
using tessellatorObject = tessellatorTest::object;
tessellatorTest tessellatorTestCase("tessellator");

namespace {
Path line(float x0 = 0.f, float x1 = 10.f) {
    Path path;
    return path.moveTo(x0, 0.f).lineTo(x1, 0.f);
}

float minX(const Mesh& mesh) {
    float result = std::numeric_limits<float>::max();
    for (const auto& vertex : mesh.vertices) result = std::min(result, vertex.position.x);
    return result;
}

float maxX(const Mesh& mesh) {
    float result = std::numeric_limits<float>::lowest();
    for (const auto& vertex : mesh.vertices) result = std::max(result, vertex.position.x);
    return result;
}

float opaqueYSpan(const Mesh& mesh) {
    float minimum = std::numeric_limits<float>::max();
    float maximum = std::numeric_limits<float>::lowest();
    for (const auto& vertex : mesh.vertices) {
        if (vertex.color.a < .99f) continue;
        minimum = std::min(minimum, vertex.position.y);
        maximum = std::max(maximum, vertex.position.y);
    }
    return maximum - minimum;
}
} // namespace

template<> template<> void tessellatorObject::test<1>() {
    const Color white;
    const Mesh one = tessellateStroke(line(), white, 1.f, 0.f);
    const Mesh two = tessellateStroke(line(), white, 2.f, 0.f);
    ensure_equals("one pixel opaque width", opaqueYSpan(one), 1.f);
    ensure_equals("two pixel opaque width", opaqueYSpan(two), 2.f);
}

template<> template<> void tessellatorObject::test<2>() {
    const Mesh mesh = tessellateStroke(line(), Color(), 2.f, 1.f);
    bool transparent = false;
    bool opaque = false;
    for (const auto& vertex : mesh.vertices) {
        transparent |= vertex.color.a == 0.f;
        opaque |= vertex.color.a == 1.f;
    }
    ensure("stroke has opaque core", opaque);
    ensure("stroke has transparent AA fringe", transparent);
}

template<> template<> void tessellatorObject::test<3>() {
    const Path path = line();
    const Mesh butt = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Butt);
    const Mesh square = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Square);
    const Mesh round = tessellateStroke(path, Color(), 2.f, 0.f, StrokeCap::Round);
    ensure_equals("butt starts at endpoint", minX(butt), 0.f);
    ensure("square extends start", minX(square) < minX(butt));
    ensure("round extends end", maxX(round) > maxX(butt));
}

template<> template<> void tessellatorObject::test<4>() {
    Path curve;
    curve.moveTo(0.f, 0.f).cubicTo(0.f, 20.f, 20.f, 20.f, 20.f, 0.f);
    const Mesh mesh = tessellateStroke(curve, Color(), 1.f, 1.f, StrokeCap::Round);
    ensure("adaptive curve produces several segments", mesh.vertices.size() > 30U);
}

template<> template<> void tessellatorObject::test<5>() {
    const Mesh mesh = tessellateStroke(Path::circle({0.f, 0.f}, 10.f), Color(), 2.f, 1.f);
    ensure("closed circle tessellates", !mesh.empty());
    ensure("closed contour surrounds origin", minX(mesh) < -10.f && maxX(mesh) > 10.f);
}
} // namespace tut
