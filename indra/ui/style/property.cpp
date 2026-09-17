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
#include <initializer_list>
#include <iterator>
#include <limits>
#include <string>
#include <type_traits>
#include <utility>
#include "css/color.h"
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::lower;
using detail::normalizeCSSKeyword;
using detail::startsWith;
using detail::StylePropertyImpact;
using detail::trim;

bool hasDimensionUnit(const detail::CSSTokenStream& stream, detail::CSSTokenRange range, std::string_view unit) {
    const auto dimension = detail::parseCSSDimension(stream, range);
    return dimension && dimension->unit == unit;
}

std::optional<BorderStyle> parseBorderStyle(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "none") return BorderStyle::NoneValue;
    if (token == "solid") return BorderStyle::Solid;
    if (token == "outset") return BorderStyle::Outset;
    if (token == "inset") return BorderStyle::Inset;
    return std::nullopt;
}

bool parseStrokeCap(detail::CSSValueRange value, StrokeCap& cap) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "butt") cap = StrokeCap::Butt;
    else if (token == "round") cap = StrokeCap::Round;
    else if (token == "square") cap = StrokeCap::Square;
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

void detail::applyInvalidStyleDeclaration(ComputedStyle& style, const StylePropertyDefinition& property) {
    if (property.longhands.empty()) {
        applyStyleDeclaration(style, StyleDeclaration{property, StyleWideKeyword::Unset});
        return;
    }
    for (const std::string_view longhandName : property.longhands) {
        const StylePropertyDefinition* longhand = findStyleProperty(longhandName);
        if (longhand) applyStyleDeclaration(style, StyleDeclaration{*longhand, StyleWideKeyword::Unset});
    }
}

std::optional<bool> StyleModel::parseFontStyleValue(detail::CSSValueRange value) {
    const std::string style = normalizeCSSKeyword(value.stream, value.range);
    if (style == "normal") return false;
    if (style == "italic" || style == "oblique") return true;
    return std::nullopt;
}

std::optional<float> StyleModel::parseFontWeightValue(detail::CSSValueRange value) {
    const std::string weight = normalizeCSSKeyword(value.stream, value.range);
    if (weight == "normal") return 400.f;
    if (weight == "bold") return 700.f;
    if (hasDimensionUnit(value.stream, value.range, "px") || endsWith(weight, "%")) return std::nullopt;
    const float parsed = parseNumberValue(value, std::numeric_limits<float>::quiet_NaN());
    return std::isfinite(parsed) && parsed >= 1.f && parsed <= 1000.f && std::floor(parsed) == parsed ? std::optional<float>(parsed) : std::nullopt;
}

std::optional<LineHeight> StyleModel::parseLineHeightValue(detail::CSSValueRange value) {
    const std::string keyword = normalizeCSSKeyword(value.stream, value.range);
    if (keyword == "normal") return LineHeight{LineHeight::Kind::Normal, 0.f};
    if (hasDimensionUnit(value.stream, value.range, "px")) {
        const std::optional<Length> parsed = parseLengthValue(value);
        return parsed && parsed->pixels >= 0.f && parsed->percent == 0.f
            ? std::optional<LineHeight>(LineHeight{LineHeight::Kind::Length, parsed->pixels})
            : std::nullopt;
    }
    if (endsWith(keyword, "%")) {
        const std::optional<Length> parsed = parseLengthValue(value);
        return parsed && parsed->percent >= 0.f && parsed->pixels == 0.f
            ? std::optional<LineHeight>(LineHeight{LineHeight::Kind::Percentage, parsed->percent})
            : std::nullopt;
    }
    const float parsed = parseNumberValue(value, std::numeric_limits<float>::quiet_NaN());
    return std::isfinite(parsed) && parsed >= 0.f ? std::optional<LineHeight>(LineHeight{LineHeight::Kind::Number, parsed}) : std::nullopt;
}

std::optional<std::vector<StyleDeclaration>> StyleModel::parseFontShorthand(detail::CSSValueRange value) {
    std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range, true);
    if (tokens.size() < 2) return std::nullopt;
    const std::string family = normalizeCSSKeyword(value.stream, tokens.back());
    if (family != "sans-serif" && family != "monospace") return std::nullopt;
    tokens.pop_back();

    LineHeight lineHeight;
    std::size_t sizeIndex = tokens.size() - 1;
    const auto slash =
        std::find_if(tokens.begin(), tokens.end(), [&](detail::CSSTokenRange token) { return normalizeCSSKeyword(value.stream, token) == "/"; });
    if (slash != tokens.end()) {
        const std::size_t slashIndex = static_cast<std::size_t>(slash - tokens.begin());
        const auto secondSlash =
            std::find_if(slash + 1, tokens.end(), [&](detail::CSSTokenRange token) { return normalizeCSSKeyword(value.stream, token) == "/"; });
        if (slashIndex == 0 || slashIndex + 2 != tokens.size() || secondSlash != tokens.end()) return std::nullopt;
        sizeIndex = slashIndex - 1;
        const std::optional<LineHeight> parsedLineHeight = parseLineHeightValue({value.stream, tokens.back()});
        if (!parsedLineHeight) return std::nullopt;
        lineHeight = *parsedLineHeight;
    }

    const std::string size = normalizeCSSKeyword(value.stream, tokens[sizeIndex]);
    const float parsedSize = parseNumberValue({value.stream, tokens[sizeIndex]}, std::numeric_limits<float>::quiet_NaN());
    if (!std::isfinite(parsedSize) || parsedSize < 0.f || endsWith(size, "%")) return std::nullopt;

    bool italic = false;
    float weight = 400.f;
    std::optional<RelativeFontWeight> relativeWeight;
    bool sawStyle = false;
    bool sawWeight = false;
    for (std::size_t index = 0; index < sizeIndex; ++index) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens[index]);
        if (token == "normal") {
            if (!sawStyle) sawStyle = true;
            else if (!sawWeight) sawWeight = true;
            else return std::nullopt;
            continue;
        }
        if (!sawStyle) {
            const std::optional<bool> parsedStyle = parseFontStyleValue({value.stream, tokens[index]});
            if (parsedStyle) {
                italic = *parsedStyle;
                sawStyle = true;
                continue;
            }
        }
        if (!sawWeight) {
            if (token == "bolder" || token == "lighter") {
                relativeWeight = RelativeFontWeight{token == "lighter"};
                sawWeight = true;
                continue;
            }
            const std::optional<float> parsedWeight = parseFontWeightValue({value.stream, tokens[index]});
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
        {"font-weight", relativeWeight ? StyleValue(*relativeWeight) : StyleValue(weight)},
        {"font-size", parsedSize},
        {"line-height", lineHeight},
        {"font-family", family == "monospace" ? StyleValue(FontFamily::Monospace) : StyleValue(FontFamily::SansSerif)},
    });
}

namespace { using CompileResult = detail::StyleCompileResult; } // namespace

struct StyleCompileValue {
    std::string text;
    const detail::CSSTokenStream& stream;
    detail::CSSTokenRange range;
};

