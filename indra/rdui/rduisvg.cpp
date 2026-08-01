/**
 * @file rduisvg.cpp
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

#include "linden_common.h"
#include "rduisvg.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <initializer_list>
#include "llxmlnode.h"

namespace rdui {
namespace {
std::string elementName(LLXMLNode* node) {
    return node && node->getName() && node->getName()->mString ? node->getName()->mString : std::string();
}

std::size_t lineOf(LLXMLNode* node) {
    const S32 line = node ? node->getLineNumber() : -1;
    return line > 0 ? static_cast<std::size_t>(line) : 0;
}

bool attribute(LLXMLNode* node, const char* name, std::string& value) {
    return node && node->getAttributeString(name, value);
}

std::string lowerCopy(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
    return value;
}

void skipWhitespace(const char*& cursor) {
    while (*cursor && std::isspace(static_cast<unsigned char>(*cursor))) ++cursor;
}

bool parseFloat(const char*& cursor, float& value) {
    skipWhitespace(cursor);
    char* end = nullptr;
    value = std::strtof(cursor, &end);
    if (end == cursor || !std::isfinite(value)) return false;
    cursor = end;
    return true;
}

bool parseSingleFloat(const std::string& raw, float& value) {
    const char* cursor = raw.c_str();
    if (!parseFloat(cursor, value)) return false;
    skipWhitespace(cursor);
    return *cursor == '\0';
}

bool parseViewBox(const std::string& raw, Rect& value) {
    const char* cursor = raw.c_str();
    float values[4]{};
    for (int index = 0; index < 4; ++index) {
        if (!parseFloat(cursor, values[index])) return false;
        if (index == 3) break;
        const char* separator = cursor;
        skipWhitespace(cursor);
        if (*cursor == ',') {
            ++cursor;
            skipWhitespace(cursor);
            if (!*cursor || *cursor == ',') return false;
        } else if (cursor == separator) return false;
    }
    skipWhitespace(cursor);
    if (*cursor || values[2] <= 0.f || values[3] <= 0.f) return false;
    value = {values[0], values[1], values[2], values[3]};
    return true;
}

bool allowedAttribute(const std::string& name, std::initializer_list<const char*> allowed) {
    for (const char* candidate : allowed)
        if (name == candidate) return true;
    return false;
}

bool hasNonWhitespaceText(LLXMLNode* node) {
    if (!node) return false;
    for (const unsigned char character : node->getValue())
        if (!std::isspace(character)) return true;
    return false;
}

void validateAttributes(LLXMLNode* node, std::initializer_list<const char*> allowed, SvgCompileResult& result, const std::string& source) {
    if (!node) return;
    for (const auto& entry : node->mAttributes) {
        const std::string name = elementName(entry.second.get());
        if (!allowedAttribute(name, allowed))
            result.error("svg.attribute.unsupported", "Unsupported attribute '" + name + "' on <" + elementName(node) + ">.", source, lineOf(node));
    }
}

bool validatePresentationValue(LLXMLNode* node, const char* name, const char* expected, SvgCompileResult& result, const std::string& source) {
    std::string value;
    if (!attribute(node, name, value)) return true;
    if (value == expected) return true;
    result.error("svg.presentation.unsupported", "Unsupported " + std::string(name) + " value '" + value + "'. Expected '" + expected + "'.", source,
                 lineOf(node));
    return false;
}

bool parseStrokeCap(LLXMLNode* node, StrokeCap& cap, SvgCompileResult& result, const std::string& source) {
    std::string raw;
    if (!attribute(node, "stroke-linecap", raw)) return true;
    const std::string value = lowerCopy(raw);
    if (value == "butt") cap = StrokeCap::Butt;
    else if (value == "round") cap = StrokeCap::Round;
    else if (value == "square") cap = StrokeCap::Square;
    else {
        result.error("svg.stroke_cap.invalid", "Invalid SVG stroke-linecap value: " + raw + ".", source, lineOf(node));
        return false;
    }
    return true;
}

void appendPathDiagnostics(SvgCompileResult& result, PathCompileResult&& path_result) {
    result.append(std::move(path_result));
}
} // namespace

SvgCompileResult compileSvgIcon(const std::string& svg, const std::string& source) {
    SvgCompileResult result;
    if (svg.empty()) {
        result.error("svg.empty", "SVG source is empty.", source);
        return result;
    }

    LLXMLNodePtr root;
    if (!LLXMLNode::parseBuffer(svg.data(), svg.size(), root, nullptr) || root.isNull()) {
        result.error("svg.xml.invalid", "Could not parse SVG XML.", source);
        return result;
    }
    if (elementName(root.get()) != "svg") {
        result.error("svg.root.invalid", "SVG resource root must be <svg>.", source, lineOf(root.get()));
        return result;
    }

    validateAttributes(root.get(),
                       {"xmlns", "width", "height", "viewBox", "fill", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin", "class"},
                       result, source);
    if (hasNonWhitespaceText(root.get())) result.error("svg.text.unsupported", "SVG icons cannot contain text content.", source, lineOf(root.get()));

    SvgIcon candidate;
    std::string raw;
    if (!attribute(root.get(), "viewBox", raw)) result.error("svg.view_box.missing", "SVG icon requires a viewBox.", source, lineOf(root.get()));
    else if (!parseViewBox(raw, candidate.view_box))
        result.error("svg.view_box.invalid", "SVG viewBox must contain four finite numbers with positive width and height.", source,
                     lineOf(root.get()));

    for (const char* dimension : {"width", "height"}) {
        if (attribute(root.get(), dimension, raw)) {
            float value = 0.f;
            if (!parseSingleFloat(raw, value) || value <= 0.f)
                result.error("svg.dimension.invalid", "SVG " + std::string(dimension) + " must be a positive finite number.", source,
                             lineOf(root.get()));
        }
    }

    if (attribute(root.get(), "stroke-width", raw)) {
        if (!parseSingleFloat(raw, candidate.stroke_width) || candidate.stroke_width <= 0.f)
            result.error("svg.stroke_width.invalid", "SVG stroke-width must be a positive finite number.", source, lineOf(root.get()));
    }
    parseStrokeCap(root.get(), candidate.stroke_cap, result, source);
    validatePresentationValue(root.get(), "fill", "none", result, source);
    validatePresentationValue(root.get(), "stroke", "currentColor", result, source);
    validatePresentationValue(root.get(), "stroke-linejoin", "round", result, source);

    for (LLXMLNodePtr child = root->getFirstChild(); child.notNull(); child = child->getNextSibling()) {
        const std::string name = elementName(child.get());
        if (name == "path") {
            validateAttributes(child.get(), {"d"}, result, source);
            if (hasNonWhitespaceText(child.get()))
                result.error("svg.text.unsupported", "SVG <path> cannot contain text content.", source, lineOf(child.get()));
            if (!attribute(child.get(), "d", raw) || raw.empty()) {
                result.error("svg.path.data_missing", "SVG <path> requires non-empty d data.", source, lineOf(child.get()));
                continue;
            }
            PathCompileResult path_result = compileSvgPathData(raw, source, lineOf(child.get()));
            if (path_result.ok()) candidate.paths.push_back(std::move(*path_result.path));
            appendPathDiagnostics(result, std::move(path_result));
        } else if (name == "circle") {
            validateAttributes(child.get(), {"cx", "cy", "r"}, result, source);
            if (hasNonWhitespaceText(child.get()))
                result.error("svg.text.unsupported", "SVG <circle> cannot contain text content.", source, lineOf(child.get()));
            float cx = 0.f;
            float cy = 0.f;
            float radius = 0.f;
            std::string cx_raw;
            std::string cy_raw;
            std::string radius_raw;
            if (!attribute(child.get(), "cx", cx_raw)
                || !parseSingleFloat(cx_raw, cx)
                || !attribute(child.get(), "cy", cy_raw)
                || !parseSingleFloat(cy_raw, cy)
                || !attribute(child.get(), "r", radius_raw)
                || !parseSingleFloat(radius_raw, radius)
                || radius <= 0.f) {
                result.error("svg.circle.invalid", "SVG <circle> requires finite cx/cy and a positive finite radius.", source, lineOf(child.get()));
                continue;
            }
            candidate.paths.push_back(Path::circle({cx, cy}, radius));
        } else {
            result.error("svg.element.unsupported", "Unsupported SVG element: <" + name + ">.", source, lineOf(child.get()));
        }
    }

    if (candidate.paths.empty() && !result.hasErrors())
        result.error("svg.shapes.empty", "SVG icon contains no supported shapes.", source, lineOf(root.get()));
    if (!result.hasErrors()) result.icon = std::move(candidate);
    return result;
}

Path transformSvgPath(const Path& path, const Rect& view_box, const Rect& target) {
    Path result;
    const float sx = target.w / std::max(0.0001f, view_box.w);
    const float sy = target.h / std::max(0.0001f, view_box.h);
    auto map = [&](const Vec2& point) { return Vec2(target.x + (point.x - view_box.x) * sx, target.y + target.h - (point.y - view_box.y) * sy); };

    for (const PathCommand& command : path.commands()) {
        switch (command.verb) {
            case PathVerb::MoveTo: {
                const Vec2 point = map(command.p0);
                result.moveTo(point.x, point.y);
                break;
            }
            case PathVerb::LineTo: {
                const Vec2 point = map(command.p0);
                result.lineTo(point.x, point.y);
                break;
            }
            case PathVerb::QuadTo: {
                const Vec2 control = map(command.p0);
                const Vec2 point = map(command.p1);
                result.quadTo(control.x, control.y, point.x, point.y);
                break;
            }
            case PathVerb::CubicTo: {
                const Vec2 control0 = map(command.p0);
                const Vec2 control1 = map(command.p1);
                const Vec2 point = map(command.p2);
                result.cubicTo(control0.x, control0.y, control1.x, control1.y, point.x, point.y);
                break;
            }
            case PathVerb::Close: result.close(); break;
        }
    }
    return result;
}
} // namespace rdui
