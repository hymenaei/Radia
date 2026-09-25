/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "css/color.h"
#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <numbers>
#include <sstream>
#include <string_view>
#include <utility>
#include <vector>
#include "css/syntax.h"

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

Color rgb(unsigned value) {
    return {static_cast<float>((value >> 16) & 0xff) / 255.f, static_cast<float>((value >> 8) & 0xff) / 255.f,
            static_cast<float>(value & 0xff) / 255.f, 1.f};
}

std::optional<Color> namedColor(const std::string& value) {
    static const std::pair<std::string_view, Color> colors[] = {
        {"aliceblue", rgb(0xf0f8ff)},
        {"antiquewhite", rgb(0xfaebd7)},
        {"aqua", rgb(0x00ffff)},
        {"aquamarine", rgb(0x7fffd4)},
        {"azure", rgb(0xf0ffff)},
        {"beige", rgb(0xf5f5dc)},
        {"bisque", rgb(0xffe4c4)},
        {"black", rgb(0x000000)},
        {"blanchedalmond", rgb(0xffebcd)},
        {"blue", rgb(0x0000ff)},
        {"blueviolet", rgb(0x8a2be2)},
        {"brown", rgb(0xa52a2a)},
        {"burlywood", rgb(0xdeb887)},
        {"cadetblue", rgb(0x5f9ea0)},
        {"chartreuse", rgb(0x7fff00)},
        {"chocolate", rgb(0xd2691e)},
        {"coral", rgb(0xff7f50)},
        {"cornflowerblue", rgb(0x6495ed)},
        {"cornsilk", rgb(0xfff8dc)},
        {"crimson", rgb(0xdc143c)},
        {"cyan", rgb(0x00ffff)},
        {"darkblue", rgb(0x00008b)},
        {"darkcyan", rgb(0x008b8b)},
        {"darkgoldenrod", rgb(0xb8860b)},
        {"darkgray", rgb(0xa9a9a9)},
        {"darkgrey", rgb(0xa9a9a9)},
        {"darkgreen", rgb(0x006400)},
        {"darkkhaki", rgb(0xbdb76b)},
        {"darkmagenta", rgb(0x8b008b)},
        {"darkolivegreen", rgb(0x556b2f)},
        {"darkorange", rgb(0xff8c00)},
        {"darkorchid", rgb(0x9932cc)},
        {"darkred", rgb(0x8b0000)},
        {"darksalmon", rgb(0xe9967a)},
        {"darkseagreen", rgb(0x8fbc8f)},
        {"darkslateblue", rgb(0x483d8b)},
        {"darkslategray", rgb(0x2f4f4f)},
        {"darkslategrey", rgb(0x2f4f4f)},
        {"darkturquoise", rgb(0x00ced1)},
        {"darkviolet", rgb(0x9400d3)},
        {"deeppink", rgb(0xff1493)},
        {"deepskyblue", rgb(0x00bfff)},
        {"dimgray", rgb(0x696969)},
        {"dimgrey", rgb(0x696969)},
        {"dodgerblue", rgb(0x1e90ff)},
        {"firebrick", rgb(0xb22222)},
        {"floralwhite", rgb(0xfffaf0)},
        {"forestgreen", rgb(0x228b22)},
        {"fuchsia", rgb(0xff00ff)},
        {"gainsboro", rgb(0xdcdcdc)},
        {"ghostwhite", rgb(0xf8f8ff)},
        {"gold", rgb(0xffd700)},
        {"goldenrod", rgb(0xdaa520)},
        {"gray", rgb(0x808080)},
        {"grey", rgb(0x808080)},
        {"green", rgb(0x008000)},
        {"greenyellow", rgb(0xadff2f)},
        {"honeydew", rgb(0xf0fff0)},
        {"hotpink", rgb(0xff69b4)},
        {"indianred", rgb(0xcd5c5c)},
        {"indigo", rgb(0x4b0082)},
        {"ivory", rgb(0xfffff0)},
        {"khaki", rgb(0xf0e68c)},
        {"lavender", rgb(0xe6e6fa)},
        {"lavenderblush", rgb(0xfff0f5)},
        {"lawngreen", rgb(0x7cfc00)},
        {"lemonchiffon", rgb(0xfffacd)},
        {"lightblue", rgb(0xadd8e6)},
        {"lightcoral", rgb(0xf08080)},
        {"lightcyan", rgb(0xe0ffff)},
        {"lightgoldenrodyellow", rgb(0xfafad2)},
        {"lightgray", rgb(0xd3d3d3)},
        {"lightgrey", rgb(0xd3d3d3)},
        {"lightgreen", rgb(0x90ee90)},
        {"lightpink", rgb(0xffb6c1)},
        {"lightsalmon", rgb(0xffa07a)},
        {"lightseagreen", rgb(0x20b2aa)},
        {"lightskyblue", rgb(0x87cefa)},
        {"lightslategray", rgb(0x778899)},
        {"lightslategrey", rgb(0x778899)},
        {"lightsteelblue", rgb(0xb0c4de)},
        {"lightyellow", rgb(0xffffe0)},
        {"lime", rgb(0x00ff00)},
        {"limegreen", rgb(0x32cd32)},
        {"linen", rgb(0xfaf0e6)},
        {"magenta", rgb(0xff00ff)},
        {"maroon", rgb(0x800000)},
        {"mediumaquamarine", rgb(0x66cdaa)},
        {"mediumblue", rgb(0x0000cd)},
        {"mediumorchid", rgb(0xba55d3)},
        {"mediumpurple", rgb(0x9370db)},
        {"mediumseagreen", rgb(0x3cb371)},
        {"mediumslateblue", rgb(0x7b68ee)},
        {"mediumspringgreen", rgb(0x00fa9a)},
        {"mediumturquoise", rgb(0x48d1cc)},
        {"mediumvioletred", rgb(0xc71585)},
        {"midnightblue", rgb(0x191970)},
        {"mintcream", rgb(0xf5fffa)},
        {"mistyrose", rgb(0xffe4e1)},
        {"moccasin", rgb(0xffe4b5)},
        {"navajowhite", rgb(0xffdead)},
        {"navy", rgb(0x000080)},
        {"oldlace", rgb(0xfdf5e6)},
        {"olive", rgb(0x808000)},
        {"olivedrab", rgb(0x6b8e23)},
        {"orange", rgb(0xffa500)},
        {"orangered", rgb(0xff4500)},
        {"orchid", rgb(0xda70d6)},
        {"palegoldenrod", rgb(0xeee8aa)},
        {"palegreen", rgb(0x98fb98)},
        {"paleturquoise", rgb(0xafeeee)},
        {"palevioletred", rgb(0xdb7093)},
        {"papayawhip", rgb(0xffefd5)},
        {"peachpuff", rgb(0xffdab9)},
        {"peru", rgb(0xcd853f)},
        {"pink", rgb(0xffc0cb)},
        {"plum", rgb(0xdda0dd)},
        {"powderblue", rgb(0xb0e0e6)},
        {"purple", rgb(0x800080)},
        {"rebeccapurple", rgb(0x663399)},
        {"red", rgb(0xff0000)},
        {"rosybrown", rgb(0xbc8f8f)},
        {"royalblue", rgb(0x4169e1)},
        {"saddlebrown", rgb(0x8b4513)},
        {"salmon", rgb(0xfa8072)},
        {"sandybrown", rgb(0xf4a460)},
        {"seagreen", rgb(0x2e8b57)},
        {"seashell", rgb(0xfff5ee)},
        {"sienna", rgb(0xa0522d)},
        {"silver", rgb(0xc0c0c0)},
        {"skyblue", rgb(0x87ceeb)},
        {"slateblue", rgb(0x6a5acd)},
        {"slategray", rgb(0x708090)},
        {"slategrey", rgb(0x708090)},
        {"snow", rgb(0xfffafa)},
        {"springgreen", rgb(0x00ff7f)},
        {"steelblue", rgb(0x4682b4)},
        {"tan", rgb(0xd2b48c)},
        {"teal", rgb(0x008080)},
        {"thistle", rgb(0xd8bfd8)},
        {"tomato", rgb(0xff6347)},
        {"turquoise", rgb(0x40e0d0)},
        {"violet", rgb(0xee82ee)},
        {"wheat", rgb(0xf5deb3)},
        {"white", rgb(0xffffff)},
        {"whitesmoke", rgb(0xf5f5f5)},
        {"yellow", rgb(0xffff00)},
        {"yellowgreen", rgb(0x9acd32)},
    };
    const auto found = std::find_if(std::begin(colors), std::end(colors), [&value](const auto& entry) { return entry.first == value; });
    return found == std::end(colors) ? std::nullopt : std::optional<Color>(found->second);
}
} // namespace

bool isColorSyntax(const std::string& raw) {
    const std::string value = lower(trim(raw));
    return (!value.empty() && value.front() == '#')
        || value == "transparent"
        || namedColor(value).has_value()
        || value.rfind("rgb(", 0) == 0
        || value.rfind("hsl(", 0) == 0
        || value.rfind("hwb(", 0) == 0
        || value.rfind("lab(", 0) == 0
        || value.rfind("lch(", 0) == 0
        || value.rfind("oklab(", 0) == 0
        || value.rfind("oklch(", 0) == 0
        || value.rfind("light-dark(", 0) == 0;
}

std::optional<Color> parseColor(const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "transparent") return Color(0.f, 0.f, 0.f, 0.f);
    if (const std::optional<Color> named = namedColor(value)) return named;
    if (!value.empty() && value.front() == '#') return parseHex(value);

    const std::size_t open = value.find('(');
    if (open == std::string::npos || value.empty() || value.back() != ')') return std::nullopt;
    const std::string name = value.substr(0, open);
    const bool supportedFunction =
        name == "rgb" || name == "hsl" || name == "hwb" || name == "lab" || name == "lch" || name == "oklab" || name == "oklch";
    if (!supportedFunction) return std::nullopt;

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
