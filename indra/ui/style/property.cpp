/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/property.h"
#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <initializer_list>
#include <iterator>
#include <limits>
#include <string>
#include <type_traits>
#include "css/color.h"
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::lower;
using detail::startsWith;
using detail::StylePropertyImpact;
using detail::trim;

std::optional<BorderStyle> parseBorderStyle(const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "solid") return BorderStyle::Solid;
    if (value == "outset") return BorderStyle::Outset;
    if (value == "inset") return BorderStyle::Inset;
    return std::nullopt;
}

bool parseStrokeCap(const std::string& raw, StrokeCap& cap) {
    const std::string value = lower(trim(raw));
    if (value == "butt") cap = StrokeCap::Butt;
    else if (value == "round") cap = StrokeCap::Round;
    else if (value == "square") cap = StrokeCap::Square;
    else return false;
    return true;
}
} // namespace

namespace {
StyleDeclaration makeDeclaration(std::string_view name, StyleValue value) {
    const detail::StylePropertyDefinition* property = detail::findStyleProperty(name);
    llassert_always(property != nullptr);
    llassert_always(property->apply != nullptr || std::holds_alternative<InitialStyleValue>(value));
    return {*property, std::move(value)};
}

std::vector<StyleDeclaration> makeDeclarations(std::initializer_list<std::pair<std::string_view, StyleValue>> values) {
    std::vector<StyleDeclaration> declarations;
    declarations.reserve(values.size());
    for (auto& value : values) declarations.push_back(makeDeclaration(value.first, std::move(value.second)));
    return declarations;
}

std::vector<StyleDeclaration> makeDeclarations(const detail::StylePropertyDefinition& property, StyleValue value) {
    if (property.longhands.empty()) return {{property, std::move(value)}};

    std::vector<StyleDeclaration> declarations;
    declarations.reserve(property.longhands.size());
    for (const std::string_view name : property.longhands) {
        const detail::StylePropertyDefinition* longhand = detail::findStyleProperty(name);
        llassert_always(longhand != nullptr);
        declarations.emplace_back(*longhand, value);
    }
    return declarations;
}
} // namespace

namespace {
void resetStyleProperty(ComputedStyle& style, const detail::StylePropertyDefinition& property) {
    if (!property.reset) {
        LL_ERRS("UI") << "Attempted to reset a style property without a reset function: " << property.name << LL_ENDL;
        return;
    }
    property.reset(style);
}

void clearExplicitInheritance(ComputedStyle& style, std::string_view propertyName) {
    auto& properties = style.explicitlyInheritedProperties;
    properties.erase(std::remove(properties.begin(), properties.end(), propertyName), properties.end());
}

void markExplicitInheritance(ComputedStyle& style, std::string_view propertyName) {
    if (std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        == style.explicitlyInheritedProperties.end())
        style.explicitlyInheritedProperties.push_back(propertyName);
}
} // namespace

void detail::applyStyleDeclaration(ComputedStyle& style, const StyleDeclaration& declaration) {
    const detail::StylePropertyDefinition& property = declaration.property.get();
    if (std::holds_alternative<InitialStyleValue>(declaration.value)) {
        clearExplicitInheritance(style, property.name);
        resetStyleProperty(style, property);
        if (property.specify) property.specify(style);
        return;
    }
    if (const auto keyword = std::get_if<StyleWideKeyword>(&declaration.value)) {
        clearExplicitInheritance(style, property.name);
        resetStyleProperty(style, property);
        const bool inherit = *keyword == StyleWideKeyword::Inherit || (*keyword == StyleWideKeyword::Unset && property.isInherited());
        if (inherit) {
            style.specifiedInheritedProperties &= static_cast<InheritedStyleProperties>(~property.inheritedBit());
            markExplicitInheritance(style, property.name);
        } else if (property.specify) property.specify(style);
        return;
    }
    clearExplicitInheritance(style, property.name);
    if (!property.apply) {
        LL_ERRS("UI") << "Attempted to apply a stylesheet declaration without an applicable property definition." << LL_ENDL;
        return;
    }
    property.apply(style, declaration.value);
    if (property.specify) property.specify(style);
}

std::optional<bool> StyleModel::parseFontStyleValue(const std::string& value) const {
    const std::string style = lower(trim(value));
    if (style == "normal") return false;
    if (style == "italic" || style == "oblique") return true;
    return std::nullopt;
}

std::optional<float> StyleModel::parseFontWeightValue(const std::string& value) const {
    const std::string weight = lower(trim(value));
    if (weight == "normal") return 400.f;
    if (weight == "bold") return 700.f;
    if (endsWith(weight, "px") || endsWith(weight, "%")) return std::nullopt;
    const float parsed = parseNumberValue(weight, std::numeric_limits<float>::quiet_NaN());
    return std::isfinite(parsed) && parsed >= 1.f && parsed <= 1000.f && std::floor(parsed) == parsed ? std::optional<float>(parsed) : std::nullopt;
}

std::optional<Length> StyleModel::parseLineHeightValue(const std::string& value) const {
    const std::optional<Length> parsed = parseLengthValue(value);
    return parsed && parsed->pixels >= 0.f && parsed->percent == 0.f ? parsed : std::nullopt;
}

std::optional<std::vector<StyleDeclaration>> StyleModel::parseFontShorthand(const std::string& value) const {
    std::vector<std::string> tokens = detail::tokenizeTopLevel(value, true);
    if (tokens.size() < 2 || lower(tokens.back()) != "sans") return std::nullopt;
    tokens.pop_back();

    std::optional<Length> lineHeight;
    std::size_t sizeIndex = tokens.size() - 1;
    const auto slash = std::find(tokens.begin(), tokens.end(), "/");
    if (slash != tokens.end()) {
        const std::size_t slashIndex = static_cast<std::size_t>(slash - tokens.begin());
        if (slashIndex == 0 || slashIndex + 2 != tokens.size() || std::find(slash + 1, tokens.end(), "/") != tokens.end()) return std::nullopt;
        sizeIndex = slashIndex - 1;
        lineHeight = parseLineHeightValue(tokens.back());
        if (!lineHeight) return std::nullopt;
    }

    const std::string size = lower(tokens[sizeIndex]);
    const float parsedSize = parseNumberValue(size, std::numeric_limits<float>::quiet_NaN());
    if (!std::isfinite(parsedSize) || parsedSize < 0.f || endsWith(size, "%")) return std::nullopt;

    bool italic = false;
    float weight = 400.f;
    bool sawStyle = false;
    bool sawWeight = false;
    for (std::size_t index = 0; index < sizeIndex; ++index) {
        const std::string token = lower(tokens[index]);
        if (token == "normal") {
            if (!sawStyle) sawStyle = true;
            else if (!sawWeight) sawWeight = true;
            else return std::nullopt;
            continue;
        }
        if (!sawStyle) {
            const std::optional<bool> parsedStyle = parseFontStyleValue(token);
            if (parsedStyle) {
                italic = *parsedStyle;
                sawStyle = true;
                continue;
            }
        }
        if (!sawWeight) {
            const std::optional<float> parsedWeight = parseFontWeightValue(token);
            if (parsedWeight) {
                weight = *parsedWeight;
                sawWeight = true;
                continue;
            }
        }
        return std::nullopt;
    }

    return makeDeclarations({
        {"font-style", italic},
        {"font-weight", weight},
        {"font-size", parsedSize},
        {"line-height", lineHeight},
        {"font-family", FontFamily::Sans},
    });
}

namespace { using CompileResult = detail::StyleCompileResult; } // namespace

struct detail::StyleCompileContext {
    const StyleModel& model;
    const detail::StylePropertyDefinition& property;
    const std::string& value;
    const std::string& selector;
    StyleSheetLoadResult& result;
    const std::string& sourceName;

    CompileResult invalid() const {
        result.error("stylesheet.property.value_invalid", "Invalid value for " + std::string(property.name) + ": " + value + ".", sourceName);
        return std::nullopt;
    }

    CompileResult compiled(StyleValue parsed) const { return std::vector<StyleDeclaration>{makeDeclaration(property.name, std::move(parsed))}; }

    std::optional<Color> color(const std::string& raw = {}) const {
        const Color marker(-1.f, -1.f, -1.f, -1.f);
        const Color parsed = model.parseColorValue(raw.empty() ? value : raw, marker);
        return parsed.a < 0.f ? std::nullopt : std::optional<Color>(parsed);
    }

    std::optional<StyleColorValue> colorValue(const std::string& raw = {}) const { return model.parseColorChoiceValue(raw.empty() ? value : raw); }