struct detail::StyleCompileContext {
    const detail::StylePropertyDefinition& property;
    StyleCompileValue value;
    const std::string& selector;
    StyleSheetLoadResult& result;
    const std::string& sourceName;

    CompileResult invalid() const {
        const std::size_t offset =
            value.range.begin < value.stream.tokens().size() ? value.stream.tokens()[value.range.begin].begin : value.stream.source().size();
        const auto [line, column] = detail::cssSourcePosition(value.stream.source(), offset);
        result.warning("stylesheet.property.value_invalid", "Invalid value for " + std::string(property.name) + ": " + value.text + ".", sourceName,
                       line, column);
        return std::nullopt;
    }

    CompileResult compiled(StyleValue parsed) const { return std::vector<StyleDeclaration>{makeDeclaration(property.name, std::move(parsed))}; }

    std::optional<Color> color() const {
        const Color marker(-1.f, -1.f, -1.f, -1.f);
        const Color parsed = StyleModel::parseColorValue({value.stream, value.range}, marker);
        return parsed.a < 0.f ? std::nullopt : std::optional<Color>(parsed);
    }

    std::optional<Color> color(detail::CSSValueRange raw) const {
        const Color marker(-1.f, -1.f, -1.f, -1.f);
        const Color parsed = StyleModel::parseColorValue(raw, marker);
        return parsed.a < 0.f ? std::nullopt : std::optional<Color>(parsed);
    }

    std::optional<StyleColorValue> colorValue() const { return StyleModel::parseColorChoiceValue({value.stream, value.range}); }

    std::optional<StyleColorValue> colorValue(detail::CSSValueRange raw) const { return StyleModel::parseColorChoiceValue(raw); }

