/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <limits>
#include "css/rules.h"
#include "css/syntax.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::normalizeCSSKeyword;
using detail::trim;

std::string valueText(detail::CSSValueRange value) {
    return trim(detail::serializeCSSRange(value.stream, detail::trimCSSRange(value.stream, value.range)));
}

detail::CSSValueRange subValue(detail::CSSValueRange value, detail::CSSTokenRange range) {
    return {value.stream, range};
}

bool parseFiniteFloat(const std::string& value, float& result) {
    char* end = nullptr;
    result = std::strtof(value.c_str(), &end);
    return end != value.c_str() && *end == '\0' && std::isfinite(result);
}

} // namespace

std::optional<CSS::Color> StyleModel::consumeCSSColor(detail::CSSValueRange value) {
    value.range = detail::trimCSSRange(value.stream, value.range);
    auto copy = value;
    const auto parsed = CSSValueDetail::consumeColor(copy);
    if (!parsed || CSSValueDetail::nextToken(copy)) return std::nullopt;
    return parsed;
}

float StyleModel::parseNumberValue(detail::CSSValueRange value, float fallback) {
    value.range = detail::trimCSSRange(value.stream, value.range);
    std::string scalar = valueText(value);
    if (const auto dimension = detail::parseCSSDimension(value.stream, value.range); dimension && dimension->unit == "px") scalar = dimension->number;
    char* end = nullptr;
    const float parsed = std::strtof(scalar.c_str(), &end);
    return end != scalar.c_str() && *end == '\0' ? parsed : fallback;
}

std::optional<Length> StyleModel::parseLengthValue(detail::CSSValueRange value) {
    value.range = detail::trimCSSRange(value.stream, value.range);
    std::string scalar = valueText(value);
    bool percentage = false;
    if (!scalar.empty() && scalar.back() == '%') {
        percentage = true;
        scalar = trim(scalar.substr(0, scalar.size() - 1));
    } else if (const auto dimension = detail::parseCSSDimension(value.stream, value.range); dimension && dimension->unit == "px")
        scalar = dimension->number;

    char* end = nullptr;
    const float parsed = std::strtof(scalar.c_str(), &end);
    if (end == scalar.c_str() || *end != '\0' || !std::isfinite(parsed)) return std::nullopt;
    return percentage ? Length{0.f, parsed / 100.f} : Length{parsed};
}

