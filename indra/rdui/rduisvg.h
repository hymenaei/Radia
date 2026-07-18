#ifndef LL_RDUI_SVG_H
#define LL_RDUI_SVG_H

#include "rduidiagnostic.h"
#include "rduipath.h"
#include <optional>
#include <string>
#include <vector>

namespace rdui
{
    struct SvgIcon
    {
        Rect view_box = Rect(0.f, 0.f, 24.f, 24.f);
        float stroke_width = 2.f;
        StrokeCap stroke_cap = StrokeCap::Butt;
        std::vector<Path> paths;

        bool empty() const { return paths.empty(); }
    };

    struct SvgCompileResult : DiagnosticResult
    {
        std::optional<SvgIcon> icon;
        bool ok() const { return !hasErrors() && icon.has_value(); }
    };

    SvgCompileResult compileSvgIcon(const std::string& svg, const std::string& source = {});
    Path transformSvgPath(const Path& path, const Rect& view_box, const Rect& target);
}

#endif // LL_RDUI_SVG_H
