/**
 * @file rduisvg.h
 * @brief
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

#ifndef LL_RDUI_SVG_H
#define LL_RDUI_SVG_H

#include <optional>
#include <string>
#include <vector>
#include "rduidiagnostic.h"
#include "rduipath.h"

namespace rdui {
struct SvgIcon {
    Rect view_box = Rect(0.f, 0.f, 24.f, 24.f);
    float stroke_width = 2.f;
    StrokeCap stroke_cap = StrokeCap::Butt;
    std::vector<Path> paths;

    bool empty() const { return paths.empty(); }
};

struct SvgCompileResult : DiagnosticResult {
    std::optional<SvgIcon> icon;
    bool ok() const { return !hasErrors() && icon.has_value(); }
};

SvgCompileResult compileSvgIcon(const std::string& svg, const std::string& source = {});
Path transformSvgPath(const Path& path, const Rect& view_box, const Rect& target);
} // namespace rdui
#endif // LL_RDUI_SVG_H
