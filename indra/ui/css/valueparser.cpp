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
#include <sstream>
#include "css/color.h"
#include "css/rules.h"
#include "css/syntax.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::lower;
using detail::startsWith;
using detail::trim;

bool parseFiniteFloat(const std::string& value, float& result) {
    char* end = nullptr;
    result = std::strtof(value.c_str(), &end);
    return end != value.c_str() && *end == '\0' && std::isfinite(result);
}

void assignColorValue(const StyleColorValue& value, Color& color, std::optional<LightDarkColor>& lightDarkColor) {
    if (const auto solid = std::get_if<Color>(&value)) {
        color = *solid;
        lightDarkColor.reset();
    } else {
        const LightDarkColor& themed = std::get<LightDarkColor>(value);
        color = themed.dark;
        lightDarkColor = themed;
    }
}
} // namespace

Color StyleModel::parseColorValue(const std::string& raw, const Color& fallback) const {
    const std::string value = trim(raw);
    const std::string lowered = lower(value);
    if (startsWith(lowered, "var(") && value.size() > 5 && value.back() == ')') return colorToken(trim(value.substr(4, value.size() - 5)), fallback);
    if (const std::optional<Color> parsed = parseColor(value)) return *parsed;
    return colorToken(value, fallback);
}

std::optional<StyleColorValue> StyleModel::parseColorChoiceValue(const std::string& raw) const {
    const std::string value = trim(raw);
    const std::string lowered = lower(value);
    constexpr char kLightDarkPrefix[] = "light-dark(";
    if (startsWith(lowered, kLightDarkPrefix)) {
        const std::size_t prefixSize = sizeof(kLightDarkPrefix) - 1;
        if (value.size() <= prefixSize || value.back() != ')') return std::nullopt;
        const std::vector<std::string> choices = detail::splitTopLevel(value.substr(prefixSize, value.size() - prefixSize - 1), ',');
        if (choices.size() != 2 || choices[0].empty() || choices[1].empty()) return std::nullopt;
        const Color marker(-1.f, -1.f, -1.f, -1.f);
        const Color light = parseColorValue(choices[0], marker);
        const Color dark = parseColorValue(choices[1], marker);
        if (light.a < 0.f || dark.a < 0.f) return std::nullopt;
        return LightDarkColor{light, dark};
    }

    const Color marker(-1.f, -1.f, -1.f, -1.f);
    const Color color = parseColorValue(value, marker);
    return color.a < 0.f ? std::nullopt : std::optional<StyleColorValue>(color);
}

float StyleModel::parseNumberValue(const std::string& raw, float fallback) const {
    std::string value = trim(raw);
    const std::string lowered = lower(value);
    if (startsWith(lowered, "var(") && value.size() > 5 && value.back() == ')') return numberToken(trim(value.substr(4, value.size() - 5)), fallback);
    if (endsWith(lowered, "px")) value = trim(value.substr(0, value.size() - 2));
    char* end = nullptr;
    const float parsed = std::strtof(value.c_str(), &end);
    return end != value.c_str() && *end == '\0' ? parsed : numberToken(value, fallback);
}

