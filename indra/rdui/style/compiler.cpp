/**
 * @file compiler.cpp
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
#include "style/compiler.h"
#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <iterator>
#include <limits>
#include "style/color.h"
#include "style/stylesheet.h"

namespace rdui {
namespace {
using detail::StyleCapability;
using detail::StylePropagation;
using detail::StylePropertyDescriptor;
using detail::StyleValueType;

using Property = StyleProperty;
using Capability = StyleCapability;
using Propagation = StylePropagation;
using Inherited = InheritedStyleProperty;
using ValueType = StyleValueType;

constexpr StylePropertyDescriptor PROPERTY_DESCRIPTORS[] = {
    {Property::BackgroundColor, "background-color", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Paint},
    {Property::Border, "border", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Border},
    {Property::BorderColor, "border-color", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Paint},
    {Property::BorderRadius, "border-radius", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Number},
    {Property::BorderWidth, "border-width", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Edges},
    {Property::Bottom, "bottom", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::Cursor, "cursor", Capability::Box, Propagation::Inherited, Inherited::Cursor, ValueType::Cursor},
    {Property::Effect, "effect", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Effects},
    {Property::Height, "height", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Dimension},
    {Property::Left, "left", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::Margin, "margin", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Margin},
    {Property::MinHeight, "min-height", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::MinSize, "min-size", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::MinWidth, "min-width", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::Opacity, "opacity", Capability::Box, Propagation::Composited, Inherited::NotInherited, ValueType::Number},
    {Property::Outline, "outline", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Outline},
    {Property::Overflow, "overflow", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Overflow},
    {Property::OverflowX, "overflow-x", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Overflow},
    {Property::OverflowY, "overflow-y", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Overflow},
    {Property::Padding, "padding", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Edges},
    {Property::PointerEvents, "pointer-events", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::PointerEvents},
    {Property::Right, "right", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::Shadow, "shadow", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Shadows},
    {Property::Size, "size", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Size},
    {Property::Top, "top", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Length},
    {Property::Width, "width", Capability::Box, Propagation::Local, Inherited::NotInherited, ValueType::Dimension},
    {Property::AlignItems, "align-items", Capability::Container, Propagation::Local, Inherited::NotInherited, ValueType::AlignItems},
    {Property::Flow, "flow", Capability::Container, Propagation::Local, Inherited::NotInherited, ValueType::Flow},
    {Property::Gap, "gap", Capability::Container, Propagation::Local, Inherited::NotInherited, ValueType::Gap},
    {Property::JustifyContent, "justify-content", Capability::Container, Propagation::Local, Inherited::NotInherited, ValueType::JustifyContent},
    {Property::AlignSelf, "align-self", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::AlignSelf},
    {Property::Flex, "flex", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::Flex},
    {Property::FlexBasis, "flex-basis", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::Dimension},
    {Property::FlexGrow, "flex-grow", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::Number},
    {Property::FlexShrink, "flex-shrink", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::Number},
    {Property::Order, "order", Capability::FlowItem, Propagation::Local, Inherited::NotInherited, ValueType::Integer},
    {Property::Font, "font", Capability::Typography, Propagation::Local, Inherited::NotInherited, ValueType::FontFamily},
    {Property::FontFamily, "font-family", Capability::Typography, Propagation::Inherited, Inherited::FontFamily, ValueType::FontFamily},
    {Property::FontSize, "font-size", Capability::Typography, Propagation::Inherited, Inherited::FontSize, ValueType::Number},
    {Property::FontStyle, "font-style", Capability::Typography, Propagation::Inherited, Inherited::FontStyle, ValueType::Boolean},
    {Property::FontWeight, "font-weight", Capability::Typography, Propagation::Inherited, Inherited::FontWeight, ValueType::Number},
    {Property::LineHeight, "line-height", Capability::Typography, Propagation::Inherited, Inherited::LineHeight, ValueType::Length},
    {Property::LetterSpacing, "letter-spacing", Capability::Typography, Propagation::Inherited, Inherited::LetterSpacing, ValueType::Length},
    {Property::WordSpacing, "word-spacing", Capability::Typography, Propagation::Inherited, Inherited::WordSpacing, ValueType::Length},
    {Property::TextAlign, "text-align", Capability::Typography, Propagation::Inherited, Inherited::TextAlign, ValueType::TextAlign},
    {Property::TextColor, "text-color", Capability::Typography, Propagation::Inherited, Inherited::TextColor, ValueType::Color},
    {Property::TextOverflow, "text-overflow", Capability::Typography, Propagation::Local, Inherited::NotInherited, ValueType::TextOverflow},
    {Property::TextWrap, "text-wrap", Capability::Typography, Propagation::Inherited, Inherited::TextWrap, ValueType::TextWrap},
    {Property::VerticalAlign, "vertical-align", Capability::Container, Propagation::Local, Inherited::NotInherited, ValueType::VerticalAlign},
    {Property::IconStroke, "stroke", Capability::Icon, Propagation::Local, Inherited::NotInherited, ValueType::IconStroke},
    {Property::IconStrokeColor, "stroke-color", Capability::Icon, Propagation::Local, Inherited::NotInherited, ValueType::Color},
    {Property::IconStrokeLinecap, "stroke-linecap", Capability::Icon, Propagation::Local, Inherited::NotInherited, ValueType::StrokeCap},
    {Property::IconStrokeWidth, "stroke-width", Capability::Icon, Propagation::Local, Inherited::NotInherited, ValueType::Length},
};

std::string trim(const std::string& value) {
    std::size_t begin = 0;
    while (begin < value.size() && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    std::size_t end = value.size();
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return value.substr(begin, end - begin);
}

std::string lower(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
    return value;
}

bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

std::vector<std::string> tokenizeTopLevel(const std::string& value) {
    std::vector<std::string> result;
    std::size_t start = std::string::npos;
    int depth = 0;
    const auto finish = [&](std::size_t end) {
        if (start == std::string::npos) return;
        result.push_back(value.substr(start, end - start));
        start = std::string::npos;
    };
    for (std::size_t index = 0; index <= value.size(); ++index) {
        const bool at_end = index == value.size();
        const char character = at_end ? ' ' : value[index];
        if (!at_end && character == '(') ++depth;
        else if (!at_end && character == ')') --depth;
        if (depth < 0) return {};
        const bool punctuation = !at_end && depth == 0 && character == '/';
        const bool separator = at_end || (depth == 0 && std::isspace(static_cast<unsigned char>(character)));
        if (separator || punctuation) {
            finish(index);
            if (punctuation) result.emplace_back("/");
        } else if (start == std::string::npos) start = index;
    }
    return depth == 0 ? result : std::vector<std::string>();
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

const detail::StylePropertyDescriptor* detail::findStyleProperty(std::string_view name) {
    const auto found = std::find_if(std::begin(PROPERTY_DESCRIPTORS), std::end(PROPERTY_DESCRIPTORS),
                                    [name](const StylePropertyDescriptor& descriptor) { return descriptor.name == name; });
    return found == std::end(PROPERTY_DESCRIPTORS) ? nullptr : found;
}

const detail::StylePropertyDescriptor& detail::styleProperty(Property property) {
    const auto found = std::find_if(std::begin(PROPERTY_DESCRIPTORS), std::end(PROPERTY_DESCRIPTORS),
                                    [property](const StylePropertyDescriptor& descriptor) { return descriptor.property == property; });
    llassert_always(found != std::end(PROPERTY_DESCRIPTORS));
    return *found;
}

void detail::applyStyleDeclaration(Style& style, const StyleDeclaration& declaration) {
    switch (declaration.property) {
        case Property::BackgroundColor: {
            const StylePaint& paint = std::get<StylePaint>(declaration.value);
            style.background_color = paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
            style.background_gradient = paint.gradient;
            break;
        }
        case Property::Border: {
            const StyleBorder& border = std::get<StyleBorder>(declaration.value);
            style.border_width = {border.width, border.width, border.width, border.width};
            style.border_color = border.paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : border.paint.color;
            style.border_gradient = border.paint.gradient;
            break;
        }
        case Property::BorderColor: {
            const StylePaint& paint = std::get<StylePaint>(declaration.value);
            style.border_color = paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
            style.border_gradient = paint.gradient;
            break;
        }
        case Property::BorderRadius: style.border_radius = std::get<float>(declaration.value); break;
        case Property::BorderWidth: style.border_width = std::get<EdgeInsets>(declaration.value); break;
        case Property::Bottom: style.bottom = std::get<Length>(declaration.value); break;
        case Property::Cursor: style.cursor = std::get<CursorStyle>(declaration.value); break;
        case Property::Effect: style.effects = std::get<std::vector<Effect>>(declaration.value); break;
        case Property::Height: style.height = std::get<Dimension>(declaration.value); break;
        case Property::Left: style.left = std::get<Length>(declaration.value); break;
        case Property::Margin: style.margin = std::get<MarginInsets>(declaration.value); break;
        case Property::MinHeight: style.min_height = std::get<Length>(declaration.value); break;
        case Property::MinWidth: style.min_width = std::get<Length>(declaration.value); break;
        case Property::Opacity: style.opacity = std::get<float>(declaration.value); break;
        case Property::Outline: style.outline = std::get<Outline>(declaration.value); break;
        case Property::Overflow: break;
        case Property::OverflowX: style.overflow_x = std::get<Overflow>(declaration.value); break;
        case Property::OverflowY: style.overflow_y = std::get<Overflow>(declaration.value); break;
        case Property::Padding: style.padding = std::get<EdgeInsets>(declaration.value); break;
        case Property::PointerEvents: style.pointer_events = std::get<PointerEvents>(declaration.value); break;
        case Property::Right: style.right = std::get<Length>(declaration.value); break;
        case Property::Shadow: style.shadows = std::get<std::vector<BoxShadow>>(declaration.value); break;
        case Property::Size: {
            const StyleSize& size = std::get<StyleSize>(declaration.value);
            style.height = size.height;
            style.width = size.width;
            break;
        }
        case Property::Top: style.top = std::get<Length>(declaration.value); break;
        case Property::Width: style.width = std::get<Dimension>(declaration.value); break;
        case Property::AlignItems: style.align_items = std::get<AlignItems>(declaration.value); break;
        case Property::Flow:
            style.flow = std::get<Flow>(declaration.value);
            style.flow_set = true;
            break;
        case Property::Gap: style.gap = std::get<GapValue>(declaration.value); break;
        case Property::JustifyContent:
            style.justify_content = std::get<JustifyContent>(declaration.value);
            style.justify_content_set = true;
            break;
        case Property::AlignSelf: style.align_self = std::get<AlignSelf>(declaration.value); break;
        case Property::Flex: break;
        case Property::FlexBasis: style.flex_basis = std::get<Dimension>(declaration.value); break;
        case Property::FlexGrow: style.flex_grow = std::get<float>(declaration.value); break;
        case Property::FlexShrink: style.flex_shrink = std::get<float>(declaration.value); break;
        case Property::Order: style.order = std::get<int>(declaration.value); break;
        case Property::Font: break;
        case Property::FontFamily: style.font_family = std::get<FontFamily>(declaration.value); break;
        case Property::FontSize: style.font_size = std::get<float>(declaration.value); break;
        case Property::FontStyle: style.font_italic = std::get<bool>(declaration.value); break;
        case Property::FontWeight: style.font_weight = static_cast<U16>(std::get<float>(declaration.value)); break;
        case Property::LineHeight: style.line_height = std::get<std::optional<Length>>(declaration.value); break;
        case Property::LetterSpacing: style.letter_spacing = std::get<Length>(declaration.value); break;
        case Property::WordSpacing: style.word_spacing = std::get<Length>(declaration.value); break;
        case Property::TextAlign: style.text_align = std::get<TextAlign>(declaration.value); break;
        case Property::TextColor: style.text_color = std::get<Color>(declaration.value); break;
        case Property::TextOverflow: style.text_overflow = std::get<TextOverflow>(declaration.value); break;
        case Property::TextWrap: style.text_wrap = std::get<TextWrap>(declaration.value); break;
        case Property::VerticalAlign:
            style.vertical_align = std::get<VerticalAlign>(declaration.value);
            style.vertical_align_set = true;
            break;
        case Property::IconStroke: {
            const StyleIconStroke& stroke = std::get<StyleIconStroke>(declaration.value);
            style.svg_stroke_width = Length{stroke.width};
            style.icon_stroke_color = stroke.color;
            break;
        }
        case Property::IconStrokeColor: style.icon_stroke_color = std::get<Color>(declaration.value); break;
        case Property::IconStrokeLinecap:
            style.svg_stroke_cap = std::get<StrokeCap>(declaration.value);
            style.svg_stroke_cap_set = true;
            break;
        case Property::IconStrokeWidth: style.svg_stroke_width = std::get<Length>(declaration.value); break;
        case Property::MinSize: {
            const Length& minimum = std::get<Length>(declaration.value);
            style.min_height = minimum;
            style.min_width = minimum;
            break;
        }
    }

    const StylePropertyDescriptor& descriptor = styleProperty(declaration.property);
    if (descriptor.propagation == Propagation::Inherited) markSpecified(style, descriptor.inherited_property);
}

std::optional<bool> StyleSheet::Impl::parseFontStyleValue(const std::string& value) const {
    const std::string style = lower(trim(value));
    if (style == "normal") return false;
    if (style == "italic" || style == "oblique") return true;
    return std::nullopt;
}

std::optional<float> StyleSheet::Impl::parseFontWeightValue(const std::string& value) const {
    const std::string weight = lower(trim(value));
    if (weight == "normal") return 400.f;
    if (weight == "bold") return 700.f;
    if (endsWith(weight, "px") || endsWith(weight, "%")) return std::nullopt;
    const float parsed = parseNumberValue(weight, std::numeric_limits<float>::quiet_NaN());
    return std::isfinite(parsed) && parsed >= 1.f && parsed <= 1000.f && std::floor(parsed) == parsed ? std::optional<float>(parsed) : std::nullopt;
}

std::optional<Length> StyleSheet::Impl::parseLineHeightValue(const std::string& value) const {
    const std::optional<Length> parsed = parseLengthValue(value);
    return parsed && parsed->pixels >= 0.f && parsed->percent == 0.f ? parsed : std::nullopt;
}

std::optional<std::vector<StyleDeclaration>> StyleSheet::Impl::parseFontShorthand(const std::string& value) const {
    std::vector<std::string> tokens = tokenizeTopLevel(value);
    if (tokens.size() < 2 || lower(tokens.back()) != "sans") return std::nullopt;
    tokens.pop_back();

    std::optional<Length> line_height;
    std::size_t size_index = tokens.size() - 1;
    const auto slash = std::find(tokens.begin(), tokens.end(), "/");
    if (slash != tokens.end()) {
        const std::size_t slash_index = static_cast<std::size_t>(slash - tokens.begin());
        if (slash_index == 0 || slash_index + 2 != tokens.size() || std::find(slash + 1, tokens.end(), "/") != tokens.end()) return std::nullopt;
        size_index = slash_index - 1;
        line_height = parseLineHeightValue(tokens.back());
        if (!line_height) return std::nullopt;
    }

    const std::string size = lower(tokens[size_index]);
    const float parsed_size = parseNumberValue(size, std::numeric_limits<float>::quiet_NaN());
    if (!std::isfinite(parsed_size) || parsed_size < 0.f || endsWith(size, "%")) return std::nullopt;

    bool italic = false;
    float weight = 400.f;
    bool saw_style = false;
    bool saw_weight = false;
    for (std::size_t index = 0; index < size_index; ++index) {
        const std::string token = lower(tokens[index]);
        if (token == "normal") {
            if (!saw_style) saw_style = true;
            else if (!saw_weight) saw_weight = true;
            else return std::nullopt;
            continue;
        }
        if (!saw_style) {
            const std::optional<bool> parsed_style = parseFontStyleValue(token);
            if (parsed_style) {
                italic = *parsed_style;
                saw_style = true;
                continue;
            }
        }
        if (!saw_weight) {
            const std::optional<float> parsed_weight = parseFontWeightValue(token);
            if (parsed_weight) {
                weight = *parsed_weight;
                saw_weight = true;
                continue;
            }
        }
        return std::nullopt;
    }

    return std::vector<StyleDeclaration>{
        {Property::FontStyle, italic},       {Property::FontWeight, weight},           {Property::FontSize, parsed_size},
        {Property::LineHeight, line_height}, {Property::FontFamily, FontFamily::Sans},
    };
}

std::optional<std::vector<StyleDeclaration>> StyleSheet::Impl::compileDeclaration(Property property, const std::string& value,
                                                                                  const std::string& selector, StyleSheetLoadResult& result,
                                                                                  const std::string& source_name) const {
    const detail::StylePropertyDescriptor& descriptor = detail::styleProperty(property);
    auto invalid = [&]() -> std::optional<std::vector<StyleDeclaration>> {
        result.error("stylesheet.property.value_invalid", "Invalid value for " + std::string(descriptor.name) + ": " + value + ".", source_name);
        return std::nullopt;
    };
    auto compiled = [property](StyleValue parsed) -> std::optional<std::vector<StyleDeclaration>> {
        std::vector<StyleDeclaration> declarations;
        declarations.push_back({property, std::move(parsed)});
        return declarations;
    };
    auto color = [&](const std::string& raw = std::string()) -> std::optional<Color> {
        const Color marker(-1.f, -1.f, -1.f, -1.f);
        const Color parsed = parseColorValue(raw.empty() ? value : raw, marker);
        return parsed.a < 0.f ? std::nullopt : std::optional<Color>(parsed);
    };
    auto number = [&](const std::string& raw = std::string()) -> std::optional<float> {
        const float parsed = parseNumberValue(raw.empty() ? value : raw, std::numeric_limits<float>::quiet_NaN());
        return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
    };
    auto length = [&](const std::string& raw = std::string()) -> std::optional<Length> { return parseLengthValue(raw.empty() ? value : raw); };
    auto nonnegativeLength = [&](const std::string& raw) -> std::optional<Length> {
        const std::optional<Length> parsed = length(raw);
        if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
        return parsed;
    };

    switch (property) {
        case Property::Shadow: {
            const auto parsed = parseShadows(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Effect: {
            const auto parsed = parseEffects(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Outline: {
            const auto parsed = parseOutline(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::BackgroundColor:
        case Property::BorderColor: {
            if (const std::optional<Gradient> gradient = parseGradient(value)) return compiled(StylePaint{Color(), *gradient});
            const auto parsed = color();
            return parsed ? compiled(StylePaint{*parsed, std::nullopt}) : invalid();
        }
        case Property::TextColor:
        case Property::IconStrokeColor: {
            const auto parsed = color();
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Border:
        case Property::IconStroke: {
            const std::vector<std::string> tokens = tokenizeTopLevel(value);
            if (tokens.size() != 2) return invalid();
            const auto width = number(tokens[0]);
            if (!width || *width < 0.f) return invalid();
            if (property == Property::Border) {
                if (const std::optional<Gradient> gradient = parseGradient(tokens[1]))
                    return compiled(StyleBorder{*width, StylePaint{Color(), *gradient}});
                const auto parsed = color(tokens[1]);
                return parsed ? compiled(StyleBorder{*width, StylePaint{*parsed, std::nullopt}}) : invalid();
            }
            const auto parsed = color(tokens[1]);
            return parsed ? compiled(StyleIconStroke{*width, *parsed}) : invalid();
        }
        case Property::BorderWidth:
        case Property::Padding: {
            const float nan = std::numeric_limits<float>::quiet_NaN();
            const EdgeInsets parsed = parseEdgeInsets(value, {nan, nan, nan, nan});
            return std::isfinite(parsed.top) ? compiled(parsed) : invalid();
        }
        case Property::Margin: {
            const auto parsed = parseMargin(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Gap: {
            if (lower(trim(value)) == "auto") return compiled(GapValue::automatic());
            const auto parsed = number();
            return parsed && *parsed >= 0.f ? compiled(GapValue::fromPixels(*parsed)) : invalid();
        }
        case Property::Size: {
            const std::vector<std::string> tokens = tokenizeTopLevel(value);
            if (tokens.empty() || tokens.size() > 2) return invalid();
            auto dimension = [&](const std::string& raw) -> std::optional<Dimension> {
                if (lower(trim(raw)) == "auto") return Dimension();
                const auto parsed = nonnegativeLength(raw);
                return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
            };
            const auto height = dimension(tokens[0]);
            const auto width = dimension(tokens.size() == 1 ? tokens[0] : tokens[1]);
            return height && width ? compiled(StyleSize{*height, *width}) : invalid();
        }
        case Property::MinSize: {
            const std::vector<std::string> tokens = tokenizeTopLevel(value);
            if (tokens.empty() || tokens.size() > 2) return invalid();
            const auto height = nonnegativeLength(tokens[0]);
            const auto width = nonnegativeLength(tokens.size() == 1 ? tokens[0] : tokens[1]);
            if (!height || !width) return invalid();
            return std::vector<StyleDeclaration>{
                {Property::MinHeight, *height},
                {Property::MinWidth, *width},
            };
        }
        case Property::IconStrokeLinecap: {
            StrokeCap cap;
            return parseStrokeCap(value, cap) ? compiled(cap) : invalid();
        }
        case Property::FontFamily: {
            const std::string family = lower(trim(value));
            return family == "sans" ? compiled(FontFamily::Sans) : invalid();
        }
        case Property::Font: {
            const auto parsed = parseFontShorthand(value);
            return parsed ? parsed : invalid();
        }
        case Property::FontWeight: {
            const auto parsed = parseFontWeightValue(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::FontStyle: {
            const auto parsed = parseFontStyleValue(value);
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::TextAlign: {
            const std::string alignment = lower(trim(value));
            std::optional<TextAlign> parsed;
            if (alignment == "left") parsed = TextAlign::Left;
            else if (alignment == "start") parsed = TextAlign::Start;
            else if (alignment == "center") parsed = TextAlign::Center;
            else if (alignment == "right") parsed = TextAlign::Right;
            else if (alignment == "end") parsed = TextAlign::End;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::TextOverflow: {
            const std::string overflow = lower(trim(value));
            if (overflow == "clip") return compiled(TextOverflow::Clip);
            if (overflow == "ellipsis") return compiled(TextOverflow::Ellipsis);
            if (overflow == "ellipsis-center") return compiled(TextOverflow::EllipsisCenter);
            return invalid();
        }
        case Property::TextWrap: {
            const std::string wrap = lower(trim(value));
            if (wrap == "wrap") return compiled(TextWrap::Wrap);
            if (wrap == "nowrap") return compiled(TextWrap::NoWrap);
            return invalid();
        }
        case Property::VerticalAlign: {
            const std::string alignment = lower(trim(value));
            std::optional<VerticalAlign> parsed;
            if (alignment == "top") parsed = VerticalAlign::Top;
            else if (alignment == "middle") parsed = VerticalAlign::Middle;
            else if (alignment == "bottom") parsed = VerticalAlign::Bottom;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Flow: {
            const std::string flow = lower(trim(value));
            if (flow == "row") return compiled(Flow::Row);
            if (flow == "column") return compiled(Flow::Column);
            if (flow == "free") return compiled(Flow::Free);
            const std::string target = selector.empty() ? std::string("*") : selector;
            if (flow == "grid") {
                result.warning("stylesheet.flow.unsupported",
                               "flow: grid is not yet implemented (selector \"" + target + "\"); falling back to flow: free.", source_name);
                return compiled(Flow::Free);
            }
            result.error("stylesheet.flow.unknown",
                         "Unknown flow value \"" + value + "\" (selector \"" + target + "\"); expected free, row, column, or grid.", source_name);
            return std::nullopt;
        }
        case Property::JustifyContent: {
            const std::string alignment = lower(trim(value));
            std::optional<JustifyContent> parsed;
            if (alignment == "start") parsed = JustifyContent::Start;
            else if (alignment == "left") parsed = JustifyContent::Left;
            else if (alignment == "center") parsed = JustifyContent::Center;
            else if (alignment == "end") parsed = JustifyContent::End;
            else if (alignment == "right") parsed = JustifyContent::Right;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::AlignItems: {
            const std::string alignment = lower(trim(value));
            std::optional<AlignItems> parsed;
            if (alignment == "normal") parsed = AlignItems::Normal;
            else if (alignment == "start") parsed = AlignItems::Start;
            else if (alignment == "center") parsed = AlignItems::Center;
            else if (alignment == "end") parsed = AlignItems::End;
            else if (alignment == "stretch") parsed = AlignItems::Stretch;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::AlignSelf: {
            const std::string alignment = lower(trim(value));
            std::optional<AlignSelf> parsed;
            if (alignment == "auto") parsed = AlignSelf::Auto;
            else if (alignment == "start") parsed = AlignSelf::Start;
            else if (alignment == "center") parsed = AlignSelf::Center;
            else if (alignment == "end") parsed = AlignSelf::End;
            else if (alignment == "stretch") parsed = AlignSelf::Stretch;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Flex: {
            const std::vector<std::string> tokens = tokenizeTopLevel(value);
            if (tokens.empty() || tokens.size() > 3) return invalid();

            const std::string keyword = lower(trim(value));
            if (keyword == "none")
                return std::vector<StyleDeclaration>{
                    {Property::FlexGrow, 0.f},
                    {Property::FlexShrink, 0.f},
                    {Property::FlexBasis, Dimension()},
                };
            if (keyword == "auto")
                return std::vector<StyleDeclaration>{
                    {Property::FlexGrow, 1.f},
                    {Property::FlexShrink, 1.f},
                    {Property::FlexBasis, Dimension()},
                };

            auto nonnegativeNumber = [&](const std::string& raw) -> std::optional<float> {
                const std::string token = lower(trim(raw));
                if (endsWith(token, "px") || endsWith(token, "%")) return std::nullopt;
                const auto parsed = number(raw);
                return parsed && *parsed >= 0.f ? parsed : std::nullopt;
            };
            auto basis = [&](const std::string& raw) -> std::optional<Dimension> {
                if (lower(trim(raw)) == "auto") return Dimension();
                const auto parsed = nonnegativeLength(raw);
                return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
            };

            float grow = 1.f;
            float shrink = 1.f;
            Dimension flex_basis = Dimension::fromLength(Length{});
            if (tokens.size() == 1) {
                if (const auto parsed = nonnegativeNumber(tokens[0])) grow = *parsed;
                else if (const auto parsed = basis(tokens[0])) flex_basis = *parsed;
                else return invalid();
            } else if (tokens.size() == 2) {
                const auto parsed_grow = nonnegativeNumber(tokens[0]);
                if (!parsed_grow) return invalid();
                grow = *parsed_grow;
                if (const auto parsed_shrink = nonnegativeNumber(tokens[1])) shrink = *parsed_shrink;
                else if (const auto parsed_basis = basis(tokens[1])) flex_basis = *parsed_basis;
                else return invalid();
            } else {
                const auto parsed_grow = nonnegativeNumber(tokens[0]);
                const auto parsed_shrink = nonnegativeNumber(tokens[1]);
                const auto parsed_basis = basis(tokens[2]);
                if (!parsed_grow || !parsed_shrink || !parsed_basis) return invalid();
                grow = *parsed_grow;
                shrink = *parsed_shrink;
                flex_basis = *parsed_basis;
            }
            return std::vector<StyleDeclaration>{
                {Property::FlexGrow, grow},
                {Property::FlexShrink, shrink},
                {Property::FlexBasis, flex_basis},
            };
        }
        case Property::PointerEvents: {
            const std::string policy = lower(trim(value));
            std::optional<PointerEvents> parsed;
            if (policy == "auto") parsed = PointerEvents::Auto;
            else if (policy == "none") parsed = PointerEvents::PassThrough;
            else if (policy == "default") parsed = PointerEvents::Default;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Overflow: {
            const std::vector<std::string> tokens = tokenizeTopLevel(value);
            if (tokens.empty() || tokens.size() > 2) return invalid();
            auto parse = [](const std::string& raw) -> std::optional<Overflow> {
                const std::string token = lower(trim(raw));
                if (token == "visible") return Overflow::Visible;
                if (token == "hidden") return Overflow::Hidden;
                return std::nullopt;
            };
            const auto horizontal = parse(tokens[0]);
            const auto vertical = parse(tokens.size() == 1 ? tokens[0] : tokens[1]);
            if (!horizontal || !vertical) return invalid();
            return std::vector<StyleDeclaration>{
                {Property::OverflowX, *horizontal},
                {Property::OverflowY, *vertical},
            };
        }
        case Property::OverflowX:
        case Property::OverflowY: {
            const std::string overflow = lower(trim(value));
            std::optional<Overflow> parsed;
            if (overflow == "visible") parsed = Overflow::Visible;
            else if (overflow == "hidden") parsed = Overflow::Hidden;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Order: {
            const std::string raw = lower(trim(value));
            const auto parsed = number();
            const double numeric_order = parsed ? static_cast<double>(*parsed) : 0.0;
            if (!parsed
                || endsWith(raw, "px")
                || std::trunc(*parsed) != *parsed
                || numeric_order < static_cast<double>(std::numeric_limits<int>::min())
                || numeric_order > static_cast<double>(std::numeric_limits<int>::max()))
                return invalid();
            return compiled(static_cast<int>(*parsed));
        }
        case Property::Cursor: {
            const std::string cursor = lower(trim(value));
            std::optional<CursorStyle> parsed;
            if (cursor == "auto") parsed = CursorStyle::Auto;
            else if (cursor == "default") parsed = CursorStyle::Default;
            else if (cursor == "pointer") parsed = CursorStyle::Pointer;
            else if (cursor == "progress") parsed = CursorStyle::Progress;
            else if (cursor == "wait") parsed = CursorStyle::Wait;
            else if (cursor == "crosshair") parsed = CursorStyle::Crosshair;
            else if (cursor == "text") parsed = CursorStyle::Text;
            else if (cursor == "vertical-text") parsed = CursorStyle::VerticalText;
            else if (cursor == "alias") parsed = CursorStyle::Alias;
            else if (cursor == "copy") parsed = CursorStyle::Copy;
            else if (cursor == "move") parsed = CursorStyle::Move;
            else if (cursor == "no-drop") parsed = CursorStyle::NoDrop;
            else if (cursor == "not-allowed") parsed = CursorStyle::NotAllowed;
            else if (cursor == "grab") parsed = CursorStyle::Grab;
            else if (cursor == "grabbing") parsed = CursorStyle::Grabbing;
            else if (cursor == "col-resize") parsed = CursorStyle::ColumnResize;
            else if (cursor == "row-resize") parsed = CursorStyle::RowResize;
            else if (cursor == "ew-resize" || cursor == "e-resize" || cursor == "w-resize") parsed = CursorStyle::EastWestResize;
            else if (cursor == "ns-resize" || cursor == "n-resize" || cursor == "s-resize") parsed = CursorStyle::NorthSouthResize;
            else if (cursor == "nesw-resize" || cursor == "ne-resize" || cursor == "sw-resize") parsed = CursorStyle::NortheastSouthwestResize;
            else if (cursor == "nwse-resize" || cursor == "nw-resize" || cursor == "se-resize") parsed = CursorStyle::NorthwestSoutheastResize;
            else if (cursor == "all-scroll") parsed = CursorStyle::AllScroll;
            else if (cursor == "zoom-in") parsed = CursorStyle::ZoomIn;
            else if (cursor == "zoom-out") parsed = CursorStyle::ZoomOut;
            else if (cursor == "help") parsed = CursorStyle::Help;
            else if (cursor == "context-menu") parsed = CursorStyle::ContextMenu;
            else if (cursor == "cell") parsed = CursorStyle::Cell;
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::Width:
        case Property::Height:
            if (lower(trim(value)) == "auto") return compiled(Dimension());
            [[fallthrough]];
        case Property::FlexBasis:
            if (lower(trim(value)) == "auto") return compiled(Dimension());
            [[fallthrough]];
        case Property::MinWidth:
        case Property::MinHeight: {
            const auto parsed = length();
            if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return invalid();
            if (property == Property::Width || property == Property::Height || property == Property::FlexBasis)
                return compiled(Dimension::fromLength(*parsed));
            return compiled(*parsed);
        }
        case Property::Left:
        case Property::Right:
        case Property::Top:
        case Property::Bottom: {
            const auto parsed = length();
            return parsed ? compiled(*parsed) : invalid();
        }
        case Property::BorderRadius:
        case Property::IconStrokeWidth:
        case Property::FontSize:
        case Property::Opacity:
        case Property::FlexGrow:
        case Property::FlexShrink: {
            const std::string raw = lower(trim(value));
            if ((property == Property::FlexGrow || property == Property::FlexShrink) && (endsWith(raw, "px") || endsWith(raw, "%"))) return invalid();
            const auto parsed = number();
            if (!parsed) return invalid();
            const bool nonnegative = property != Property::Opacity;
            if ((nonnegative && *parsed < 0.f) || (property == Property::Opacity && (*parsed < 0.f || *parsed > 1.f))) return invalid();
            if (property == Property::IconStrokeWidth) return compiled(Length{*parsed});
            return compiled(*parsed);
        }
        case Property::LineHeight: {
            const auto parsed = parseLineHeightValue(value);
            if (!parsed) return invalid();
            return compiled(std::optional<Length>(*parsed));
        }
        case Property::LetterSpacing:
        case Property::WordSpacing: {
            if (lower(trim(value)) == "normal") return compiled(Length{});
            const auto parsed = length();
            return parsed ? compiled(*parsed) : invalid();
        }
    }
    return invalid();
}
} // namespace rdui
