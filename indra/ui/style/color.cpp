/**
 * @file color.cpp
 * @brief Parses CSS-style colors into typed UI color values.
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
#include "style/color.h"
#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <numbers>
#include <sstream>
#include <vector>
#include "style/syntax.h"

namespace radia::ui {
namespace {
constexpr float kPi = std::numbers::pi_v<float>;
using detail::lower;
using detail::trim;

bool parseFloat(const std::string& token, float& result) {
    char* end = nullptr;
    result = std::strtof(token.c_str(), &end);
    return end != token.c_str() && *end == '\0' && std::isfinite(result);
}

bool parsePercent(const std::string& token, float& result) {
    if (token.empty() || token.back() != '%') return false;
    if (!parseFloat(token.substr(0, token.size() - 1), result)) return false;
    result = std::clamp(result / 100.f, 0.f, 1.f);
    return true;
}

bool parseNumberOrPercent(const std::string& token, float percentScale, float& result) {
    if (!token.empty() && token.back() == '%') {
        if (!parseFloat(token.substr(0, token.size() - 1), result)) return false;
        result *= percentScale / 100.f;
        return true;
    }
    return parseFloat(token, result);
}

bool parseRgbChannel(const std::string& token, float& result) {
    if (parsePercent(token, result)) return true;
    if (!parseFloat(token, result)) return false;
    result = std::clamp(result / 255.f, 0.f, 1.f);
    return true;
}

bool parseAlpha(const std::string& token, float& result) {
    if (parsePercent(token, result)) return true;
    if (!parseFloat(token, result)) return false;
    result = std::clamp(result, 0.f, 1.f);
    return true;
}

bool parseHue(std::string token, float& degrees) {
    float scale = 1.f;
    if (token.size() >= 4 && token.compare(token.size() - 4, 4, "turn") == 0) {
        token.erase(token.size() - 4);
        scale = 360.f;
    } else if (token.size() >= 4 && token.compare(token.size() - 4, 4, "grad") == 0) {
        token.erase(token.size() - 4);
        scale = .9f;
    } else if (token.size() >= 3 && token.compare(token.size() - 3, 3, "deg") == 0) token.erase(token.size() - 3);
    else if (token.size() >= 3 && token.compare(token.size() - 3, 3, "rad") == 0) {
        token.erase(token.size() - 3);
        scale = 180.f / kPi;
    }
    if (!parseFloat(token, degrees)) return false;
    degrees = std::fmod(degrees * scale, 360.f);
    if (degrees < 0.f) degrees += 360.f;
    return true;
}

std::vector<std::string> splitComma(const std::string& value) {
    std::vector<std::string> result;
    std::size_t start = 0;
    while (start <= value.size()) {
        const std::size_t comma = value.find(',', start);
        result.push_back(trim(value.substr(start, comma == std::string::npos ? std::string::npos : comma - start)));
        if (comma == std::string::npos) break;
        start = comma + 1;
    }
    return result;
}

std::vector<std::string> splitSpace(const std::string& value) {
    std::stringstream stream(value);
    std::vector<std::string> result;
    std::string token;
    while (stream >> token) result.push_back(token);
    return result;
}

bool functionArguments(const std::string& body, std::vector<std::string>& channels, std::string& alpha) {
    if (body.find(',') != std::string::npos) {
        if (body.find('/') != std::string::npos) return false;
        channels = splitComma(body);
        if (channels.size() == 4) {
            alpha = channels.back();
            channels.pop_back();
        }
        return channels.size() == 3;
    }

    const std::size_t slash = body.find('/');
    if (slash != std::string::npos && body.find('/', slash + 1) != std::string::npos) return false;
    channels = splitSpace(trim(body.substr(0, slash)));
    if (slash != std::string::npos) alpha = trim(body.substr(slash + 1));
    return channels.size() == 3 && (slash == std::string::npos || !alpha.empty());
}

Color hsl(float hue, float saturation, float lightness, float alpha) {
    const float chroma = (1.f - std::abs(2.f * lightness - 1.f)) * saturation;
    const float segment = hue / 60.f;
    const float secondary = chroma * (1.f - std::abs(std::fmod(segment, 2.f) - 1.f));
    float r = 0.f, g = 0.f, b = 0.f;
    if (segment < 1.f) r = chroma, g = secondary;
    else if (segment < 2.f) r = secondary, g = chroma;
    else if (segment < 3.f) g = chroma, b = secondary;
    else if (segment < 4.f) g = secondary, b = chroma;
    else if (segment < 5.f) r = secondary, b = chroma;
    else r = chroma, b = secondary;
    const float match = lightness - chroma * .5f;
    return {r + match, g + match, b + match, alpha};
}

float srgb(float linear) {
    const float encoded = linear <= .0031308f ? 12.92f * linear : 1.055f * std::pow(linear, 1.f / 2.4f) - .055f;
    return std::clamp(encoded, 0.f, 1.f);
}

Color linearSrgb(float r, float g, float b, float alpha) {
    return {srgb(r), srgb(g), srgb(b), alpha};
}

Color hwb(float hue, float whiteness, float blackness, float alpha) {
    const float sum = whiteness + blackness;
    if (sum >= 1.f) {
        const float gray = whiteness / sum;
        return {gray, gray, gray, alpha};
    }

    const Color base = hsl(hue, 1.f, .5f, alpha);
    const float scale = 1.f - sum;
    return {base.r * scale + whiteness, base.g * scale + whiteness, base.b * scale + whiteness, alpha};
}

Color lab(float lightness, float a, float b, float alpha) {
    const float f1 = (lightness + 16.f) / 116.f;
    const float f0 = a / 500.f + f1;
    const float f2 = f1 - b / 200.f;
    auto inverse = [](float value) {
        const float cube = value * value * value;
        return cube > 216.f / 24389.f ? cube : (116.f * value - 16.f) / (24389.f / 27.f);
    };

    const float x50 = inverse(f0) * .964295676f;
    const float y50 = inverse(f1);
    const float z50 = inverse(f2) * .825104603f;
    const float x65 = .955473453f * x50 - .023098537f * y50 + .063259309f * z50;
    const float y65 = -.028369707f * x50 + 1.009995458f * y50 + .021041399f * z50;
    const float z65 = .012314002f * x50 - .020507696f * y50 + 1.330365937f * z50;
    return linearSrgb(3.240969942f * x65 - 1.537383178f * y65 - .498610760f * z65, -.969243636f * x65 + 1.875967502f * y65 + .041555057f * z65,
                      .055630080f * x65 - .203976959f * y65 + 1.056971514f * z65, alpha);
}

Color oklab(float lightness, float a, float b, float alpha) {
    const float lRoot = lightness + .3963377774f * a + .2158037573f * b;
    const float mRoot = lightness - .1055613458f * a - .0638541728f * b;
    const float sRoot = lightness - .0894841775f * a - 1.2914855480f * b;
    const float l = lRoot * lRoot * lRoot;
    const float m = mRoot * mRoot * mRoot;
    const float s = sRoot * sRoot * sRoot;
    return linearSrgb(4.0767416621f * l - 3.3077115913f * m + .2309699292f * s, -1.2684380046f * l + 2.6097574011f * m - .3413193965f * s,
                      -.0041960863f * l - .7034186147f * m + 1.7076147010f * s, alpha);
}

void polarCoordinates(float chroma, float hue, float& a, float& b) {
    const float radians = hue * kPi / 180.f;
    a = chroma * std::cos(radians);
    b = chroma * std::sin(radians);
}

int hexDigit(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

std::optional<Color> parseHex(const std::string& value) {
    if (value.size() != 4 && value.size() != 5 && value.size() != 7 && value.size() != 9) return std::nullopt;
    for (std::size_t i = 1; i < value.size(); ++i)
        if (hexDigit(value[i]) < 0) return std::nullopt;

    auto shortChannel = [&](std::size_t index) { return static_cast<float>(hexDigit(value[index])) / 15.f; };
    auto channel = [&](std::size_t index) { return static_cast<float>((hexDigit(value[index]) << 4) | hexDigit(value[index + 1])) / 255.f; };
    if (value.size() <= 5) return Color(shortChannel(1), shortChannel(2), shortChannel(3), value.size() == 5 ? shortChannel(4) : 1.f);
    return Color(channel(1), channel(3), channel(5), value.size() == 9 ? channel(7) : 1.f);
}
} // namespace

bool isColorSyntax(const std::string& raw) {
    const std::string value = lower(trim(raw));
    return (!value.empty() && value.front() == '#')
        || value == "transparent"
        || value.rfind("rgb(", 0) == 0
        || value.rfind("hsl(", 0) == 0
        || value.rfind("hwb(", 0) == 0
        || value.rfind("lab(", 0) == 0
        || value.rfind("lch(", 0) == 0
        || value.rfind("oklab(", 0) == 0
        || value.rfind("oklch(", 0) == 0;
}

std::optional<Color> parseColor(const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "transparent") return Color(0.f, 0.f, 0.f, 0.f);
    if (!value.empty() && value.front() == '#') return parseHex(value);

    const std::size_t open = value.find('(');
    if (open == std::string::npos || value.empty() || value.back() != ')') return std::nullopt;
    const std::string name = value.substr(0, open);
    if (name != "rgb" && name != "hsl" && name != "hwb" && name != "lab" && name != "lch" && name != "oklab" && name != "oklch") return std::nullopt;

    const std::string body = value.substr(open + 1, value.size() - open - 2);
    if (name != "rgb" && name != "hsl" && body.find(',') != std::string::npos) return std::nullopt;

    std::vector<std::string> channels;
    std::string alphaToken;
    if (!functionArguments(body, channels, alphaToken)) return std::nullopt;
    float alpha = 1.f;
    if (!alphaToken.empty() && !parseAlpha(alphaToken, alpha)) return std::nullopt;

    if (name == "rgb") {
        Color result;
        if (!parseRgbChannel(channels[0], result.r) || !parseRgbChannel(channels[1], result.g) || !parseRgbChannel(channels[2], result.b))
            return std::nullopt;
        result.a = alpha;
        return result;
    }

    if (name == "hsl" || name == "hwb") {
        float hue = 0.f, first = 0.f, second = 0.f;
        if (!parseHue(channels[0], hue) || !parsePercent(channels[1], first) || !parsePercent(channels[2], second)) return std::nullopt;
        return name == "hsl" ? hsl(hue, first, second, alpha) : hwb(hue, first, second, alpha);
    }

    float lightness = 0.f, first = 0.f, second = 0.f;
    const bool okSpace = name == "oklab" || name == "oklch";
    if (!parseNumberOrPercent(channels[0], okSpace ? 1.f : 100.f, lightness)) return std::nullopt;
    lightness = std::clamp(lightness, 0.f, okSpace ? 1.f : 100.f);

    const bool cylindrical = name == "lch" || name == "oklch";
    if (!parseNumberOrPercent(channels[1], okSpace ? .4f : (cylindrical ? 150.f : 125.f), first)) return std::nullopt;
    if (cylindrical) {
        float hue = 0.f;
        if (!parseHue(channels[2], hue)) return std::nullopt;
        polarCoordinates(std::max(0.f, first), hue, first, second);
    } else if (!parseNumberOrPercent(channels[2], okSpace ? .4f : 125.f, second)) return std::nullopt;

    return okSpace ? oklab(lightness, first, second, alpha) : lab(lightness, first, second, alpha);
}
} // namespace radia::ui