    std::optional<float> number(const std::string& raw = {}) const {
        const float parsed = model.parseNumberValue(raw.empty() ? value : raw, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    }

    std::optional<Length> length(const std::string& raw = {}) const { return model.parseLengthValue(raw.empty() ? value : raw); }

    std::optional<Length> nonnegativeLength(const std::string& raw) const {
        const std::optional<Length> parsed = length(raw);
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    }

    std::vector<std::string> tokens(bool splitSlash = true) const { return detail::tokenizeTopLevel(value, splitSlash); }
};

namespace {
CompileResult compileShadow(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseShadows(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

struct ParsedContent {
    bool valid = false;
    std::optional<std::string> value;
};

std::optional<std::pair<std::string, std::size_t>> parseCssString(const std::string& value, std::size_t start) {
    if (start >= value.size() || (value[start] != '\'' && value[start] != '"')) return std::nullopt;
    const char quote = value[start];
    std::string result;
    const auto appendCodePoint = [&result](std::uint32_t codePoint) {
        if (codePoint > 0x10ffff || (codePoint >= 0xd800 && codePoint <= 0xdfff)) return false;
        if (codePoint <= 0x7f) result.push_back(static_cast<char>(codePoint));
        else if (codePoint <= 0x7ff) {
            result.push_back(static_cast<char>(0xc0 | (codePoint >> 6)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        } else if (codePoint <= 0xffff) {
            result.push_back(static_cast<char>(0xe0 | (codePoint >> 12)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        } else {
            result.push_back(static_cast<char>(0xf0 | (codePoint >> 18)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 12) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        }
        return true;
    };

    for (std::size_t index = start + 1; index < value.size();) {
        const char character = value[index++];
        if (character == quote) return std::pair{std::move(result), index};
        if (character != '\\') {
            result.push_back(character);
            continue;
        }
        if (index >= value.size()) return std::nullopt;
        const char escaped = value[index++];
        if (escaped == '\n' || escaped == '\r' || escaped == '\f') continue;
        if (!std::isxdigit(static_cast<unsigned char>(escaped))) {
            result.push_back(escaped);
            continue;
        }
        std::uint32_t codePoint = 0;
        std::size_t digits = 0;
        --index;
        while (index < value.size() && digits < 6 && std::isxdigit(static_cast<unsigned char>(value[index]))) {
            const char digit = value[index++];
            codePoint = codePoint * 16
                + static_cast<std::uint32_t>(std::isdigit(static_cast<unsigned char>(digit))
                                                 ? digit - '0'
                                                 : std::tolower(static_cast<unsigned char>(digit)) - 'a' + 10);
            ++digits;
        }
        if (index < value.size() && std::isspace(static_cast<unsigned char>(value[index]))) ++index;
        if (!appendCodePoint(codePoint)) return std::nullopt;
    }
    return std::nullopt;
}

ParsedContent parseContent(const std::string& raw) {
    const std::string value = trim(raw);
    const std::string keyword = lower(value);
    if (keyword == "none" || keyword == "normal") return {true, std::nullopt};

    const auto primary = parseCssString(value, 0);
    if (!primary) return {};
    std::size_t position = primary->second;
    while (position < value.size() && std::isspace(static_cast<unsigned char>(value[position]))) ++position;
    if (position == value.size()) return {true, primary->first};
    if (value[position++] != '/') return {};
    while (position < value.size() && std::isspace(static_cast<unsigned char>(value[position]))) ++position;
    const auto alternative = parseCssString(value, position);
    if (!alternative) return {};
    position = alternative->second;
    while (position < value.size() && std::isspace(static_cast<unsigned char>(value[position]))) ++position;
    return position == value.size() ? ParsedContent{true, primary->first} : ParsedContent{};
}

CompileResult compileContent(detail::StyleCompileContext& context) {
    const ParsedContent parsed = parseContent(context.value);
    return parsed.valid ? context.compiled(parsed.value) : context.invalid();
}

CompileResult compileEffect(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseEffects(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOutline(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseOutline(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileBorderRadius(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseBorderRadius(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOutlineOffset(detail::StyleCompileContext& context) {
    if (endsWith(lower(trim(context.value)), "%")) return context.invalid();
    const auto parsed = context.length();
    return parsed && parsed->percent == 0.f ? context.compiled(*parsed) : context.invalid();
}

bool isEscaped(std::string_view value, std::size_t position) {
    std::size_t backslashes = 0;
    while (position > 0 && value[position - 1] == '\\') {
        --position;
        ++backslashes;
    }
    return (backslashes & 1U) != 0;
}

bool isValidUnquotedCSSUrl(std::string_view value) {
    const auto isHexDigit = [](char character) {
        const auto code = static_cast<unsigned char>(character);
        return (code >= '0' && code <= '9') || (code >= 'a' && code <= 'f') || (code >= 'A' && code <= 'F');
    };

    for (std::size_t position = 0; position < value.size();) {
        if (value[position] == '\\') {
            ++position;
            if (position == value.size()) return false;
            if (value[position] == '\r') {
                ++position;
                if (position < value.size() && value[position] == '\n') ++position;
                continue;
            }
            if (value[position] == '\n' || value[position] == '\f') {
                ++position;
                continue;
            }
            if (isHexDigit(value[position])) {
                std::size_t digits = 0;
                while (position < value.size() && digits < 6 && isHexDigit(value[position])) {
                    ++position;
                    ++digits;
                }
                if (position < value.size() && detail::isCSSWhitespace(value[position])) ++position;
            } else ++position;
            continue;
        }
        if (detail::isCSSWhitespace(value[position])
            || value[position] == '\''
            || value[position] == '"'
            || value[position] == '('
            || value[position] == ')')
            return false;
        ++position;
    }
    return true;
}

std::optional<std::string> parseCSSUrl(const std::string& raw) {
    const std::string value = trim(raw);
    const std::string lowered = lower(value);
    if (!startsWith(lowered, "url(") || value.size() < 5 || value.back() != ')') return std::nullopt;
    std::string url = trim(value.substr(4, value.size() - 5));
    if (url.size() >= 2 && (url.front() == '\'' || url.front() == '"')) {
        const char quote = url.front();
        std::size_t closing = 1;
        for (; closing < url.size(); ++closing)
            if (url[closing] == quote && !isEscaped(url, closing)) break;
        if (closing == url.size() || !trim(url.substr(closing + 1)).empty()) return std::nullopt;
        url = url.substr(1, closing - 1);
    } else if (!isValidUnquotedCSSUrl(url)) return std::nullopt;
    url = detail::decodeCSSIdentifier(url);
    return url.empty() ? std::nullopt : std::optional<std::string>(std::move(url));
}

std::optional<BackgroundLayer> parseBackgroundImage(const StyleModel& model, const std::string& raw) {
    const std::string value = trim(raw);
    BackgroundLayer layer;
    if (lower(value) == "none") return layer;
    if (const std::optional<std::string> url = parseCSSUrl(value)) {
        layer.resource = *url;
        return layer;
    }
    if (const std::optional<Gradient> gradient = model.parseGradient(value)) {
        layer.gradient = *gradient;
        return layer;
    }
    return std::nullopt;
}

template<typename Parse> std::optional<std::vector<BackgroundLayer>> parseBackgroundLayerList(const std::string& raw, Parse parse) {
    const std::vector<std::string> values = detail::splitTopLevel(raw, ',');
    if (values.empty()) return std::nullopt;
    std::vector<BackgroundLayer> result;
    result.reserve(values.size());
    for (const std::string& value : values) {
        const std::optional<BackgroundLayer> layer = parse(value);
        if (!layer) return std::nullopt;
        result.push_back(*layer);
    }
    return result;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundImages(const StyleModel& model, const std::string& raw) {
    return parseBackgroundLayerList(raw, [&model](const std::string& value) { return parseBackgroundImage(model, value); });
}

std::optional<Length> parsePositionHorizontal(const StyleModel& model, const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "left") return Length{0.f};
    if (value == "center") return Length{0.f, .5f};
    if (value == "right") return Length{0.f, 1.f};
    return model.parseLengthValue(raw);
}

std::optional<Length> parsePositionVertical(const StyleModel& model, const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "bottom") return Length{0.f};
    if (value == "center") return Length{0.f, .5f};
    if (value == "top") return Length{0.f, 1.f};
    const std::optional<Length> length = model.parseLengthValue(raw);
    if (!length) return std::nullopt;
    return Length{-length->pixels, 1.f - length->percent};
}

bool isPositionToken(const std::string& raw) {
    const std::string value = lower(trim(raw));
    return value == "left"
        || value == "right"
        || value == "top"
        || value == "bottom"
        || value == "center"
        || endsWith(value, "px")
        || (!value.empty() && (value.back() == '%' || value.find_first_of("0123456789.-") == 0));
}

std::optional<BackgroundPosition> parseBackgroundPosition(const StyleModel& model, const std::string& raw) {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(raw);
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 1) {
        const std::string value = lower(trim(tokens.front()));
        if (value == "top" || value == "bottom") return BackgroundPosition{{0.f, .5f}, *parsePositionVertical(model, value)};
        const std::optional<Length> x = parsePositionHorizontal(model, tokens.front());
        return x ? std::optional<BackgroundPosition>(BackgroundPosition{*x, {0.f, .5f}}) : std::nullopt;
    }

    std::optional<Length> x = parsePositionHorizontal(model, tokens[0]);
    std::optional<Length> y = parsePositionVertical(model, tokens[1]);
    if (!x || !y) {
        x = parsePositionHorizontal(model, tokens[1]);
        y = parsePositionVertical(model, tokens[0]);
    }
    return x && y ? std::optional<BackgroundPosition>(BackgroundPosition{*x, *y}) : std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundPositions(const StyleModel& model, const std::string& raw) {
    return parseBackgroundLayerList(raw, [&model](const std::string& value) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundPosition> position = parseBackgroundPosition(model, value);
        if (!position) return std::nullopt;
        BackgroundLayer layer;
        layer.position = *position;
        return layer;
    });
}

std::optional<BackgroundSize> parseBackgroundSize(const StyleModel& model, const std::string& raw) {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(raw);
    if (tokens.size() == 1) {
        const std::string value = lower(trim(tokens.front()));
        if (value == "cover") return BackgroundSize{BackgroundSizeMode::Cover};
        if (value == "contain") return BackgroundSize{BackgroundSizeMode::Contain};
        if (value == "auto") return BackgroundSize{};
    }
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 2 && lower(trim(tokens[0])) == "auto" && lower(trim(tokens[1])) == "auto") return BackgroundSize{};
    const auto parse = [&model](const std::string& value) -> std::optional<Length> {
        if (lower(trim(value)) == "auto") return std::optional<Length>{};
        const std::optional<Length> length = model.parseLengthValue(value);
        return length && length->pixels >= 0.f && length->percent >= 0.f ? length : std::nullopt;
    };
    const std::optional<Length> width = parse(tokens[0]);
    const std::optional<Length> height = parse(tokens.size() == 1 ? "auto" : tokens[1]);
    if ((!width && lower(trim(tokens[0])) != "auto") || (!height && tokens.size() == 2 && lower(trim(tokens[1])) != "auto")) return std::nullopt;
    return BackgroundSize{BackgroundSizeMode::Explicit, width, height};
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundSizes(const StyleModel& model, const std::string& raw) {
    return parseBackgroundLayerList(raw, [&model](const std::string& value) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundSize> size = parseBackgroundSize(model, value);
        if (!size) return std::nullopt;
        BackgroundLayer layer;
        layer.size = *size;
        return layer;
    });
}

std::optional<BackgroundRepeat> parseBackgroundRepeat(const std::string& raw) {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(raw);
    if (tokens.size() == 1) {
        const std::string value = lower(trim(tokens.front()));
        if (value == "repeat") return BackgroundRepeat::Repeat;
        if (value == "no-repeat") return BackgroundRepeat::NoRepeat;
        if (value == "repeat-x") return BackgroundRepeat::RepeatX;
        if (value == "repeat-y") return BackgroundRepeat::RepeatY;
    }
    if (tokens.size() == 2 && lower(trim(tokens[0])) == "repeat" && lower(trim(tokens[1])) == "no-repeat") return BackgroundRepeat::RepeatX;
    if (tokens.size() == 2 && lower(trim(tokens[0])) == "no-repeat" && lower(trim(tokens[1])) == "repeat") return BackgroundRepeat::RepeatY;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundRepeats(const std::string& raw) {
    return parseBackgroundLayerList(raw, [](const std::string& value) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat(value);
        if (!repeat) return std::nullopt;
        BackgroundLayer layer;
        layer.repeat = *repeat;
        return layer;
    });
}

std::optional<BackgroundBox> parseBackgroundBox(const std::string& raw) {
    const std::string value = lower(trim(raw));
    if (value == "border-box") return BackgroundBox::BorderBox;
    if (value == "padding-box") return BackgroundBox::PaddingBox;
    if (value == "content-box") return BackgroundBox::ContentBox;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundBoxes(const std::string& raw) {
    return parseBackgroundLayerList(raw, [](const std::string& value) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundBox> box = parseBackgroundBox(value);
        if (!box) return std::nullopt;
        BackgroundLayer layer;
        layer.origin = *box;
        layer.clip = *box;
        return layer;
    });
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundAttachments(const std::string& raw) {
    return parseBackgroundLayerList(raw, [](const std::string& value) -> std::optional<BackgroundLayer> {
        const std::string token = lower(trim(value));
        BackgroundLayer layer;
        if (token == "scroll") layer.attachment = BackgroundAttachment::Scroll;
        else if (token == "fixed") layer.attachment = BackgroundAttachment::Fixed;
        else if (token == "local") layer.attachment = BackgroundAttachment::Local;
        else return std::nullopt;
        return layer;
    });
}

std::optional<BackgroundAttachment> parseBackgroundAttachment(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "scroll") return BackgroundAttachment::Scroll;
    if (token == "fixed") return BackgroundAttachment::Fixed;
    if (token == "local") return BackgroundAttachment::Local;
    return std::nullopt;
}

void copyLayerComponent(BackgroundLayer& destination, const BackgroundLayer& source, StyleImageComponent component) {
    switch (component) {
        case StyleImageComponent::Image: destination.resource = source.resource, destination.gradient = source.gradient; break;
        case StyleImageComponent::Position: destination.position = source.position; break;
        case StyleImageComponent::Size: destination.size = source.size; break;
        case StyleImageComponent::Repeat: destination.repeat = source.repeat; break;
        case StyleImageComponent::Origin: destination.origin = source.origin; break;
        case StyleImageComponent::Clip: destination.clip = source.clip; break;
        case StyleImageComponent::Attachment: destination.attachment = source.attachment; break;
        case StyleImageComponent::All:
        case StyleImageComponent::Mode:
        case StyleImageComponent::Composite:
        case StyleImageComponent::Type: break;
    }
}

void copyLayerComponent(MaskLayer& destination, const MaskLayer& source, StyleImageComponent component) {
    switch (component) {
        case StyleImageComponent::Image:
            destination.image.resource = source.image.resource, destination.image.gradient = source.image.gradient;
            break;
        case StyleImageComponent::Position: destination.image.position = source.image.position; break;
        case StyleImageComponent::Size: destination.image.size = source.image.size; break;
        case StyleImageComponent::Repeat: destination.image.repeat = source.image.repeat; break;
        case StyleImageComponent::Origin: destination.image.origin = source.image.origin; break;
        case StyleImageComponent::Clip: destination.image.clip = source.image.clip; break;
        case StyleImageComponent::Attachment: destination.image.attachment = source.image.attachment; break;
        case StyleImageComponent::Mode: destination.mode = source.mode; break;
        case StyleImageComponent::Composite: destination.composite = source.composite; break;
        case StyleImageComponent::Type: destination.type = source.type; break;
        case StyleImageComponent::All: break;
    }
}

template<typename Layer, typename Value>
void applyLayerComponent(std::vector<Layer>& destination, const Value& value, StyleImageComponent component) {
    const auto& source = value.layers;
    if (component == StyleImageComponent::All) {
        destination = source;
        return;
    }
    if (source.empty()) return;
    if (destination.empty()) destination.resize(source.size());
    else if (component == StyleImageComponent::Image && destination.size() != source.size()) {
        const std::size_t previousSize = destination.size();
        std::vector<Layer> normalized(source.size());
        for (std::size_t index = 0; index < normalized.size() && previousSize > 0; ++index) normalized[index] = destination[index % previousSize];
        destination = std::move(normalized);
    } else if (destination.size() < source.size()) {
        const std::size_t previousSize = destination.size();
        destination.resize(source.size());
        for (std::size_t index = previousSize; index < destination.size(); ++index) destination[index] = destination[index % previousSize];
    }
    for (std::size_t index = 0; index < destination.size(); ++index) copyLayerComponent(destination[index], source[index % source.size()], component);
}

void applyBackgroundImages(ComputedStyle& style, const StyleValue& value) {
    const StyleImageLayers& layers = std::get<StyleImageLayers>(value);
    applyLayerComponent(style.backgroundLayers, layers, layers.component);
}

void applyMaskComponent(std::vector<MaskLayer>& destination, const StyleMaskLayers& value) {
    applyLayerComponent(destination, value, value.component);
}

MaskLayer makeMaskLayer(BackgroundLayer image) {
    MaskLayer result;
    result.image = std::move(image);
    result.image.origin = BackgroundBox::BorderBox;
    return result;
}

std::vector<MaskLayer> makeMaskLayers(const std::vector<BackgroundLayer>& images) {
    std::vector<MaskLayer> result;
    result.reserve(images.size());
    for (const BackgroundLayer& image : images) result.push_back(makeMaskLayer(image));
    return result;
}

std::optional<std::vector<MaskLayer>> parseMaskImages(const StyleModel& model, const std::string& raw) {
    const std::optional<std::vector<BackgroundLayer>> images =
        parseBackgroundLayerList(raw, [&model](const std::string& value) { return parseBackgroundImage(model, value); });
    if (!images) return std::nullopt;
    return makeMaskLayers(*images);
}

CompileResult compileBackgroundImage(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundImages(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileBackgroundPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Position});
}

CompileResult compileBackgroundSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Size});
}

CompileResult compileBackgroundRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats(context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Repeat});
}

CompileResult compileBackgroundBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes(context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, component});
}

CompileResult compileBackgroundOrigin(detail::StyleCompileContext& context) {
    return compileBackgroundBox(context, StyleImageComponent::Origin);
}

CompileResult compileBackgroundClip(detail::StyleCompileContext& context) {
    return compileBackgroundBox(context, StyleImageComponent::Clip);
}

CompileResult compileBackgroundAttachment(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundAttachments(context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Attachment});
}

CompileResult compileMaskImage(detail::StyleCompileContext& context) {
    const auto parsed = parseMaskImages(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileMaskPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Position});
}

CompileResult compileMaskSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes(context.model, context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Size});
}

