/**
 * @file compiler.cpp
 * @brief Compiles RSL declarations through the private property registry.
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

namespace rdui {
namespace {
using detail::endsWith;
using detail::lower;
using detail::StylePropertyImpact;
using detail::trim;

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
    llassert_always(property->apply != nullptr);
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
    if (!property.apply) {
        LL_ERRS("rdui") << "Attempted to apply a stylesheet declaration without an applicable property definition." << LL_ENDL;
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

CompileResult compilePaint(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    if (const std::optional<Gradient> gradient = model.parseGradient(value)) return context.compiled(StylePaint{Color(), *gradient});
    const auto parsed = context.color();
    return parsed ? context.compiled(StylePaint{*parsed, std::nullopt}) : context.invalid();
}

CompileResult compileColor(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const auto parsed = context.color();
    return parsed ? context.compiled(*parsed) : context.invalid();
}

CompileResult compileBorder(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() != 2) return context.invalid();
    const auto width = context.number(tokens[0]);
    if (!width || *width < 0.f) return context.invalid();
    if (const std::optional<Gradient> gradient = model.parseGradient(tokens[1]))
        return context.compiled(StyleBorder{*width, StylePaint{Color(), *gradient}});
    const auto parsed = context.color(tokens[1]);
    return parsed ? context.compiled(StyleBorder{*width, StylePaint{*parsed, std::nullopt}}) : context.invalid();
}

CompileResult compileStroke(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::vector<std::string> tokens = context.tokens();
    if (tokens.size() != 2) return context.invalid();
    const auto width = context.number(tokens[0]);
    if (!width || *width < 0.f) return context.invalid();
    const auto parsed = context.color(tokens[1]);
    return parsed ? context.compiled(StyleIconStroke{*width, *parsed}) : context.invalid();
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

CompileResult compileFlow(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    const std::string flow = lower(trim(value));
    if (flow == "row") return context.compiled(Flow::Row);
    if (flow == "column") return context.compiled(Flow::Column);
    if (flow == "free") return context.compiled(Flow::Free);
    const std::string target = selector.empty() ? std::string("*") : selector;
    if (flow == "grid") {
        result.warning("stylesheet.flow.unsupported", "flow: grid is not yet implemented (selector \"" + target + "\"); falling back to flow: free.",
                       sourceName);
        return context.compiled(Flow::Free);
    }
    result.error("stylesheet.flow.unknown",
                 "Unknown flow value \"" + value + "\" (selector \"" + target + "\"); expected free, row, column, or grid.", sourceName);
    return std::nullopt;
}

template<typename Enum, std::size_t Size> CompileResult compileAlignment(const detail::StyleCompileContext& context, const std::string& value,
                                                                         const std::array<std::pair<std::string_view, Enum>, Size>& values) {
    const std::string normalized = lower(trim(value));
    const auto found = std::find_if(values.begin(), values.end(), [&normalized](const auto& entry) { return entry.first == normalized; });
    return found == values.end() ? context.invalid() : context.compiled(found->second);
}

CompileResult compileJustifyContent(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, JustifyContent>, 5> values{{
        {"start", JustifyContent::Start},
        {"left", JustifyContent::Left},
        {"center", JustifyContent::Center},
        {"end", JustifyContent::End},
        {"right", JustifyContent::Right},
    }};
    return compileAlignment(context, value, values);
}

CompileResult compileAlignItems(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignItems>, 5> values{{
        {"normal", AlignItems::Normal},
        {"start", AlignItems::Start},
        {"center", AlignItems::Center},
        {"end", AlignItems::End},
        {"stretch", AlignItems::Stretch},
    }};
    return compileAlignment(context, value, values);
}

CompileResult compileAlignSelf(detail::StyleCompileContext& context) {
    auto& [model, property, value, selector, result, sourceName] = context;
    static constexpr std::array<std::pair<std::string_view, AlignSelf>, 5> values{{
        {"auto", AlignSelf::Auto},
        {"start", AlignSelf::Start},
        {"center", AlignSelf::Center},
        {"end", AlignSelf::End},
        {"stretch", AlignSelf::Stretch},
    }};
    return compileAlignment(context, value, values);
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
    static constexpr std::array<std::pair<std::string_view, PointerEvents>, 3> values{{
        {"auto", PointerEvents::Auto},
        {"none", PointerEvents::PassThrough},
        {"default", PointerEvents::Default},
    }};
    return compileAlignment(context, value, values);
}

std::optional<Overflow> parseOverflow(const std::string& raw) {
    const std::string token = lower(trim(raw));
    if (token == "visible") return Overflow::Visible;
    if (token == "hidden") return Overflow::Hidden;
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
    static constexpr std::array<std::pair<std::string_view, CursorStyle>, 35> values{{
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
    return compileAlignment(context, value, values);
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

template<InheritedStyleProperty Property> void specifyInherited(Style& style) {
    style.specifiedInheritedProperties |= static_cast<InheritedStyleProperties>(Property);
}

void applyPaint(Color& color, std::optional<Gradient>& gradient, const StyleValue& value) {
    const StylePaint& paint = std::get<StylePaint>(value);
    color = paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
    gradient = paint.gradient;
}

void applyBackground(Style& style, const StyleValue& value) {
    applyPaint(style.backgroundColor, style.backgroundGradient, value);
}
void applyBorder(Style& style, const StyleValue& value) {
    const StyleBorder& border = std::get<StyleBorder>(value);
    style.borderWidth = {border.width, border.width, border.width, border.width};
    style.borderColor = border.paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : border.paint.color;
    style.borderGradient = border.paint.gradient;
}
void applySize(Style& style, const StyleValue& value) {
    const StyleSize& size = std::get<StyleSize>(value);
    style.height = size.height;
    style.width = size.width;
}
void applyFlow(Style& style, const StyleValue& value) {
    style.flow = std::get<Flow>(value);
    style.flowSet = true;
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
}
void applyIconStrokeLinecap(Style& style, const StyleValue& value) {
    style.svgStrokeCap = std::get<StrokeCap>(value);
    style.svgStrokeCapSet = true;
}
void applyIconStrokeWidth(Style& style, const StyleValue& value) {
    style.svgStrokeWidth = std::get<Length>(value);
}

const detail::StylePropertyDefinition PROPERTY_DEFINITIONS[] = {
    {"background-color", compilePaint, applyBackground, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border", compileBorder, applyBorder, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"border-color", compilePaint, [](Style& style, const StyleValue& value) { applyPaint(style.borderColor, style.borderGradient, value); }, nullptr,
     nullptr, StylePropertyImpact::Paint},
    {"border-radius", compileNonnegativeNumber, applyMember<&Style::borderRadius>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"border-width", compileEdges, applyMember<&Style::borderWidth>, nullptr, nullptr, StylePropertyImpact::Layout | StylePropertyImpact::Paint},
    {"bottom", compilePosition, applyLengthToOptional<&Style::bottom>},
    {"cursor", compileCursor, applyMember<&Style::cursor>, specifyInherited<InheritedStyleProperty::Cursor>,
     inheritMember<InheritedStyleProperty::Cursor, &Style::cursor>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"effect", compileEffect, applyMember<&Style::effects>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"height", compileDimension, applyMember<&Style::height>},
    {"left", compilePosition, applyLengthToOptional<&Style::left>},
    {"margin", compileMargin, applyMember<&Style::margin>},
    {"min-height", compileNonnegativeLength, applyLengthToOptional<&Style::minHeight>},
    {"min-size", compileMinSize},
    {"min-width", compileNonnegativeLength, applyLengthToOptional<&Style::minWidth>},
    {"opacity", compileOpacity, applyMember<&Style::opacity>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"outline", compileOutline, applyMember<&Style::outline>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"overflow", compileOverflow, nullptr, nullptr, nullptr, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-x", compileOverflowAxis, applyMember<&Style::overflowX>, nullptr, nullptr, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"overflow-y", compileOverflowAxis, applyMember<&Style::overflowY>, nullptr, nullptr, StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"padding", compileEdges, applyMember<&Style::padding>},
    {"pointer-events", compilePointerEvents, applyMember<&Style::pointerEvents>, nullptr, nullptr,
     StylePropertyImpact::Paint | StylePropertyImpact::HitTest},
    {"right", compilePosition, applyLengthToOptional<&Style::right>},
    {"shadow", compileShadow, applyMember<&Style::shadows>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"size", compileSize, applySize},
    {"top", compilePosition, applyLengthToOptional<&Style::top>},
    {"width", compileDimension, applyMember<&Style::width>},
    {"align-items", compileAlignItems, applyMember<&Style::alignItems>},
    {"flow", compileFlow, applyFlow},
    {"gap", compileGap, applyMember<&Style::gap>},
    {"justify-content", compileJustifyContent, applyJustifyContent},
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
    {"text-color", compileColor, applyMember<&Style::textColor>, specifyInherited<InheritedStyleProperty::TextColor>,
     inheritMember<InheritedStyleProperty::TextColor, &Style::textColor>, StylePropertyImpact::Paint | StylePropertyImpact::Inherited},
    {"text-overflow", compileTextOverflow, applyMember<&Style::textOverflow>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"text-wrap", compileTextWrap, applyMember<&Style::textWrap>, specifyInherited<InheritedStyleProperty::TextWrap>,
     inheritMember<InheritedStyleProperty::TextWrap, &Style::textWrap>, StylePropertyImpact::Layout | StylePropertyImpact::Inherited},
    {"vertical-align", compileVerticalAlign, applyVerticalAlign},
    {"stroke", compileStroke, applyIconStroke, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-color", compileColor, applyMember<&Style::iconStrokeColor>, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-linecap", compileStrokeLinecap, applyIconStrokeLinecap, nullptr, nullptr, StylePropertyImpact::Paint},
    {"stroke-width", compileStrokeWidth, applyIconStrokeWidth, nullptr, nullptr, StylePropertyImpact::Paint},
};
} // namespace

namespace detail {
const StylePropertyDefinition* findStyleProperty(std::string_view name) {
    const auto found = std::find_if(std::begin(PROPERTY_DEFINITIONS), std::end(PROPERTY_DEFINITIONS),
                                    [name](const StylePropertyDefinition& property) { return property.name == name; });
    return found == std::end(PROPERTY_DEFINITIONS) ? nullptr : found;
}

const StylePropertyDefinition* stylePropertyBegin() {
    return std::begin(PROPERTY_DEFINITIONS);
}
const StylePropertyDefinition* stylePropertyEnd() {
    return std::end(PROPERTY_DEFINITIONS);
}
} // namespace detail
} // namespace rdui