    std::optional<float> number() const {
        const float parsed = StyleModel::parseNumberValue({value.stream, value.range}, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    }

    std::optional<float> number(detail::CSSValueRange raw) const {
        const float parsed = StyleModel::parseNumberValue(raw, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    }

    std::optional<Length> length() const { return StyleModel::parseLengthValue({value.stream, value.range}); }

    std::optional<Length> length(detail::CSSValueRange raw) const { return StyleModel::parseLengthValue(raw); }

    std::vector<detail::CSSTokenRange> commaSeparatedRanges() const { return detail::splitCSSOnDelimiter(value.stream, value.range, ','); }

    std::optional<Length> nonnegativeLength() const {
        const std::optional<Length> parsed = length();
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    }

    std::optional<Length> nonnegativeLength(detail::CSSValueRange raw) const {
        const std::optional<Length> parsed = length(raw);
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    }

    std::vector<detail::CSSTokenRange> ranges(bool splitSlash = true) const {
        return detail::splitCSSComponents(value.stream, value.range, splitSlash);
    }

    std::string keyword() const { return normalizeCSSKeyword(value.stream, value.range); }
};

namespace {
CompileResult compileShadow(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseShadows({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

struct ParsedContent {
    bool valid = false;
    std::optional<std::string> value;
};

std::optional<std::string> decodeCSSStringRange(const detail::CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = detail::trimCSSRange(stream, range);
    if (range.end != range.begin + 1 || stream.tokens()[range.begin].kind != detail::CSSTokenKind::String) return std::nullopt;
    return detail::decodeCSSString(stream.text(range.begin));
}

ParsedContent parseContent(const StyleCompileValue& raw) {
    const auto& stream = raw.stream;
    const std::vector<detail::CSSTokenRange> components = detail::splitCSSComponents(stream, raw.range, true);
    if (components.size() == 1) {
        const std::string keyword = normalizeCSSKeyword(stream, components.front());
        if (keyword == "none" || keyword == "normal") return {true, std::nullopt};
        if (const std::optional<std::string> primary = decodeCSSStringRange(stream, components.front())) return {true, *primary};
    }
    if (components.size() != 3 || normalizeCSSKeyword(stream, components[1]) != "/") return {};
    const std::optional<std::string> primary = decodeCSSStringRange(stream, components[0]);
    const std::optional<std::string> alternative = decodeCSSStringRange(stream, components[2]);
    return primary && alternative ? ParsedContent{true, *primary} : ParsedContent{};
}

CompileResult compileContent(detail::StyleCompileContext& context) {
    const ParsedContent parsed = parseContent(context.value);
    return parsed.valid ? context.compiled(parsed.value) : context.invalid();
}

CompileResult compileFilter(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseFilter({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOutline(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseOutline({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileBorderRadius(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseBorderRadius({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOutlineOffset(detail::StyleCompileContext& context) {
    if (endsWith(context.keyword(), "%")) return context.invalid();
    const auto parsed = context.length();
    return parsed && parsed->percent == 0.f ? context.compiled(*parsed) : context.invalid();
}

std::optional<BackgroundLayer> parseBackgroundImage(detail::CSSValueRange value);
template<typename Parse> std::optional<std::vector<BackgroundLayer>> parseBackgroundLayerList(detail::CSSValueRange value, Parse parse);

std::optional<BackgroundLayer> parseBackgroundImage(detail::CSSValueRange value) {
    BackgroundLayer layer;
    if (normalizeCSSKeyword(value.stream, value.range) == "none") return layer;
    if (const std::optional<std::string> url = detail::parseCSSUrl(value.stream, value.range)) {
        layer.resource = *url;
        return layer;
    }
    if (const std::optional<Gradient> gradient = StyleModel::parseGradient(value)) {
        layer.gradient = *gradient;
        return layer;
    }
    return std::nullopt;
}

template<typename Parse> std::optional<std::vector<BackgroundLayer>> parseBackgroundLayerList(detail::CSSValueRange value, Parse parse) {
    const std::vector<detail::CSSTokenRange> values = detail::splitCSSOnDelimiter(value.stream, value.range, ',');
    if (values.empty()) return std::nullopt;
    std::vector<BackgroundLayer> result;
    result.reserve(values.size());
    for (const detail::CSSTokenRange range : values) {
        const std::optional<BackgroundLayer> layer = parse({value.stream, range});
        if (!layer) return std::nullopt;
        result.push_back(*layer);
    }
    return result;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundImages(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) { return parseBackgroundImage(range); });
}

std::optional<Length> parsePositionHorizontal(detail::CSSValueRange value);
std::optional<Length> parsePositionVertical(detail::CSSValueRange value);
std::optional<BackgroundPosition> parseBackgroundPosition(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundPositions(detail::CSSValueRange value);
std::optional<BackgroundSize> parseBackgroundSize(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundSizes(detail::CSSValueRange value);
std::optional<BackgroundRepeat> parseBackgroundRepeat(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundRepeats(detail::CSSValueRange value);
std::optional<BackgroundBox> parseBackgroundBox(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundBoxes(detail::CSSValueRange value);
std::optional<std::vector<BackgroundLayer>> parseBackgroundAttachments(detail::CSSValueRange value);
std::optional<BackgroundAttachment> parseBackgroundAttachment(detail::CSSValueRange value);

std::optional<Length> parsePositionHorizontal(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "left") return Length{0.f};
    if (token == "center") return Length{0.f, .5f};
    if (token == "right") return Length{0.f, 1.f};
    return StyleModel::parseLengthValue(value);
}

std::optional<Length> parsePositionVertical(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "bottom") return Length{0.f};
    if (token == "center") return Length{0.f, .5f};
    if (token == "top") return Length{0.f, 1.f};
    const std::optional<Length> length = StyleModel::parseLengthValue(value);
    if (!length) return std::nullopt;
    return Length{-length->pixels, 1.f - length->percent};
}

bool isPositionToken(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    const auto dimension = detail::parseCSSDimension(value.stream, value.range);
    const auto& tokens = value.stream.tokens();
    const detail::CSSTokenRange trimmed = detail::trimCSSRange(value.stream, value.range);
    const bool scalar = trimmed.end == trimmed.begin + 1
        && trimmed.begin < tokens.size()
        && (tokens[trimmed.begin].kind == detail::CSSTokenKind::Number || tokens[trimmed.begin].kind == detail::CSSTokenKind::Percentage);
    return token == "left"
        || token == "right"
        || token == "top"
        || token == "bottom"
        || token == "center"
        || (dimension && dimension->unit == "px")
        || scalar;
}

std::optional<BackgroundPosition> parseBackgroundPosition(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "top" || token == "bottom") return BackgroundPosition{{0.f, .5f}, *parsePositionVertical({value.stream, tokens.front()})};
        const std::optional<Length> x = parsePositionHorizontal({value.stream, tokens.front()});
        return x ? std::optional<BackgroundPosition>(BackgroundPosition{*x, {0.f, .5f}}) : std::nullopt;
    }

    std::optional<Length> x = parsePositionHorizontal({value.stream, tokens[0]});
    std::optional<Length> y = parsePositionVertical({value.stream, tokens[1]});
    if (!x || !y) {
        x = parsePositionHorizontal({value.stream, tokens[1]});
        y = parsePositionVertical({value.stream, tokens[0]});
    }
    return x && y ? std::optional<BackgroundPosition>(BackgroundPosition{*x, *y}) : std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundPositions(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundPosition> position = parseBackgroundPosition(range);
        if (!position) return std::nullopt;
        BackgroundLayer layer;
        layer.position = *position;
        return layer;
    });
}

std::optional<BackgroundSize> parseBackgroundSize(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "cover") return BackgroundSize{BackgroundSizeMode::Cover};
        if (token == "contain") return BackgroundSize{BackgroundSizeMode::Contain};
        if (token == "auto") return BackgroundSize{};
    }
    if (tokens.empty() || tokens.size() > 2) return std::nullopt;
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "auto" && normalizeCSSKeyword(value.stream, tokens[1]) == "auto")
        return BackgroundSize{};
    const auto parse = [&value](detail::CSSTokenRange token) -> std::optional<Length> {
        if (normalizeCSSKeyword(value.stream, token) == "auto") return std::optional<Length>{};
        const std::optional<Length> length = StyleModel::parseLengthValue({value.stream, token});
        return length && length->pixels >= 0.f && length->percent >= 0.f ? length : std::nullopt;
    };
    const std::optional<Length> width = parse(tokens[0]);
    const bool heightAuto = tokens.size() == 1 || normalizeCSSKeyword(value.stream, tokens[1]) == "auto";
    const std::optional<Length> height = tokens.size() == 1 ? std::optional<Length>{} : parse(tokens[1]);
    if ((!width && normalizeCSSKeyword(value.stream, tokens[0]) != "auto") || (!height && !heightAuto)) return std::nullopt;
    return BackgroundSize{BackgroundSizeMode::Explicit, width, height};
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundSizes(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundSize> size = parseBackgroundSize(range);
        if (!size) return std::nullopt;
        BackgroundLayer layer;
        layer.size = *size;
        return layer;
    });
}

std::optional<BackgroundRepeat> parseBackgroundRepeat(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range);
    if (tokens.size() == 1) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens.front());
        if (token == "repeat") return BackgroundRepeat::Repeat;
        if (token == "no-repeat") return BackgroundRepeat::NoRepeat;
        if (token == "repeat-x") return BackgroundRepeat::RepeatX;
        if (token == "repeat-y") return BackgroundRepeat::RepeatY;
    }
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "repeat" && normalizeCSSKeyword(value.stream, tokens[1]) == "no-repeat")
        return BackgroundRepeat::RepeatX;
    if (tokens.size() == 2 && normalizeCSSKeyword(value.stream, tokens[0]) == "no-repeat" && normalizeCSSKeyword(value.stream, tokens[1]) == "repeat")
        return BackgroundRepeat::RepeatY;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundRepeats(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat(range);
        if (!repeat) return std::nullopt;
        BackgroundLayer layer;
        layer.repeat = *repeat;
        return layer;
    });
}

std::optional<BackgroundBox> parseBackgroundBox(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "border-box") return BackgroundBox::BorderBox;
    if (token == "padding-box") return BackgroundBox::PaddingBox;
    if (token == "content-box") return BackgroundBox::ContentBox;
    return std::nullopt;
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundBoxes(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::optional<BackgroundBox> box = parseBackgroundBox(range);
        if (!box) return std::nullopt;
        BackgroundLayer layer;
        layer.origin = *box;
        layer.clip = *box;
        return layer;
    });
}

std::optional<std::vector<BackgroundLayer>> parseBackgroundAttachments(detail::CSSValueRange value) {
    return parseBackgroundLayerList(value, [](detail::CSSValueRange range) -> std::optional<BackgroundLayer> {
        const std::string token = normalizeCSSKeyword(range.stream, range.range);
        BackgroundLayer layer;
        if (token == "scroll") layer.attachment = BackgroundAttachment::Scroll;
        else if (token == "fixed") layer.attachment = BackgroundAttachment::Fixed;
        else if (token == "local") layer.attachment = BackgroundAttachment::Local;
        else return std::nullopt;
        return layer;
    });
}

std::optional<BackgroundAttachment> parseBackgroundAttachment(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
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

std::optional<std::vector<MaskLayer>> parseMaskImages(detail::CSSValueRange value) {
    const std::optional<std::vector<BackgroundLayer>> images =
        parseBackgroundLayerList(value, [](detail::CSSValueRange range) { return parseBackgroundImage(range); });
    if (!images) return std::nullopt;
    return makeMaskLayers(*images);
}

CompileResult compileBackgroundImage(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundImages({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileBackgroundPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Position});
}

CompileResult compileBackgroundSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Size});
}

CompileResult compileBackgroundRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Repeat});
}

CompileResult compileBackgroundBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes({context.value.stream, context.value.range});
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
    const auto parsed = parseBackgroundAttachments({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleImageLayers{*parsed, StyleImageComponent::Attachment});
}