CompileResult compileMaskRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats(context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Repeat});
}

CompileResult compileMaskBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes(context.value);
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), component});
}

CompileResult compileMaskOrigin(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Origin);
}

CompileResult compileMaskClip(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Clip);
}

std::optional<MaskMode> parseMaskMode(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "match-source") return MaskMode::MatchSource;
    if (token == "alpha") return MaskMode::Alpha;
    if (token == "luminance") return MaskMode::Luminance;
    return std::nullopt;
}

std::optional<MaskComposite> parseMaskComposite(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "add") return MaskComposite::Add;
    if (token == "subtract") return MaskComposite::Subtract;
    if (token == "intersect") return MaskComposite::Intersect;
    if (token == "exclude") return MaskComposite::Exclude;
    return std::nullopt;
}

std::optional<MaskType> parseMaskType(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "luminance") return MaskType::Luminance;
    if (token == "alpha") return MaskType::Alpha;
    return std::nullopt;
}

template<typename Enum> CompileResult compileMaskEnum(detail::StyleCompileContext& context, StyleImageComponent component,
                                                      std::optional<Enum> (*parse)(const std::string&), Enum MaskLayer::* member) {
    const std::vector<std::string> values = detail::splitTopLevel(context.value, ',');
    if (values.empty()) return context.invalid();
    std::vector<MaskLayer> layers;
    layers.reserve(values.size());
    for (const std::string& value : values) {
        MaskLayer layer;
        const std::optional<Enum> parsed = parse(value);
        if (!parsed) return context.invalid();
        layer.*member = *parsed;
        layers.push_back(std::move(layer));
    }
    return context.compiled(StyleMaskLayers{std::move(layers), component});
}

CompileResult compileMaskMode(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Mode, parseMaskMode, &MaskLayer::mode);
}

CompileResult compileMaskComposite(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Composite, parseMaskComposite, &MaskLayer::composite);
}

CompileResult compileMaskType(detail::StyleCompileContext& context) {
    return compileMaskEnum(context, StyleImageComponent::Type, parseMaskType, &MaskLayer::type);
}

struct ParsedImageLayer {
    BackgroundLayer image;
    std::vector<BackgroundBox> boxes;
    std::vector<std::string> extras;
    bool repeatSpecified = false;
    bool attachmentSpecified = false;
};

std::optional<ParsedImageLayer> parseImageLayer(const StyleModel& model, const std::string& raw) {
    const std::vector<std::string> tokens = detail::tokenizeTopLevel(raw, true);
    if (tokens.empty()) return std::nullopt;

    ParsedImageLayer result;
    std::vector<std::string> positions;
    std::vector<std::string> sizes;
    bool afterSlash = false;
    bool sawImage = false;
    for (std::size_t index = 0; index < tokens.size(); ++index) {
        const std::string& token = tokens[index];
        const auto parseRepeat = [&]() -> std::optional<BackgroundRepeat> {
            if (index + 1 < tokens.size() && tokens[index + 1] != "/") {
                if (const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat(token + " " + tokens[index + 1])) {
                    ++index;
                    return repeat;
                }
            }
            return parseBackgroundRepeat(token);
        };
        if (token == "/") {
            if (afterSlash) return std::nullopt;
            afterSlash = true;
        } else if (afterSlash) {
            if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
                if (result.repeatSpecified) return std::nullopt;
                result.image.repeat = *repeat;
                result.repeatSpecified = true;
            } else if (const std::optional<BackgroundBox> box = parseBackgroundBox(token)) result.boxes.push_back(*box);
            else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment(token)) {
                if (result.attachmentSpecified) return std::nullopt;
                result.image.attachment = *attachment;
                result.attachmentSpecified = true;
            } else if (parseBackgroundSize(model, token)) sizes.push_back(token);
            else result.extras.push_back(token);
        } else if (const std::optional<BackgroundLayer> image = parseBackgroundImage(model, token)) {
            if (sawImage) return std::nullopt;
            result.image.resource = image->resource;
            result.image.gradient = image->gradient;
            sawImage = true;
        } else if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
            if (result.repeatSpecified) return std::nullopt;
            result.image.repeat = *repeat;
            result.repeatSpecified = true;
        } else if (const std::optional<BackgroundBox> box = parseBackgroundBox(token)) result.boxes.push_back(*box);
        else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment(token)) {
            if (result.attachmentSpecified) return std::nullopt;
            result.image.attachment = *attachment;
            result.attachmentSpecified = true;
        } else if (isPositionToken(token)) positions.push_back(token);
        else result.extras.push_back(token);
    }
    if (positions.size() > 2 || sizes.size() > 2 || (afterSlash && sizes.empty())) return std::nullopt;
    if (!positions.empty()) {
        const std::optional<BackgroundPosition> position =
            parseBackgroundPosition(model, positions[0] + (positions.size() > 1 ? " " + positions[1] : ""));
        if (!position) return std::nullopt;
        result.image.position = *position;
    }
    if (!sizes.empty()) {
        const std::optional<BackgroundSize> size = parseBackgroundSize(model, sizes[0] + (sizes.size() > 1 ? " " + sizes[1] : ""));
        if (!size) return std::nullopt;
        result.image.size = *size;
    }
    if (result.boxes.size() > 2) return std::nullopt;
    return result;
}

bool applyImageBoxes(BackgroundLayer& image, const std::vector<BackgroundBox>& boxes, BackgroundBox defaultOrigin, BackgroundBox defaultClip) {
    if (boxes.empty()) {
        image.origin = defaultOrigin;
        image.clip = defaultClip;
    } else if (boxes.size() == 1) image.origin = image.clip = boxes.front();
    else if (boxes.size() == 2) {
        image.origin = boxes[0];
        image.clip = boxes[1];
    } else return false;
    return true;
}

CompileResult compileBackground(detail::StyleCompileContext& context) {
    const std::vector<std::string> layers = detail::splitTopLevel(context.value, ',');
    if (layers.empty()) return context.invalid();
    std::vector<BackgroundLayer> parsed;
    std::optional<StyleColorValue> color;
    bool currentColor = false;
    for (std::size_t layerIndex = 0; layerIndex < layers.size(); ++layerIndex) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer(context.model, layers[layerIndex]);
        if (!parsedLayer || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::PaddingBox, BackgroundBox::BorderBox))
            return context.invalid();
        for (const std::string& token : parsedLayer->extras) {
            if (const std::optional<StyleColorValue> parsedColor = context.colorValue(token)) {
                if (layerIndex + 1 != layers.size() || color) return context.invalid();
                color = *parsedColor;
            } else if (lower(trim(token)) == "currentcolor") {
                if (layerIndex + 1 != layers.size() || color || currentColor) return context.invalid();
                currentColor = true;
            } else return context.invalid();
        }
        parsed.push_back(std::move(parsedLayer->image));
    }
    StylePaint paint{Color(0.f, 0.f, 0.f, 0.f)};
    if (color) {
        if (const auto solid = std::get_if<Color>(&*color)) paint.color = *solid;
        else if (const auto current = std::get_if<LightDarkColor>(&*color)) {
            paint.color = current->dark;
            paint.lightDarkColor = *current;
        }
    }
    paint.currentColor = currentColor;
    std::vector<StyleDeclaration> declarations;
    declarations.push_back(makeDeclaration("background-color", paint));
    const auto imageValue = [&parsed](StyleImageComponent component) { return StyleImageLayers{parsed, component}; };
    declarations.push_back(makeDeclaration("background-image", imageValue(StyleImageComponent::Image)));
    declarations.push_back(makeDeclaration("background-position", imageValue(StyleImageComponent::Position)));
    declarations.push_back(makeDeclaration("background-size", imageValue(StyleImageComponent::Size)));
    declarations.push_back(makeDeclaration("background-repeat", imageValue(StyleImageComponent::Repeat)));
    declarations.push_back(makeDeclaration("background-origin", imageValue(StyleImageComponent::Origin)));
    declarations.push_back(makeDeclaration("background-clip", imageValue(StyleImageComponent::Clip)));
    declarations.push_back(makeDeclaration("background-attachment", imageValue(StyleImageComponent::Attachment)));
    return declarations;
}

