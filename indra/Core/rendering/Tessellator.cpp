/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "Tessellator.h"
#include <algorithm>
#include <cmath>
#include <numbers>

namespace Core {
namespace {
constexpr float kPi = std::numbers::pi_v<float>;

bool samePoint(const Layout::Vec2& a, const Layout::Vec2& b) { return std::fabs(a.x - b.x) <= 0.0001f && std::fabs(a.y - b.y) <= 0.0001f; }

void triangle(Mesh& mesh, const Layout::Vec2& a, const Layout::Vec2& b, const Layout::Vec2& c, const Color& color) {
    mesh.vertices.push_back({a, color});
    mesh.vertices.push_back({b, color});
    mesh.vertices.push_back({c, color});
}

void quad(Mesh& mesh, const Layout::Vec2& a, const Layout::Vec2& b, const Layout::Vec2& c, const Layout::Vec2& d, const Color& color) {
    triangle(mesh, a, b, c, color);
    triangle(mesh, a, c, d, color);
}

void gradientQuad(Mesh& mesh, const Layout::Vec2& a, const Layout::Vec2& b, const Layout::Vec2& c, const Layout::Vec2& d,
    const Color& inner, const Color& outer) {
    mesh.vertices.insert(mesh.vertices.end(), {{a, inner}, {b, inner}, {c, outer}, {a, inner}, {c, outer}, {d, outer}});
}

Layout::Vec2 normal(const Layout::Vec2& a, const Layout::Vec2& b) {
    const Layout::Vec2 edge = b - a;
    return Layout::normalize({-edge.y, edge.x});
}

Layout::Vec2 offsetJoin(const Layout::Vec2& point, const Layout::Vec2& previous, const Layout::Vec2& next, float distance) {
    if (std::fabs(distance) <= 0.0001f)
        return point;
    Layout::Vec2 miter = previous + next;
    if (Layout::length(miter) <= 0.0001f)
        return point + next * distance;
    miter = Layout::normalize(miter);
    const float denominator = Layout::dot(miter, next);
    if (std::fabs(denominator) <= 0.1f)
        return point + next * distance;
    const float miterLength = distance / denominator;
    return std::fabs(miterLength) > std::fabs(distance) * 4.f ? point + next * distance : point + miter * miterLength;
}

std::vector<Layout::Vec2> offsetContour(const std::vector<Layout::Vec2>& contour, bool closed, float distance) {
    std::vector<Layout::Vec2> result(contour.size());
    if (contour.size() < 2)
        return result;
    if (!closed) {
        result.front() = contour.front() + normal(contour[0], contour[1]) * distance;
        result.back() = contour.back() + normal(contour[contour.size() - 2], contour.back()) * distance;
    }
    for (std::size_t i = closed ? 0 : 1, end = closed ? contour.size() : contour.size() - 1; i < end; ++i) {
        const std::size_t previous = (i + contour.size() - 1) % contour.size();
        const std::size_t next = (i + 1) % contour.size();
        result[i] = offsetJoin(contour[i], normal(contour[previous], contour[i]), normal(contour[i], contour[next]), distance);
    }
    return result;
}

void roundCap(Mesh& mesh, const Layout::Vec2& center, const Layout::Vec2& tangent, float radius, float fringe, bool start,
    const Color& color) {
    if (radius < 0.f || (radius == 0.f && fringe == 0.f))
        return;
    const Layout::Vec2 perpendicular(-tangent.y, tangent.x);
    const int steps = std::max(6, static_cast<int>(std::ceil((radius + fringe) * 3.f)));
    const float first = start ? kPi : 0.f;
    auto point = [&](float r, float angle) {
        return center + perpendicular * (std::cos(angle) * r) + tangent * (std::sin(angle) * r);
    };
    std::vector<Layout::Vec2> inner;
    std::vector<Layout::Vec2> outer;
    for (int i = 0; i <= steps; ++i) {
        const float angle = first + kPi * static_cast<float>(i) / static_cast<float>(steps);
        inner.push_back(point(radius, angle));
        if (fringe > 0.f)
            outer.push_back(point(radius + fringe, angle));
    }
    for (std::size_t i = 0; i + 1 < inner.size(); ++i) {
        triangle(mesh, center, inner[i], inner[i + 1], color);
        if (fringe > 0.f)
            gradientQuad(mesh, inner[i], inner[i + 1], outer[i + 1], outer[i], color, color.withAlpha(0.f));
    }
}
} // namespace

Mesh tessellateStroke(const Path& path, const Color& color, float width, float fringeWidth, Style::StrokeCap cap) {
    Mesh mesh;
    if (color.a <= 0.f || width <= 0.f)
        return mesh;
    const float halfWidth = width * 0.5f;
    const float aaHalf = std::min(std::max(0.f, fringeWidth) * 0.5f, halfWidth);
    const float core = halfWidth - aaHalf;
    const float fringe = aaHalf * 2.f;

    for (std::vector<Layout::Vec2> contour : path.flatten()) {
        if (contour.size() < 2)
            continue;
        bool closed = contour.size() > 2 && samePoint(contour.front(), contour.back());
        if (closed)
            contour.pop_back();
        if (closed && contour.size() == 2)
            closed = false;
        if (contour.size() < (closed ? 3U : 2U))
            continue;

        std::vector<Layout::Vec2> stroke = contour;
        Layout::Vec2 startTangent;
        Layout::Vec2 endTangent;
        if (!closed) {
            startTangent = Layout::normalize(contour[1] - contour[0]);
            endTangent = Layout::normalize(contour.back() - contour[contour.size() - 2]);
            if (cap == Style::StrokeCap::Square) {
                stroke.front() = stroke.front() - startTangent * core;
                stroke.back() = stroke.back() + endTangent * core;
            } else if (cap == Style::StrokeCap::Butt && aaHalf > 0.f) {
                stroke.front() = stroke.front() + startTangent * aaHalf;
                stroke.back() = stroke.back() - endTangent * aaHalf;
            }
        }

        const std::vector<Layout::Vec2> left = offsetContour(stroke, closed, core);
        const std::vector<Layout::Vec2> right = offsetContour(stroke, closed, -core);
        const std::size_t segments = closed ? stroke.size() : stroke.size() - 1;
        for (std::size_t i = 0; i < segments; ++i) {
            const std::size_t next = (i + 1) % stroke.size();
            quad(mesh, left[i], left[next], right[next], right[i], color);
        }

        if (!closed && cap == Style::StrokeCap::Round) {
            roundCap(mesh, contour.front(), startTangent, core, fringe, true, color);
            roundCap(mesh, contour.back(), endTangent, core, fringe, false, color);
        }
        if (fringe <= 0.f)
            continue;

        const Color transparent = color.withAlpha(0.f);
        const std::vector<Layout::Vec2> leftOuter = offsetContour(stroke, closed, core + fringe);
        const std::vector<Layout::Vec2> rightOuter = offsetContour(stroke, closed, -core - fringe);
        for (std::size_t i = 0; i < segments; ++i) {
            const std::size_t next = (i + 1) % stroke.size();
            gradientQuad(mesh, left[i], left[next], leftOuter[next], leftOuter[i], color, transparent);
            gradientQuad(mesh, right[next], right[i], rightOuter[i], rightOuter[next], color, transparent);
        }
        if (!closed && cap != Style::StrokeCap::Round) {
            const std::size_t last = stroke.size() - 1;
            gradientQuad(mesh, right[0], left[0], left[0] - startTangent * fringe, right[0] - startTangent * fringe, color, transparent);
            gradientQuad(mesh, left[last], right[last], right[last] + endTangent * fringe, left[last] + endTangent * fringe, color,
                transparent);
        }
    }
    return mesh;
}
} // namespace Core
