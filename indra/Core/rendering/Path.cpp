/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "Path.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <locale>
#include <numbers>
#include <sstream>
#include <string>

namespace Core {
Path& Path::moveTo(float x, float y) {
    mCommands.push_back({PathVerb::MoveTo, {x, y}, {}, {}});
    return *this;
}
Path& Path::lineTo(float x, float y) {
    mCommands.push_back({PathVerb::LineTo, {x, y}, {}, {}});
    return *this;
}
Path& Path::quadTo(float cx, float cy, float x, float y) {
    mCommands.push_back({PathVerb::QuadTo, {cx, cy}, {x, y}, {}});
    return *this;
}
Path& Path::cubicTo(float c0x, float c0y, float c1x, float c1y, float x, float y) {
    mCommands.push_back({PathVerb::CubicTo, {c0x, c0y}, {c1x, c1y}, {x, y}});
    return *this;
}
Path& Path::close() {
    mCommands.push_back({PathVerb::Close, {}, {}, {}});
    return *this;
}

namespace {
float distanceToLine(const Layout::Vec2& point, const Layout::Vec2& start, const Layout::Vec2& end) {
    const Layout::Vec2 edge = end - start;
    const float edgeLength = Layout::length(edge);
    return edgeLength <= 0.0001f ? Layout::length(point - start)
                                 : std::fabs(edge.x * (start.y - point.y) - (start.x - point.x) * edge.y) / edgeLength;
}

void flattenQuad(std::vector<Layout::Vec2>& out, const Layout::Vec2& a, const Layout::Vec2& b, const Layout::Vec2& c, float tolerance,
    int depth) {
    if (depth == 12 || distanceToLine(b, a, c) <= tolerance) {
        out.push_back(c);
        return;
    }
    const Layout::Vec2 ab = (a + b) * 0.5f;
    const Layout::Vec2 bc = (b + c) * 0.5f;
    const Layout::Vec2 midpoint = (ab + bc) * 0.5f;
    flattenQuad(out, a, ab, midpoint, tolerance, depth + 1);
    flattenQuad(out, midpoint, bc, c, tolerance, depth + 1);
}

void flattenCubic(std::vector<Layout::Vec2>& out, const Layout::Vec2& a, const Layout::Vec2& b, const Layout::Vec2& c,
    const Layout::Vec2& d, float tolerance, int depth) {
    if (depth == 12 || std::max(distanceToLine(b, a, d), distanceToLine(c, a, d)) <= tolerance) {
        out.push_back(d);
        return;
    }
    const Layout::Vec2 ab = (a + b) * 0.5f;
    const Layout::Vec2 bc = (b + c) * 0.5f;
    const Layout::Vec2 cd = (c + d) * 0.5f;
    const Layout::Vec2 abc = (ab + bc) * 0.5f;
    const Layout::Vec2 bcd = (bc + cd) * 0.5f;
    const Layout::Vec2 midpoint = (abc + bcd) * 0.5f;
    flattenCubic(out, a, ab, abc, midpoint, tolerance, depth + 1);
    flattenCubic(out, midpoint, bcd, cd, d, tolerance, depth + 1);
}

void finishContour(std::vector<std::vector<Layout::Vec2>>& contours, std::vector<Layout::Vec2>& contour) {
    if (contour.size() >= 2)
        contours.push_back(std::move(contour));
    contour.clear();
}

void skipWhitespace(const char*& cursor) {
    while (*cursor && std::isspace(static_cast<unsigned char>(*cursor)))
        ++cursor;
}

bool parseNumber(std::istringstream& numberStream, const char*& cursor, const char* inputStart, const char* inputEnd, float& value,
    bool allowComma) {
    skipWhitespace(cursor);
    if (*cursor == ',') {
        if (!allowComma)
            return false;
        ++cursor;
        skipWhitespace(cursor);
        if (!*cursor || *cursor == ',')
            return false;
    }

    const char* numberEnd = cursor;
    if (*numberEnd == '+' || *numberEnd == '-')
        ++numberEnd;
    bool hasDigits = false;
    while (*numberEnd >= '0' && *numberEnd <= '9') {
        ++numberEnd;
        hasDigits = true;
    }
    if (*numberEnd == '.') {
        ++numberEnd;
        const char* const fractionStart = numberEnd;
        while (*numberEnd >= '0' && *numberEnd <= '9')
            ++numberEnd;
        hasDigits = hasDigits || numberEnd != fractionStart;
    }
    if (!hasDigits)
        return false;

    if (*numberEnd == 'e' || *numberEnd == 'E') {
        const char* exponentEnd = numberEnd + 1;
        if (*exponentEnd == '+' || *exponentEnd == '-')
            ++exponentEnd;
        const char* const exponentDigits = exponentEnd;
        while (*exponentEnd >= '0' && *exponentEnd <= '9')
            ++exponentEnd;
        if (exponentEnd != exponentDigits)
            numberEnd = exponentEnd;
    }

    numberStream.clear();
    numberStream.seekg(cursor - inputStart);
    float parsedValue = 0.f;
    numberStream >> parsedValue;
    const bool reachedEnd = numberStream.eof();
    if (numberStream.fail()) {
        if (parsedValue != 0.f)
            return false;
        numberStream.clear();
    }
    if (!std::isfinite(parsedValue))
        return false;

    const std::streampos streamEnd = reachedEnd ? std::streampos(inputEnd - inputStart) : numberStream.tellg();
    if (streamEnd == std::streampos(-1) || static_cast<std::streamoff>(streamEnd) != numberEnd - inputStart)
        return false;

    value = parsedValue;
    cursor = numberEnd;
    return true;
}

Layout::Vec2 endpoint(bool relative, const Layout::Vec2& current, float x, float y) {
    return relative ? current + Layout::Vec2(x, y) : Layout::Vec2(x, y);
}
} // namespace

std::vector<std::vector<Layout::Vec2>> Path::flatten(float flatness) const {
    std::vector<std::vector<Layout::Vec2>> contours;
    std::vector<Layout::Vec2> contour;
    Layout::Vec2 current;
    Layout::Vec2 start;
    const float tolerance = std::max(0.01f, flatness);
    for (const PathCommand& command : mCommands) {
        switch (command.verb) {
        case PathVerb::MoveTo:
            finishContour(contours, contour);
            current = start = command.p0;
            contour.push_back(current);
            break;
        case PathVerb::LineTo:
            current = command.p0;
            contour.push_back(current);
            break;
        case PathVerb::QuadTo:
            flattenQuad(contour, current, command.p0, command.p1, tolerance, 0);
            current = command.p1;
            break;
        case PathVerb::CubicTo:
            flattenCubic(contour, current, command.p0, command.p1, command.p2, tolerance, 0);
            current = command.p2;
            break;
        case PathVerb::Close:
            if (!contour.empty() && (contour.back().x != start.x || contour.back().y != start.y))
                contour.push_back(start);
            finishContour(contours, contour);
            current = start;
            break;
        }
    }
    finishContour(contours, contour);
    return contours;
}

Path Path::circle(const Layout::Vec2& center, float radius, int segments) {
    Path path;
    const int count = std::max(8, segments);
    constexpr float kTwoPi = 2.0f * std::numbers::pi_v<float>;
    path.moveTo(center.x + radius, center.y);
    for (int i = 1; i <= count; ++i) {
        const float angle = static_cast<float>(i) / static_cast<float>(count) * kTwoPi;
        path.lineTo(center.x + std::cos(angle) * radius, center.y + std::sin(angle) * radius);
    }
    return path.close();
}

PathCompileResult compileSvgPathData(const std::string& data, const std::string& source, std::size_t line) {
    PathCompileResult result;
    Path path;
    const char* cursor = data.c_str();
    const char* const inputEnd = cursor + data.size();
    std::istringstream numberStream {data};
    numberStream.imbue(std::locale::classic());
    numberStream >> std::noskipws;
    char command = 0;
    Layout::Vec2 current;
    Layout::Vec2 start;
    bool hasSubpath = false;

    auto fail = [&](const std::string& code, const std::string& message) {
        const std::size_t column = static_cast<std::size_t>(cursor - data.c_str()) + 1;
        result.error(code, message, source, line, column);
    };

    while (*cursor) {
        skipWhitespace(cursor);
        if (!*cursor)
            break;
        bool commandWasRead = false;
        if (std::isalpha(static_cast<unsigned char>(*cursor))) {
            command = *cursor++;
            commandWasRead = true;
            const char normalized = static_cast<char>(std::toupper(static_cast<unsigned char>(command)));
            if (normalized != 'M' && normalized != 'L' && normalized != 'H' && normalized != 'V' && normalized != 'Q' && normalized != 'C'
                && normalized != 'Z') {
                fail("svg.path.command_unsupported", "Unsupported SVG path command: " + std::string(1, command) + ".");
                return result;
            }
        }
        if (!command) {
            fail("svg.path.command_missing", "SVG path data must begin with a command.");
            return result;
        }
        const bool relative = std::islower(static_cast<unsigned char>(command)) != 0;
        const char op = static_cast<char>(std::toupper(static_cast<unsigned char>(command)));
        if (op == 'Z') {
            if (!hasSubpath) {
                fail("svg.path.close_without_subpath", "SVG path cannot close before a move command.");
                return result;
            }
            path.close();
            current = start;
            command = 0;
            continue;
        }
        if (!hasSubpath && op != 'M') {
            fail("svg.path.move_missing", "SVG path must begin with a move command.");
            return result;
        }

        float x = 0.f, y = 0.f;
        if (op == 'H') {
            if (!parseNumber(numberStream, cursor, data.c_str(), inputEnd, x, !commandWasRead)) {
                fail("svg.path.arguments_invalid", "Horizontal line command requires one finite coordinate.");
                return result;
            }
            current.x = relative ? current.x + x : x;
            path.lineTo(current.x, current.y);
        } else if (op == 'V') {
            if (!parseNumber(numberStream, cursor, data.c_str(), inputEnd, y, !commandWasRead)) {
                fail("svg.path.arguments_invalid", "Vertical line command requires one finite coordinate.");
                return result;
            }
            current.y = relative ? current.y + y : y;
            path.lineTo(current.x, current.y);
        } else if (op == 'M' || op == 'L') {
            if (!parseNumber(numberStream, cursor, data.c_str(), inputEnd, x, !commandWasRead)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, y, true)) {
                fail("svg.path.arguments_invalid", "Move and line commands require two finite coordinates.");
                return result;
            }
            current = endpoint(relative, current, x, y);
            if (op == 'M') {
                path.moveTo(current.x, current.y);
                start = current;
                hasSubpath = true;
                command = relative ? 'l' : 'L';
            } else
                path.lineTo(current.x, current.y);
        } else if (op == 'Q') {
            float cx = 0.f, cy = 0.f;
            if (!parseNumber(numberStream, cursor, data.c_str(), inputEnd, cx, !commandWasRead)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, cy, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, x, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, y, true)) {
                fail("svg.path.arguments_invalid", "Quadratic curve command requires four finite coordinates.");
                return result;
            }
            const Layout::Vec2 control = endpoint(relative, current, cx, cy);
            const Layout::Vec2 next = endpoint(relative, current, x, y);
            path.quadTo(control.x, control.y, next.x, next.y);
            current = next;
        } else if (op == 'C') {
            float c0x = 0.f, c0y = 0.f, c1x = 0.f, c1y = 0.f;
            if (!parseNumber(numberStream, cursor, data.c_str(), inputEnd, c0x, !commandWasRead)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, c0y, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, c1x, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, c1y, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, x, true)
                || !parseNumber(numberStream, cursor, data.c_str(), inputEnd, y, true)) {
                fail("svg.path.arguments_invalid", "Cubic curve command requires six finite coordinates.");
                return result;
            }
            const Layout::Vec2 c0 = endpoint(relative, current, c0x, c0y);
            const Layout::Vec2 c1 = endpoint(relative, current, c1x, c1y);
            const Layout::Vec2 next = endpoint(relative, current, x, y);
            path.cubicTo(c0.x, c0.y, c1.x, c1.y, next.x, next.y);
            current = next;
        }
    }
    if (path.empty()) {
        result.error("svg.path.empty", "SVG path data contains no commands.", source, line, 1);
        return result;
    }
    result.path = std::move(path);
    return result;
}
} // namespace Core