CompileResult compileMask(detail::StyleCompileContext& context) {
    const std::vector<std::string> layers = detail::splitTopLevel(context.value, ',');
    if (layers.empty()) return context.invalid();
    std::vector<MaskLayer> parsed;
    for (const std::string& rawLayer : layers) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer(context.model, rawLayer);
        if (!parsedLayer
            || parsedLayer->attachmentSpecified
            || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::BorderBox, BackgroundBox::BorderBox))
            return context.invalid();
        MaskLayer layer;
        layer.image = std::move(parsedLayer->image);
        bool modeSpecified = false;
        bool compositeSpecified = false;
        for (const std::string& token : parsedLayer->extras) {
            if (const std::optional<MaskMode> mode = parseMaskMode(token)) {
                if (modeSpecified) return context.invalid();
                layer.mode = *mode;
                modeSpecified = true;
            } else if (const std::optional<MaskComposite> composite = parseMaskComposite(token)) {
                if (compositeSpecified) return context.invalid();
                layer.composite = *composite;
                compositeSpecified = true;
            } else return context.invalid();
        }
        parsed.push_back(std::move(layer));
    }
    std::vector<StyleDeclaration> declarations;
    const auto imageValue = [&parsed](StyleImageComponent component) { return StyleMaskLayers{parsed, component}; };
    declarations.push_back(makeDeclaration("mask-image", imageValue(StyleImageComponent::Image)));
    declarations.push_back(makeDeclaration("mask-mode", imageValue(StyleImageComponent::Mode)));
    declarations.push_back(makeDeclaration("mask-position", imageValue(StyleImageComponent::Position)));
    declarations.push_back(makeDeclaration("mask-size", imageValue(StyleImageComponent::Size)));
    declarations.push_back(makeDeclaration("mask-repeat", imageValue(StyleImageComponent::Repeat)));
    declarations.push_back(makeDeclaration("mask-origin", imageValue(StyleImageComponent::Origin)));
    declarations.push_back(makeDeclaration("mask-clip", imageValue(StyleImageComponent::Clip)));
    declarations.push_back(makeDeclaration("mask-composite", imageValue(StyleImageComponent::Composite)));
    declarations.push_back(makeDeclaration("mask-type", imageValue(StyleImageComponent::Type)));
    return declarations;
}

CompileResult compilePaint(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (const std::optional<Gradient> gradient = model.parseGradient(value)) return context.compiled(StylePaint{Color(), *gradient});
    if (detail::lower(detail::trim(value)) == "currentcolor") return context.compiled(StylePaint{Color(), std::nullopt, std::nullopt, true});
    const auto parsed = context.colorValue();
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(StylePaint{*color, std::nullopt});
    return context.compiled(StylePaint{Color(0.f, 0.f, 0.f, 0.f), std::nullopt, std::get<LightDarkColor>(*parsed)});
}

CompileResult compileColorValue(detail::StyleCompileContext& context) {
    const auto parsed = context.colorValue();
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(*color);
    return context.compiled(std::get<LightDarkColor>(*parsed));
}

CompileResult compileColor(detail::StyleCompileContext& context) {
    return compileColorValue(context);
}

CompileResult compileAccentColor(detail::StyleCompileContext& context) {
    const std::string value = lower(trim(context.value));
    if (value == "auto") return context.compiled(AccentColor{});
    if (value == "currentcolor") return context.compiled(AccentColor::currentColor());
    const auto parsed = context.colorValue();
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(AccentColor::fromColor(*color));
    return context.compiled(AccentColor::fromLightDark(std::get<LightDarkColor>(*parsed)));
}

CompileResult compileBorder(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() < 2 || tokens.size() > 3) return context.invalid();
    const auto width = context.number(tokens[0]);
    if (!width || *width < 0.f) return context.invalid();
    BorderStyle borderStyle = BorderStyle::Solid;
    const std::string& colorToken = tokens.size() == 2 ? tokens[1] : tokens[2];
    if (tokens.size() == 3) {
        const std::optional<BorderStyle> parsedStyle = parseBorderStyle(tokens[1]);
        if (!parsedStyle) return context.invalid();
        borderStyle = *parsedStyle;
    }
    if (detail::lower(detail::trim(colorToken)) == "currentcolor")
        return context.compiled(StyleBorder{*width, StylePaint{Color(), std::nullopt, std::nullopt, true}, borderStyle});
    if (const std::optional<Gradient> gradient = model.parseGradient(colorToken))
        return context.compiled(StyleBorder{*width, StylePaint{Color(), *gradient}, borderStyle});
    const auto parsed = context.colorValue(colorToken);
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(StyleBorder{*width, StylePaint{*color, std::nullopt}, borderStyle});
    return context.compiled(StyleBorder{*width, StylePaint{Color(0.f, 0.f, 0.f, 0.f), std::nullopt, std::get<LightDarkColor>(*parsed)}, borderStyle});
}

