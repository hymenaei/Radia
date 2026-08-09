/**
 * @file svg_test.cpp
 * @brief Tests SVG path and icon rendering data conversion.
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
#include "../test/lltut.h"
#include "render/svg.h"
#include "render/tessellator.h"

namespace tut {
struct svg_data {};
typedef test_group<svg_data> svg_test;
typedef svg_test::object svg_object;
using rduisvg_object = svg_object;
svg_test svg_testcase("svg");

template<> template<> void rduisvg_object::test<1>() {
    const rdui::SvgCompileResult compiled = rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\" stroke-width=\"2\" stroke-linecap=\"round\">"
                                                                 "<path d=\"M18 6 6 18\"/><path d=\"m6 6 12 12\"/></svg>");
    ensure("valid SVG compiles", compiled.ok());
    const rdui::SvgIcon& icon = *compiled.icon;
    ensure_equals("two paths", icon.paths.size(), 2U);
    ensure_equals("view box", icon.view_box.w, 24.f);
    ensure_equals("stroke width", icon.stroke_width, 2.f);
    ensure_equals("round cap", static_cast<int>(icon.stroke_cap), static_cast<int>(rdui::StrokeCap::Round));
}

template<> template<> void rduisvg_object::test<2>() {
    const rdui::SvgCompileResult compiled = rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><path d=\"m21 21-4.34-4.34\"/>"
                                                                 "<circle cx=\"11\" cy=\"11\" r=\"8\"/></svg>");
    ensure("path and circle SVG compiles", compiled.ok());
    const rdui::SvgIcon& icon = *compiled.icon;
    ensure_equals("path and circle", icon.paths.size(), 2U);
    ensure_equals("default butt", static_cast<int>(icon.stroke_cap), static_cast<int>(rdui::StrokeCap::Butt));
    ensure("circle becomes closed contour", icon.paths[1].flatten().front().size() >= 24U);
}

template<> template<> void rduisvg_object::test<3>() {
    const rdui::SvgCompileResult compiled = rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><path d=\"M20 10h-6V4\"/></svg>");
    ensure("transform source compiles", compiled.ok());
    const rdui::SvgIcon& icon = *compiled.icon;
    const rdui::Path transformed = rdui::transformSvgPath(icon.paths[0], icon.view_box, {10.f, 20.f, 16.f, 16.f});
    const rdui::Mesh mesh = rdui::tessellateStroke(transformed, {1.f, 1.f, 1.f, 1.f}, 2.f, 1.f);
    ensure("transformed icon tessellates", !mesh.empty());
    for (const rdui::Vertex& vertex : mesh.triangles) {
        ensure("x inside target", vertex.position.x >= 9.f && vertex.position.x <= 27.f);
        ensure("y inside target", vertex.position.y >= 19.f && vertex.position.y <= 37.f);
    }
}

template<> template<> void rduisvg_object::test<4>() {
    const rdui::SvgCompileResult compiled = rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\">\n<path d=\"M0 0 L10\"/>\n</svg>", "icons/broken.svg");
    ensure("malformed path rejects whole SVG", !compiled.ok());
    ensure("partial icon never exposed", !compiled.icon.has_value());
    ensure_equals("diagnostic source retained", compiled.errors.front().source, "icons/broken.svg");
    ensure("diagnostic has source line", compiled.errors.front().line > 0U);
}

template<> template<> void rduisvg_object::test<5>() {
    const rdui::SvgCompileResult unsupported =
        rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><rect x=\"0\" y=\"0\" width=\"4\" height=\"4\"/></svg>");
    ensure("unsupported element rejected", !unsupported.ok());
    ensure("unsupported element exposes no icon", !unsupported.icon.has_value());

    const rdui::SvgCompileResult unknown_attribute = rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L4 4\" opacity=\"0.5\"/></svg>");
    ensure("unsupported attribute rejected", !unknown_attribute.ok());
}

template<> template<> void rduisvg_object::test<6>() {
    ensure("missing viewBox rejected", !rdui::compileSvgIcon("<svg><path d=\"M0 0 L4 4\"/></svg>").ok());
    ensure("invalid circle rejected", !rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><circle cx=\"1\" cy=\"2\" r=\"0\"/></svg>").ok());
    ensure("malformed XML rejected", !rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\"><path></svg>").ok());
    ensure("text content rejected", !rdui::compileSvgIcon("<svg viewBox=\"0 0 24 24\">text<path d=\"M0 0 L4 4\"/></svg>").ok());
}
} // namespace tut
