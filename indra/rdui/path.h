/**
 * @file path.h
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

#ifndef RD_PATH_H
#define RD_PATH_H

#include <optional>
#include <string>
#include <vector>
#include "diagnostic.h"
#include "types.h"

namespace rdui {
enum class PathVerb { MoveTo, LineTo, QuadTo, CubicTo, Close };

struct PathCommand {
    PathVerb verb = PathVerb::MoveTo;
    Vec2 p0;
    Vec2 p1;
    Vec2 p2;
};

class Path {
public:
    Path& moveTo(float x, float y);
    Path& lineTo(float x, float y);
    Path& quadTo(float cx, float cy, float x, float y);
    Path& cubicTo(float c0x, float c0y, float c1x, float c1y, float x, float y);
    Path& close();

    bool empty() const { return mCommands.empty(); }
    const std::vector<PathCommand>& commands() const { return mCommands; }
    std::vector<std::vector<Vec2>> flatten(float flatness = 0.25f) const;

    static Path circle(const Vec2& center, float radius, int segments = 24);

private:
    std::vector<PathCommand> mCommands;
};

struct PathCompileResult : DiagnosticResult {
    std::optional<Path> path;
    bool ok() const { return !hasErrors() && path.has_value(); }
};

PathCompileResult compileSvgPathData(const std::string& data, const std::string& source = {}, std::size_t line = 0);
} // namespace rdui
#endif // RD_PATH_H
