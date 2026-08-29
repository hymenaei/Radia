/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <initializer_list>
#include <iterator>
#include <limits>
#include <type_traits>
#include "style/color.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"

namespace radia::ui {
namespace {
using detail::endsWith;
using detail::lower;
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
} // namespace

void detail::applyStyleDeclaration(Style& style, const StyleDeclaration& declaration) {
    const detail::StylePropertyDefinition& property = declaration.property.get();
    if (std::holds_alternative<InitialStyleValue>(declaration.value)) {
        const Style initial;
        if (property.name == "appearance") style.appearance = initial.appearance;
        else if (property.name == "color-scheme") style.colorScheme = initial.colorScheme;
        else if (property.name == "box-sizing") style.boxSizing = initial.boxSizing;
        else if (property.name == "accent-color") style.accentColor = initial.accentColor;
        else if (property.name == "background-color") {
            style.backgroundColor = initial.backgroundColor;
            style.backgroundColorLightDark = initial.backgroundColorLightDark;
            style.backgroundGradient = initial.backgroundGradient;
        } else if (property.name == "border") {
            style.borderWidth = initial.borderWidth;
            style.borderColor = initial.borderColor;
            style.borderColorLightDark = initial.borderColorLightDark;
            style.borderStyle = initial.borderStyle;
            style.borderGradient = initial.borderGradient;
        } else if (property.name == "border-color") {
            style.borderColor = initial.borderColor;
            style.borderColorLightDark = initial.borderColorLightDark;
            style.borderGradient = initial.borderGradient;
        } else if (property.name == "border-radius") style.borderRadius = initial.borderRadius;
        else if (property.name == "border-width") style.borderWidth = initial.borderWidth;
        else if (property.name == "border-style") style.borderStyle = initial.borderStyle;
        else if (property.name == "bottom") style.bottom = initial.bottom;
        else if (property.name == "cursor") style.cursor = initial.cursor;
        else if (property.name == "display") {
            style.display = initial.display;
            style.displaySet = true;
        } else if (property.name == "effect") style.effects = initial.effects;
        else if (property.name == "height") style.height = initial.height;
        else if (property.name == "left") style.left = initial.left;
        else if (property.name == "margin") style.margin = initial.margin;
        else if (property.name == "min-height") style.minHeight = initial.minHeight;
        else if (property.name == "min-width") style.minWidth = initial.minWidth;
        else if (property.name == "opacity") style.opacity = initial.opacity;
        else if (property.name == "outline") style.outline = initial.outline;
        else if (property.name == "outline-offset") style.outline.offset = initial.outline.offset;
        else if (property.name == "overflow-x") style.overflowX = initial.overflowX;
        else if (property.name == "overflow-y") style.overflowY = initial.overflowY;
        else if (property.name == "padding") style.padding = initial.padding;
        else if (property.name == "pointer-events") style.pointerEvents = initial.pointerEvents;
        else if (property.name == "position") style.position = initial.position;
        else if (property.name == "right") style.right = initial.right;
        else if (property.name == "box-shadow") style.shadows = initial.shadows;
        else if (property.name == "size") {
            style.height = initial.height;
            style.width = initial.width;
        } else if (property.name == "top") style.top = initial.top;
        else if (property.name == "translate") style.translate = initial.translate;
        else if (property.name == "width") style.width = initial.width;
        else if (property.name == "align-items") style.alignItems = initial.alignItems;
        else if (property.name == "flex-direction") {
            style.flexDirection = initial.flexDirection;
            style.flexDirectionSet = true;
        } else if (property.name == "gap") style.gap = initial.gap;
        else if (property.name == "grid-area") style.gridArea = initial.gridArea;
        else if (property.name == "justify-content") {
            style.justifyContent = initial.justifyContent;
            style.justifyContentSet = true;
        } else if (property.name == "justify-self") style.justifySelf = initial.justifySelf;
        else if (property.name == "-internal-align-content-block") style.alignContentBlockCenter = initial.alignContentBlockCenter;
        else if (property.name == "align-self") style.alignSelf = initial.alignSelf;
        else if (property.name == "flex-basis") style.flexBasis = initial.flexBasis;
        else if (property.name == "flex-grow") style.flexGrow = initial.flexGrow;
        else if (property.name == "flex-shrink") style.flexShrink = initial.flexShrink;
        else if (property.name == "order") style.order = initial.order;
        else if (property.name == "font-family") style.fontFamily = initial.fontFamily;
        else if (property.name == "font-size") style.fontSize = initial.fontSize;
        else if (property.name == "font-style") style.fontItalic = initial.fontItalic;
        else if (property.name == "text-decoration") style.textDecoration = initial.textDecoration;
        else if (property.name == "font-weight") style.fontWeight = initial.fontWeight;
        else if (property.name == "line-height") style.lineHeight = initial.lineHeight;
        else if (property.name == "letter-spacing") style.letterSpacing = initial.letterSpacing;
        else if (property.name == "word-spacing") style.wordSpacing = initial.wordSpacing;
        else if (property.name == "text-align") style.textAlign = initial.textAlign;
        else if (property.name == "color") {
            style.color = initial.color;
            style.colorLightDark = initial.colorLightDark;
        } else if (property.name == "text-overflow") style.textOverflow = initial.textOverflow;
        else if (property.name == "text-wrap") style.textWrap = initial.textWrap;
        else if (property.name == "vertical-align") {
            style.verticalAlign = initial.verticalAlign;
            style.verticalAlignSet = true;
        } else if (property.name == "visibility") style.visibility = initial.visibility;
        else if (property.name == "scrollbar-mode") {
            style.scrollbarMode = initial.scrollbarMode;
            style.scrollbarModeSet = true;
        } else if (property.name == "scrollbar-width") style.scrollbarWidth = initial.scrollbarWidth;
        else if (property.name == "scrollbar-gutter") style.scrollbarGutter = initial.scrollbarGutter;
        else if (property.name == "scrollbar-color") style.scrollbarColor = initial.scrollbarColor;
        else if (property.name == "stroke") {
            style.svgStrokeWidth = initial.svgStrokeWidth;
            style.iconStrokeColor = initial.iconStrokeColor;
            style.iconStrokeColorLightDark = initial.iconStrokeColorLightDark;
        } else if (property.name == "stroke-color") {
            style.iconStrokeColor = initial.iconStrokeColor;
            style.iconStrokeColorLightDark = initial.iconStrokeColorLightDark;
        } else if (property.name == "stroke-linecap") {
            style.svgStrokeCap = initial.svgStrokeCap;
            style.svgStrokeCapSet = true;
        } else if (property.name == "stroke-width") style.svgStrokeWidth = initial.svgStrokeWidth;
        else LL_ERRS("UI") << "Attempted to reset an unknown initial style property: " << property.name << LL_ENDL;
        if (property.specify) property.specify(style);
        return;
    }
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

CompileResult compilePaint(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (const std::optional<Gradient> gradient = model.parseGradient(value)) return context.compiled(StylePaint{Color(), *gradient});
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
    if (const std::optional<Gradient> gradient = model.parseGradient(colorToken))
        return context.compiled(StyleBorder{*width, StylePaint{Color(), *gradient}, borderStyle});
    const auto parsed = context.colorValue(colorToken);
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(StyleBorder{*width, StylePaint{*color, std::nullopt}, borderStyle});
    return context.compiled(
        StyleBorder{*width, StylePaint{Color(0.f, 0.f, 0.f, 0.f), std::nullopt, std::get<LightDarkColor>(*parsed)}, borderStyle});
}

CompileResult compileBorderStyle(detail::StyleCompileContext& context) {
    const std::optional<BorderStyle> parsed = parseBorderStyle(context.value);
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileStroke(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() != 2) return context.invalid();
    const auto width = context.number(tokens[0]);
    if (!width || *width < 0.f) return context.invalid();
    const auto parsed = context.colorValue(tokens[1]);
    if (!parsed) return context.invalid();
    if (const auto color = std::get_if<Color>(&*parsed)) return context.compiled(StyleIconStroke{*width, *color});
    return context.compiled(StyleIconStroke{*width, Color(0.f, 0.f, 0.f, 0.f), std::get<LightDarkColor>(*parsed)});
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
    return compileAlignment(context, value, sCursorValues);
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
    if (detail::lower(detail::trim(value)) == "initial") {
        if (property.name == "overflow") return makeDeclarations({{"overflow-x", InitialStyleValue{}}, {"overflow-y", InitialStyleValue{}}});
        if (property.name == "min-size") return makeDeclarations({{"min-height", InitialStyleValue{}}, {"min-width", InitialStyleValue{}}});
        if (property.name == "flex")
            return makeDeclarations({{"flex-grow", InitialStyleValue{}}, {"flex-shrink", InitialStyleValue{}}, {"flex-basis", InitialStyleValue{}}});
        if (property.name == "font")
            return makeDeclarations({{"font-style", InitialStyleValue{}},
                                     {"font-weight", InitialStyleValue{}},
                                     {"font-size", InitialStyleValue{}},
                                     {"line-height", InitialStyleValue{}},
                                     {"font-family", InitialStyleValue{}}});
        return std::vector<StyleDeclaration>{makeDeclaration(property.name, InitialStyleValue{})};
    }
    if (!property.compile) {
        result.error("stylesheet.property.value_invalid", "Property has no compiler: " + std::string(property.name) + ".", sourceName);
        return std::nullopt;
    }
    detail::StyleCompileContext context{*this, property, value, selector, result, sourceName};
    return property.compile(context);
}

namespace {

template<auto Member> void applyMember(Style& style, const StyleValue& value) {
    using Value = std::decay_t<decltype(style.*Member)>;
    style.*Member = std::get<Value>(value);
}

template<auto Member> void applyLengthToOptional(Style& style, const StyleValue& value) {
    style.*Member = std::get<Length>(value);
}

template<InheritedStyleProperty Property, auto Member> void inheritMember(Style& style, const Style& parent) {
    const auto flag = static_cast<InheritedStyleProperties>(Property);
    if ((style.specifiedInheritedProperties & flag) == 0) style.*Member = parent.*Member;
}

void inheritColor(Style& style, const Style& parent) {
    const auto flag = static_cast<InheritedStyleProperties>(InheritedStyleProperty::Color);
    if ((style.specifiedInheritedProperties & flag) == 0) {
        style.color = parent.color;
        style.colorLightDark = parent.colorLightDark;
    }
}

template<InheritedStyleProperty Property> void specifyInherited(Style& style) {
    style.specifiedInheritedProperties |= static_cast<InheritedStyleProperties>(Property);
}

void applyPaint(Color& color, std::optional<LightDarkColor>& lightDarkColor, std::optional<Gradient>& gradient, const StyleValue& value) {
    const StylePaint& paint = std::get<StylePaint>(value);
    lightDarkColor = paint.lightDarkColor;
    color = paint.gradient || paint.lightDarkColor ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
    gradient = paint.gradient;
}

void applyBackground(Style& style, const StyleValue& value) {
    applyPaint(style.backgroundColor, style.backgroundColorLightDark, style.backgroundGradient, value);
}
void applyBorder(Style& style, const StyleValue& value) {
    const StyleBorder& border = std::get<StyleBorder>(value);
    style.borderWidth = {border.width, border.width, border.width, border.width};
    style.borderColorLightDark = border.paint.lightDarkColor;
    style.borderColor =
        border.paint.gradient || border.paint.lightDarkColor ? Color(0.f, 0.f, 0.f, 0.f) : border.paint.color;
    style.borderStyle = border.style;
    style.borderGradient = border.paint.gradient;
}
void applyBorderStyle(Style& style, const StyleValue& value) {
    style.borderStyle = std::get<BorderStyle>(value);
}
void applyColor(Style& style, const StyleValue& value) {
    if (const auto lightDarkColor = std::get_if<LightDarkColor>(&value)) {
        style.colorLightDark = *lightDarkColor;
        style.color = lightDarkColor->dark;
    } else {
        style.colorLightDark.reset();
        style.color = std::get<Color>(value);
    }
}
void applyAccentColor(Style& style, const StyleValue& value) {
    style.accentColor = std::get<AccentColor>(value);
}
void applyOutline(Style& style, const StyleValue& value) {
    const Outline& outline = std::get<Outline>(value);
    style.outline.width = outline.width;
    style.outline.color = outline.color;
    style.outline.style = outline.style;
    style.outline.lightDarkColor = outline.lightDarkColor;
}
void applyOutlineOffset(Style& style, const StyleValue& value) {
    style.outline.offset = std::get<Length>(value).pixels;
}
void applySize(Style& style, const StyleValue& value) {
    const StyleSize& size = std::get<StyleSize>(value);
    style.height = size.height;
    style.width = size.width;
}
void applyDisplay(Style& style, const StyleValue& value) {
    style.display = std::get<DisplayMode>(value);
    style.displaySet = true;
}
void applyScrollbarMode(Style& style, const StyleValue& value) {
    style.scrollbarMode = std::get<ScrollbarMode>(value);
    style.scrollbarModeSet = true;
}
void applyFlexDirection(Style& style, const StyleValue& value) {
    style.flexDirection = std::get<FlexDirection>(value);
    style.flexDirectionSet = true;
}
void applyJustifyContent(Style& style, const StyleValue& value) {
    style.justifyContent = std::get<JustifyContent>(value);
    style.justifyContentSet = true;
}
void applyVerticalAlign(Style& style, const StyleValue& value) {
    style.verticalAlign = std::get<VerticalAlign>(value);
    style.verticalAlignSet = true;
}
void applyFontWeight(Style& style, const StyleValue& value) {
    style.fontWeight = static_cast<U16>(std::get<float>(value));
}
void applyIconStroke(Style& style, const StyleValue& value) {
    const StyleIconStroke& stroke = std::get<StyleIconStroke>(value);
    style.svgStrokeWidth = Length{stroke.width};
    style.iconStrokeColor = stroke.color;
    style.iconStrokeColorLightDark = stroke.lightDarkColor;
}
void applyIconStrokeLinecap(Style& style, const StyleValue& value) {
    style.svgStrokeCap = std::get<StrokeCap>(value);
    style.svgStrokeCapSet = true;
}
void applyIconStrokeWidth(Style& style, const StyleValue& value) {
    style.svgStrokeWidth = std::get<Length>(value);
}

void applyIconStrokeColor(Style& style, const StyleValue& value) {
    if (const auto lightDarkColor = std::get_if<LightDarkColor>(&value)) {
        style.iconStrokeColor = lightDarkColor->dark;
        style.iconStrokeColorLightDark = *lightDarkColor;
    } else {
        style.iconStrokeColor = std::get<Color>(value);
        style.iconStrokeColorLightDark.reset();
    }
}

const detail::StylePropertyDefinition kPropertyDefinitions[] = {
    {"accent-color", compileAccentColor, applyAccentColor, specifyInherited<InheritedStyleProperty::AccentColor>,
     inheritMember<InheritedStyleProperty::AccentColor, &Style::accentColor>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"appearance", compileAppearance, applyMember<&Style::appearance>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"color-scheme", compileColorScheme, applyMember<&Style::colorScheme>, specifyInherited<InheritedStyleProperty::ColorScheme>,
     inheritMember<InheritedStyleProperty::ColorScheme, &Style::colorScheme>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"box-sizing", compileBoxSizing, applyMember<&Style::boxSizing>, nullptr, nullptr, StylePropertyImpact::Layout},
    {"background-color", compilePaint, applyBackground, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border", compileBorder, applyBorder, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-color", compilePaint,
     [](Style& style, const StyleValue& value) {
         applyPaint(style.borderColor, style.borderColorLightDark, style.borderGradient, value);
     },
     nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-radius", compileBorderRadius, applyMember<&Style::borderRadius>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-style", compileBorderStyle, applyBorderStyle, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-width", compileEdges, applyMember<&Style::borderWidth>, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"bottom", compilePosition, applyLengthToOptional<&Style::bottom>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"cursor", compileCursor, applyMember<&Style::cursor>, specifyInherited<InheritedStyleProperty::Cursor>,
     inheritMember<InheritedStyleProperty::Cursor, &Style::cursor>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"display", compileDisplay, applyDisplay, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"effect", compileEffect, applyMember<&Style::effects>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"height", compileDimension, applyMember<&Style::height>},
    {"left", compilePosition, applyLengthToOptional<&Style::left>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"margin", compileMargin, applyMember<&Style::margin>},
    {"min-height", compileNonnegativeLength, applyLengthToOptional<&Style::minHeight>},
    {"min-size", compileMinSize},
    {"min-width", compileNonnegativeLength, applyLengthToOptional<&Style::minWidth>},
    {"opacity", compileOpacity, applyMember<&Style::opacity>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"outline", compileOutline, applyOutline, nullptr, nullptr, StylePropertyImpact::Paint},
    {"outline-offset", compileOutlineOffset, applyOutlineOffset, nullptr, nullptr, StylePropertyImpact::Paint},
    {"overflow", compileOverflow, nullptr, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-x", compileOverflowAxis, applyMember<&Style::overflowX>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-y", compileOverflowAxis, applyMember<&Style::overflowY>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"padding", compileEdges, applyMember<&Style::padding>},
    {"pointer-events", compilePointerEvents, applyMember<&Style::pointerEvents>, nullptr, nullptr,
     StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"position", compilePositionMode, applyMember<&Style::position>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"right", compilePosition, applyLengthToOptional<&Style::right>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-gutter", compileScrollbarGutter, applyMember<&Style::scrollbarGutter>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-mode", compileScrollbarMode, applyScrollbarMode, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-width", compileScrollbarWidth, applyMember<&Style::scrollbarWidth>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"scrollbar-color", compileScrollbarColor, applyMember<&Style::scrollbarColor>, specifyInherited<InheritedStyleProperty::ScrollbarColor>,
     inheritMember<InheritedStyleProperty::ScrollbarColor, &Style::scrollbarColor>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"box-shadow", compileShadow, applyMember<&Style::shadows>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"size", compileSize, applySize},
    {"top", compilePosition, applyLengthToOptional<&Style::top>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"translate", compileTranslate, applyMember<&Style::translate>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"width", compileDimension, applyMember<&Style::width>},
    {"align-items", compileAlignItems, applyMember<&Style::alignItems>},
    {"flex-direction", compileFlexDirection, applyFlexDirection},
    {"gap", compileGap, applyMember<&Style::gap>},
    {"grid-area", compileGridArea, [](Style& style, const StyleValue& value) { style.gridArea = std::get<GridArea>(value); }, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"justify-content", compileJustifyContent, applyJustifyContent},
    {"justify-self", compileJustifySelf, applyMember<&Style::justifySelf>, nullptr, nullptr,
     StylePropertyImpact::Layout | StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"-internal-align-content-block", compileInternalAlignContentBlock, applyMember<&Style::alignContentBlockCenter>, nullptr, nullptr,
     StylePropertyImpact::Layout, true},
    {"align-self", compileAlignSelf, applyMember<&Style::alignSelf>},
    {"flex", compileFlex},
    {"flex-basis", compileDimension, applyMember<&Style::flexBasis>},
    {"flex-grow", compileUnitlessNonnegativeNumber, applyMember<&Style::flexGrow>},
    {"flex-shrink", compileUnitlessNonnegativeNumber, applyMember<&Style::flexShrink>},
    {"order", compileOrder, applyMember<&Style::order>},
    {"font", compileFont},
    {"font-family", compileFontFamily, applyMember<&Style::fontFamily>, specifyInherited<InheritedStyleProperty::FontFamily>,
     inheritMember<InheritedStyleProperty::FontFamily, &Style::fontFamily>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"font-size", compileNonnegativeNumber, applyMember<&Style::fontSize>, specifyInherited<InheritedStyleProperty::FontSize>,
     inheritMember<InheritedStyleProperty::FontSize, &Style::fontSize>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"font-style", compileFontStyle, applyMember<&Style::fontItalic>, specifyInherited<InheritedStyleProperty::FontStyle>,
     inheritMember<InheritedStyleProperty::FontStyle, &Style::fontItalic>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"text-decoration", compileTextDecoration, applyMember<&Style::textDecoration>, specifyInherited<InheritedStyleProperty::TextDecoration>,
     inheritMember<InheritedStyleProperty::TextDecoration, &Style::textDecoration>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"font-weight", compileFontWeight, applyFontWeight, specifyInherited<InheritedStyleProperty::FontWeight>,
     inheritMember<InheritedStyleProperty::FontWeight, &Style::fontWeight>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"line-height", compileLineHeight, applyMember<&Style::lineHeight>, specifyInherited<InheritedStyleProperty::LineHeight>,
     inheritMember<InheritedStyleProperty::LineHeight, &Style::lineHeight>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"letter-spacing", compileSpacing, applyMember<&Style::letterSpacing>, specifyInherited<InheritedStyleProperty::LetterSpacing>,
     inheritMember<InheritedStyleProperty::LetterSpacing, &Style::letterSpacing>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"word-spacing", compileSpacing, applyMember<&Style::wordSpacing>, specifyInherited<InheritedStyleProperty::WordSpacing>,
     inheritMember<InheritedStyleProperty::WordSpacing, &Style::wordSpacing>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"text-align", compileTextAlign, applyMember<&Style::textAlign>, specifyInherited<InheritedStyleProperty::TextAlign>,
     inheritMember<InheritedStyleProperty::TextAlign, &Style::textAlign>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"color", compileColor, applyColor, specifyInherited<InheritedStyleProperty::Color>, inheritColor,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"text-overflow", compileTextOverflow, applyMember<&Style::textOverflow>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"text-wrap", compileTextWrap, applyMember<&Style::textWrap>, specifyInherited<InheritedStyleProperty::TextWrap>,
     inheritMember<InheritedStyleProperty::TextWrap, &Style::textWrap>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"vertical-align", compileVerticalAlign, applyVerticalAlign},
    {"visibility", compileVisibility, applyMember<&Style::visibility>, specifyInherited<InheritedStyleProperty::Visibility>,
     inheritMember<InheritedStyleProperty::Visibility, &Style::visibility>,
     StylePropertyImpact::Paint | StylePropertyImpact::Inherited | StylePropertyImpact::HitTest},
    {"stroke", compileStroke, applyIconStroke, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-color", compileColorValue, applyIconStrokeColor, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-linecap", compileStrokeLinecap, applyIconStrokeLinecap, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-width", compileStrokeWidth, applyIconStrokeWidth, nullptr, nullptr, StylePropertyImpact::Paint},
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