CompileResult compileBorderStyle(detail::StyleCompileContext& context) {
    const std::optional<BorderStyle> parsed = parseBorderStyle(context.value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileEdges(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const float nan = std::numeric_limits<float>::quiet_NaN();
    const EdgeInsets parsed = model.parseEdgeInsets(value, {nan, nan, nan, nan});
    return std::isfinite(parsed.top) ? context.compiled(parsed) : context.invalid();
}

CompileResult compileMargin(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseMargin(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileGap(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (lower(trim(value)) == "auto") return context.compiled(GapValue::automatic());
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(GapValue::fromPixels(*parsed)) : context.invalid();
}

CompileResult compileSize(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto dimension = [&context](const std::string& raw) -> std::optional<Dimension> {
        if (lower(trim(raw)) == "auto") return Dimension();
        const auto parsed = context.nonnegativeLength(raw);
        return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
    };
    const auto height = dimension(tokens[0]);
    const auto width = dimension(tokens.size() == 1 ? tokens[0] : tokens[1]);
    return height && width ? context.compiled(StyleSize{*height, *width}) : context.invalid();
}

CompileResult compileMinSize(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto height = context.nonnegativeLength(tokens[0]);
    const auto width = context.nonnegativeLength(tokens.size() == 1 ? tokens[0] : tokens[1]);
    if (!height || !width) return context.invalid();
    return makeDeclarations({{"min-height", *height}, {"min-width", *width}});
}

CompileResult compileStrokeLinecap(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    StrokeCap cap;
    return parseStrokeCap(value, cap) ? context.compiled(cap) : context.invalid();
}

CompileResult compileFontFamily(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    return lower(trim(value)) == "sans" ? context.compiled(FontFamily::Sans) : context.invalid();
}

CompileResult compileFont(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseFontShorthand(value);
    return parsed ? parsed : context.invalid();
}

CompileResult compileFontWeight(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseFontWeightValue(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileFontStyle(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseFontStyleValue(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileTextDecoration(detail::StyleCompileContext& context) {
    const std::string decoration = lower(trim(context.value));
    if (decoration == "none") return context.compiled(TextDecoration::NoneValue);
    if (decoration == "underline") return context.compiled(TextDecoration::Underline);
    if (decoration == "line-through") return context.compiled(TextDecoration::LineThrough);
    return context.invalid();
}

CompileResult compileTextAlign(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string alignment = lower(trim(value));
    std::optional<TextAlign> parsed;
    if (alignment == "left") parsed = TextAlign::Left;
    else if (alignment == "start") parsed = TextAlign::Start;
    else if (alignment == "center") parsed = TextAlign::Center;
    else if (alignment == "right") parsed = TextAlign::Right;
    else if (alignment == "end") parsed = TextAlign::End;
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileTextOverflow(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string overflow = lower(trim(value));
    if (overflow == "clip") return context.compiled(TextOverflow::Clip);
    if (overflow == "ellipsis") return context.compiled(TextOverflow::Ellipsis);
    if (overflow == "ellipsis-center") return context.compiled(TextOverflow::EllipsisCenter);
    return context.invalid();
}

CompileResult compileTextWrap(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string wrap = lower(trim(value));
    if (wrap == "wrap") return context.compiled(TextWrap::Wrap);
    if (wrap == "nowrap") return context.compiled(TextWrap::NoWrap);
    return context.invalid();
}

CompileResult compileVerticalAlign(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string alignment = lower(trim(value));
    std::optional<VerticalAlign> parsed;
    if (alignment == "top") parsed = VerticalAlign::Top;
    else if (alignment == "middle") parsed = VerticalAlign::Middle;
    else if (alignment == "bottom") parsed = VerticalAlign::Bottom;
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileFlexDirection(detail::StyleCompileContext& context) {
    const std::string direction = lower(trim(context.value));
    if (direction == "row") return context.compiled(FlexDirection::Row);
    if (direction == "column") return context.compiled(FlexDirection::Column);
    return context.invalid();
}

template<typename Enum, std::size_t Size> CompileResult compileAlignment(const detail::StyleCompileContext& context, const std::string& value,
                                                                         const std::array<std::pair<std::string_view, Enum>, Size>& values) {
    const std::string normalized = lower(trim(value));
    const auto found = std::find_if(values.begin(), values.end(), [&normalized](const auto& entry) { return entry.first == normalized; });
    return found == values.end() ? context.invalid() : context.compiled(found->second);
}

CompileResult compileJustifyContent(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, JustifyContent>, 5> sJustifyContentValues{{
        {"start", JustifyContent::Start},
        {"left", JustifyContent::Left},
        {"center", JustifyContent::Center},
        {"end", JustifyContent::End},
        {"right", JustifyContent::Right},
    }};
    return compileAlignment(context, value, sJustifyContentValues);
}

CompileResult compileAlignItems(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignItems>, 5> sAlignItemsValues{{
        {"normal", AlignItems::Normal},
        {"start", AlignItems::Start},
        {"center", AlignItems::Center},
        {"end", AlignItems::End},
        {"stretch", AlignItems::Stretch},
    }};
    return compileAlignment(context, value, sAlignItemsValues);
}

CompileResult compileInternalAlignContentBlock(detail::StyleCompileContext& context) {
    const std::string alignment = lower(trim(context.value));
    if (alignment == "normal") return context.compiled(false);
    if (alignment == "center") return context.compiled(true);
    return context.invalid();
}

CompileResult compileAlignSelf(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignSelf>, 5> sAlignSelfValues{{
        {"auto", AlignSelf::Auto},
        {"start", AlignSelf::Start},
        {"center", AlignSelf::Center},
        {"end", AlignSelf::End},
        {"stretch", AlignSelf::Stretch},
    }};
    return compileAlignment(context, value, sAlignSelfValues);
}

CompileResult compileJustifySelf(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, JustifySelf>, 5> sJustifySelfValues{{
        {"auto", JustifySelf::Auto},
        {"start", JustifySelf::Start},
        {"center", JustifySelf::Center},
        {"end", JustifySelf::End},
        {"stretch", JustifySelf::Stretch},
    }};
    return compileAlignment(context, value, sJustifySelfValues);
}

CompileResult compileFlex(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.empty() || tokens.size() > 3) return context.invalid();
    const std::string keyword = lower(trim(value));
    if (keyword == "none") return makeDeclarations({{"flex-grow", 0.f}, {"flex-shrink", 0.f}, {"flex-basis", Dimension()}});
    if (keyword == "auto") return makeDeclarations({{"flex-grow", 1.f}, {"flex-shrink", 1.f}, {"flex-basis", Dimension()}});

    const auto nonnegativeNumber = [&context](const std::string& raw) -> std::optional<float> {
        const std::string token = lower(trim(raw));
        if (endsWith(token, "px") || endsWith(token, "%")) return std::nullopt;
        const auto parsed = context.number(raw);
        return parsed && *parsed >= 0.f ? parsed : std::nullopt;
    };
    const auto basis = [&context](const std::string& raw) -> std::optional<Dimension> {
        if (lower(trim(raw)) == "auto") return Dimension();
        const auto parsed = context.nonnegativeLength(raw);
        return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
    };

    float grow = 1.f;
    float shrink = 1.f;
    Dimension flexBasis = Dimension::fromLength(Length{});
    if (tokens.size() == 1) {
        if (const auto parsed = nonnegativeNumber(tokens[0])) grow = *parsed;
        else if (const auto parsed = basis(tokens[0])) flexBasis = *parsed;
        else return context.invalid();
    } else if (tokens.size() == 2) {
        const auto parsedGrow = nonnegativeNumber(tokens[0]);
        if (!parsedGrow) return context.invalid();
        grow = *parsedGrow;
        if (const auto parsedShrink = nonnegativeNumber(tokens[1])) shrink = *parsedShrink;
        else if (const auto parsedBasis = basis(tokens[1])) flexBasis = *parsedBasis;
        else return context.invalid();
    } else {
        const auto parsedGrow = nonnegativeNumber(tokens[0]);
        const auto parsedShrink = nonnegativeNumber(tokens[1]);
        const auto parsedBasis = basis(tokens[2]);
        if (!parsedGrow || !parsedShrink || !parsedBasis) return context.invalid();
        grow = *parsedGrow;
        shrink = *parsedShrink;
        flexBasis = *parsedBasis;
    }
    return makeDeclarations({{"flex-grow", grow}, {"flex-shrink", shrink}, {"flex-basis", flexBasis}});
}

CompileResult compilePointerEvents(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, PointerEvents>, 3> sPointerEventsValues{{
        {"auto", PointerEvents::Auto},
        {"none", PointerEvents::PassThrough},
        {"default", PointerEvents::Default},
    }};
    return compileAlignment(context, value, sPointerEventsValues);
}

CompileResult compileDisplay(detail::StyleCompileContext& context) {
    const std::string display = lower(trim(context.value));
    if (display == "none") return context.compiled(DisplayMode::NoneValue);
    if (display == "flex") return context.compiled(DisplayMode::Flex);
    if (display == "inline-flex") return context.compiled(DisplayMode::InlineFlex);
    if (display == "grid") return context.compiled(DisplayMode::Grid);
    if (display == "inline-grid") return context.compiled(DisplayMode::InlineGrid);
    if (display == "block") return context.compiled(DisplayMode::Block);
    if (display == "inline-block") return context.compiled(DisplayMode::InlineBlock);
    if (display == "inline") return context.compiled(DisplayMode::Inline);
    return context.invalid();
}

CompileResult compileAppearance(detail::StyleCompileContext& context) {
    const std::string appearance = lower(trim(context.value));
    if (appearance == "auto") return context.compiled(AppearanceMode::Auto);
    if (appearance == "base") return context.compiled(AppearanceMode::Base);
    if (appearance == "none") return context.compiled(AppearanceMode::Unstyled);
    return context.invalid();
}

CompileResult compileBoxSizing(detail::StyleCompileContext& context) {
    const std::string boxSizing = lower(trim(context.value));
    if (boxSizing == "content-box") return context.compiled(BoxSizing::ContentBox);
    if (boxSizing == "border-box") return context.compiled(BoxSizing::BorderBox);
    return context.invalid();
}

CompileResult compileColorScheme(detail::StyleCompileContext& context) {
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() == 1) {
        const std::string scheme = lower(trim(tokens.front()));
        if (scheme == "auto" || scheme == "normal") return context.compiled(ColorScheme::Auto);
        if (scheme == "light") return context.compiled(ColorScheme::Light);
        if (scheme == "dark") return context.compiled(ColorScheme::Dark);
    } else if (tokens.size() == 2) {
        const std::string first = lower(trim(tokens[0]));
        const std::string second = lower(trim(tokens[1]));
        if ((first == "light" && second == "dark") || (first == "dark" && second == "light")) return context.compiled(ColorScheme::LightDark);
    }
    return context.invalid();
}

CompileResult compileGridArea(detail::StyleCompileContext& context) {
    const std::vector<std::string> tokens = context.tokens(true);
    if (tokens.size() != 3 || tokens[1] != "/") return context.invalid();
    const auto line = [&context](const std::string& raw) -> std::optional<int> {
        const std::string token = lower(trim(raw));
        if (endsWith(token, "px") || endsWith(token, "%")) return std::nullopt;
        const auto parsed = context.number(raw);
        if (!parsed || *parsed < 1.f || std::floor(*parsed) != *parsed || *parsed > static_cast<float>(std::numeric_limits<int>::max()))
            return std::nullopt;
        return static_cast<int>(*parsed);
    };
    const auto row = line(tokens[0]);
    const auto column = line(tokens[2]);
    return row && column ? context.compiled(GridArea{*row, *column}) : context.invalid();
}

CompileResult compilePositionMode(detail::StyleCompileContext& context) {
    const std::string position = lower(trim(context.value));
    if (position == "static") return context.compiled(PositionMode::Static);
    if (position == "relative") return context.compiled(PositionMode::Relative);
    return context.invalid();
}

CompileResult compileTranslate(detail::StyleCompileContext& context) {
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto fixedLength = [&context](const std::string& raw) -> std::optional<float> {
        const auto parsed = context.length(raw);
        return parsed && parsed->percent == 0.f ? std::optional<float>(parsed->pixels) : std::nullopt;
    };
    const auto x = fixedLength(tokens[0]);
    const auto y = fixedLength(tokens.size() == 1 ? std::string("0") : tokens[1]);
    return x && y ? context.compiled(Translate{*x, *y}) : context.invalid();
}

CompileResult compileVisibility(detail::StyleCompileContext& context) {
    const std::string visibility = lower(trim(context.value));
    if (visibility == "visible") return context.compiled(Visibility::Visible);
    if (visibility == "hidden") return context.compiled(Visibility::Hidden);
    if (visibility == "collapse") return context.compiled(Visibility::Collapse);
    return context.invalid();
}

std::optional<Overflow> parseOverflow(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "visible") return Overflow::Visible;
    if (token == "hidden") return Overflow::Hidden;
    if (token == "scroll") return Overflow::Scroll;
    if (token == "auto") return Overflow::Auto;
    return std::nullopt;
}

CompileResult compileOverflow(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto horizontal = parseOverflow(tokens[0]);
    const auto vertical = parseOverflow(tokens.size() == 1 ? tokens[0] : tokens[1]);
    if (!horizontal || !vertical) return context.invalid();
    return makeDeclarations({{"overflow-x", *horizontal}, {"overflow-y", *vertical}});
}

CompileResult compileOverflowAxis(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = parseOverflow(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileScrollbarMode(detail::StyleCompileContext& context) {
    const std::string mode = lower(trim(context.value));
    if (mode == "classic") return context.compiled(ScrollbarMode::Classic);
    if (mode == "overlay") return context.compiled(ScrollbarMode::Overlay);
    return context.invalid();
}

CompileResult compileScrollbarWidth(detail::StyleCompileContext& context) {
    const std::string width = lower(trim(context.value));
    if (width == "auto") return context.compiled(ScrollbarWidth::Auto);
    if (width == "thin") return context.compiled(ScrollbarWidth::Thin);
    if (width == "none") return context.compiled(ScrollbarWidth::NoneValue);
    return context.invalid();
}

CompileResult compileScrollbarGutter(detail::StyleCompileContext& context) {
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() == 1) {
        const std::string gutter = lower(trim(tokens[0]));
        if (gutter == "auto") return context.compiled(ScrollbarGutter::Auto);
        if (gutter == "stable") return context.compiled(ScrollbarGutter::Stable);
    } else if (tokens.size() == 2 && lower(trim(tokens[0])) == "stable" && lower(trim(tokens[1])) == "both-edges")
        return context.compiled(ScrollbarGutter::StableBothEdges);
    return context.invalid();
}

CompileResult compileScrollbarColor(detail::StyleCompileContext& context) {
    const std::string value = lower(trim(context.value));
    if (value == "auto") return context.compiled(ScrollbarColors{});
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() != 2) return context.invalid();
    const std::optional<StyleColorValue> thumb = context.colorValue(tokens[0]);
    const std::optional<StyleColorValue> track = context.colorValue(tokens[1]);
    if (!thumb || !track) return context.invalid();
    ScrollbarColors colors;
    colors.automatic = false;
    if (const auto solid = std::get_if<Color>(&*thumb)) colors.thumb = *solid;
    else {
        colors.thumb = std::get<LightDarkColor>(*thumb).dark;
        colors.thumbLightDarkColor = std::get<LightDarkColor>(*thumb);
    }
    if (const auto solid = std::get_if<Color>(&*track)) colors.track = *solid;
    else {
        colors.track = std::get<LightDarkColor>(*track).dark;
        colors.trackLightDarkColor = std::get<LightDarkColor>(*track);
    }
    return context.compiled(colors);
}

CompileResult compileOrder(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string raw = lower(trim(value));
    const auto parsed = context.number();
    const double numericOrder = parsed ? static_cast<double>(*parsed) : 0.0;
    if (!parsed
        || endsWith(raw, "px")
        || std::trunc(*parsed) != *parsed
        || numericOrder < static_cast<double>(std::numeric_limits<int>::min())
        || numericOrder > static_cast<double>(std::numeric_limits<int>::max()))
        return context.invalid();
    return context.compiled(static_cast<int>(*parsed));
}

CompileResult compileCursor(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, CursorStyle>, 35> sCursorValues{{
        {"auto", CursorStyle::Auto},
        {"default", CursorStyle::Default},
        {"pointer", CursorStyle::Pointer},
        {"progress", CursorStyle::Progress},
        {"wait", CursorStyle::Wait},
        {"crosshair", CursorStyle::Crosshair},
        {"text", CursorStyle::Text},
        {"vertical-text", CursorStyle::VerticalText},
        {"alias", CursorStyle::Alias},
        {"copy", CursorStyle::Copy},
        {"move", CursorStyle::Move},
        {"no-drop", CursorStyle::NoDrop},
        {"not-allowed", CursorStyle::NotAllowed},
        {"grab", CursorStyle::Grab},
        {"grabbing", CursorStyle::Grabbing},
        {"col-resize", CursorStyle::ColumnResize},
        {"row-resize", CursorStyle::RowResize},
        {"ew-resize", CursorStyle::EastWestResize},
        {"e-resize", CursorStyle::EastWestResize},
        {"w-resize", CursorStyle::EastWestResize},
        {"ns-resize", CursorStyle::NorthSouthResize},
        {"n-resize", CursorStyle::NorthSouthResize},
        {"s-resize", CursorStyle::NorthSouthResize},
        {"nesw-resize", CursorStyle::NortheastSouthwestResize},
        {"ne-resize", CursorStyle::NortheastSouthwestResize},
        {"sw-resize", CursorStyle::NortheastSouthwestResize},
        {"nwse-resize", CursorStyle::NorthwestSoutheastResize},
        {"nw-resize", CursorStyle::NorthwestSoutheastResize},
        {"se-resize", CursorStyle::NorthwestSoutheastResize},
        {"all-scroll", CursorStyle::AllScroll},
        {"zoom-in", CursorStyle::ZoomIn},
        {"zoom-out", CursorStyle::ZoomOut},
        {"help", CursorStyle::Help},
        {"context-menu", CursorStyle::ContextMenu},
        {"cell", CursorStyle::Cell},
    }};
    const auto parseKeyword = [](const std::string& raw) -> std::optional<CursorStyle> {
        const std::string normalized = lower(trim(raw));
        const auto found =
            std::find_if(sCursorValues.begin(), sCursorValues.end(), [&normalized](const auto& entry) { return entry.first == normalized; });
        return found == sCursorValues.end() ? std::nullopt : std::optional<CursorStyle>(found->second);
    };

    const std::vector<std::string> candidates = detail::splitTopLevel(value, ',');
    if (candidates.size() == 1) {
        const std::optional<CursorStyle> keyword = parseKeyword(candidates.front());
        return keyword ? context.compiled(CursorValue{*keyword, {}}) : context.invalid();
    }
    const auto parseHotspot = [](const std::string& raw) -> std::optional<float> {
        const std::string token = trim(raw);
        char* end = nullptr;
        const float parsed = std::strtof(token.c_str(), &end);
        if (end == token.c_str() || *end != '\0' || !std::isfinite(parsed)) return std::nullopt;
        return parsed;
    };
    std::vector<CursorImage> images;
    for (std::size_t index = 0; index + 1 < candidates.size(); ++index) {
        const std::vector<std::string> tokens = detail::tokenizeTopLevel(candidates[index]);
        if (tokens.size() != 1 && tokens.size() != 3) return context.invalid();
        const std::optional<std::string> resource = parseCSSUrl(tokens.front());
        if (!resource) return context.invalid();
        CursorImage image;
        image.resource = *resource;
        if (tokens.size() > 1) image.hotspotX = parseHotspot(tokens[1]);
        if (tokens.size() > 2) image.hotspotY = parseHotspot(tokens[2]);
        if ((tokens.size() > 1 && !image.hotspotX) || (tokens.size() > 2 && !image.hotspotY)) return context.invalid();
        images.push_back(std::move(image));
    }
    const std::optional<CursorStyle> fallback = parseKeyword(candidates.back());
    return fallback && !images.empty() ? context.compiled(CursorValue{*fallback, std::move(images)}) : context.invalid();
}

CompileResult compileDimension(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (lower(trim(value)) == "auto") return context.compiled(Dimension());
    const auto parsed = context.length();
    if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return context.invalid();
    return context.compiled(Dimension::fromLength(*parsed));
}

CompileResult compilePosition(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.length();
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileNonnegativeLength(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.nonnegativeLength(value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileUnitlessNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string raw = lower(trim(value));
    if (endsWith(raw, "px") || endsWith(raw, "%")) return context.invalid();
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOpacity(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f && *parsed <= 1.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileStrokeWidth(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(Length{*parsed}) : context.invalid();
}

CompileResult compileLineHeight(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = model.parseLineHeightValue(value);
    return parsed ? context.compiled(std::optional<Length>(*parsed)) : context.invalid();
}

CompileResult compileSpacing(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (lower(trim(value)) == "normal") return context.compiled(Length{});
    const auto parsed = context.length();
    return parsed ? context.compiled(*parsed) : context.invalid();
}
} // namespace

std::optional<std::vector<StyleDeclaration>> StyleModel::compileDeclaration(const detail::StylePropertyDefinition& property, const std::string& value,
                                                                            const std::string& selector, StyleSheetLoadResult& result,
                                                                            const std::string& sourceName) const {
    const std::string normalizedValue = detail::lower(detail::trim(value));
    if (normalizedValue == "initial") return makeDeclarations(property, InitialStyleValue{});
    if (normalizedValue == "inherit" || normalizedValue == "unset") {
        const StyleWideKeyword keyword = normalizedValue == "inherit" ? StyleWideKeyword::Inherit : StyleWideKeyword::Unset;
        return makeDeclarations(property, keyword);
    }
    if (!property.compile) {
        result.error("stylesheet.property.value_invalid", "Property has no compiler: " + std::string(property.name) + ".", sourceName);
        return std::nullopt;
    }
    detail::StyleCompileContext context{*this, property, value, selector, result, sourceName};
    return property.compile(context);
}

namespace {

template<auto Member> void applyMember(ComputedStyle& style, const StyleValue& value) {
    using Value = std::decay_t<decltype(style.*Member)>;
    style.*Member = std::get<Value>(value);
}

template<auto Member> void applyLengthToOptional(ComputedStyle& style, const StyleValue& value) {
    style.*Member = std::get<Length>(value);
}

template<InheritedStyleProperty Property, auto Member> void inheritMember(ComputedStyle& style, const ComputedStyle& parent) {
    const auto flag = static_cast<InheritedStyleProperties>(Property);
    if ((style.specifiedInheritedProperties & flag) == 0) style.*Member = parent.*Member;
}

template<auto Member> void copyMember(ComputedStyle& style, const ComputedStyle& parent) {
    style.*Member = parent.*Member;
}

void copyBackgroundColor(ComputedStyle& style, const ComputedStyle& parent) {
    style.backgroundColor = parent.backgroundColor;
    style.backgroundColorLightDark = parent.backgroundColorLightDark;
    style.backgroundGradient = parent.backgroundGradient;
    style.backgroundColorCurrent = parent.backgroundColorCurrent;
}

void copyBackgroundLayers(ComputedStyle& style, const ComputedStyle& parent) {
    style.backgroundLayers = parent.backgroundLayers;
}

void copyMask(ComputedStyle& style, const ComputedStyle& parent) {
    style.maskLayers = parent.maskLayers;
}

void copyCursor(ComputedStyle& style, const ComputedStyle& parent) {
    style.cursor = parent.cursor;
    style.cursorImages = parent.cursorImages;
}

void inheritCursor(ComputedStyle& style, const ComputedStyle& parent) {
    const auto flag = static_cast<InheritedStyleProperties>(InheritedStyleProperty::CursorPresentation);
    if ((style.specifiedInheritedProperties & flag) == 0) copyCursor(style, parent);
}

void copyBorder(ComputedStyle& style, const ComputedStyle& parent) {
    style.borderWidth = parent.borderWidth;
    style.borderColor = parent.borderColor;
    style.borderColorLightDark = parent.borderColorLightDark;
    style.borderStyle = parent.borderStyle;
    style.borderGradient = parent.borderGradient;
    style.borderColorCurrent = parent.borderColorCurrent;
    style.borderWidthSet = parent.borderWidthSet;
    style.borderColorSet = parent.borderColorSet;
}

void copyBorderColor(ComputedStyle& style, const ComputedStyle& parent) {
    style.borderColor = parent.borderColor;
    style.borderColorLightDark = parent.borderColorLightDark;
    style.borderGradient = parent.borderGradient;
    style.borderColorCurrent = parent.borderColorCurrent;
    style.borderColorSet = parent.borderColorSet;
}

void copyBorderWidth(ComputedStyle& style, const ComputedStyle& parent) {
    style.borderWidth = parent.borderWidth;
    style.borderWidthSet = parent.borderWidthSet;
}

void copyDisplay(ComputedStyle& style, const ComputedStyle& parent) {
    style.display = parent.display;
    style.displaySet = parent.displaySet;
}

void copyFlexDirection(ComputedStyle& style, const ComputedStyle& parent) {
    style.flexDirection = parent.flexDirection;
    style.flexDirectionSet = parent.flexDirectionSet;
}

void copyJustifyContent(ComputedStyle& style, const ComputedStyle& parent) {
    style.justifyContent = parent.justifyContent;
    style.justifyContentSet = parent.justifyContentSet;
}

void copyOutlineOffset(ComputedStyle& style, const ComputedStyle& parent) {
    style.outline.offset = parent.outline.offset;
}

void copySize(ComputedStyle& style, const ComputedStyle& parent) {
    style.height = parent.height;
    style.width = parent.width;
}

void copyStroke(ComputedStyle& style, const ComputedStyle& parent) {
    style.strokeColor = parent.strokeColor;
    style.strokeColorLightDark = parent.strokeColorLightDark;
    style.strokeColorCurrent = parent.strokeColorCurrent;
    style.strokeGradient = parent.strokeGradient;
}

void copyStrokeLinecap(ComputedStyle& style, const ComputedStyle& parent) {
    style.svgStrokeCap = parent.svgStrokeCap;
    style.svgStrokeCapSet = parent.svgStrokeCapSet;
}

void copyScrollbarMode(ComputedStyle& style, const ComputedStyle& parent) {
    style.scrollbarMode = parent.scrollbarMode;
    style.scrollbarModeSet = parent.scrollbarModeSet;
}

void copyVerticalAlign(ComputedStyle& style, const ComputedStyle& parent) {
    style.verticalAlign = parent.verticalAlign;
    style.verticalAlignSet = parent.verticalAlignSet;
}

template<auto Member> void resetMember(ComputedStyle& style) {
    const ComputedStyle initial;
    style.*Member = initial.*Member;
}

template<void (*Copy)(ComputedStyle&, const ComputedStyle&)> void resetWith(ComputedStyle& style) {
    const ComputedStyle initial;
    Copy(style, initial);
}

void resetDisplay(ComputedStyle& style) {
    const ComputedStyle initial;
    style.display = initial.display;
    style.displaySet = true;
}

void resetFlexDirection(ComputedStyle& style) {
    const ComputedStyle initial;
    style.flexDirection = initial.flexDirection;
    style.flexDirectionSet = true;
}

void resetJustifyContent(ComputedStyle& style) {
    const ComputedStyle initial;
    style.justifyContent = initial.justifyContent;
    style.justifyContentSet = true;
}

void resetScrollbarMode(ComputedStyle& style) {
    const ComputedStyle initial;
    style.scrollbarMode = initial.scrollbarMode;
    style.scrollbarModeSet = true;
}

void resetSize(ComputedStyle& style) {
    const ComputedStyle initial;
    style.height = initial.height;
    style.width = initial.width;
}

void resetBorderWidth(ComputedStyle& style) {
    const ComputedStyle initial;
    style.borderWidth = initial.borderWidth;
    style.borderWidthSet = false;
}

void resetColor(ComputedStyle& style) {
    const ComputedStyle initial;
    style.color = initial.color;
    style.colorLightDark = initial.colorLightDark;
}

void resetBackgroundImages(ComputedStyle& style) {
    style.backgroundLayers = {BackgroundLayer{}};
}

void resetBackgroundPositions(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.position = {};
}

void resetBackgroundSizes(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.size = {};
}

void resetBackgroundRepeats(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.repeat = BackgroundRepeat::Repeat;
}

void resetBackgroundOrigins(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.origin = BackgroundBox::PaddingBox;
}

void resetBackgroundClips(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.clip = BackgroundBox::BorderBox;
}

void resetBackgroundAttachments(ComputedStyle& style) {
    if (style.backgroundLayers.empty()) style.backgroundLayers.emplace_back();
    for (BackgroundLayer& layer : style.backgroundLayers) layer.attachment = BackgroundAttachment::Scroll;
}

void resetMaskImages(ComputedStyle& style) {
    style.maskLayers = {MaskLayer{}};
}

void resetMaskPositions(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.position = {};
}

void resetMaskSizes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.size = {};
}

void resetMaskRepeats(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.repeat = BackgroundRepeat::Repeat;
}

void resetMaskOrigins(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.origin = BackgroundBox::BorderBox;
}

void resetMaskClips(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.image.clip = BackgroundBox::BorderBox;
}

void resetMaskModes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.mode = MaskMode::MatchSource;
}

void resetMaskComposites(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.composite = MaskComposite::Add;
}

void resetMaskTypes(ComputedStyle& style) {
    if (style.maskLayers.empty()) style.maskLayers.emplace_back();
    for (MaskLayer& layer : style.maskLayers) layer.type = MaskType::Alpha;
}

void resetCursor(ComputedStyle& style) {
    style.cursor = CursorStyle::Auto;
    style.cursorImages.clear();
}

void resetStrokeLinecap(ComputedStyle& style) {
    const ComputedStyle initial;
    style.svgStrokeCap = initial.svgStrokeCap;
    style.svgStrokeCapSet = true;
}

void resetVerticalAlign(ComputedStyle& style) {
    const ComputedStyle initial;
    style.verticalAlign = initial.verticalAlign;
    style.verticalAlignSet = true;
}

void inheritColor(ComputedStyle& style, const ComputedStyle& parent) {
    const auto flag = static_cast<InheritedStyleProperties>(InheritedStyleProperty::Color);
    if ((style.specifiedInheritedProperties & flag) == 0) {
        style.color = parent.color;
        style.colorLightDark = parent.colorLightDark;
    }
}

template<InheritedStyleProperty Property> void specifyInherited(ComputedStyle& style) {
    style.specifiedInheritedProperties |= static_cast<InheritedStyleProperties>(Property);
}

void applyPaint(Color& color, std::optional<LightDarkColor>& lightDarkColor, std::optional<Gradient>& gradient, bool& currentColor,
                const StyleValue& value) {
    const StylePaint& paint = std::get<StylePaint>(value);
    lightDarkColor = paint.lightDarkColor;
    currentColor = paint.currentColor;
    color = paint.gradient || paint.lightDarkColor || paint.currentColor ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
    gradient = paint.gradient;
}

void applyBackground(ComputedStyle& style, const StyleValue& value) {
    applyPaint(style.backgroundColor, style.backgroundColorLightDark, style.backgroundGradient, style.backgroundColorCurrent, value);
}
void applyMask(ComputedStyle& style, const StyleValue& value) {
    applyMaskComponent(style.maskLayers, std::get<StyleMaskLayers>(value));
}
void applyCursor(ComputedStyle& style, const StyleValue& value) {
    const CursorValue& cursor = std::get<CursorValue>(value);
    style.cursor = cursor.style;
    style.cursorImages = cursor.images;
}
void applyBorderWidth(ComputedStyle& style, const StyleValue& value) {
    style.borderWidth = std::get<EdgeInsets>(value);
    style.borderWidthSet = true;
}
void applyBorder(ComputedStyle& style, const StyleValue& value) {
    const StyleBorder& border = std::get<StyleBorder>(value);
    style.borderWidth = {border.width, border.width, border.width, border.width};
    style.borderColorLightDark = border.paint.lightDarkColor;
    style.borderColorCurrent = border.paint.currentColor;
    style.borderWidthSet = true;
    style.borderColorSet = true;
    style.borderColor =
        border.paint.gradient || border.paint.lightDarkColor || border.paint.currentColor ? Color(0.f, 0.f, 0.f, 0.f) : border.paint.color;
    style.borderStyle = border.style;
    style.borderGradient = border.paint.gradient;
}
void applyBorderStyle(ComputedStyle& style, const StyleValue& value) {
    style.borderStyle = std::get<BorderStyle>(value);
}
void applyColor(ComputedStyle& style, const StyleValue& value) {
    if (const auto lightDarkColor = std::get_if<LightDarkColor>(&value)) {
        style.colorLightDark = *lightDarkColor;
        style.color = lightDarkColor->dark;
    } else {
        style.colorLightDark.reset();
        style.color = std::get<Color>(value);
    }
}
void applyAccentColor(ComputedStyle& style, const StyleValue& value) {
    style.accentColor = std::get<AccentColor>(value);
}
void applyOutline(ComputedStyle& style, const StyleValue& value) {
    const Outline& outline = std::get<Outline>(value);
    style.outline.width = outline.width;
    style.outline.color = outline.color;
    style.outline.style = outline.style;
    style.outline.lightDarkColor = outline.lightDarkColor;
}
void applyOutlineOffset(ComputedStyle& style, const StyleValue& value) {
    style.outline.offset = std::get<Length>(value).pixels;
}
void applySize(ComputedStyle& style, const StyleValue& value) {
    const StyleSize& size = std::get<StyleSize>(value);
    style.height = size.height;
    style.width = size.width;
}
void applyDisplay(ComputedStyle& style, const StyleValue& value) {
    style.display = std::get<DisplayMode>(value);
    style.displaySet = true;
}
void applyScrollbarMode(ComputedStyle& style, const StyleValue& value) {
    style.scrollbarMode = std::get<ScrollbarMode>(value);
    style.scrollbarModeSet = true;
}
void applyFlexDirection(ComputedStyle& style, const StyleValue& value) {
    style.flexDirection = std::get<FlexDirection>(value);
    style.flexDirectionSet = true;
}
void applyJustifyContent(ComputedStyle& style, const StyleValue& value) {
    style.justifyContent = std::get<JustifyContent>(value);
    style.justifyContentSet = true;
}
void applyVerticalAlign(ComputedStyle& style, const StyleValue& value) {
    style.verticalAlign = std::get<VerticalAlign>(value);
    style.verticalAlignSet = true;
}
void applyFontWeight(ComputedStyle& style, const StyleValue& value) {
    style.fontWeight = static_cast<U16>(std::get<float>(value));
}
void applyStrokeLinecap(ComputedStyle& style, const StyleValue& value) {
    style.svgStrokeCap = std::get<StrokeCap>(value);
    style.svgStrokeCapSet = true;
}
void applyStrokeWidth(ComputedStyle& style, const StyleValue& value) {
    style.svgStrokeWidth = std::get<Length>(value);
}

void applyStroke(ComputedStyle& style, const StyleValue& value) {
    applyPaint(style.strokeColor, style.strokeColorLightDark, style.strokeGradient, style.strokeColorCurrent, value);
}

constexpr std::array<std::string_view, 2> kOverflowLonghands{"overflow-x", "overflow-y"};
constexpr std::array<std::string_view, 2> kMinSizeLonghands{"min-height", "min-width"};
constexpr std::array<std::string_view, 3> kFlexLonghands{"flex-grow", "flex-shrink", "flex-basis"};
constexpr std::array<std::string_view, 5> kFontLonghands{"font-style", "font-weight", "font-size", "line-height", "font-family"};
constexpr std::array<std::string_view, 8> kBackgroundLonghands{"background-color", "background-image",     "background-position",
                                                               "background-size",  "background-repeat",    "background-origin",
                                                               "background-clip",  "background-attachment"};
constexpr std::array<std::string_view, 9> kMaskLonghands{"mask-image",  "mask-mode", "mask-position",  "mask-size", "mask-repeat",
                                                         "mask-origin", "mask-clip", "mask-composite", "mask-type"};

const detail::StylePropertyDefinition kPropertyDefinitions[] = {
    {"accent-color", compileAccentColor, applyAccentColor, resetMember<&ComputedStyle::accentColor>,
     specifyInherited<InheritedStyleProperty::AccentColor>, inheritMember<InheritedStyleProperty::AccentColor, &ComputedStyle::accentColor>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::AccentColor},
    {"appearance", compileAppearance, applyMember<&ComputedStyle::appearance>, resetMember<&ComputedStyle::appearance>, nullptr,
     copyMember<&ComputedStyle::appearance>, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"color-scheme", compileColorScheme, applyMember<&ComputedStyle::colorScheme>, resetMember<&ComputedStyle::colorScheme>,
     specifyInherited<InheritedStyleProperty::ColorScheme>, inheritMember<InheritedStyleProperty::ColorScheme, &ComputedStyle::colorScheme>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::ColorScheme},
    {"box-sizing", compileBoxSizing, applyMember<&ComputedStyle::boxSizing>, resetMember<&ComputedStyle::boxSizing>, nullptr,
     copyMember<&ComputedStyle::boxSizing>, StylePropertyImpact::Layout},
    {"background", compileBackground, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kBackgroundLonghands)},
    {"background-color", compilePaint, applyBackground, resetWith<copyBackgroundColor>, nullptr, copyBackgroundColor, StylePropertyImpact::Paint},
    {"background-image", compileBackgroundImage, applyBackgroundImages, resetBackgroundImages, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-position", compileBackgroundPosition, applyBackgroundImages, resetBackgroundPositions, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-size", compileBackgroundSize, applyBackgroundImages, resetBackgroundSizes, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-repeat", compileBackgroundRepeat, applyBackgroundImages, resetBackgroundRepeats, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-origin", compileBackgroundOrigin, applyBackgroundImages, resetBackgroundOrigins, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-clip", compileBackgroundClip, applyBackgroundImages, resetBackgroundClips, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"background-attachment", compileBackgroundAttachment, applyBackgroundImages, resetBackgroundAttachments, nullptr, copyBackgroundLayers,
     StylePropertyImpact::Paint},
    {"border", compileBorder, applyBorder, resetWith<copyBorder>, nullptr, copyBorder, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-color", compilePaint,
     [](ComputedStyle& style, const StyleValue& value) {
         applyPaint(style.borderColor, style.borderColorLightDark, style.borderGradient, style.borderColorCurrent, value);
         style.borderColorSet = true;
     },
     resetWith<copyBorderColor>, nullptr, copyBorderColor, StylePropertyImpact::Paint},
    {"border-radius", compileBorderRadius, applyMember<&ComputedStyle::borderRadius>, resetMember<&ComputedStyle::borderRadius>, nullptr,
     copyMember<&ComputedStyle::borderRadius>, StylePropertyImpact::Paint},
    {"border-style", compileBorderStyle, applyBorderStyle, resetMember<&ComputedStyle::borderStyle>, nullptr, copyMember<&ComputedStyle::borderStyle>,
     StylePropertyImpact::Paint},
    {"border-width", compileEdges, applyBorderWidth, resetBorderWidth, nullptr, copyBorderWidth,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"bottom", compilePosition, applyLengthToOptional<&ComputedStyle::bottom>, resetMember<&ComputedStyle::bottom>, nullptr,
     copyMember<&ComputedStyle::bottom>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"cursor", compileCursor, applyCursor, resetCursor, specifyInherited<InheritedStyleProperty::CursorPresentation>, inheritCursor,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::CursorPresentation},
    {"display", compileDisplay, applyDisplay, resetDisplay, nullptr, copyDisplay,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"effect", compileEffect, applyMember<&ComputedStyle::effects>, resetMember<&ComputedStyle::effects>, nullptr,
     copyMember<&ComputedStyle::effects>, StylePropertyImpact::Paint},
    {"height", compileDimension, applyMember<&ComputedStyle::height>, resetMember<&ComputedStyle::height>, nullptr,
     copyMember<&ComputedStyle::height>},
    {"left", compilePosition, applyLengthToOptional<&ComputedStyle::left>, resetMember<&ComputedStyle::left>, nullptr,
     copyMember<&ComputedStyle::left>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"margin", compileMargin, applyMember<&ComputedStyle::margin>, resetMember<&ComputedStyle::margin>, nullptr, copyMember<&ComputedStyle::margin>},
    {"min-height", compileNonnegativeLength, applyLengthToOptional<&ComputedStyle::minHeight>, resetMember<&ComputedStyle::minHeight>, nullptr,
     copyMember<&ComputedStyle::minHeight>},
    {"min-size", compileMinSize, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kMinSizeLonghands)},
    {"min-width", compileNonnegativeLength, applyLengthToOptional<&ComputedStyle::minWidth>, resetMember<&ComputedStyle::minWidth>, nullptr,
     copyMember<&ComputedStyle::minWidth>},
    {"opacity", compileOpacity, applyMember<&ComputedStyle::opacity>, resetMember<&ComputedStyle::opacity>, nullptr,
     copyMember<&ComputedStyle::opacity>, StylePropertyImpact::Paint},
    {"mask", compileMask, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Paint, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kMaskLonghands)},
    {"mask-image", compileMaskImage, applyMask, resetMaskImages, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-mode", compileMaskMode, applyMask, resetMaskModes, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-position", compileMaskPosition, applyMask, resetMaskPositions, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-size", compileMaskSize, applyMask, resetMaskSizes, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-repeat", compileMaskRepeat, applyMask, resetMaskRepeats, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-origin", compileMaskOrigin, applyMask, resetMaskOrigins, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-clip", compileMaskClip, applyMask, resetMaskClips, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-composite", compileMaskComposite, applyMask, resetMaskComposites, nullptr, copyMask, StylePropertyImpact::Paint},
    {"mask-type", compileMaskType, applyMask, resetMaskTypes, nullptr, copyMask, StylePropertyImpact::Paint},
    {"outline", compileOutline, applyOutline, resetMember<&ComputedStyle::outline>, nullptr, copyMember<&ComputedStyle::outline>,
     StylePropertyImpact::Paint},
    {"outline-offset", compileOutlineOffset, applyOutlineOffset, resetWith<copyOutlineOffset>, nullptr, copyOutlineOffset,
     StylePropertyImpact::Paint},
    {"overflow", compileOverflow, nullptr, nullptr, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kOverflowLonghands)},
    {"overflow-x", compileOverflowAxis, applyMember<&ComputedStyle::overflowX>, resetMember<&ComputedStyle::overflowX>, nullptr,
     copyMember<&ComputedStyle::overflowX>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-y", compileOverflowAxis, applyMember<&ComputedStyle::overflowY>, resetMember<&ComputedStyle::overflowY>, nullptr,
     copyMember<&ComputedStyle::overflowY>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"padding", compileEdges, applyMember<&ComputedStyle::padding>, resetMember<&ComputedStyle::padding>, nullptr,
     copyMember<&ComputedStyle::padding>},
    {"pointer-events", compilePointerEvents, applyMember<&ComputedStyle::pointerEvents>, resetMember<&ComputedStyle::pointerEvents>, nullptr,
     copyMember<&ComputedStyle::pointerEvents>, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"position", compilePositionMode, applyMember<&ComputedStyle::position>, resetMember<&ComputedStyle::position>, nullptr,
     copyMember<&ComputedStyle::position>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"right", compilePosition, applyLengthToOptional<&ComputedStyle::right>, resetMember<&ComputedStyle::right>, nullptr,
     copyMember<&ComputedStyle::right>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-gutter", compileScrollbarGutter, applyMember<&ComputedStyle::scrollbarGutter>, resetMember<&ComputedStyle::scrollbarGutter>, nullptr,
     copyMember<&ComputedStyle::scrollbarGutter>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-mode", compileScrollbarMode, applyScrollbarMode, resetScrollbarMode, nullptr, copyScrollbarMode,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-width", compileScrollbarWidth, applyMember<&ComputedStyle::scrollbarWidth>, resetMember<&ComputedStyle::scrollbarWidth>, nullptr,
     copyMember<&ComputedStyle::scrollbarWidth>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-color", compileScrollbarColor, applyMember<&ComputedStyle::scrollbarColor>, resetMember<&ComputedStyle::scrollbarColor>,
     specifyInherited<InheritedStyleProperty::ScrollbarColor>, inheritMember<InheritedStyleProperty::ScrollbarColor, &ComputedStyle::scrollbarColor>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::ScrollbarColor},
    {"box-shadow", compileShadow, applyMember<&ComputedStyle::shadows>, resetMember<&ComputedStyle::shadows>, nullptr,
     copyMember<&ComputedStyle::shadows>, StylePropertyImpact::Paint},
    {"size", compileSize, applySize, resetSize, nullptr, copySize},
    {"top", compilePosition, applyLengthToOptional<&ComputedStyle::top>, resetMember<&ComputedStyle::top>, nullptr, copyMember<&ComputedStyle::top>,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"translate", compileTranslate, applyMember<&ComputedStyle::translate>, resetMember<&ComputedStyle::translate>, nullptr,
     copyMember<&ComputedStyle::translate>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"width", compileDimension, applyMember<&ComputedStyle::width>, resetMember<&ComputedStyle::width>, nullptr, copyMember<&ComputedStyle::width>},
    {"align-items", compileAlignItems, applyMember<&ComputedStyle::alignItems>, resetMember<&ComputedStyle::alignItems>, nullptr,
     copyMember<&ComputedStyle::alignItems>},
    {"flex-direction", compileFlexDirection, applyFlexDirection, resetFlexDirection, nullptr, copyFlexDirection},
    {"gap", compileGap, applyMember<&ComputedStyle::gap>, resetMember<&ComputedStyle::gap>, nullptr, copyMember<&ComputedStyle::gap>},
    {"grid-area", compileGridArea, [](ComputedStyle& style, const StyleValue& value) { style.gridArea = std::get<GridArea>(value); },
     resetMember<&ComputedStyle::gridArea>, nullptr, copyMember<&ComputedStyle::gridArea>,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"justify-content", compileJustifyContent, applyJustifyContent, resetJustifyContent, nullptr, copyJustifyContent},
    {"justify-self", compileJustifySelf, applyMember<&ComputedStyle::justifySelf>, resetMember<&ComputedStyle::justifySelf>, nullptr,
     copyMember<&ComputedStyle::justifySelf>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"-internal-align-content-block", compileInternalAlignContentBlock, applyMember<&ComputedStyle::alignContentBlockCenter>,
     resetMember<&ComputedStyle::alignContentBlockCenter>, nullptr, copyMember<&ComputedStyle::alignContentBlockCenter>, StylePropertyImpact::Layout,
     true},
    {"align-self", compileAlignSelf, applyMember<&ComputedStyle::alignSelf>, resetMember<&ComputedStyle::alignSelf>, nullptr,
     copyMember<&ComputedStyle::alignSelf>},
    {"flex", compileFlex, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kFlexLonghands)},
    {"flex-basis", compileDimension, applyMember<&ComputedStyle::flexBasis>, resetMember<&ComputedStyle::flexBasis>, nullptr,
     copyMember<&ComputedStyle::flexBasis>},
    {"flex-grow", compileUnitlessNonnegativeNumber, applyMember<&ComputedStyle::flexGrow>, resetMember<&ComputedStyle::flexGrow>, nullptr,
     copyMember<&ComputedStyle::flexGrow>},
    {"flex-shrink", compileUnitlessNonnegativeNumber, applyMember<&ComputedStyle::flexShrink>, resetMember<&ComputedStyle::flexShrink>, nullptr,
     copyMember<&ComputedStyle::flexShrink>},
    {"order", compileOrder, applyMember<&ComputedStyle::order>, resetMember<&ComputedStyle::order>, nullptr, copyMember<&ComputedStyle::order>},
    {"font", compileFont, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kFontLonghands)},
    {"font-family", compileFontFamily, applyMember<&ComputedStyle::fontFamily>, resetMember<&ComputedStyle::fontFamily>,
     specifyInherited<InheritedStyleProperty::FontFamily>, inheritMember<InheritedStyleProperty::FontFamily, &ComputedStyle::fontFamily>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::FontFamily},
    {"font-size", compileNonnegativeNumber, applyMember<&ComputedStyle::fontSize>, resetMember<&ComputedStyle::fontSize>,
     specifyInherited<InheritedStyleProperty::FontSize>, inheritMember<InheritedStyleProperty::FontSize, &ComputedStyle::fontSize>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::FontSize},
    {"font-style", compileFontStyle, applyMember<&ComputedStyle::fontItalic>, resetMember<&ComputedStyle::fontItalic>,
     specifyInherited<InheritedStyleProperty::FontStyle>, inheritMember<InheritedStyleProperty::FontStyle, &ComputedStyle::fontItalic>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::FontStyle},
    {"text-decoration", compileTextDecoration, applyMember<&ComputedStyle::textDecoration>, resetMember<&ComputedStyle::textDecoration>,
     specifyInherited<InheritedStyleProperty::TextDecoration>, inheritMember<InheritedStyleProperty::TextDecoration, &ComputedStyle::textDecoration>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::TextDecoration},
    {"content", compileContent, applyMember<&ComputedStyle::content>, resetMember<&ComputedStyle::content>, nullptr,
     copyMember<&ComputedStyle::content>, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"font-weight", compileFontWeight, applyFontWeight, resetMember<&ComputedStyle::fontWeight>, specifyInherited<InheritedStyleProperty::FontWeight>,
     inheritMember<InheritedStyleProperty::FontWeight, &ComputedStyle::fontWeight>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited,
     false, InheritedStyleProperty::FontWeight},
    {"line-height", compileLineHeight, applyMember<&ComputedStyle::lineHeight>, resetMember<&ComputedStyle::lineHeight>,
     specifyInherited<InheritedStyleProperty::LineHeight>, inheritMember<InheritedStyleProperty::LineHeight, &ComputedStyle::lineHeight>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::LineHeight},
    {"letter-spacing", compileSpacing, applyMember<&ComputedStyle::letterSpacing>, resetMember<&ComputedStyle::letterSpacing>,
     specifyInherited<InheritedStyleProperty::LetterSpacing>, inheritMember<InheritedStyleProperty::LetterSpacing, &ComputedStyle::letterSpacing>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::LetterSpacing},
    {"word-spacing", compileSpacing, applyMember<&ComputedStyle::wordSpacing>, resetMember<&ComputedStyle::wordSpacing>,
     specifyInherited<InheritedStyleProperty::WordSpacing>, inheritMember<InheritedStyleProperty::WordSpacing, &ComputedStyle::wordSpacing>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::WordSpacing},
    {"text-align", compileTextAlign, applyMember<&ComputedStyle::textAlign>, resetMember<&ComputedStyle::textAlign>,
     specifyInherited<InheritedStyleProperty::TextAlign>, inheritMember<InheritedStyleProperty::TextAlign, &ComputedStyle::textAlign>,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::TextAlign},
    {"color", compileColor, applyColor, resetColor, specifyInherited<InheritedStyleProperty::Color>, inheritColor,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited, false, InheritedStyleProperty::Color},
    {"text-overflow", compileTextOverflow, applyMember<&ComputedStyle::textOverflow>, resetMember<&ComputedStyle::textOverflow>, nullptr,
     copyMember<&ComputedStyle::textOverflow>, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"text-wrap", compileTextWrap, applyMember<&ComputedStyle::textWrap>, resetMember<&ComputedStyle::textWrap>,
     specifyInherited<InheritedStyleProperty::TextWrap>, inheritMember<InheritedStyleProperty::TextWrap, &ComputedStyle::textWrap>,
     StylePropertyImpact::Layout | StylePropertyImpact::Inherited, false, InheritedStyleProperty::TextWrap},
    {"vertical-align", compileVerticalAlign, applyVerticalAlign, resetVerticalAlign, nullptr, copyVerticalAlign},
    {"visibility", compileVisibility, applyMember<&ComputedStyle::visibility>, resetMember<&ComputedStyle::visibility>,
     specifyInherited<InheritedStyleProperty::Visibility>, inheritMember<InheritedStyleProperty::Visibility, &ComputedStyle::visibility>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited | StylePropertyImpact::HitTest, false, InheritedStyleProperty::Visibility},
    {"stroke", compilePaint, applyStroke, resetWith<copyStroke>, nullptr, copyStroke, StylePropertyImpact::Paint},
    {"stroke-linecap", compileStrokeLinecap, applyStrokeLinecap, resetStrokeLinecap, nullptr, copyStrokeLinecap, StylePropertyImpact::Paint},
    {"stroke-width", compileStrokeWidth, applyStrokeWidth, resetMember<&ComputedStyle::svgStrokeWidth>, nullptr,
     copyMember<&ComputedStyle::svgStrokeWidth>, StylePropertyImpact::Paint},
};
} // namespace

namespace detail {
const StylePropertyDefinition* findStyleProperty(std::string_view name) {
    const auto found = std::find_if(std::begin(kPropertyDefinitions), std::end(kPropertyDefinitions),
                                    [name](const StylePropertyDefinition& property) { return property.name == name; });
    return found == std::end(kPropertyDefinitions) ? nullptr : found;
}

const StylePropertyDefinition* stylePropertyBegin() {
    return std::begin(kPropertyDefinitions);
}
const StylePropertyDefinition* stylePropertyEnd() {
    return std::end(kPropertyDefinitions);
}
} // namespace detail
} // namespace radia::ui
