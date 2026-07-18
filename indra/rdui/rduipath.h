#ifndef LL_RDUI_PATH_H
#define LL_RDUI_PATH_H

#include "rduidiagnostic.h"
#include "rduitypes.h"
#include <optional>
#include <string>
#include <vector>

namespace rdui
{
    enum class PathVerb { MoveTo, LineTo, QuadTo, CubicTo, Close };

    struct PathCommand
    {
        PathVerb verb = PathVerb::MoveTo;
        Vec2 p0;
        Vec2 p1;
        Vec2 p2;
    };

    class Path
    {
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

    struct PathCompileResult : DiagnosticResult
    {
        std::optional<Path> path;
        bool ok() const { return !hasErrors() && path.has_value(); }
    };

    PathCompileResult compileSvgPathData(const std::string& data, const std::string& source = {}, std::size_t line = 0);
}

#endif // LL_RDUI_PATH_H
