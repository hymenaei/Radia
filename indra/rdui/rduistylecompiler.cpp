#include "linden_common.h"
#include "rduistylecompiler.h"
#include "rduicolor.h"
#include "rduistylesheet.h"
#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <iterator>
#include <limits>

namespace rdui
{
    namespace
    {
        using detail::StyleCapability;
        using detail::StylePropagation;
        using detail::StylePropertyDescriptor;
        using detail::StyleValueType;

        constexpr StylePropertyDescriptor PROPERTY_DESCRIPTORS[] = {
            {StyleProperty::BackgroundColor,  "background-color", StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Paint},
            {StyleProperty::Border,           "border",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Border},
            {StyleProperty::BorderColor,      "border-color",     StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Paint},
            {StyleProperty::BorderRadius,     "border-radius",    StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Number},
            {StyleProperty::BorderWidth,      "border-width",     StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Edges},
            {StyleProperty::Bottom,           "bottom",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::Cursor,           "cursor",           StyleCapability::Box,        StylePropagation::Inherited,  InheritedStyleProperty::Cursor,     StyleValueType::Cursor},
            {StyleProperty::Effect,           "effect",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Effects},
            {StyleProperty::Height,           "height",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Dimension},
            {StyleProperty::Left,             "left",             StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::Margin,           "margin",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Margin},
            {StyleProperty::MinHeight,        "min-height",       StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::MinSize,          "min-size",         StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::MinWidth,         "min-width",        StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::Opacity,          "opacity",          StyleCapability::Box,        StylePropagation::Composited, InheritedStyleProperty::NotInherited, StyleValueType::Number},
            {StyleProperty::Outline,          "outline",          StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Outline},
            {StyleProperty::Overflow,         "overflow",         StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Overflow},
            {StyleProperty::Padding,          "padding",          StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Edges},
            {StyleProperty::PointerEvents,    "pointer-events",   StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::PointerEvents},
            {StyleProperty::Right,            "right",            StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::Shadow,           "shadow",           StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Shadows},
            {StyleProperty::Size,             "size",             StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Size},
            {StyleProperty::Top,              "top",              StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
            {StyleProperty::Width,            "width",            StyleCapability::Box,        StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Dimension},
            {StyleProperty::AlignItems,       "align-items",      StyleCapability::Container,  StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::AlignItems},
            {StyleProperty::Flow,             "flow",             StyleCapability::Container,  StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Flow},
            {StyleProperty::Gap,              "gap",              StyleCapability::Container,  StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Gap},
            {StyleProperty::JustifyContent,   "justify-content",  StyleCapability::Container,  StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::JustifyContent},
            {StyleProperty::AlignSelf,        "align-self",       StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::AlignSelf},
            {StyleProperty::Flex,             "flex",             StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Flex},
            {StyleProperty::FlexBasis,        "flex-basis",       StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Dimension},
            {StyleProperty::FlexGrow,         "flex-grow",        StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Number},
            {StyleProperty::FlexShrink,       "flex-shrink",      StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Number},
            {StyleProperty::Order,            "order",            StyleCapability::FlowItem,   StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Integer},
            {StyleProperty::FontFamily,       "font-family",      StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::FontFamily, StyleValueType::FontFamily},
            {StyleProperty::FontSize,         "font-size",        StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::FontSize,   StyleValueType::Number},
            {StyleProperty::FontStyle,        "font-style",       StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::FontStyle,  StyleValueType::Boolean},
            {StyleProperty::FontWeight,       "font-weight",      StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::FontWeight, StyleValueType::Boolean},
            {StyleProperty::LineHeight,       "line-height",      StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::LineHeight, StyleValueType::Length},
            {StyleProperty::TextAlign,        "text-align",       StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::TextAlign,  StyleValueType::TextAlign},
            {StyleProperty::TextColor,        "text-color",       StyleCapability::Typography, StylePropagation::Inherited,  InheritedStyleProperty::TextColor,  StyleValueType::Color},
            {StyleProperty::VerticalAlign,    "vertical-align",   StyleCapability::Container,  StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::VerticalAlign},
            {StyleProperty::IconStroke,       "stroke",           StyleCapability::Icon,       StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::IconStroke},
            {StyleProperty::IconStrokeColor,  "stroke-color",     StyleCapability::Icon,       StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Color},
            {StyleProperty::IconStrokeLinecap,"stroke-linecap",   StyleCapability::Icon,       StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::StrokeCap},
            {StyleProperty::IconStrokeWidth,  "stroke-width",     StyleCapability::Icon,       StylePropagation::Local,      InheritedStyleProperty::NotInherited, StyleValueType::Length},
        };

        std::string trim(const std::string& value)
        {
            std::size_t begin = 0;
            while (begin < value.size() && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
            std::size_t end = value.size();
            while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
            return value.substr(begin, end - begin);
        }

        std::string lower(std::string value)
        {
            std::transform(value.begin(), value.end(), value.begin(), [](unsigned char character)
            {
                return static_cast<char>(std::tolower(character));
            });
            return value;
        }

        bool endsWith(const std::string& value, const std::string& suffix)
        {
            return value.size() >= suffix.size()
                && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
        }

        std::vector<std::string> tokenizeTopLevel(const std::string& value)
        {
            std::vector<std::string> result;
            std::size_t start = std::string::npos;
            int depth = 0;
            for (std::size_t index = 0; index <= value.size(); ++index)
            {
                const bool at_end = index == value.size();
                const char character = at_end ? ' ' : value[index];
                if (!at_end && character == '(') ++depth;
                else if (!at_end && character == ')') --depth;
                const bool separator = at_end || (depth == 0 && std::isspace(static_cast<unsigned char>(character)));
                if (!separator && start == std::string::npos) start = index;
                if (separator && start != std::string::npos)
                {
                    result.push_back(value.substr(start, index - start));
                    start = std::string::npos;
                }
                if (depth < 0) return {};
            }
            return depth == 0 ? result : std::vector<std::string>();
        }

        bool parseStrokeCap(const std::string& raw, StrokeCap& cap)
        {
            const std::string value = lower(trim(raw));
            if (value == "butt") cap = StrokeCap::Butt;
            else if (value == "round") cap = StrokeCap::Round;
            else if (value == "square") cap = StrokeCap::Square;
            else return false;
            return true;
        }
    }

    const detail::StylePropertyDescriptor* detail::findStyleProperty(std::string_view name)
    {
        const auto found = std::find_if(std::begin(PROPERTY_DESCRIPTORS), std::end(PROPERTY_DESCRIPTORS),
            [name](const StylePropertyDescriptor& descriptor) { return descriptor.name == name; });
        return found == std::end(PROPERTY_DESCRIPTORS) ? nullptr : found;
    }

    const detail::StylePropertyDescriptor& detail::styleProperty(StyleProperty property)
    {
        const auto found = std::find_if(std::begin(PROPERTY_DESCRIPTORS), std::end(PROPERTY_DESCRIPTORS),
            [property](const StylePropertyDescriptor& descriptor) { return descriptor.property == property; });
        llassert_always(found != std::end(PROPERTY_DESCRIPTORS));
        return *found;
    }

    void detail::applyStyleDeclaration(Style& style, const StyleDeclaration& declaration)
    {
        switch (declaration.property)
        {
            case StyleProperty::BackgroundColor:
            {
                const StylePaint& paint = std::get<StylePaint>(declaration.value);
                style.background_color = paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
                style.background_gradient = paint.gradient;
                break;
            }
            case StyleProperty::Border:
            {
                const StyleBorder& border = std::get<StyleBorder>(declaration.value);
                style.border_width = {border.width, border.width, border.width, border.width};
                style.border_color = border.paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : border.paint.color;
                style.border_gradient = border.paint.gradient;
                break;
            }
            case StyleProperty::BorderColor:
            {
                const StylePaint& paint = std::get<StylePaint>(declaration.value);
                style.border_color = paint.gradient ? Color(0.f, 0.f, 0.f, 0.f) : paint.color;
                style.border_gradient = paint.gradient;
                break;
            }
            case StyleProperty::BorderRadius: style.border_radius = std::get<float>(declaration.value); break;
            case StyleProperty::BorderWidth: style.border_width = std::get<EdgeInsets>(declaration.value); break;
            case StyleProperty::Bottom: style.bottom = std::get<Length>(declaration.value); break;
            case StyleProperty::Cursor: style.cursor = std::get<CursorStyle>(declaration.value); break;
            case StyleProperty::Effect: style.effects = std::get<std::vector<Effect>>(declaration.value); break;
            case StyleProperty::Height: style.height = std::get<Dimension>(declaration.value); break;
            case StyleProperty::Left: style.left = std::get<Length>(declaration.value); break;
            case StyleProperty::Margin: style.margin = std::get<MarginInsets>(declaration.value); break;
            case StyleProperty::MinHeight: style.min_height = std::get<Length>(declaration.value); break;
            case StyleProperty::MinWidth: style.min_width = std::get<Length>(declaration.value); break;
            case StyleProperty::Opacity: style.opacity = std::get<float>(declaration.value); break;
            case StyleProperty::Outline: style.outline = std::get<Outline>(declaration.value); break;
            case StyleProperty::Overflow: style.overflow = std::get<Overflow>(declaration.value); break;
            case StyleProperty::Padding: style.padding = std::get<EdgeInsets>(declaration.value); break;
            case StyleProperty::PointerEvents: style.pointer_events = std::get<PointerEvents>(declaration.value); break;
            case StyleProperty::Right: style.right = std::get<Length>(declaration.value); break;
            case StyleProperty::Shadow: style.shadows = std::get<std::vector<BoxShadow>>(declaration.value); break;
            case StyleProperty::Size:
            {
                const StyleSize& size = std::get<StyleSize>(declaration.value);
                style.height = size.height;
                style.width = size.width;
                break;
            }
            case StyleProperty::Top: style.top = std::get<Length>(declaration.value); break;
            case StyleProperty::Width: style.width = std::get<Dimension>(declaration.value); break;
            case StyleProperty::AlignItems: style.align_items = std::get<AlignItems>(declaration.value); break;
            case StyleProperty::Flow:
                style.flow = std::get<Flow>(declaration.value);
                style.flow_set = true;
                break;
            case StyleProperty::Gap: style.gap = std::get<GapValue>(declaration.value); break;
            case StyleProperty::JustifyContent:
                style.justify_content = std::get<JustifyContent>(declaration.value);
                style.justify_content_set = true;
                break;
            case StyleProperty::AlignSelf: style.align_self = std::get<AlignSelf>(declaration.value); break;
            case StyleProperty::Flex: break;
            case StyleProperty::FlexBasis: style.flex_basis = std::get<Dimension>(declaration.value); break;
            case StyleProperty::FlexGrow: style.flex_grow = std::get<float>(declaration.value); break;
            case StyleProperty::FlexShrink: style.flex_shrink = std::get<float>(declaration.value); break;
            case StyleProperty::Order: style.order = std::get<int>(declaration.value); break;
            case StyleProperty::FontFamily: style.font_family = std::get<FontFamily>(declaration.value); break;
            case StyleProperty::FontSize: style.font_size = std::get<float>(declaration.value); break;
            case StyleProperty::FontStyle: style.font_italic = std::get<bool>(declaration.value); break;
            case StyleProperty::FontWeight: style.font_bold = std::get<bool>(declaration.value); break;
            case StyleProperty::LineHeight: style.line_height = std::get<Length>(declaration.value); break;
            case StyleProperty::TextAlign: style.text_align = std::get<TextAlign>(declaration.value); break;
            case StyleProperty::TextColor: style.text_color = std::get<Color>(declaration.value); break;
            case StyleProperty::VerticalAlign:
                style.vertical_align = std::get<VerticalAlign>(declaration.value);
                style.vertical_align_set = true;
                break;
            case StyleProperty::IconStroke:
            {
                const StyleIconStroke& stroke = std::get<StyleIconStroke>(declaration.value);
                style.svg_stroke_width = Length{stroke.width};
                style.icon_stroke_color = stroke.color;
                break;
            }
            case StyleProperty::IconStrokeColor: style.icon_stroke_color = std::get<Color>(declaration.value); break;
            case StyleProperty::IconStrokeLinecap:
                style.svg_stroke_cap = std::get<StrokeCap>(declaration.value);
                style.svg_stroke_cap_set = true;
                break;
            case StyleProperty::IconStrokeWidth: style.svg_stroke_width = std::get<Length>(declaration.value); break;
            case StyleProperty::MinSize:
            {
                const Length& minimum = std::get<Length>(declaration.value);
                style.min_height = minimum;
                style.min_width = minimum;
                break;
            }
        }

        const StylePropertyDescriptor& descriptor = styleProperty(declaration.property);
        if (descriptor.propagation == StylePropagation::Inherited)
            markSpecified(style, descriptor.inherited_property);
    }

    std::optional<std::vector<StyleDeclaration>> StyleSheet::Impl::compileDeclaration(
        StyleProperty property,
        const std::string& value,
        const std::string& selector,
        StyleSheetLoadResult& result,
        const std::string& source_name) const
    {
        const detail::StylePropertyDescriptor& descriptor = detail::styleProperty(property);
        auto invalid = [&]() -> std::optional<std::vector<StyleDeclaration>>
        {
            result.error("stylesheet.property.value_invalid",
                         "Invalid value for " + std::string(descriptor.name) + ": " + value + ".", source_name);
            return std::nullopt;
        };
        auto compiled = [property](StyleValue parsed) -> std::optional<std::vector<StyleDeclaration>>
        {
            std::vector<StyleDeclaration> declarations;
            declarations.push_back({property, std::move(parsed)});
            return declarations;
        };
        auto color = [&](const std::string& raw = std::string()) -> std::optional<Color>
        {
            const Color marker(-1.f, -1.f, -1.f, -1.f);
            const Color parsed = parseColorValue(raw.empty() ? value : raw, marker);
            return parsed.a < 0.f ? std::nullopt : std::optional<Color>(parsed);
        };
        auto number = [&](const std::string& raw = std::string()) -> std::optional<float>
        {
            const float parsed = parseNumberValue(raw.empty() ? value : raw, std::numeric_limits<float>::quiet_NaN());
            return std::isfinite(parsed) ? std::optional<float>(parsed) : std::nullopt;
        };
        auto length = [&](const std::string& raw = std::string()) -> std::optional<Length>
        {
            return parseLengthValue(raw.empty() ? value : raw);
        };
        auto nonnegativeLength = [&](const std::string& raw) -> std::optional<Length>
        {
            const std::optional<Length> parsed = length(raw);
            if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return std::nullopt;
            return parsed;
        };

        switch (property)
        {
            case StyleProperty::Shadow:
            {
                const auto parsed = parseShadows(value);
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Effect:
            {
                const auto parsed = parseEffects(value);
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Outline:
            {
                const auto parsed = parseOutline(value);
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::BackgroundColor:
            case StyleProperty::BorderColor:
            {
                if (const std::optional<Gradient> gradient = parseGradient(value))
                    return compiled(StylePaint{Color(), *gradient});
                const auto parsed = color();
                return parsed ? compiled(StylePaint{*parsed, std::nullopt}) : invalid();
            }
            case StyleProperty::TextColor:
            case StyleProperty::IconStrokeColor:
            {
                const auto parsed = color();
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Border:
            case StyleProperty::IconStroke:
            {
                const std::vector<std::string> tokens = tokenizeTopLevel(value);
                if (tokens.size() != 2) return invalid();
                const auto width = number(tokens[0]);
                if (!width || *width < 0.f) return invalid();
                if (property == StyleProperty::Border)
                {
                    if (const std::optional<Gradient> gradient = parseGradient(tokens[1]))
                        return compiled(StyleBorder{*width, StylePaint{Color(), *gradient}});
                    const auto parsed = color(tokens[1]);
                    return parsed ? compiled(StyleBorder{*width, StylePaint{*parsed, std::nullopt}}) : invalid();
                }
                const auto parsed = color(tokens[1]);
                return parsed ? compiled(StyleIconStroke{*width, *parsed}) : invalid();
            }
            case StyleProperty::BorderWidth:
            case StyleProperty::Padding:
            {
                const float nan = std::numeric_limits<float>::quiet_NaN();
                const EdgeInsets parsed = parseEdgeInsets(value, {nan, nan, nan, nan});
                return std::isfinite(parsed.top) ? compiled(parsed) : invalid();
            }
            case StyleProperty::Margin:
            {
                const auto parsed = parseMargin(value);
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Gap:
            {
                if (lower(trim(value)) == "auto") return compiled(GapValue::automatic());
                const auto parsed = number();
                return parsed && *parsed >= 0.f ? compiled(GapValue::fromPixels(*parsed)) : invalid();
            }
            case StyleProperty::Size:
            {
                const std::vector<std::string> tokens = tokenizeTopLevel(value);
                if (tokens.empty() || tokens.size() > 2) return invalid();
                auto dimension = [&](const std::string& raw) -> std::optional<Dimension>
                {
                    if (lower(trim(raw)) == "auto") return Dimension();
                    const auto parsed = nonnegativeLength(raw);
                    return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
                };
                const auto height = dimension(tokens[0]);
                const auto width = dimension(tokens.size() == 1 ? tokens[0] : tokens[1]);
                return height && width ? compiled(StyleSize{*height, *width}) : invalid();
            }
            case StyleProperty::MinSize:
            {
                const std::vector<std::string> tokens = tokenizeTopLevel(value);
                if (tokens.empty() || tokens.size() > 2) return invalid();
                const auto height = nonnegativeLength(tokens[0]);
                const auto width = nonnegativeLength(tokens.size() == 1 ? tokens[0] : tokens[1]);
                if (!height || !width) return invalid();
                return std::vector<StyleDeclaration>{
                    {StyleProperty::MinHeight, *height},
                    {StyleProperty::MinWidth, *width},
                };
            }
            case StyleProperty::IconStrokeLinecap:
            {
                StrokeCap cap;
                return parseStrokeCap(value, cap) ? compiled(cap) : invalid();
            }
            case StyleProperty::FontFamily:
            {
                const std::string family = lower(trim(value));
                return family == "sans" ? compiled(FontFamily::Sans) : invalid();
            }
            case StyleProperty::FontWeight:
            {
                const std::string weight = lower(trim(value));
                const bool bold = weight == "bold" || weight == "600" || weight == "700"
                               || weight == "800" || weight == "900";
                return (bold || weight == "normal" || weight == "400") ? compiled(bold) : invalid();
            }
            case StyleProperty::FontStyle:
            {
                const std::string font_style = lower(trim(value));
                const bool italic = font_style == "italic" || font_style == "oblique";
                return (italic || font_style == "normal") ? compiled(italic) : invalid();
            }
            case StyleProperty::TextAlign:
            {
                const std::string alignment = lower(trim(value));
                std::optional<TextAlign> parsed;
                if (alignment == "left") parsed = TextAlign::Left;
                else if (alignment == "start") parsed = TextAlign::Start;
                else if (alignment == "center") parsed = TextAlign::Center;
                else if (alignment == "right") parsed = TextAlign::Right;
                else if (alignment == "end") parsed = TextAlign::End;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::VerticalAlign:
            {
                const std::string alignment = lower(trim(value));
                std::optional<VerticalAlign> parsed;
                if (alignment == "top") parsed = VerticalAlign::Top;
                else if (alignment == "middle") parsed = VerticalAlign::Middle;
                else if (alignment == "bottom") parsed = VerticalAlign::Bottom;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Flow:
            {
                const std::string flow = lower(trim(value));
                if (flow == "row") return compiled(Flow::Row);
                if (flow == "column") return compiled(Flow::Column);
                if (flow == "free") return compiled(Flow::Free);
                const std::string target = selector.empty() ? std::string("*") : selector;
                if (flow == "grid")
                {
                    result.warning("stylesheet.flow.unsupported",
                                   "flow: grid is not yet implemented (selector \"" + target
                                       + "\"); falling back to flow: free.", source_name);
                    return compiled(Flow::Free);
                }
                result.error("stylesheet.flow.unknown",
                             "Unknown flow value \"" + value + "\" (selector \"" + target
                                 + "\"); expected free, row, column, or grid.", source_name);
                return std::nullopt;
            }
            case StyleProperty::JustifyContent:
            {
                const std::string alignment = lower(trim(value));
                std::optional<JustifyContent> parsed;
                if (alignment == "start") parsed = JustifyContent::Start;
                else if (alignment == "left") parsed = JustifyContent::Left;
                else if (alignment == "center") parsed = JustifyContent::Center;
                else if (alignment == "end") parsed = JustifyContent::End;
                else if (alignment == "right") parsed = JustifyContent::Right;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::AlignItems:
            {
                const std::string alignment = lower(trim(value));
                std::optional<AlignItems> parsed;
                if (alignment == "normal") parsed = AlignItems::Normal;
                else if (alignment == "start") parsed = AlignItems::Start;
                else if (alignment == "center") parsed = AlignItems::Center;
                else if (alignment == "end") parsed = AlignItems::End;
                else if (alignment == "stretch") parsed = AlignItems::Stretch;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::AlignSelf:
            {
                const std::string alignment = lower(trim(value));
                std::optional<AlignSelf> parsed;
                if (alignment == "auto") parsed = AlignSelf::Auto;
                else if (alignment == "start") parsed = AlignSelf::Start;
                else if (alignment == "center") parsed = AlignSelf::Center;
                else if (alignment == "end") parsed = AlignSelf::End;
                else if (alignment == "stretch") parsed = AlignSelf::Stretch;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Flex:
            {
                const std::vector<std::string> tokens = tokenizeTopLevel(value);
                if (tokens.empty() || tokens.size() > 3) return invalid();

                const std::string keyword = lower(trim(value));
                if (keyword == "none")
                    return std::vector<StyleDeclaration>{
                        {StyleProperty::FlexGrow, 0.f},
                        {StyleProperty::FlexShrink, 0.f},
                        {StyleProperty::FlexBasis, Dimension()},
                    };
                if (keyword == "auto")
                    return std::vector<StyleDeclaration>{
                        {StyleProperty::FlexGrow, 1.f},
                        {StyleProperty::FlexShrink, 1.f},
                        {StyleProperty::FlexBasis, Dimension()},
                    };

                auto nonnegativeNumber = [&](const std::string& raw) -> std::optional<float>
                {
                    const std::string token = lower(trim(raw));
                    if (endsWith(token, "px") || endsWith(token, "%")) return std::nullopt;
                    const auto parsed = number(raw);
                    return parsed && *parsed >= 0.f ? parsed : std::nullopt;
                };
                auto basis = [&](const std::string& raw) -> std::optional<Dimension>
                {
                    if (lower(trim(raw)) == "auto") return Dimension();
                    const auto parsed = nonnegativeLength(raw);
                    return parsed ? std::optional<Dimension>(Dimension::fromLength(*parsed)) : std::nullopt;
                };

                float grow = 1.f;
                float shrink = 1.f;
                Dimension flex_basis = Dimension::fromLength(Length{});
                if (tokens.size() == 1)
                {
                    if (const auto parsed = nonnegativeNumber(tokens[0])) grow = *parsed;
                    else if (const auto parsed = basis(tokens[0])) flex_basis = *parsed;
                    else return invalid();
                }
                else if (tokens.size() == 2)
                {
                    const auto parsed_grow = nonnegativeNumber(tokens[0]);
                    if (!parsed_grow) return invalid();
                    grow = *parsed_grow;
                    if (const auto parsed_shrink = nonnegativeNumber(tokens[1])) shrink = *parsed_shrink;
                    else if (const auto parsed_basis = basis(tokens[1])) flex_basis = *parsed_basis;
                    else return invalid();
                }
                else
                {
                    const auto parsed_grow = nonnegativeNumber(tokens[0]);
                    const auto parsed_shrink = nonnegativeNumber(tokens[1]);
                    const auto parsed_basis = basis(tokens[2]);
                    if (!parsed_grow || !parsed_shrink || !parsed_basis) return invalid();
                    grow = *parsed_grow;
                    shrink = *parsed_shrink;
                    flex_basis = *parsed_basis;
                }
                return std::vector<StyleDeclaration>{
                    {StyleProperty::FlexGrow, grow},
                    {StyleProperty::FlexShrink, shrink},
                    {StyleProperty::FlexBasis, flex_basis},
                };
            }
            case StyleProperty::PointerEvents:
            {
                const std::string policy = lower(trim(value));
                std::optional<PointerEvents> parsed;
                if (policy == "auto") parsed = PointerEvents::Auto;
                else if (policy == "none") parsed = PointerEvents::PassThrough;
                else if (policy == "default") parsed = PointerEvents::Default;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Overflow:
            {
                const std::string overflow = lower(trim(value));
                std::optional<Overflow> parsed;
                if (overflow == "visible") parsed = Overflow::Visible;
                else if (overflow == "hidden") parsed = Overflow::Hidden;
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::Order:
            {
                const std::string raw = lower(trim(value));
                const auto parsed = number();
                const double numeric_order = parsed ? static_cast<double>(*parsed) : 0.0;
                if (!parsed || endsWith(raw, "px") || std::trunc(*parsed) != *parsed
                    || numeric_order < static_cast<double>(std::numeric_limits<int>::min())
                    || numeric_order > static_cast<double>(std::numeric_limits<int>::max())) return invalid();
                return compiled(static_cast<int>(*parsed));
            }
            case StyleProperty::Cursor:
            {
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
            case StyleProperty::Width:
            case StyleProperty::Height:
                if (lower(trim(value)) == "auto") return compiled(Dimension());
                [[fallthrough]];
            case StyleProperty::FlexBasis:
                if (lower(trim(value)) == "auto") return compiled(Dimension());
                [[fallthrough]];
            case StyleProperty::MinWidth:
            case StyleProperty::MinHeight:
            {
                const auto parsed = length();
                if (!parsed || parsed->pixels < 0.f || parsed->percent < 0.f) return invalid();
                if (property == StyleProperty::Width || property == StyleProperty::Height
                    || property == StyleProperty::FlexBasis)
                    return compiled(Dimension::fromLength(*parsed));
                return compiled(*parsed);
            }
            case StyleProperty::Left:
            case StyleProperty::Right:
            case StyleProperty::Top:
            case StyleProperty::Bottom:
            {
                const auto parsed = length();
                return parsed ? compiled(*parsed) : invalid();
            }
            case StyleProperty::BorderRadius:
            case StyleProperty::IconStrokeWidth:
            case StyleProperty::FontSize:
            case StyleProperty::LineHeight:
            case StyleProperty::Opacity:
            case StyleProperty::FlexGrow:
            case StyleProperty::FlexShrink:
            {
                const std::string raw = lower(trim(value));
                if ((property == StyleProperty::FlexGrow || property == StyleProperty::FlexShrink)
                    && (endsWith(raw, "px") || endsWith(raw, "%"))) return invalid();
                const auto parsed = number();
                if (!parsed) return invalid();
                const bool nonnegative = property != StyleProperty::Opacity;
                if ((nonnegative && *parsed < 0.f) || (property == StyleProperty::Opacity && (*parsed < 0.f || *parsed > 1.f)))
                    return invalid();
                if (property == StyleProperty::IconStrokeWidth || property == StyleProperty::LineHeight)
                    return compiled(Length{*parsed});
                return compiled(*parsed);
            }
        }
        return invalid();
    }
}