CompileResult compileMaskImage(detail::StyleCompileContext& context) {
    const auto parsed = parseMaskImages({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{*parsed, StyleImageComponent::Image});
}

CompileResult compileMaskPosition(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundPositions({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Position});
}

CompileResult compileMaskSize(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundSizes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Size});
}

CompileResult compileMaskRepeat(detail::StyleCompileContext& context) {
    const auto parsed = parseBackgroundRepeats({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), StyleImageComponent::Repeat});
}

CompileResult compileMaskBox(detail::StyleCompileContext& context, StyleImageComponent component) {
    const auto parsed = parseBackgroundBoxes({context.value.stream, context.value.range});
    if (!parsed) return context.invalid();
    return context.compiled(StyleMaskLayers{makeMaskLayers(*parsed), component});
}

CompileResult compileMaskOrigin(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Origin);
}

CompileResult compileMaskClip(detail::StyleCompileContext& context) {
    return compileMaskBox(context, StyleImageComponent::Clip);
}

std::optional<MaskMode> parseMaskMode(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "match-source") return MaskMode::MatchSource;
    if (token == "alpha") return MaskMode::Alpha;
    if (token == "luminance") return MaskMode::Luminance;
    return std::nullopt;
}

std::optional<MaskComposite> parseMaskComposite(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "add") return MaskComposite::Add;
    if (token == "subtract") return MaskComposite::Subtract;
    if (token == "intersect") return MaskComposite::Intersect;
    if (token == "exclude") return MaskComposite::Exclude;
    return std::nullopt;
}

std::optional<MaskType> parseMaskType(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "luminance") return MaskType::Luminance;
    if (token == "alpha") return MaskType::Alpha;
    return std::nullopt;
}