std::optional<Gradient> StyleModel::parseGradient(detail::CSSValueRange value, ColorSchemeMode scheme) {
    value.range = detail::trimCSSRange(value.stream, value.range);
    const auto function = detail::parseCSSFunction(value.stream, value.range);
    if (!function) return std::nullopt;
    Gradient gradient;
    if (function->name == "linear-gradient") {
    } else if (function->name == "repeating-linear-gradient") {
        gradient.repeating = true;
    } else if (function->name == "radial-gradient") {
        gradient.kind = GradientKind::Radial;
    } else if (function->name == "repeating-radial-gradient") {
        gradient.kind = GradientKind::Radial;
        gradient.repeating = true;
    } else if (function->name == "conic-gradient") {
        gradient.kind = GradientKind::Conic;
        gradient.angleDegrees = 0.f;
    } else if (function->name == "repeating-conic-gradient") {
        gradient.kind = GradientKind::Conic;
        gradient.angleDegrees = 0.f;
        gradient.repeating = true;
    } else return std::nullopt;

    const std::vector<detail::CSSTokenRange> arguments = detail::splitCSSOnDelimiter(value.stream, function->body, ',');
    if (arguments.size() < 2) return std::nullopt;

    auto parseAngle = [&](detail::CSSTokenRange token, float& degrees) {
        const auto dimension = detail::parseCSSDimension(value.stream, token);
        if (!dimension) return false;
        const std::string& number = dimension->number;
        float scale;
        if (dimension->unit == "deg") scale = 1.f;
        else if (dimension->unit == "turn") scale = 360.f;
        else if (dimension->unit == "rad") scale = 57.2957795131f;
        else return false;
        if (!parseFiniteFloat(trim(number), degrees)) return false;
        degrees *= scale;
        return true;
    };

    auto parseCenter = [&](const std::vector<detail::CSSTokenRange>& tokens, Vec2& center) {
        if (tokens.empty() || tokens.size() > 2) return false;
        auto consumePercentage = [&](detail::CSSTokenRange tokenRange, float& percentage) {
            const std::string token = valueText(subValue(value, tokenRange));
            if (token.empty() || token.back() != '%') return false;
            if (!parseFiniteFloat(token.substr(0, token.size() - 1), percentage) || percentage < 0.f || percentage > 100.f) return false;
            percentage /= 100.f;
            return true;
        };
        auto horizontal = [&](detail::CSSTokenRange rawToken, float& result) {
            const std::string token = normalizeCSSKeyword(value.stream, rawToken);
            if (token == "left") result = 0.f;
            else if (token == "center") result = .5f;
            else if (token == "right") result = 1.f;
            else if (!consumePercentage(rawToken, result)) return false;
            return true;
        };
        auto vertical = [&](detail::CSSTokenRange rawToken, float& result) {
            const std::string token = normalizeCSSKeyword(value.stream, rawToken);
            if (token == "bottom") result = 0.f;
            else if (token == "center") result = .5f;
            else if (token == "top") result = 1.f;
            else {
                if (!consumePercentage(rawToken, result)) return false;
                result = 1.f - result;
            }
            return true;
        };

        if (tokens.size() == 1) {
            const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
            if (token == "top" || token == "bottom") {
                center.x = .5f;
                return vertical(tokens.front(), center.y);
            }
            center.y = .5f;
            return horizontal(tokens.front(), center.x);
        }

        float x = 0.f, y = 0.f;
        if (horizontal(tokens[0], x) && vertical(tokens[1], y)) {
            center = {x, y};
            return true;
        }
        if (vertical(tokens[0], y) && horizontal(tokens[1], x)) {
            center = {x, y};
            return true;
        }
        return false;
    };

    auto parsesAsColorStop = [&](detail::CSSTokenRange argument) {
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, argument);
        if (tokens.empty()) return false;
        return consumeCSSColor(subValue(value, tokens.front())).has_value();
    };

    std::size_t firstStop = 0;
    if (!parsesAsColorStop(arguments.front())) {
        const std::string prelude = normalizeCSSKeyword(value.stream, arguments.front());
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, arguments.front());
        if (gradient.kind == GradientKind::Linear) {
            if (prelude == "to top"
                || prelude == "to right"
                || prelude == "to bottom"
                || prelude == "to left"
                || prelude == "to top right"
                || prelude == "to right top"
                || prelude == "to bottom right"
                || prelude == "to right bottom"
                || prelude == "to bottom left"
                || prelude == "to left bottom"
                || prelude == "to top left"
                || prelude == "to left top") {
                const bool top = prelude.find("top") != std::string::npos;
                const bool right = prelude.find("right") != std::string::npos;
                const bool bottom = prelude.find("bottom") != std::string::npos;
                const bool left = prelude.find("left") != std::string::npos;
                if (top && right) gradient.angleDegrees = 45.f;
                else if (bottom && right) gradient.angleDegrees = 135.f;
                else if (bottom && left) gradient.angleDegrees = 225.f;
                else if (top && left) gradient.angleDegrees = 315.f;
                else if (top) gradient.angleDegrees = 0.f;
                else if (right) gradient.angleDegrees = 90.f;
                else if (bottom) gradient.angleDegrees = 180.f;
                else if (left) gradient.angleDegrees = 270.f;
                gradient.cornerDirection = top != bottom && right != left;
            } else if (!parseAngle(arguments.front(), gradient.angleDegrees)) return std::nullopt;
        } else if (gradient.kind == GradientKind::Radial) {
            const auto at = std::find_if(tokens.begin(), tokens.end(),
                                         [&](detail::CSSTokenRange token) { return normalizeCSSKeyword(value.stream, token) == "at"; });
            const std::size_t descriptorCount = static_cast<std::size_t>(at - tokens.begin());
            if (descriptorCount > 1) return std::nullopt;
            if (descriptorCount == 1) {
                const std::string shape = normalizeCSSKeyword(value.stream, tokens.front());
                if (shape == "circle") gradient.radialShape = RadialGradientShape::Circle;
                else if (shape != "ellipse") return std::nullopt;
            }
            if (at != tokens.end()) {
                const std::vector<detail::CSSTokenRange> centerTokens(at + 1, tokens.end());
                if (!parseCenter(centerTokens, gradient.center)) return std::nullopt;
            }
        } else {
            std::size_t index = 0;
            if (index < tokens.size() && normalizeCSSKeyword(value.stream, tokens[index]) == "from") {
                if (++index == tokens.size() || !parseAngle(tokens[index++], gradient.angleDegrees)) return std::nullopt;
            }
            if (index < tokens.size() && normalizeCSSKeyword(value.stream, tokens[index]) == "at") {
                const std::vector<detail::CSSTokenRange> centerTokens(tokens.begin() + index + 1, tokens.end());
                if (!parseCenter(centerTokens, gradient.center)) return std::nullopt;
                index = tokens.size();
            }
            if (index != tokens.size()) return std::nullopt;
        }
        firstStop = 1;
    }

    if (arguments.size() - firstStop < 2) return std::nullopt;
    const float unspecified = std::numeric_limits<float>::quiet_NaN();
    auto parseStopPosition = [&](detail::CSSTokenRange rawPosition, float& position) {
        const std::string token = normalizeCSSKeyword(value.stream, rawPosition);
        if (!token.empty() && token.back() == '%') {
            if (!parseFiniteFloat(token.substr(0, token.size() - 1), position)) return false;
            position /= 100.f;
        } else {
            float degrees = 0.f;
            if (gradient.kind != GradientKind::Conic || !parseAngle(rawPosition, degrees)) return false;
            position = degrees / 360.f;
        }
        return position >= 0.f && position <= 1.f;
    };
    for (std::size_t index = firstStop; index < arguments.size(); ++index) {
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, arguments[index]);
        if (tokens.empty() || tokens.size() > 3) return std::nullopt;
        const auto specifiedColor = consumeCSSColor(subValue(value, tokens.front()));
        const auto color = specifiedColor ? StyleColor::fromCSS(*specifiedColor, scheme) : std::nullopt;
        if (!color) return std::nullopt;
        float position = unspecified;
        if (tokens.size() >= 2 && !parseStopPosition(tokens[1], position)) return std::nullopt;
        gradient.stops.emplace_back();
        gradient.stops.back().color = *color;
        gradient.stops.back().position = position;
        if (tokens.size() == 3) {
            if (!parseStopPosition(tokens[2], position)) return std::nullopt;
            gradient.stops.emplace_back();
            gradient.stops.back().color = *color;
            gradient.stops.back().position = position;
        }
        if (gradient.stops.size() > 8) return std::nullopt;
    }

    if (gradient.stops.size() < 2) return std::nullopt;
    if (!std::isfinite(gradient.stops.front().position)) gradient.stops.front().position = 0.f;
    if (!std::isfinite(gradient.stops.back().position)) gradient.stops.back().position = 1.f;
    std::size_t runStart = 0;
    while (runStart + 1 < gradient.stops.size()) {
        std::size_t runEnd = runStart + 1;
        while (runEnd < gradient.stops.size() && !std::isfinite(gradient.stops[runEnd].position)) ++runEnd;
        if (runEnd == gradient.stops.size()) return std::nullopt;
        if (gradient.stops[runEnd].position < gradient.stops[runStart].position) return std::nullopt;
        const float step = (gradient.stops[runEnd].position - gradient.stops[runStart].position) / static_cast<float>(runEnd - runStart);
        for (std::size_t index = runStart + 1; index < runEnd; ++index)
            gradient.stops[index].position = gradient.stops[runStart].position + step * static_cast<float>(index - runStart);
        runStart = runEnd;
    }
    if (gradient.repeating && gradient.stops.back().position <= gradient.stops.front().position) return std::nullopt;
    return gradient;
}

