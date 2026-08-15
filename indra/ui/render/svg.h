/**
 * @file svg.h
 * @brief Parses validated SVG icons and transforms their paths into target rectangles.
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

#ifndef RD_RENDER_SVG_H
#define RD_RENDER_SVG_H

#include <optional>
#include <string>
#include <vector>
#include "diagnostic.h"
#include "path.h"

namespace radia::ui {
struct SvgIcon {
    Rect viewBox = Rect(0.f, 0.f, 24.f, 24.f);
    float strokeWidth = 2.f;
    StrokeCap strokeCap = StrokeCap::Butt;
    std::vector<Path> paths;

    bool empty() const { return paths.empty(); }
};

struct SvgCompileResult : DiagnosticResult {
    std::optional<SvgIcon> icon;
    bool ok() const { return !hasErrors() && icon.has_value(); }
};

SvgCompileResult compileSvgIcon(const std::string& svg, const std::string& source = {});
Path transformSvgPath(const Path& path, const Rect& viewBox, const Rect& target);
} // namespace radia::ui
#endif // RD_RENDER_SVG_H
