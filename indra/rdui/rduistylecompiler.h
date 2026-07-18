#ifndef LL_RDUI_STYLE_COMPILER_H
#define LL_RDUI_STYLE_COMPILER_H

#include "rduistylesheet.h"
#include <map>
#include <string_view>
#include <variant>

namespace rdui
{
    enum class StyleProperty : uint8_t
    {
        BackgroundColor,
        Border,
        BorderColor,
        BorderRadius,
        BorderWidth,
        Bottom,
        Cursor,
        Effect,
        Height,
        Left,
        Margin,
        MinHeight,
        MinSize,
        MinWidth,
        Opacity,
        Outline,
        Overflow,
        Padding,
        PointerEvents,
        Right,
        Shadow,
        Size,
        Top,
        Width,
        AlignItems,
        Flow,
        Gap,
        JustifyContent,
        AlignSelf,
        Grow,
        Order,
        FontFamily,
        FontSize,
        FontStyle,
        FontWeight,
        LineHeight,
        TextAlign,
        TextColor,
        VerticalAlign,
        IconStroke,
        IconStrokeColor,
        IconStrokeLinecap,
        IconStrokeWidth,
    };

    struct StylePaint
    {
        Color color;
        std::optional<Gradient> gradient;
    };

    struct StyleBorder
    {
        float width = 0.f;
        StylePaint paint;
    };

    struct StyleSize
    {
        Dimension height;
        Dimension width;
    };

    struct StyleIconStroke
    {
        float width = 0.f;
        Color color;
    };

    using StyleValue = std::variant<
        Color,
        StylePaint,
        StyleBorder,
        StyleSize,
        StyleIconStroke,
        EdgeInsets,
        MarginInsets,
        Dimension,
        Length,
        GapValue,
        std::vector<BoxShadow>,
        std::vector<Effect>,
        Outline,
        float,
        int,
        bool,
        FontFamily,
        TextAlign,
        VerticalAlign,
        Flow,
        JustifyContent,
        AlignItems,
        AlignSelf,
        Overflow,
        PointerEvents,
        CursorStyle,
        StrokeCap>;

    struct StyleDeclaration
    {
        StyleProperty property;
        StyleValue value;
    };

    void markSpecified(Style& style, InheritedStyleProperty property);

    enum class SelectorCombinator
    {
        Descendant,
        Child,
    };

    struct StyleSelector
    {
        bool universal = false;
        std::string element;
        std::string id;
        std::string class_name;
        std::string state;
        std::string part_state;
        std::vector<std::string> parts;
    };

    struct StyleRule
    {
        std::vector<StyleSelector> selectors;
        std::vector<SelectorCombinator> combinators;
        std::vector<StyleDeclaration> declarations;
        int source_order = 0;
    };

    struct StyleSheet::Impl
    {
        void setColorToken(const std::string& name, const Color& color);
        void setNumberToken(const std::string& name, float value);
        void addRule(const StyleRule& rule);

        Color colorToken(const std::string& name, const Color& fallback) const;
        float numberToken(const std::string& name, float fallback) const;
        Color parseColorValue(const std::string& value, const Color& fallback) const;
        float parseNumberValue(const std::string& value, float fallback) const;
        std::optional<Length> parseLengthValue(const std::string& value) const;
        std::optional<Gradient> parseGradient(const std::string& value) const;
        std::optional<std::vector<BoxShadow>> parseShadows(const std::string& value) const;
        std::optional<std::vector<Effect>> parseEffects(const std::string& value) const;
        std::optional<Outline> parseOutline(const std::string& value) const;
        EdgeInsets parseEdgeInsets(const std::string& value, const EdgeInsets& fallback) const;
        std::optional<MarginInsets> parseMargin(const std::string& value) const;
        std::optional<std::vector<StyleDeclaration>> compileDeclaration(
            StyleProperty property,
            const std::string& value,
            const std::string& selector,
            StyleSheetLoadResult& result,
            const std::string& source_name) const;
        Style resolveInternal(const std::string& element,
                              const std::string& id,
                              const std::set<std::string>& classes,
                              uint8_t owner_states,
                              const std::vector<std::string>& part_path,
                              uint8_t part_states,
                              const Widget* widget = nullptr) const;
        void parseBlock(const std::string& selector,
                        const std::string& body,
                        const StyleRule& parent,
                        StyleSheetLoadResult& result,
                        const std::string& source_name);

        std::map<std::string, Color> color_tokens;
        std::map<std::string, float> number_tokens;
        StyleSheet::DependencyMap dependencies;
        std::vector<StyleRule> rules;
        std::uint64_t generation = 0;
    };
}

namespace rdui::detail
{
    enum class StyleCapability : uint8_t
    {
        Box,
        Container,
        FlowItem,
        Typography,
        Text,
        Icon,
    };

    enum class StylePropagation : uint8_t
    {
        Local,
        Inherited,
        Composited,
    };

    enum class StyleValueType : uint8_t
    {
        Color,
        Paint,
        Border,
        Size,
        IconStroke,
        Edges,
        Margin,
        Dimension,
        Length,
        Gap,
        Shadows,
        Effects,
        Outline,
        Number,
        Integer,
        Boolean,
        FontFamily,
        TextAlign,
        VerticalAlign,
        Flow,
        JustifyContent,
        AlignItems,
        AlignSelf,
        Overflow,
        PointerEvents,
        Cursor,
        StrokeCap,
    };

    struct StylePropertyDescriptor
    {
        StyleProperty property;
        std::string_view name;
        StyleCapability capability;
        StylePropagation propagation;
        InheritedStyleProperty inherited_property;
        StyleValueType value_type;
    };

    const StylePropertyDescriptor* findStyleProperty(std::string_view name);
    const StylePropertyDescriptor& styleProperty(StyleProperty property);
    void applyStyleDeclaration(Style& style, const StyleDeclaration& declaration);

    std::vector<std::string> splitPartPath(const std::string& part);
    StyleRule parseSelector(const std::string& selector);
}

#endif // LL_RDUI_STYLE_COMPILER_H