std::optional<Filter> StyleModel::parseFilter(detail::CSSValueRange value) {
    value.range = detail::trimCSSRange(value.stream, value.range);
    if (normalizeCSSKeyword(value.stream, value.range) == "none") return Filter{};
    const std::vector<detail::CSSTokenRange> functions = detail::splitCSSComponents(value.stream, value.range);
    if (functions.empty()) return std::nullopt;

    const auto parseStdDeviation = [&value](detail::CSSTokenRange range) -> std::optional<float> {
        range = detail::trimCSSRange(value.stream, range);
        const std::optional<Length> parsed = parseLengthValue(subValue(value, range));
        if (!parsed || value.stream.tokens()[range.begin].kind == detail::CSSTokenKind::Percentage || parsed->percent != 0.f || parsed->pixels < 0.f)
            return std::nullopt;
        const detail::CSSToken& token = value.stream.tokens()[range.begin];
        if (token.kind == detail::CSSTokenKind::Number && parsed->pixels != 0.f) return std::nullopt;
        return parsed->pixels;
    };

    const auto parseDirection = [&value](detail::CSSTokenRange range, float& degrees) {
        const std::string direction = normalizeCSSKeyword(value.stream, range);
        if (direction == "to top") degrees = 0.f;
        else if (direction == "to top right" || direction == "to right top") degrees = 45.f;
        else if (direction == "to right") degrees = 90.f;
        else if (direction == "to bottom right" || direction == "to right bottom") degrees = 135.f;
        else if (direction == "to bottom") degrees = 180.f;
        else if (direction == "to bottom left" || direction == "to left bottom") degrees = 225.f;
        else if (direction == "to left") degrees = 270.f;
        else if (direction == "to top left" || direction == "to left top") degrees = 315.f;
        else {
            const auto dimension = detail::parseCSSDimension(value.stream, range);
            if (!dimension) return false;
            float scale = 0.f;
            if (dimension->unit == "deg") scale = 1.f;
            else if (dimension->unit == "turn") scale = 360.f;
            else if (dimension->unit == "rad") scale = 57.2957795131f;
            else return false;
            if (!parseFiniteFloat(trim(dimension->number), degrees)) return false;
            degrees *= scale;
        }
        return true;
    };

    const auto parsePosition = [&value](detail::CSSTokenRange range) -> std::optional<float> {
        const std::string token = normalizeCSSKeyword(value.stream, range);
        if (!endsWith(token, "%")) return std::nullopt;
        const std::optional<Length> parsed = parseLengthValue(subValue(value, range));
        if (!parsed || parsed->pixels != 0.f || parsed->percent < 0.f || parsed->percent > 1.f) return std::nullopt;
        return parsed->percent;
    };

    struct ParsedStop {
        float stdDeviation = 0.f;
        std::optional<float> position;
    };

    const auto parseStop = [&value, &parseStdDeviation, &parsePosition](detail::CSSTokenRange range) -> std::optional<ParsedStop> {
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, range);
        if (tokens.empty() || tokens.size() > 2) return std::nullopt;
        const std::optional<float> stdDeviation = parseStdDeviation(tokens.front());
        if (!stdDeviation) return std::nullopt;
        ParsedStop stop{*stdDeviation};
        if (tokens.size() == 2) {
            stop.position = parsePosition(tokens.back());
            if (!stop.position) return std::nullopt;
        }
        return stop;
    };

    Filter filter;
    std::vector<FilterOperation>& operations = filter.operations;
    operations.reserve(functions.size());
    for (const detail::CSSTokenRange function : functions) {
        const auto parsedFunction = detail::parseCSSFunction(value.stream, function);
        if (!parsedFunction) return std::nullopt;

        if (parsedFunction->name == "blur") {
            const std::vector<detail::CSSTokenRange> arguments = detail::splitCSSComponents(value.stream, parsedFunction->body);
            BlurFilter blur;
            if (arguments.empty()) return std::nullopt;
            if (arguments.size() != 1) return std::nullopt;
            const std::optional<float> stdDeviation = parseStdDeviation(arguments.front());
            if (!stdDeviation) return std::nullopt;
            blur.stdDeviation = *stdDeviation;
            operations.emplace_back(blur);
        } else if (parsedFunction->name == "linear-blur") {
            const std::vector<detail::CSSTokenRange> arguments = detail::splitCSSOnDelimiter(value.stream, parsedFunction->body, ',');
            if (arguments.empty()) return std::nullopt;

            LinearBlurFilter linearBlur;
            std::size_t firstStop = 0;
            std::optional<ParsedStop> start = parseStop(arguments.front());
            if (!start) {
                if (!parseDirection(arguments.front(), linearBlur.angleDegrees)) return std::nullopt;
                firstStop = 1;
            }
            const std::size_t stopCount = arguments.size() - firstStop;
            if (stopCount == 0 || stopCount > 2) return std::nullopt;

            if (!start) start = parseStop(arguments[firstStop]);
            if (!start) return std::nullopt;
            if (stopCount == 1) {
                if (start->position) return std::nullopt;
                linearBlur.stops = {{start->stdDeviation, 0.f}, {start->stdDeviation, 1.f}};
            } else {
                const std::optional<ParsedStop> end = parseStop(arguments[firstStop + 1]);
                if (!end) return std::nullopt;
                linearBlur.stops = {{start->stdDeviation, start->position.value_or(0.f)}, {end->stdDeviation, end->position.value_or(1.f)}};
                if (linearBlur.stops[0].position > linearBlur.stops[1].position) return std::nullopt;
            }
            operations.emplace_back(std::move(linearBlur));
        } else return std::nullopt;
    }
    return filter;
}

