/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <vector>
#include "ComputedStyle.h"
#include "Diagnostic.h"
#include "Path.h"

namespace Core {
struct SVGImage {
    Layout::Rect viewBox = Layout::Rect(0.f, 0.f, 24.f, 24.f);
    float strokeWidth = 2.f;
    Style::StrokeCap strokeCap = Style::StrokeCap::Butt;
    std::vector<Path> paths;

    bool empty() const { return paths.empty(); }
};

struct SVGCompileResult : DiagnosticResult {
    std::optional<SVGImage> image;
    bool ok() const { return !hasErrors() && image.has_value(); }
};

SVGCompileResult compileSVGImage(const std::string& svg, const std::string& source = {});
Path transformSVGPath(const Path& path, const Layout::Rect& viewBox, const Layout::Rect& target);
} // namespace Core
