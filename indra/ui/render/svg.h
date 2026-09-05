/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

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