std::optional<Outline> StyleModel::parseOutline(detail::CSSValueRange value, ColorSchemeMode scheme) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.size() < 2 || tokens.size() > 3) return std::nullopt;
    Outline outline;
    bool hasWidth = false;
    bool hasColor = false;
    bool hasStyle = false;
    for (const detail::CSSTokenRange rawToken : tokens) {
        const std::string token = normalizeCSSKeyword(value.stream, rawToken);
        if (const std::optional<Length> width = parseLengthValue(subValue(value, rawToken))) {
            const detail::CSSTokenRange trimmed = detail::trimCSSRange(value.stream, rawToken);
            if (hasWidth
                || value.stream.tokens()[trimmed.begin].kind == detail::CSSTokenKind::Percentage
                || width->percent != 0.f
                || width->pixels < 0.f)
                return std::nullopt;
            outline.width = width->pixels;
            hasWidth = true;
            continue;
        }

        if (token == "solid" || token == "dashed") {
            if (hasStyle) return std::nullopt;
            outline.style = token == "dashed" ? OutlineStyle::Dashed : OutlineStyle::Solid;
            hasStyle = true;
            continue;
        }

        const auto specifiedColor = consumeCSSColor(subValue(value, rawToken));
        const auto color = specifiedColor ? StyleColor::fromCSS(*specifiedColor, scheme) : std::nullopt;
        if (!color || hasColor) return std::nullopt;
        outline.color = *color;
        hasColor = true;
    }
    return hasWidth && hasColor ? std::optional<Outline>(outline) : std::nullopt;
}

} // namespace radia::ui