template<typename Enum> CompileResult compileMaskEnum(detail::StyleCompileContext& context, StyleImageComponent component,
                                                      std::optional<Enum> (*parse)(detail::CSSValueRange), Enum MaskLayer::* member) {
    const std::vector<detail::CSSTokenRange> values = context.commaSeparatedRanges();
    if (values.empty()) return context.invalid();
    std::vector<MaskLayer> layers;
    layers.reserve(values.size());
    for (const detail::CSSTokenRange range : values) {
        MaskLayer layer;
        const std::optional<Enum> parsed = parse({context.value.stream, range});
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
    std::vector<detail::CSSTokenRange> extras;
    bool repeatSpecified = false;
    bool attachmentSpecified = false;
};

std::optional<ParsedImageLayer> parseImageLayer(detail::CSSValueRange value);

std::optional<ParsedImageLayer> parseImageLayer(detail::CSSValueRange value) {
    const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, value.range, true);
    if (tokens.empty()) return std::nullopt;

    ParsedImageLayer result;
    std::vector<detail::CSSTokenRange> positions;
    std::vector<detail::CSSTokenRange> sizes;
    bool afterSlash = false;
    bool sawImage = false;
    for (std::size_t index = 0; index < tokens.size(); ++index) {
        const std::string token = normalizeCSSKeyword(value.stream, tokens[index]);
        const auto parseRepeat = [&]() -> std::optional<BackgroundRepeat> {
            if (index + 1 < tokens.size() && normalizeCSSKeyword(value.stream, tokens[index + 1]) != "/") {
                const detail::CSSTokenRange combined{tokens[index].begin, tokens[index + 1].end};
                if (const std::optional<BackgroundRepeat> repeat = parseBackgroundRepeat({value.stream, combined})) {
                    ++index;
                    return repeat;
                }
            }
            return parseBackgroundRepeat({value.stream, tokens[index]});
        };
        if (token == "/") {
            if (afterSlash) return std::nullopt;
            afterSlash = true;
        } else if (afterSlash) {
            if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
                if (result.repeatSpecified) return std::nullopt;
                result.image.repeat = *repeat;
                result.repeatSpecified = true;
            } else if (const std::optional<BackgroundBox> box = parseBackgroundBox({value.stream, tokens[index]})) result.boxes.push_back(*box);
            else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment({value.stream, tokens[index]})) {
                if (result.attachmentSpecified) return std::nullopt;
                result.image.attachment = *attachment;
                result.attachmentSpecified = true;
            } else if (parseBackgroundSize({value.stream, tokens[index]})) sizes.push_back(tokens[index]);
            else result.extras.push_back(tokens[index]);
        } else if (const std::optional<BackgroundLayer> image = parseBackgroundImage({value.stream, tokens[index]})) {
            if (sawImage) return std::nullopt;
            result.image.resource = image->resource;
            result.image.gradient = image->gradient;
            sawImage = true;
        } else if (const std::optional<BackgroundRepeat> repeat = parseRepeat()) {
            if (result.repeatSpecified) return std::nullopt;
            result.image.repeat = *repeat;
            result.repeatSpecified = true;
        } else if (const std::optional<BackgroundBox> box = parseBackgroundBox({value.stream, tokens[index]})) result.boxes.push_back(*box);
        else if (const std::optional<BackgroundAttachment> attachment = parseBackgroundAttachment({value.stream, tokens[index]})) {
            if (result.attachmentSpecified) return std::nullopt;
            result.image.attachment = *attachment;
            result.attachmentSpecified = true;
        } else if (isPositionToken(detail::CSSValueRange{value.stream, tokens[index]})) positions.push_back(tokens[index]);
        else result.extras.push_back(tokens[index]);
    }
    if (positions.size() > 2 || sizes.size() > 2 || (afterSlash && sizes.empty())) return std::nullopt;
    if (!positions.empty()) {
        const detail::CSSTokenRange positionRange = positions.size() > 1 ? detail::CSSTokenRange{positions[0].begin, positions[1].end} : positions[0];
        const std::optional<BackgroundPosition> position = parseBackgroundPosition({value.stream, positionRange});
        if (!position) return std::nullopt;
        result.image.position = *position;
    }
    if (!sizes.empty()) {
        const detail::CSSTokenRange sizeRange = sizes.size() > 1 ? detail::CSSTokenRange{sizes[0].begin, sizes[1].end} : sizes[0];
        const std::optional<BackgroundSize> size = parseBackgroundSize({value.stream, sizeRange});
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
    const std::vector<detail::CSSTokenRange> layers = context.commaSeparatedRanges();
    if (layers.empty()) return context.invalid();
    std::vector<BackgroundLayer> parsed;
    std::optional<StyleColorValue> color;
    bool currentColor = false;
    for (std::size_t layerIndex = 0; layerIndex < layers.size(); ++layerIndex) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer({context.value.stream, layers[layerIndex]});
        if (!parsedLayer || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::PaddingBox, BackgroundBox::BorderBox))
            return context.invalid();
        for (const detail::CSSTokenRange token : parsedLayer->extras) {
            const detail::CSSValueRange value{context.value.stream, token};
            if (const std::optional<StyleColorValue> parsedColor = context.colorValue(value)) {
                if (layerIndex + 1 != layers.size() || color) return context.invalid();
                color = *parsedColor;
            } else if (normalizeCSSKeyword(value.stream, value.range) == "currentcolor") {
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
    const std::vector<detail::CSSTokenRange> layers = context.commaSeparatedRanges();
    if (layers.empty()) return context.invalid();
    std::vector<MaskLayer> parsed;
    for (const detail::CSSTokenRange rawLayer : layers) {
        std::optional<ParsedImageLayer> parsedLayer = parseImageLayer({context.value.stream, rawLayer});
        if (!parsedLayer
            || parsedLayer->attachmentSpecified
            || !applyImageBoxes(parsedLayer->image, parsedLayer->boxes, BackgroundBox::BorderBox, BackgroundBox::BorderBox))
            return context.invalid();
        MaskLayer layer;
        layer.image = std::move(parsedLayer->image);
        bool modeSpecified = false;
        bool compositeSpecified = false;
        for (const detail::CSSTokenRange token : parsedLayer->extras) {
            const detail::CSSValueRange value{context.value.stream, token};
            if (const std::optional<MaskMode> mode = parseMaskMode(value)) {
                if (modeSpecified) return context.invalid();
                layer.mode = *mode;
                modeSpecified = true;
            } else if (const std::optional<MaskComposite> composite = parseMaskComposite(value)) {
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
    auto& [property, value, selector, result, sourceName] = context;
    if (const std::optional<Gradient> gradient = StyleModel::parseGradient({value.stream, value.range}))
        return context.compiled(StylePaint{Color(), *gradient});
    if (normalizeCSSKeyword(value.stream, value.range) == "currentcolor")
        return context.compiled(StylePaint{Color(), std::nullopt, std::nullopt, true});
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
    const std::string value = context.keyword();
    if (value == "auto") return context.compiled(AccentColor{});
    if (value == "currentcolor") return context.compiled(AccentColor::currentColor());
    const auto parsed = context.colorValue();
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(AccentColor::fromColor(*color));
    return context.compiled(AccentColor::fromLightDark(std::get<LightDarkColor>(*parsed)));
}

CompileResult compileBorder(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.size() == 1 && context.keyword() == "none")
        return context.compiled(StyleBorder{0.f, StylePaint{Color(0.f, 0.f, 0.f, 0.f)}, BorderStyle::NoneValue});
    if (tokens.size() < 2 || tokens.size() > 3) return context.invalid();
    std::optional<float> width;
    std::optional<BorderStyle> borderStyle;
    std::optional<StylePaint> paint;
    for (const detail::CSSTokenRange token : tokens) {
        if (!width) {
            if (const auto parsedWidth = context.number({value.stream, token}); parsedWidth && *parsedWidth >= 0.f) {
                width = *parsedWidth;
                continue;
            }
        }
        if (!borderStyle) {
            if (const std::optional<BorderStyle> parsedStyle = parseBorderStyle({value.stream, token})) {
                borderStyle = *parsedStyle;
                continue;
            }
        }
        if (!paint && normalizeCSSKeyword(value.stream, token) == "currentcolor") {
            paint = StylePaint{Color(), std::nullopt, std::nullopt, true};
            continue;
        }
        if (!paint) {
            if (const std::optional<Gradient> gradient = StyleModel::parseGradient({value.stream, token})) {
                paint = StylePaint{Color(), *gradient};
                continue;
            }
            if (const auto parsedColor = context.colorValue({value.stream, token})) {
                if (const auto color = std::get_if<Color>(&*parsedColor)) paint = StylePaint{*color, std::nullopt};
                else paint = StylePaint{Color(0.f, 0.f, 0.f, 0.f), std::nullopt, std::get<LightDarkColor>(*parsedColor)};
                continue;
            }
        }
        return context.invalid();
    }
    return width && paint ? context.compiled(StyleBorder{*width, *paint, borderStyle.value_or(BorderStyle::NoneValue)}) : context.invalid();
}

CompileResult compileBorderStyle(detail::StyleCompileContext& context) {
    const std::optional<BorderStyle> parsed = parseBorderStyle({context.value.stream, context.value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileEdges(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const float nan = std::numeric_limits<float>::quiet_NaN();
    const EdgeInsets parsed = StyleModel::parseEdgeInsets({value.stream, value.range}, {nan, nan, nan, nan});
    return std::isfinite(parsed.top) ? context.compiled(parsed) : context.invalid();
}

CompileResult compileMargin(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseMargin({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileGap(detail::StyleCompileContext& context) {
    const std::vector<detail::CSSTokenRange> tokens = context.ranges(false);
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto fixedGap = [&context](detail::CSSTokenRange range) -> std::optional<Length> {
        range = detail::trimCSSRange(context.value.stream, range);
        if (range.begin == range.end || context.value.stream.tokens()[range.begin].kind == detail::CSSTokenKind::Percentage) return std::nullopt;
        const std::optional<Length> parsed = context.nonnegativeLength({context.value.stream, range});
        return parsed && parsed->percent == 0.f ? parsed : std::nullopt;
    };
    const std::optional<Length> row = fixedGap(tokens.front());
    const std::optional<Length> column = fixedGap(tokens.size() == 1 ? tokens.front() : tokens.back());
    if (!row || !column) return context.invalid();
    return makeDeclarations({{"row-gap", GapValue::fromLength(*row)}, {"column-gap", GapValue::fromLength(*column)}});
}

CompileResult compileGapLonghand(detail::StyleCompileContext& context) {
    const std::vector<detail::CSSTokenRange> tokens = context.ranges(false);
    if (tokens.size() != 1) return context.invalid();
    const detail::CSSTokenRange range = detail::trimCSSRange(context.value.stream, tokens.front());
    if (range.begin == range.end || context.value.stream.tokens()[range.begin].kind == detail::CSSTokenKind::Percentage) return context.invalid();
    const std::optional<Length> parsed = context.nonnegativeLength({context.value.stream, range});
    if (!parsed || parsed->percent != 0.f) return context.invalid();
    return context.compiled(GapValue::fromLength(*parsed));
}

CompileResult compileMinimumLength(detail::StyleCompileContext& context) {
    if (context.keyword() == "auto") return context.compiled(std::optional<Length>{});
    const std::vector<detail::CSSTokenRange> tokens = context.ranges(false);
    if (tokens.size() != 1) return context.invalid();
    const std::optional<Length> parsed = context.nonnegativeLength({context.value.stream, tokens.front()});
    return parsed ? context.compiled(std::optional<Length>(*parsed)) : context.invalid();
}

CompileResult compileSize(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto dimension = [&context, &value](detail::CSSTokenRange raw) -> std::optional<Dimension> {
        if (normalizeCSSKeyword(value.stream, raw) == "auto") return Dimension();
        const auto parsed = context.nonnegativeLength({value.stream, raw});
        return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
    };
    const auto height = dimension(tokens[0]);
    const auto width = dimension(tokens.size() == 1 ? tokens[0] : tokens[1]);
    return height && width ? context.compiled(StyleSize{*height, *width}) : context.invalid();
}

CompileResult compileMinSize(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto height = context.nonnegativeLength({value.stream, tokens[0]});
    const auto width = context.nonnegativeLength({value.stream, tokens.size() == 1 ? tokens[0] : tokens[1]});
    if (!height || !width) return context.invalid();
    return makeDeclarations({{"min-height", std::optional<Length>(*height)}, {"min-width", std::optional<Length>(*width)}});
}

CompileResult compileStrokeLinecap(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    StrokeCap cap;
    return parseStrokeCap({context.value.stream, context.value.range}, cap) ? context.compiled(cap) : context.invalid();
}

CompileResult compileFontFamily(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    if (context.keyword() == "sans-serif") return context.compiled(FontFamily::SansSerif);
    if (context.keyword() == "monospace") return context.compiled(FontFamily::Monospace);
    return context.invalid();
}

CompileResult compileFont(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseFontShorthand({value.stream, value.range});
    return parsed ? parsed : context.invalid();
}

CompileResult compileFontWeight(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    if (context.keyword() == "bolder") return context.compiled(RelativeFontWeight{false});
    if (context.keyword() == "lighter") return context.compiled(RelativeFontWeight{true});
    const auto parsed = StyleModel::parseFontWeightValue({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileFontStyle(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseFontStyleValue({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileTextDecoration(detail::StyleCompileContext& context) {
    const std::string decoration = context.keyword();
    if (decoration == "none") return context.compiled(TextDecoration::NoneValue);
    if (decoration == "underline") return context.compiled(TextDecoration::Underline);
    if (decoration == "line-through") return context.compiled(TextDecoration::LineThrough);
    return context.invalid();
}

CompileResult compileTextAlign(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string alignment = context.keyword();
    std::optional<TextAlign> parsed;
    if (alignment == "left") parsed = TextAlign::Left;
    else if (alignment == "start") parsed = TextAlign::Start;
    else if (alignment == "center") parsed = TextAlign::Center;
    else if (alignment == "right") parsed = TextAlign::Right;
    else if (alignment == "end") parsed = TextAlign::End;
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileTextOverflow(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string overflow = context.keyword();
    if (overflow == "clip") return context.compiled(TextOverflow::Clip);
    if (overflow == "ellipsis") return context.compiled(TextOverflow::Ellipsis);
    if (overflow == "ellipsis-center") return context.compiled(TextOverflow::EllipsisCenter);
    return context.invalid();
}

CompileResult compileTextWrap(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string wrap = context.keyword();
    if (wrap == "wrap") return context.compiled(TextWrap::Wrap);
    if (wrap == "nowrap") return context.compiled(TextWrap::NoWrap);
    return context.invalid();
}

CompileResult compileVerticalAlign(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string alignment = context.keyword();
    std::optional<VerticalAlign> parsed;
    if (alignment == "top") parsed = VerticalAlign::Top;
    else if (alignment == "middle") parsed = VerticalAlign::Middle;
    else if (alignment == "bottom") parsed = VerticalAlign::Bottom;
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileFlexDirection(detail::StyleCompileContext& context) {
    const std::string direction = context.keyword();
    if (direction == "row") return context.compiled(FlexDirection::Row);
    if (direction == "column") return context.compiled(FlexDirection::Column);
    return context.invalid();
}

template<typename Enum, std::size_t Size> CompileResult compileAlignment(const detail::StyleCompileContext& context, std::string_view value,
                                                                         const std::array<std::pair<std::string_view, Enum>, Size>& values) {
    const auto found = std::find_if(values.begin(), values.end(), [value](const auto& entry) { return entry.first == value; });
    return found == values.end() ? context.invalid() : context.compiled(found->second);
}

CompileResult compileJustifyContent(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, JustifyContent>, 5> sJustifyContentValues{{
        {"start", JustifyContent::Start},
        {"left", JustifyContent::Left},
        {"center", JustifyContent::Center},
        {"end", JustifyContent::End},
        {"right", JustifyContent::Right},
    }};
    return compileAlignment(context, context.keyword(), sJustifyContentValues);
}

CompileResult compileAlignItems(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignItems>, 5> sAlignItemsValues{{
        {"normal", AlignItems::Normal},
        {"start", AlignItems::Start},
        {"center", AlignItems::Center},
        {"end", AlignItems::End},
        {"stretch", AlignItems::Stretch},
    }};
    return compileAlignment(context, context.keyword(), sAlignItemsValues);
}

CompileResult compileInternalAlignContentBlock(detail::StyleCompileContext& context) {
    const std::string alignment = context.keyword();
    if (alignment == "normal") return context.compiled(false);
    if (alignment == "center") return context.compiled(true);
    return context.invalid();
}

CompileResult compileAlignSelf(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignSelf>, 5> sAlignSelfValues{{
        {"auto", AlignSelf::Auto},
        {"start", AlignSelf::Start},
        {"center", AlignSelf::Center},
        {"end", AlignSelf::End},
        {"stretch", AlignSelf::Stretch},
    }};
    return compileAlignment(context, context.keyword(), sAlignSelfValues);
}

CompileResult compileJustifySelf(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, JustifySelf>, 5> sJustifySelfValues{{
        {"auto", JustifySelf::Auto},
        {"start", JustifySelf::Start},
        {"center", JustifySelf::Center},
        {"end", JustifySelf::End},
        {"stretch", JustifySelf::Stretch},
    }};
    return compileAlignment(context, context.keyword(), sJustifySelfValues);
}

CompileResult compileFlex(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty() || tokens.size() > 3) return context.invalid();
    const std::string keyword = context.keyword();
    if (keyword == "none") return makeDeclarations({{"flex-grow", 0.f}, {"flex-shrink", 0.f}, {"flex-basis", Dimension()}});
    if (keyword == "auto") return makeDeclarations({{"flex-grow", 1.f}, {"flex-shrink", 1.f}, {"flex-basis", Dimension()}});

    const auto nonnegativeNumber = [&context, &value](detail::CSSTokenRange raw) -> std::optional<float> {
        const std::string token = normalizeCSSKeyword(value.stream, raw);
        if (hasDimensionUnit(value.stream, raw, "px") || endsWith(token, "%")) return std::nullopt;
        const auto parsed = context.number({value.stream, raw});
        return parsed && *parsed >= 0.f ? parsed : std::nullopt;
    };
    const auto basis = [&context, &value](detail::CSSTokenRange raw) -> std::optional<Dimension> {
        if (normalizeCSSKeyword(value.stream, raw) == "auto") return Dimension();
        const auto parsed = context.nonnegativeLength({value.stream, raw});
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
    auto& [property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, PointerEvents>, 3> sPointerEventsValues{{
        {"auto", PointerEvents::Auto},
        {"none", PointerEvents::PassThrough},
        {"default", PointerEvents::Default},
    }};
    return compileAlignment(context, context.keyword(), sPointerEventsValues);
}

CompileResult compileDisplay(detail::StyleCompileContext& context) {
    const std::string display = context.keyword();
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
    const std::string appearance = context.keyword();
    if (appearance == "auto") return context.compiled(AppearanceMode::Auto);
    if (appearance == "base") return context.compiled(AppearanceMode::Base);
    if (appearance == "none") return context.compiled(AppearanceMode::Unstyled);
    return context.invalid();
}

CompileResult compileBoxSizing(detail::StyleCompileContext& context) {
    const std::string boxSizing = context.keyword();
    if (boxSizing == "content-box") return context.compiled(BoxSizing::ContentBox);
    if (boxSizing == "border-box") return context.compiled(BoxSizing::BorderBox);
    return context.invalid();
}

CompileResult compileColorScheme(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.size() == 1) {
        const std::string scheme = normalizeCSSKeyword(value.stream, tokens.front());
        if (scheme == "auto" || scheme == "normal") return context.compiled(ColorScheme::Auto);
        if (scheme == "light") return context.compiled(ColorScheme::Light);
        if (scheme == "dark") return context.compiled(ColorScheme::Dark);
    } else if (tokens.size() == 2) {
        const std::string first = normalizeCSSKeyword(value.stream, tokens[0]);
        const std::string second = normalizeCSSKeyword(value.stream, tokens[1]);
        if ((first == "light" && second == "dark") || (first == "dark" && second == "light")) return context.compiled(ColorScheme::LightDark);
    }
    return context.invalid();
}

CompileResult compileGridArea(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges(true);
    if (tokens.size() != 3 || normalizeCSSKeyword(value.stream, tokens[1]) != "/") return context.invalid();
    const auto line = [&context, &value](detail::CSSTokenRange raw) -> std::optional<int> {
        const std::string token = normalizeCSSKeyword(value.stream, raw);
        if (hasDimensionUnit(value.stream, raw, "px") || endsWith(token, "%")) return std::nullopt;
        const auto parsed = context.number({value.stream, raw});
        if (!parsed || *parsed < 1.f || std::floor(*parsed) != *parsed || *parsed > static_cast<float>(std::numeric_limits<int>::max()))
            return std::nullopt;
        return static_cast<int>(*parsed);
    };
    const auto row = line(tokens[0]);
    const auto column = line(tokens[2]);
    return row && column ? context.compiled(GridArea{*row, *column}) : context.invalid();
}

CompileResult compilePositionMode(detail::StyleCompileContext& context) {
    const std::string position = context.keyword();
    if (position == "static") return context.compiled(PositionMode::Static);
    if (position == "relative") return context.compiled(PositionMode::Relative);
    return context.invalid();
}

CompileResult compileTranslate(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto fixedLength = [&context, &value](detail::CSSTokenRange raw) -> std::optional<float> {
        const auto parsed = context.length({value.stream, raw});
        return parsed && parsed->percent == 0.f ? std::optional<float>(parsed->pixels) : std::nullopt;
    };
    const auto x = fixedLength(tokens[0]);
    const auto y = tokens.size() == 1 ? std::optional<float>(0.f) : fixedLength(tokens[1]);
    return x && y ? context.compiled(Translate{*x, *y}) : context.invalid();
}

CompileResult compileVisibility(detail::StyleCompileContext& context) {
    const std::string visibility = context.keyword();
    if (visibility == "visible") return context.compiled(Visibility::Visible);
    if (visibility == "hidden") return context.compiled(Visibility::Hidden);
    if (visibility == "collapse") return context.compiled(Visibility::Collapse);
    return context.invalid();
}

std::optional<Overflow> parseOverflow(detail::CSSValueRange value) {
    const std::string token = normalizeCSSKeyword(value.stream, value.range);
    if (token == "visible") return Overflow::Visible;
    if (token == "hidden") return Overflow::Hidden;
    if (token == "scroll") return Overflow::Scroll;
    if (token == "auto") return Overflow::Auto;
    return std::nullopt;
}

CompileResult compileOverflow(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.empty() || tokens.size() > 2) return context.invalid();
    const auto horizontal = parseOverflow({value.stream, tokens[0]});
    const auto vertical = parseOverflow({value.stream, tokens.size() == 1 ? tokens[0] : tokens[1]});
    if (!horizontal || !vertical) return context.invalid();
    return makeDeclarations({{"overflow-x", *horizontal}, {"overflow-y", *vertical}});
}

CompileResult compileOverflowAxis(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = parseOverflow({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileScrollbarWidth(detail::StyleCompileContext& context) {
    const std::string width = context.keyword();
    if (width == "auto") return context.compiled(ScrollbarWidth::Auto);
    if (width == "thin") return context.compiled(ScrollbarWidth::Thin);
    if (width == "none") return context.compiled(ScrollbarWidth::NoneValue);
    return context.invalid();
}

CompileResult compileScrollbarGutter(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.size() == 1) {
        const std::string gutter = normalizeCSSKeyword(value.stream, tokens[0]);
        if (gutter == "auto") return context.compiled(ScrollbarGutter::Auto);
        if (gutter == "stable") return context.compiled(ScrollbarGutter::Stable);
    } else if (tokens.size() == 2
               && normalizeCSSKeyword(value.stream, tokens[0]) == "stable"
               && normalizeCSSKeyword(value.stream, tokens[1]) == "both-edges")
        return context.compiled(ScrollbarGutter::StableBothEdges);
    return context.invalid();
}

CompileResult compileScrollbarColor(detail::StyleCompileContext& context) {
    const auto& value = context.value;
    if (normalizeCSSKeyword(value.stream, value.range) == "auto") return context.compiled(ScrollbarColors{});
    const std::vector<detail::CSSTokenRange> tokens = context.ranges();
    if (tokens.size() != 2) return context.invalid();
    const std::optional<StyleColorValue> thumb = context.colorValue({value.stream, tokens[0]});
    const std::optional<StyleColorValue> track = context.colorValue({value.stream, tokens[1]});
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
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    const double numericOrder = parsed ? static_cast<double>(*parsed) : 0.0;
    if (!parsed
        || hasDimensionUnit(value.stream, value.range, "px")
        || std::trunc(*parsed) != *parsed
        || numericOrder < static_cast<double>(std::numeric_limits<int>::min())
        || numericOrder > static_cast<double>(std::numeric_limits<int>::max()))
        return context.invalid();
    return context.compiled(static_cast<int>(*parsed));
}

CompileResult compileCursor(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
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
    const auto parseKeyword = [](std::string_view normalized) -> std::optional<CursorStyle> {
        const auto found =
            std::find_if(sCursorValues.begin(), sCursorValues.end(), [&normalized](const auto& entry) { return entry.first == normalized; });
        return found == sCursorValues.end() ? std::nullopt : std::optional<CursorStyle>(found->second);
    };

    const std::vector<detail::CSSTokenRange> candidates = context.commaSeparatedRanges();
    if (candidates.size() == 1) {
        const std::optional<CursorStyle> keyword = parseKeyword(normalizeCSSKeyword(value.stream, candidates.front()));
        return keyword ? context.compiled(CursorValue{*keyword, {}}) : context.invalid();
    }
    const auto parseHotspot = [&value](detail::CSSTokenRange range) -> std::optional<float> {
        range = detail::trimCSSRange(value.stream, range);
        if (range.end != range.begin + 1 || value.stream.tokens()[range.begin].kind != detail::CSSTokenKind::Number) return std::nullopt;
        const float parsed = StyleModel::parseNumberValue({value.stream, range}, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    };
    std::vector<CursorImage> images;
    for (std::size_t index = 0; index + 1 < candidates.size(); ++index) {
        const std::vector<detail::CSSTokenRange> tokens = detail::splitCSSComponents(value.stream, candidates[index]);
        if (tokens.size() != 1 && tokens.size() != 3) return context.invalid();
        const std::optional<std::string> resource = detail::parseCSSUrl(value.stream, tokens.front());
        if (!resource) return context.invalid();
        CursorImage image;
        image.resource = *resource;
        if (tokens.size() > 1) image.hotspotX = parseHotspot(tokens[1]);
        if (tokens.size() > 2) image.hotspotY = parseHotspot(tokens[2]);
        if ((tokens.size() > 1 && !image.hotspotX) || (tokens.size() > 2 && !image.hotspotY)) return context.invalid();
        images.push_back(std::move(image));
    }
    const std::optional<CursorStyle> fallback = parseKeyword(normalizeCSSKeyword(value.stream, candidates.back()));
    return fallback && !images.empty() ? context.compiled(CursorValue{*fallback, std::move(images)}) : context.invalid();
}

CompileResult compileDimension(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    if (context.keyword() == "auto") return context.compiled(Dimension());
    const auto parsed = context.length();
    if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return context.invalid();
    return context.compiled(Dimension::fromLength(*parsed));
}

CompileResult compilePosition(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.length();
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileNonnegativeLength(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.nonnegativeLength();
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileUnitlessNonnegativeNumber(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const std::string raw = context.keyword();
    if (hasDimensionUnit(value.stream, value.range, "px") || endsWith(raw, "%")) return context.invalid();
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileOpacity(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f && *parsed <= 1.f ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileStrokeWidth(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = context.number();
    return parsed && *parsed >= 0.f ? context.compiled(Length{*parsed}) : context.invalid();
}

CompileResult compileLineHeight(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    const auto parsed = StyleModel::parseLineHeightValue({value.stream, value.range});
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileSpacing(detail::StyleCompileContext& context) {
    auto& [property, value, selector, result, sourceName] = context;
    if (context.keyword() == "normal") return context.compiled(Length{});
    const auto parsed = context.length();
    return parsed ? context.compiled(*parsed) : context.invalid();
}
} // namespace

std::optional<std::vector<StyleDeclaration>> StyleModel::compileDeclaration(const detail::StylePropertyDefinition& property,
                                                                            const detail::CSSTokenStream& stream, detail::CSSTokenRange valueRange,
                                                                            const std::string& selector, StyleSheetLoadResult& result,
                                                                            const std::string& sourceName) {
    valueRange = detail::trimCSSRange(stream, valueRange);
    const std::string value = detail::trim(detail::serializeCSSRange(stream, valueRange));
    const std::string normalizedValue = detail::normalizeCSSKeyword(stream, valueRange);
    if (normalizedValue == "initial") return makeDeclarations(property, InitialStyleValue{});
    if (normalizedValue == "inherit" || normalizedValue == "unset") {
        const StyleWideKeyword keyword = normalizedValue == "inherit" ? StyleWideKeyword::Inherit : StyleWideKeyword::Unset;
        return makeDeclarations(property, keyword);
    }
    if (!property.compile) {
        result.error("stylesheet.property.value_invalid", "Property has no compiler: " + std::string(property.name) + ".", sourceName);
        return std::nullopt;
    }
    detail::StyleCompileContext context{property, StyleCompileValue{value, stream, valueRange}, selector, result, sourceName};
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

void resetFontWeight(ComputedStyle& style) {
    const ComputedStyle initial;
    style.fontWeight = initial.fontWeight;
    style.fontWeightAdjustment.reset();
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
    if (const auto relative = std::get_if<RelativeFontWeight>(&value)) {
        style.fontWeightAdjustment = *relative;
        return;
    }
    style.fontWeightAdjustment.reset();
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
constexpr std::array<std::string_view, 2> kGapLonghands{"row-gap", "column-gap"};
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
    {"backdrop-filter", compileFilter, applyMember<&ComputedStyle::backdropFilter>, resetMember<&ComputedStyle::backdropFilter>, nullptr,
     copyMember<&ComputedStyle::backdropFilter>, StylePropertyImpact::Paint},
    {"filter", compileFilter, applyMember<&ComputedStyle::filter>, resetMember<&ComputedStyle::filter>, nullptr, copyMember<&ComputedStyle::filter>,
     StylePropertyImpact::Paint},
    {"height", compileDimension, applyMember<&ComputedStyle::height>, resetMember<&ComputedStyle::height>, nullptr,
     copyMember<&ComputedStyle::height>},
    {"left", compilePosition, applyLengthToOptional<&ComputedStyle::left>, resetMember<&ComputedStyle::left>, nullptr,
     copyMember<&ComputedStyle::left>, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"margin", compileMargin, applyMember<&ComputedStyle::margin>, resetMember<&ComputedStyle::margin>, nullptr, copyMember<&ComputedStyle::margin>},
    {"min-height", compileMinimumLength, applyMember<&ComputedStyle::minHeight>, resetMember<&ComputedStyle::minHeight>, nullptr,
     copyMember<&ComputedStyle::minHeight>},
    {"min-size", compileMinSize, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kMinSizeLonghands)},
    {"min-width", compileMinimumLength, applyMember<&ComputedStyle::minWidth>, resetMember<&ComputedStyle::minWidth>, nullptr,
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
    {"gap", compileGap, nullptr, nullptr, nullptr, nullptr, StylePropertyImpact::Layout, false, InheritedStyleProperty::NotInherited,
     std::span<const std::string_view>(kGapLonghands)},
    {"row-gap", compileGapLonghand, applyMember<&ComputedStyle::rowGap>, resetMember<&ComputedStyle::rowGap>, nullptr,
     copyMember<&ComputedStyle::rowGap>},
    {"column-gap", compileGapLonghand, applyMember<&ComputedStyle::columnGap>, resetMember<&ComputedStyle::columnGap>, nullptr,
     copyMember<&ComputedStyle::columnGap>},
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
     StylePropertyImpact::Paint | StylePropertyImpact::Descendant, false, InheritedStyleProperty::TextDecoration},
    {"content", compileContent, applyMember<&ComputedStyle::content>, resetMember<&ComputedStyle::content>, nullptr,
     copyMember<&ComputedStyle::content>, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"font-weight", compileFontWeight, applyFontWeight, resetFontWeight, specifyInherited<InheritedStyleProperty::FontWeight>,
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
