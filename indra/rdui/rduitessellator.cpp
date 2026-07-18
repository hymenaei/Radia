#include "linden_common.h"
#include "rduitessellator.h"
#include <algorithm>
#include <cmath>

namespace rdui
{
    namespace
    {
        constexpr float PI = 3.14159265358979323846f;

        bool samePoint(const Vec2& a, const Vec2& b)
        {
            return std::fabs(a.x - b.x) <= 0.0001f && std::fabs(a.y - b.y) <= 0.0001f;
        }

        void triangle(Mesh& mesh, const Vec2& a, const Vec2& b, const Vec2& c, const Color& color)
        {
            mesh.triangles.push_back({a, color});
            mesh.triangles.push_back({b, color});
            mesh.triangles.push_back({c, color});
        }

        void quad(Mesh& mesh, const Vec2& a, const Vec2& b, const Vec2& c, const Vec2& d, const Color& color)
        {
            triangle(mesh, a, b, c, color);
            triangle(mesh, a, c, d, color);
        }

        void gradientQuad(Mesh& mesh, const Vec2& a, const Vec2& b, const Vec2& c, const Vec2& d, const Color& inner, const Color& outer)
        {
            mesh.triangles.insert(mesh.triangles.end(), {{a, inner}, {b, inner}, {c, outer}, {a, inner}, {c, outer}, {d, outer}});
        }

        Vec2 normal(const Vec2& a, const Vec2& b)
        {
            const Vec2 edge = b - a;
            return normalize({-edge.y, edge.x});
        }

        Vec2 offsetJoin(const Vec2& point, const Vec2& previous, const Vec2& next, float distance)
        {
            if (std::fabs(distance) <= 0.0001f) return point;
            Vec2 miter = previous + next;
            if (length(miter) <= 0.0001f) return point + next * distance;
            miter = normalize(miter);
            const float denominator = dot(miter, next);
            if (std::fabs(denominator) <= 0.1f) return point + next * distance;
            const float miter_length = distance / denominator;
            return std::fabs(miter_length) > std::fabs(distance) * 4.f ? point + next * distance : point + miter * miter_length;
        }

        std::vector<Vec2> offsetContour(const std::vector<Vec2>& contour, bool closed, float distance)
        {
            std::vector<Vec2> result(contour.size());
            if (contour.size() < 2) return result;
            if (!closed)
            {
                result.front() = contour.front() + normal(contour[0], contour[1]) * distance;
                result.back() = contour.back() + normal(contour[contour.size() - 2], contour.back()) * distance;
            }
            for (std::size_t i = closed ? 0 : 1, end = closed ? contour.size() : contour.size() - 1; i < end; ++i)
            {
                const std::size_t previous = (i + contour.size() - 1) % contour.size();
                const std::size_t next = (i + 1) % contour.size();
                result[i] = offsetJoin(contour[i], normal(contour[previous], contour[i]), normal(contour[i], contour[next]), distance);
            }
            return result;
        }

        void roundCap(Mesh& mesh, const Vec2& center, const Vec2& tangent, float radius, float fringe, bool start, const Color& color)
        {
            if (radius < 0.f || (radius == 0.f && fringe == 0.f)) return;
            const Vec2 perpendicular(-tangent.y, tangent.x);
            const int steps = std::max(6, static_cast<int>(std::ceil((radius + fringe) * 3.f)));
            const float first = start ? PI : 0.f;
            auto point = [&](float r, float angle)
            {
                return center + perpendicular * (std::cos(angle) * r) + tangent * (std::sin(angle) * r);
            };
            std::vector<Vec2> inner;
            std::vector<Vec2> outer;
            for (int i = 0; i <= steps; ++i)
            {
                const float angle = first + PI * static_cast<float>(i) / static_cast<float>(steps);
                inner.push_back(point(radius, angle));
                if (fringe > 0.f) outer.push_back(point(radius + fringe, angle));
            }
            for (std::size_t i = 0; i + 1 < inner.size(); ++i)
            {
                triangle(mesh, center, inner[i], inner[i + 1], color);
                if (fringe > 0.f) gradientQuad(mesh, inner[i], inner[i + 1], outer[i + 1], outer[i], color, color.withAlpha(0.f));
            }
        }
    }

    Mesh tessellateStroke(const Path& path, const Color& color, float width, float fringe_width, StrokeCap cap)
    {
        Mesh mesh;
        if (color.a <= 0.f || width <= 0.f) return mesh;
        const float half_width = width * 0.5f;
        const float aa_half = std::min(std::max(0.f, fringe_width) * 0.5f, half_width);
        const float core = half_width - aa_half;
        const float fringe = aa_half * 2.f;

        for (std::vector<Vec2> contour : path.flatten())
        {
            if (contour.size() < 2) continue;
            bool closed = contour.size() > 2 && samePoint(contour.front(), contour.back());
            if (closed) contour.pop_back();
            if (contour.size() < (closed ? 3U : 2U)) continue;

            std::vector<Vec2> stroke = contour;
            Vec2 start_tangent;
            Vec2 end_tangent;
            if (!closed)
            {
                start_tangent = normalize(contour[1] - contour[0]);
                end_tangent = normalize(contour.back() - contour[contour.size() - 2]);
                if (cap == StrokeCap::Square)
                {
                    stroke.front() = stroke.front() - start_tangent * core;
                    stroke.back() = stroke.back() + end_tangent * core;
                }
                else if (cap == StrokeCap::Butt && aa_half > 0.f)
                {
                    stroke.front() = stroke.front() + start_tangent * aa_half;
                    stroke.back() = stroke.back() - end_tangent * aa_half;
                }
            }

            const std::vector<Vec2> left = offsetContour(stroke, closed, core);
            const std::vector<Vec2> right = offsetContour(stroke, closed, -core);
            const std::size_t segments = closed ? stroke.size() : stroke.size() - 1;
            for (std::size_t i = 0; i < segments; ++i)
            {
                const std::size_t next = (i + 1) % stroke.size();
                quad(mesh, left[i], left[next], right[next], right[i], color);
            }

            if (!closed && cap == StrokeCap::Round)
            {
                roundCap(mesh, contour.front(), start_tangent, core, fringe, true, color);
                roundCap(mesh, contour.back(), end_tangent, core, fringe, false, color);
            }
            if (fringe <= 0.f) continue;

            const Color transparent = color.withAlpha(0.f);
            const std::vector<Vec2> left_outer = offsetContour(stroke, closed, core + fringe);
            const std::vector<Vec2> right_outer = offsetContour(stroke, closed, -core - fringe);
            for (std::size_t i = 0; i < segments; ++i)
            {
                const std::size_t next = (i + 1) % stroke.size();
                gradientQuad(mesh, left[i], left[next], left_outer[next], left_outer[i], color, transparent);
                gradientQuad(mesh, right[next], right[i], right_outer[i], right_outer[next], color, transparent);
            }
            if (!closed && cap != StrokeCap::Round)
            {
                const std::size_t last = stroke.size() - 1;
                gradientQuad(mesh, right[0], left[0], left[0] - start_tangent * fringe, right[0] - start_tangent * fringe, color, transparent);
                gradientQuad(mesh, left[last], right[last], right[last] + end_tangent * fringe, left[last] + end_tangent * fringe, color, transparent);
            }
        }
        return mesh;
    }
}