std::optional<Length> StyleModel::parseLengthValue(const std::string& raw) const {
    std::string value = trim(raw);
    const std::string lowered = lower(value);
    if (startsWith(lowered, "var(") && value.size() > 5 && value.back() == ')') {
        const float parsed = numberToken(trim(value.substr(4, value.size() - 5)), std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<Length>(Length{parsed}) : std::nullopt;
    }
    bool percentage = false;
    if (!value.empty() && value.back() == '%') {
        percentage = true;
        value = trim(value.substr(0, value.size() - 1));
    } else if (endsWith(lowered, "px")) value = trim(value.substr(0, value.size() - 2));

    char* end = nullptr;
    const float parsed = std::strtof(value.c_str(), &end);
    if (end == value.c_str() || *end != '\0' || !std::isfinite(parsed)) return std::nullopt;
    return percentage ? Length{0.f, parsed / 100.f} : Length{parsed};
}

std::optional<BorderRadii> StyleModel::parseBorderRadius(const std::string& raw) const {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(trim(raw), true);
    if (tokens.empty()) return std::nullopt;

    std::vector<std::string> horizontalTokens;
    std::vector<std::string> verticalTokens;
    bool sawSlash = false;
    for (const std::string& token : tokens) {
        if (token == "/") {
            if (sawSlash || horizontalTokens.empty()) return std::nullopt;
            sawSlash = true;
        } else if (sawSlash) verticalTokens.push_back(token);
        else horizontalTokens.push_back(token);
    }
    if (horizontalTokens.empty() || horizontalTokens.size() > 4 || (sawSlash && (verticalTokens.empty() || verticalTokens.size() > 4)))
        return std::nullopt;

    const auto expand = [this](const std::vector<std::string>& values) -> std::optional<std::array<Length, 4>> {
        std::array<Length, 4> expanded;
        std::vector<Length> parsed;
        parsed.reserve(values.size());
        for (const std::string& value : values) {
            const std::optional<Length> length = parseLengthValue(value);
            if (!length || length->pixels < 0.f || length->percent < 0.f) return std::nullopt;
            parsed.push_back(*length);
        }
        switch (parsed.size()) {
            case 1: expanded = {parsed[0], parsed[0], parsed[0], parsed[0]}; break;
            case 2: expanded = {parsed[0], parsed[1], parsed[0], parsed[1]}; break;
            case 3: expanded = {parsed[0], parsed[1], parsed[2], parsed[1]}; break;
            case 4: expanded = {parsed[0], parsed[1], parsed[2], parsed[3]}; break;
            default: return std::nullopt;
        }
        return expanded;
    };

    const std::optional<std::array<Length, 4>> horizontal = expand(horizontalTokens);
    const std::optional<std::array<Length, 4>> vertical = sawSlash ? expand(verticalTokens) : horizontal;
    if (!horizontal || !vertical) return std::nullopt;

    return BorderRadii{
        {horizontal->at(0), vertical->at(0)},
        {horizontal->at(1), vertical->at(1)},
        {horizontal->at(2), vertical->at(2)},
        {horizontal->at(3), vertical->at(3)},
    };
}

std::optional<Gradient> StyleModel::parseGradient(const std::string& raw) const {
    const std::string value = trim(raw);
    const std::string lowered = lower(value);
    std::string prefix;
    Gradient gradient;
    if (startsWith(lowered, "linear-gradient(")) prefix = "linear-gradient(";
    else if (startsWith(lowered, "repeating-linear-gradient(")) {
        prefix = "repeating-linear-gradient(";
        gradient.repeating = true;
    } else if (startsWith(lowered, "radial-gradient(")) {
        prefix = "radial-gradient(";
        gradient.kind = GradientKind::Radial;
    } else if (startsWith(lowered, "repeating-radial-gradient(")) {
        prefix = "repeating-radial-gradient(";
        gradient.kind = GradientKind::Radial;
        gradient.repeating = true;
    } else if (startsWith(lowered, "conic-gradient(")) {
        prefix = "conic-gradient(";
        gradient.kind = GradientKind::Conic;
        gradient.angleDegrees = 0.f;
    } else if (startsWith(lowered, "repeating-conic-gradient(")) {
        prefix = "repeating-conic-gradient(";
        gradient.kind = GradientKind::Conic;
        gradient.angleDegrees = 0.f;
        gradient.repeating = true;
    } else return std::nullopt;

    if (value.size() <= prefix.size() || value.back() != ')') return std::nullopt;
    std::vector<std::string> arguments = detail::splitTopLevel(value.substr(prefix.size(), value.size() - prefix.size() - 1), ',');
    if (arguments.size() < 2) return std::nullopt;

    auto parseAngle = [&](const std::string& token, float& degrees) {
        std::string number = lower(trim(token));
        float scale = 1.f;
        if (endsWith(number, "deg")) number.erase(number.size() - 3);
        else if (endsWith(number, "turn")) number.erase(number.size() - 4), scale = 360.f;
        else if (endsWith(number, "rad")) number.erase(number.size() - 3), scale = 57.2957795131f;
        else return false;
        if (!parseFiniteFloat(trim(number), degrees)) return false;
        degrees *= scale;
        return true;
    };

    auto parseCenter = [&](const std::vector<std::string>& tokens, Vec2& center) {
        if (tokens.empty() || tokens.size() > 2) return false;
        auto parsePercentage = [&](const std::string& token, float& percentage) {
            if (token.empty() || token.back() != '%') return false;
            if (!parseFiniteFloat(token.substr(0, token.size() - 1), percentage) || percentage < 0.f || percentage > 100.f) return false;
            percentage /= 100.f;
            return true;
        };
        auto horizontal = [&](const std::string& rawToken, float& result) {
            const std::string token = lower(trim(rawToken));
            if (token == "left") result = 0.f;
            else if (token == "center") result = .5f;
            else if (token == "right") result = 1.f;
            else if (!parsePercentage(token, result)) return false;
            return true;
        };
        auto vertical = [&](const std::string& rawToken, float& result) {
            const std::string token = lower(trim(rawToken));
            if (token == "bottom") result = 0.f;
            else if (token == "center") result = .5f;
            else if (token == "top") result = 1.f;
            else {
                if (!parsePercentage(token, result)) return false;
                result = 1.f - result;
            }
            return true;
        };

        if (tokens.size() == 1) {
            const std::string token = lower(trim(tokens.front()));
            if (token == "top" || token == "bottom") {
                center.x = .5f;
                return vertical(token, center.y);
            }
            center.y = .5f;
            return horizontal(token, center.x);
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

    auto parsesAsColorStop = [&](const std::string& argument) {
        const std::vector<std::string> tokens = detail::tokenizeTopLevel(argument);
        if (tokens.empty()) return false;
        return parseColorChoiceValue(tokens.front()).has_value();
    };

    std::size_t firstStop = 0;
    if (!parsesAsColorStop(arguments.front())) {
        const std::string prelude = lower(trim(arguments.front()));
        const std::vector<std::string> tokens = detail::tokenizeTopLevel(prelude);
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
            } else if (!parseAngle(prelude, gradient.angleDegrees)) return std::nullopt;
        } else if (gradient.kind == GradientKind::Radial) {
            auto at = std::find(tokens.begin(), tokens.end(), "at");
            const std::size_t descriptorCount = static_cast<std::size_t>(at - tokens.begin());
            if (descriptorCount > 1) return std::nullopt;
            if (descriptorCount == 1) {
                if (tokens.front() == "circle") gradient.radialShape = RadialGradientShape::Circle;
                else if (tokens.front() != "ellipse") return std::nullopt;
            }
            if (at != tokens.end()) {
                const std::vector<std::string> centerTokens(at + 1, tokens.end());
                if (!parseCenter(centerTokens, gradient.center)) return std::nullopt;
            }
        } else {
            std::size_t index = 0;
            if (index < tokens.size() && tokens[index] == "from") {
                if (++index == tokens.size() || !parseAngle(tokens[index++], gradient.angleDegrees)) return std::nullopt;
            }
            if (index < tokens.size() && tokens[index] == "at") {
                const std::vector<std::string> centerTokens(tokens.begin() + index + 1, tokens.end());
                if (!parseCenter(centerTokens, gradient.center)) return std::nullopt;
                index = tokens.size();
            }
            if (index != tokens.size()) return std::nullopt;
        }
        firstStop = 1;
    }

    if (arguments.size() - firstStop < 2) return std::nullopt;
    const float unspecified = std::numeric_limits<float>::quiet_NaN();
    auto parseStopPosition = [&](const std::string& rawPosition, float& position) {
        const std::string token = lower(trim(rawPosition));
        if (!token.empty() && token.back() == '%') {
            if (!parseFiniteFloat(token.substr(0, token.size() - 1), position)) return false;
            position /= 100.f;
        } else {
            float degrees = 0.f;
            if (gradient.kind != GradientKind::Conic || !parseAngle(token, degrees)) return false;
            position = degrees / 360.f;
        }
        return position >= 0.f && position <= 1.f;
    };
    for (std::size_t index = firstStop; index < arguments.size(); ++index) {
        const std::vector<std::string> tokens = detail::tokenizeTopLevel(arguments[index]);
        if (tokens.empty() || tokens.size() > 3) return std::nullopt;
        const std::optional<StyleColorValue> color = parseColorChoiceValue(tokens.front());
        if (!color) return std::nullopt;
        float position = unspecified;
        if (tokens.size() >= 2 && !parseStopPosition(tokens[1], position)) return std::nullopt;
        gradient.stops.emplace_back();
        assignColorValue(*color, gradient.stops.back().color, gradient.stops.back().lightDarkColor);
        gradient.stops.back().position = position;
        if (tokens.size() == 3) {
            if (!parseStopPosition(tokens[2], position)) return std::nullopt;
            gradient.stops.emplace_back();
            assignColorValue(*color, gradient.stops.back().color, gradient.stops.back().lightDarkColor);
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

std::optional<std::vector<BoxShadow>> StyleModel::parseShadows(const std::string& raw) const {
    const std::vector<std::string> entries = detail::splitTopLevel(raw, ',');
    if (entries.empty()) return std::nullopt;
    std::vector<BoxShadow> shadows;
    shadows.reserve(entries.size());
    for (const std::string& entry : entries) {
        std::vector<std::string> tokens = detail::tokenizeTopLevel(entry);
        if (tokens.size() < 3) return std::nullopt;
        BoxShadow shadow;
        const std::string modifier = lower(tokens.back());
        if (modifier == "inset" || modifier == "outset") {
            shadow.inset = modifier == "inset";
            tokens.pop_back();
        }
        if (tokens.size() < 3 || tokens.size() > 5) return std::nullopt;
        auto fixedLength = [&](const std::string& token, float& result) {
            const std::optional<Length> parsed = parseLengthValue(token);
            if (!parsed || parsed->percent != 0.f) return false;
            result = parsed->pixels;
            return true;
        };
        if (!fixedLength(tokens[0], shadow.horizontal) || !fixedLength(tokens[1], shadow.vertical)) return std::nullopt;
        std::size_t colorIndex = 2;
        if (tokens.size() >= 4) {
            if (!fixedLength(tokens[2], shadow.blur) || shadow.blur < 0.f) return std::nullopt;
            colorIndex = 3;
        }
        if (tokens.size() == 5) {
            if (!fixedLength(tokens[3], shadow.spread)) return std::nullopt;
            colorIndex = 4;
        }
        const std::optional<StyleColorValue> color = parseColorChoiceValue(tokens[colorIndex]);
        if (!color) return std::nullopt;
        assignColorValue(*color, shadow.color, shadow.lightDarkColor);
        shadows.push_back(shadow);
    }
    return shadows;
}

std::optional<std::vector<Effect>> StyleModel::parseEffects(const std::string& raw) const {
    const std::string value = trim(raw);
    if (lower(value) == "none") return std::vector<Effect>();
    const std::vector<std::string> functions = detail::splitTopLevel(value, ',');
    if (functions.empty() || functions.size() > kMaxEffectCount) return std::nullopt;

    auto parseDirection = [&](const std::string& rawDirection, float& degrees) {
        const std::string direction = lower(trim(rawDirection));
        if (direction == "to top") degrees = 0.f;
        else if (direction == "to top right" || direction == "to right top") degrees = 45.f;
        else if (direction == "to right") degrees = 90.f;
        else if (direction == "to bottom right" || direction == "to right bottom") degrees = 135.f;
        else if (direction == "to bottom") degrees = 180.f;
        else if (direction == "to bottom left" || direction == "to left bottom") degrees = 225.f;
        else if (direction == "to left") degrees = 270.f;
        else if (direction == "to top left" || direction == "to left top") degrees = 315.f;
        else {
            std::string number = direction;
            float scale = 1.f;
            if (endsWith(number, "deg")) number.erase(number.size() - 3);
            else if (endsWith(number, "turn")) number.erase(number.size() - 4), scale = 360.f;
            else if (endsWith(number, "rad")) number.erase(number.size() - 3), scale = 57.2957795131f;
            else return false;
            if (!parseFiniteFloat(trim(number), degrees)) return false;
            degrees *= scale;
        }
        return true;
    };
    auto fixedRadius = [&](const std::string& token, float& radius) {
        const std::optional<Length> parsed = parseLengthValue(token);
        if (!parsed || parsed->percent != 0.f || parsed->pixels < 0.f) return false;
        radius = parsed->pixels;
        return true;
    };
    auto percentagePosition = [&](const std::string& token, float& position) {
        const std::string value = lower(trim(token));
        if (!endsWith(value, "%")) return false;
        const std::optional<Length> parsed = parseLengthValue(value);
        if (!parsed || parsed->pixels != 0.f || parsed->percent < 0.f || parsed->percent > 1.f) return false;
        position = parsed->percent;
        return true;
    };

    std::vector<Effect> effects;
    effects.reserve(functions.size());
    bool sawLayerEffect = false;
    for (const std::string& function : functions) {
        const std::string lowered = lower(function);
        Effect effect;
        std::string prefix;
        if (startsWith(lowered, "background-blur(")) {
            if (sawLayerEffect) return std::nullopt;
            effect.kind = EffectKind::BackgroundBlur;
            prefix = "background-blur(";
        } else if (startsWith(lowered, "layer-blur(")) {
            sawLayerEffect = true;
            prefix = "layer-blur(";
        } else return std::nullopt;
        if (function.size() <= prefix.size() || function.back() != ')') return std::nullopt;

        const std::vector<std::string> arguments = detail::splitTopLevel(function.substr(prefix.size(), function.size() - prefix.size() - 1), ',');
        if (arguments.size() == 1) {
            const std::vector<std::string> radii = detail::tokenizeTopLevel(arguments.front());
            if (radii.size() != 1 || !fixedRadius(radii.front(), effect.startRadius)) return std::nullopt;
            effect.endRadius = effect.startRadius;
        } else if (arguments.size() == 3) {
            const std::vector<std::string> start = detail::tokenizeTopLevel(arguments[1]);
            const std::vector<std::string> end = detail::tokenizeTopLevel(arguments[2]);
            if (!parseDirection(arguments[0], effect.angleDegrees)
                || start.size() != 2
                || end.size() != 2
                || !fixedRadius(start[0], effect.startRadius)
                || !percentagePosition(start[1], effect.startPosition)
                || !fixedRadius(end[0], effect.endRadius)
                || !percentagePosition(end[1], effect.endPosition)
                || effect.startPosition > effect.endPosition)
                return std::nullopt;
        } else return std::nullopt;
        effects.push_back(effect);
    }
    return effects;
}

std::optional<Outline> StyleModel::parseOutline(const std::string& raw) const {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(raw);
    if (tokens.size() < 2 || tokens.size() > 3) return std::nullopt;
    Outline outline;
    bool hasWidth = false;
    bool hasColor = false;
    bool hasStyle = false;
    for (const std::string& rawToken : tokens) {
        const std::string rawValue = trim(rawToken);
        const std::string token = lower(rawValue);
        if (const std::optional<Length> width = parseLengthValue(rawValue)) {
            if (hasWidth || width->percent != 0.f || width->pixels < 0.f) return std::nullopt;
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

        const std::optional<StyleColorValue> color = parseColorChoiceValue(rawValue);
        if (!color || hasColor) return std::nullopt;
        assignColorValue(*color, outline.color, outline.lightDarkColor);
        hasColor = true;
    }
    return hasWidth && hasColor ? std::optional<Outline>(outline) : std::nullopt;
}

EdgeInsets StyleModel::parseEdgeInsets(const std::string& raw, const EdgeInsets& fallback) const {
    std::stringstream stream(raw);
    std::vector<float> values;
    std::string token;
    while (stream >> token) {
        const float value = parseNumberValue(token, -1.f);
        if (!std::isfinite(value) || value < 0.f || values.size() == 4) return fallback;
        values.push_back(value);
    }
    if (values.empty()) return fallback;
    EdgeInsets result;
    if (values.size() == 1) result.top = result.right = result.bottom = result.left = values[0];
    else if (values.size() == 2) result.top = result.bottom = values[0], result.right = result.left = values[1];
    else if (values.size() == 3) result.top = values[0], result.right = result.left = values[1], result.bottom = values[2];
    else result.top = values[0], result.right = values[1], result.bottom = values[2], result.left = values[3];
    return result;
}

std::optional<MarginInsets> StyleModel::parseMargin(const std::string& raw) const {
    std::stringstream stream(raw);
    std::vector<MarginValue> values;
    std::string token;
    while (stream >> token) {
        MarginValue value;
        if (lower(trim(token)) == "auto") value = MarginValue::automatic();
        else {
            const float number = parseNumberValue(token, std::numeric_limits<float>::quiet_NaN());
            if (!std::isfinite(number)) return std::nullopt;
            value = MarginValue::fromPixels(number);
        }
        if (values.size() == 4) return std::nullopt;
        values.push_back(value);
    }
    if (values.empty()) return std::nullopt;

    MarginValue top = values[0], right = values[0], bottom = values[0], left = values[0];
    if (values.size() == 2) right = left = values[1];
    else if (values.size() == 3) right = left = values[1], bottom = values[2];
    else if (values.size() == 4) right = values[1], bottom = values[2], left = values[3];

    return MarginInsets{top, right, bottom, left};
}
} // namespace radia::ui
