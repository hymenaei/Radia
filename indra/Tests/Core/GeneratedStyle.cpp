/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/CSSPropertyParser.h>
#include <Core/CSSRules.h>
#include <Core/CSSTokenStream.h>
#include <Core/CSSValue.h>
#include <Core/Color.h>
#include <Core/ComputedStyle.h>
#include <Core/LayoutGeometry.h>
#include <Core/StyleProperty.h>
#include <array>
#include <gtest/gtest.h>
#include <optional>
#include <string_view>
#include <utility>
#include <variant>
#include "CSSKeywords.h"
#include "CSSProperties.h"
#include "CSSPropertyParsing.h"
#include "ComputedStyleProperties.h"

namespace CoreTests {
using enum Core::CSS::KeywordName;
using Core::Color;
using Core::CSS::consumeKeyword;
using Core::CSS::consumeLength;
using Core::CSS::consumeLengthPercentage;
using Core::CSS::consumeLiteral;
using Core::CSS::consumeNumber;
using Core::CSS::consumePercentage;
using Core::CSS::isSystemColorKeyword;
using Core::CSS::kAnyRange;
using Core::CSS::Keyword;
using Core::CSS::KeywordDescriptor;
using Core::CSS::KeywordName;
using Core::CSS::keywords;
using Core::CSS::kInfinite;
using Core::CSS::kNonnegative;
using Core::CSS::kOneOrMore;
using Core::CSS::Layered;
using Core::CSS::parseAllAnyOrder;
using Core::CSS::parseBlock;
using Core::CSS::parseBoxSizing;
using Core::CSS::ParsedLonghand;
using Core::CSS::ParsedProperties;
using Core::CSS::parseExactly;
using Core::CSS::parseFunction;
using Core::CSS::parseHash;
using Core::CSS::parseJustifyItems;
using Core::CSS::parseLineHeight;
using Core::CSS::parseListSeparatedBy;
using Core::CSS::parseOneOf;
using Core::CSS::parseOneOrMoreAnyOrder;
using Core::CSS::parseOptional;
using Core::CSS::parseOverflowX;
using Core::CSS::parseOverflowY;
using Core::CSS::parsePlus;
using Core::CSS::parsePosition;
using Core::CSS::parseProperty;
using Core::CSS::parseRange;
using Core::CSS::parseRequired;
using Core::CSS::parseScrollbarWidth;
using Core::CSS::parseSequence;
using Core::CSS::parseStar;
using Core::CSS::parseTextAlign;
using Core::CSS::parseTextOverflow;
using Core::CSS::parseVisibility;
using Core::CSS::Property;
using Core::CSS::PropertyList;
using Core::CSS::ShorthandPatternParser;
using Core::CSS::SlashSeparated;
using Core::CSS::SpaceSeparated;
using Core::CSS::StyleValue;
using Core::CSS::Value;
using Core::CSS::ValueRange;
using Core::Layout::borderWidths;
using Core::Layout::RectEdges;
using Core::Style::Appearance;
using Core::Style::applyProperty;
using Core::Style::ApplyType;
using Core::Style::BorderImageRepeatMode;
using Core::Style::BorderImageValueUnit;
using Core::Style::BorderStyle;
using Core::Style::BoxSizing;
using Core::Style::BuilderState;
using Core::Style::ColorScheme;
using Core::Style::ColorSchemeMode;
using Core::Style::ComputedStyle;
using Core::Style::FlexGrow;
using Core::Style::FlexWrap;
using Core::Style::FlexWrapMode;
using Core::Style::FontFamilies;
using Core::Style::FontStyle;
using Core::Style::FontWidth;
using Core::Style::GenericFontFamily;
using Core::Style::LineHeight;
using Core::Style::LineWidth;
using Core::Style::MarginEdge;
using Core::Style::Overflow;
using Core::Style::PaddingEdge;
using Core::Style::Position;
using Core::Style::resolveStyleColors;
using Core::Style::ScrollbarWidth;
using Core::Style::shorthand;
using Core::Style::TextAlign;
using Core::Style::TextOverflow;
using Core::Style::TextWrapMode;
using Core::Style::TextWrapStyle;
using Core::Style::Visibility;
using Core::Style::WordSpacing;
namespace {
template<Core::CSS::LengthUnit Unit> bool hasLengthUnit(const Core::CSS::Length& value) { return value.unit == Unit; }

std::optional<Value> consumeLineStyle(ValueRange& range) {
    return parseOneOf<consumeKeyword<Keyword::NoneValue>, consumeKeyword<Keyword::Solid>, consumeKeyword<Keyword::Inset>,
        consumeKeyword<Keyword::Outset>>(range);
}

constexpr auto compileTimeBorder = shorthand<Property::Border>();
static_assert(compileTimeBorder.property == Property::Border);
static_assert(compileTimeBorder.properties.size() == 3);
} // namespace

TEST(GeneratedStyle, LooksUpShorthandAtRuntime) {
    const auto borderWidth = shorthand(Property::BorderWidth);

    ASSERT_TRUE(borderWidth.has_value());
    EXPECT_EQ(borderWidth->property, Property::BorderWidth);
    EXPECT_EQ(borderWidth->properties.size(), 4u);
    EXPECT_FALSE(shorthand(Property::Opacity).has_value());
}

TEST(GeneratedStyle, InitialValues) {
    const ComputedStyle style;

    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    EXPECT_TRUE(style.color().isSystemColor());
    EXPECT_FLOAT_EQ(style.borderTopWidth().pixels, 3.f);
    EXPECT_TRUE(style.borderTopColor().isCurrentColor());
    EXPECT_EQ(style.fontFamily(), FontFamilies {GenericFontFamily::SansSerif});
    EXPECT_FLOAT_EQ(style.fontSize(), 13.f);
    EXPECT_EQ(style.fontStyle(), FontStyle::Normal);
    EXPECT_FLOAT_EQ(style.fontWeight().value, 400.f);
    EXPECT_FLOAT_EQ(style.fontWidth().percentage, 100.f);
    EXPECT_TRUE(std::holds_alternative<std::monostate>(style.borderImageSource().value));
    EXPECT_TRUE(style.borderImageSlice().edges.isUniform());
    EXPECT_FLOAT_EQ(style.borderImageSlice().edges.top.value, 100.f);
    EXPECT_TRUE(style.borderImageSlice().edges.top.percentage);
    EXPECT_FALSE(style.borderImageSlice().fill);
    EXPECT_TRUE(style.borderImageWidth().edges.isUniform());
    EXPECT_FLOAT_EQ(style.borderImageWidth().edges.top.value, 1.f);
    EXPECT_EQ(style.borderImageWidth().edges.top.unit, BorderImageValueUnit::Number);
    EXPECT_TRUE(style.borderImageOutset().edges.isUniform());
    EXPECT_FLOAT_EQ(style.borderImageOutset().edges.top.value, 0.f);
    EXPECT_TRUE(style.borderImageOutset().edges.top.multiplier);
    EXPECT_EQ(style.borderImageRepeat().horizontal, BorderImageRepeatMode::Stretch);
    EXPECT_EQ(style.borderImageRepeat().vertical, BorderImageRepeatMode::Stretch);
}

TEST(GeneratedStyle, Color) {
    ComputedStyle parent;
    parent.setColor(Color(0.f, 0.f, 1.f));
    ComputedStyle style;
    BuilderState builderState {style, &parent};

    Core::CSS::detail::TokenStream stream("#ff0000");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(Property::Color, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);

    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(Property::Color, builderState, ApplyType::Value, &value));
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 0.f);

    ASSERT_TRUE(applyProperty(Property::Color, builderState, ApplyType::Initial));
    EXPECT_TRUE(style.color().isSystemColor());

    ASSERT_TRUE(applyProperty(Property::Color, builderState, ApplyType::Inherit));
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 1.f);
}

TEST(GeneratedStyle, SystemColors) {
    std::size_t systemColorCount = 0;
    for (const KeywordDescriptor& keyword : keywords) {
        if (!isSystemColorKeyword(keyword.keyword))
            continue;
        ++systemColorCount;

        Core::CSS::detail::TokenStream stream(keyword.name);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(Property::Color, range, parsed)) << keyword.name;
        ASSERT_EQ(parsed.size(), 1u) << keyword.name;

        ComputedStyle style;
        BuilderState builderState {style};
        const StyleValue value = parsed[0].value;
        ASSERT_TRUE(applyProperty(Property::Color, builderState, ApplyType::Value, &value)) << keyword.name;
        EXPECT_TRUE(style.color().isResolved()) << keyword.name;
    }
    EXPECT_EQ(systemColorCount, 19u);

    constexpr std::array deprecatedColors {
        "activeborder",
        "activecaption",
        "appworkspace",
        "background",
        "buttonhighlight",
        "buttonshadow",
        "captiontext",
        "inactiveborder",
        "inactivecaption",
        "inactivecaptiontext",
        "infobackground",
        "infotext",
        "menu",
        "menutext",
        "scrollbar",
        "threeddarkshadow",
        "threedface",
        "threedhighlight",
        "threedlightshadow",
        "threedshadow",
        "window",
        "windowframe",
        "windowtext",
    };
    for (const std::string_view name : deprecatedColors) {
        Core::CSS::detail::TokenStream stream(name);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        EXPECT_FALSE(parseProperty(Property::Color, range, parsed)) << name;
    }

    const auto lightDarkColor = [](std::string_view source, ColorSchemeMode scheme) -> std::optional<Core::Style::Color> {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        const auto value = Core::CSS::detail::parseColor(range);
        EXPECT_TRUE(value.has_value());
        return value ? Core::Style::Color::fromCSS(*value, scheme) : std::nullopt;
    };
    auto light = lightDarkColor("light-dark(CanvasText, Canvas)", ColorSchemeMode::Light);
    auto dark = lightDarkColor("light-dark(CanvasText, Canvas)", ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    light->resolve(Color {}, ColorSchemeMode::Light);
    dark->resolve(Color {}, ColorSchemeMode::Dark);
    EXPECT_FLOAT_EQ(light->resolvedColor().r, 0.f);
    EXPECT_NEAR(dark->resolvedColor().r, 28.f / 255.f, 1.0e-6f);

    ComputedStyle initialColor;
    initialColor.usedColorScheme = ColorSchemeMode::Dark;
    resolveStyleColors(initialColor, Color(0.f, 0.f, 0.f));
    EXPECT_NEAR(initialColor.color().resolvedColor().r, 245.f / 255.f, 1.0e-6f);
}

TEST(GeneratedStyle, NestedLightDark) {
    const auto parse = [](std::string_view source) -> std::optional<Core::CSS::Color> {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        const auto color = Core::CSS::detail::parseColor(range);
        if (!color || Core::CSS::detail::nextToken(range))
            return std::nullopt;
        return color;
    };

    const auto value = parse("light-dark(light-dark(red, blue), light-dark(green, white))");
    ASSERT_TRUE(value.has_value());
    const auto light = Core::Style::Color::fromCSS(*value, ColorSchemeMode::Light);
    const auto dark = Core::Style::Color::fromCSS(*value, ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    const Color expectedLight {1.f, 0.f, 0.f};
    const Color expectedDark {1.f, 1.f, 1.f};
    EXPECT_EQ(light->resolvedColor(), expectedLight);
    EXPECT_EQ(dark->resolvedColor(), expectedDark);
    EXPECT_FALSE(parse("light-dark(red, light-dark(blue))").has_value());
}

TEST(GeneratedStyle, LightDarkCurrentColor) {
    Core::CSS::detail::TokenStream stream("light-dark(currentColor, red)");
    ValueRange range {stream, {0, stream.tokens().size()}};
    const auto value = Core::CSS::detail::parseColor(range);
    ASSERT_TRUE(value.has_value());

    const Color inherited {0.f, 1.f, 0.f};
    auto light = Core::Style::Color::fromCSS(*value, ColorSchemeMode::Light);
    auto dark = Core::Style::Color::fromCSS(*value, ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    EXPECT_TRUE(light->isCurrentColor());
    light->resolve(inherited, ColorSchemeMode::Light);
    dark->resolve(inherited, ColorSchemeMode::Dark);
    EXPECT_EQ(light->resolvedColor(), inherited);
    const Color expectedDark {1.f, 0.f, 0.f};
    EXPECT_EQ(dark->resolvedColor(), expectedDark);
}

TEST(GeneratedStyle, ParsesAndStoresLineHeight) {
    Core::CSS::detail::TokenStream stream("1.5");
    ValueRange range {stream, {0, stream.tokens().size()}};

    const std::optional<Value> value = parseLineHeight(range);

    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue parsedValue = *value;
    EXPECT_TRUE(applyProperty(Property::LineHeight, builderState, ApplyType::Value, &parsedValue));

    ASSERT_TRUE(std::holds_alternative<LineHeight::Number>(style.lineHeight().mValue));
    EXPECT_FLOAT_EQ(std::get<LineHeight::Number>(style.lineHeight().mValue).value, 1.5f);
}

TEST(GeneratedStyle, TextSpacing) {
    struct TestCase {
        Property property;
        std::string_view source;
        float pixels;
        float percent;
    };
    constexpr TestCase cases[] = {
        {Property::LetterSpacing, "-1.5px", -1.5f, 0.f},
        {Property::WordSpacing, "25%", 0.f, .25f},
        {Property::WordSpacing, "normal", 0.f, 0.f},
    };

    ComputedStyle initial;
    EXPECT_EQ(initial.letterSpacing(), ComputedStyle::initialLetterSpacing());
    EXPECT_EQ(initial.wordSpacing(), ComputedStyle::initialWordSpacing());

    for (const TestCase& test : cases) {
        Core::CSS::detail::TokenStream stream(test.source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(test.property, range, parsed));
        ASSERT_EQ(parsed.size(), 1u);

        ComputedStyle style;
        BuilderState builderState {style};
        const StyleValue value = parsed[0].value;
        ASSERT_TRUE(applyProperty(test.property, builderState, ApplyType::Value, &value));
        if (test.property == Property::LetterSpacing) {
            EXPECT_FLOAT_EQ(style.letterSpacing().pixels, test.pixels);
            EXPECT_FLOAT_EQ(style.letterSpacing().percent, test.percent);
        } else {
            EXPECT_FLOAT_EQ(style.wordSpacing().pixels, test.pixels);
            EXPECT_FLOAT_EQ(style.wordSpacing().percent, test.percent);
        }
    }

    ComputedStyle parent;
    parent.setFontSize(20.f);
    parent.setWordSpacing(WordSpacing {0.f, .5f});
    ComputedStyle child;
    child.setFontSize(10.f);
    BuilderState builderState {child, &parent};
    ASSERT_TRUE(applyProperty(Property::WordSpacing, builderState, ApplyType::Inherit));
    EXPECT_FLOAT_EQ(child.wordSpacing().percent, .5f);
    EXPECT_FLOAT_EQ(child.wordSpacing().resolve(child.fontSize()), 5.f);
}

TEST(GeneratedStyle, DispatchesGeneratedPropertyParser) {
    Core::CSS::detail::TokenStream stream("normal");
    ValueRange range {stream, {0, stream.tokens().size()}};

    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::LineHeight, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(parsed[0].property);
    EXPECT_EQ(*parsed[0].property, Property::LineHeight);
    ASSERT_TRUE(std::holds_alternative<Core::CSS::Keyword>(parsed[0].value));
    EXPECT_EQ(std::get<Core::CSS::Keyword>(parsed[0].value).keyword, KeywordNormal);
    EXPECT_EQ(range.range.begin, range.range.end);
}

TEST(GeneratedStyle, ParsesOrder) {
    Core::CSS::detail::TokenStream stream("-2");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::Order, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(std::holds_alternative<Core::CSS::Number>(parsed[0].value));
    EXPECT_EQ(std::get<Core::CSS::Number>(parsed[0].value).value, -2.f);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(Property::Order, builderState, ApplyType::Value, &value));
    EXPECT_EQ(style.order().value, -2);
}

TEST(GeneratedStyle, RejectsNonIntegerOrder) {
    for (const char* source : {"1.0", "1e0", "1px"}) {
        SCOPED_TRACE(source);
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(Property::Order, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyle, ParsesQuotedPropertyReference) {
    Core::CSS::detail::TokenStream stream("nowrap");
    ValueRange range {stream, {0, stream.tokens().size()}};

    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(Property::TextWrapMode, range, parsed));

    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(std::holds_alternative<Core::CSS::Keyword>(parsed[0].value));
    EXPECT_EQ(std::get<Core::CSS::Keyword>(parsed[0].value).keyword, KeywordNowrap);
    EXPECT_EQ(range.range.begin, range.range.end);

    Core::CSS::detail::TokenStream invalidStream("unknown");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    ParsedProperties invalidParsed;
    EXPECT_FALSE(parseProperty(Property::TextWrapMode, invalidRange, invalidParsed));
    EXPECT_TRUE(invalidParsed.empty());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, GroupsBorderProperties) {
    ComputedStyle style;
    style.setBorderWidth({LineWidth {1.f}, LineWidth {2.f}, LineWidth {3.f}, LineWidth {4.f}});
    style.setBorderStyle({BorderStyle::Solid, BorderStyle::Outset, BorderStyle::Inset, BorderStyle::NoneValue});
    style.setBorderColor({Core::Style::Color::fromColor(Color(1.f, 0.f, 0.f)), Core::Style::Color::fromColor(Color(0.f, 1.f, 0.f)),
        Core::Style::Color::fromColor(Color(0.f, 0.f, 1.f)), Core::Style::Color::fromColor(Color(1.f, 1.f, 0.f))});

    const RectEdges<LineWidth> widths = style.borderWidth();
    EXPECT_FLOAT_EQ(widths.top.pixels, 1.f);
    EXPECT_FLOAT_EQ(widths.right.pixels, 2.f);
    EXPECT_FLOAT_EQ(widths.bottom.pixels, 3.f);
    EXPECT_FLOAT_EQ(widths.left.pixels, 4.f);

    const RectEdges<BorderStyle> styles = style.borderStyle();
    EXPECT_EQ(styles.top, BorderStyle::Solid);
    EXPECT_EQ(styles.right, BorderStyle::Outset);
    EXPECT_EQ(styles.bottom, BorderStyle::Inset);
    EXPECT_EQ(styles.left, BorderStyle::NoneValue);

    const RectEdges<Core::Style::Color> colors = style.borderColor();
    EXPECT_FLOAT_EQ(colors.top.resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(colors.right.resolvedColor().g, 1.f);
    EXPECT_FLOAT_EQ(colors.bottom.resolvedColor().b, 1.f);
    EXPECT_FLOAT_EQ(colors.left.resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(colors.left.resolvedColor().g, 1.f);
}

TEST(GeneratedStyle, ParsesGeneratedBorderStyleRectEdges) {
    struct Case {
        const char* source;
        std::array<KeywordName, 4> expected;
    };
    const Case cases[] = {
        {"solid", {KeywordSolid, KeywordSolid, KeywordSolid, KeywordSolid}},
        {"none solid", {KeywordNone, KeywordSolid, KeywordNone, KeywordSolid}},
        {"none solid outset", {KeywordNone, KeywordSolid, KeywordOutset, KeywordSolid}},
        {"none solid outset inset", {KeywordNone, KeywordSolid, KeywordOutset, KeywordInset}},
    };
    constexpr std::array properties {
        Property::BorderTopStyle,
        Property::BorderRightStyle,
        Property::BorderBottomStyle,
        Property::BorderLeftStyle,
    };

    for (const Case& test : cases) {
        Core::CSS::detail::TokenStream stream(test.source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(Property::BorderStyle, range, parsed)) << test.source;
        EXPECT_EQ(range.range.begin, range.range.end) << test.source;
        ASSERT_EQ(parsed.size(), properties.size());
        for (std::size_t index = 0; index < parsed.size(); ++index) {
            ASSERT_TRUE(parsed[index].property);
            EXPECT_EQ(*parsed[index].property, properties[index]);
            const auto* keyword = std::get_if<Core::CSS::Keyword>(&parsed[index].value);
            ASSERT_NE(keyword, nullptr);
            EXPECT_EQ(keyword->keyword, test.expected[index]);
        }
    }

    for (const char* source : {"", "solid none inset outset hidden", "solid invalid"}) {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        EXPECT_FALSE(parseProperty(Property::BorderStyle, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyle, ParsesCommaSeparatedGeneratedList) {
    Core::CSS::detail::TokenStream stream("solid, none, outset");
    ValueRange range {stream, {0, stream.tokens().size()}};

    const auto parsed = parseListSeparatedBy<',', kOneOrMore, consumeLineStyle>(range);
    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* values = std::get_if<Core::CSS::List>(&*parsed);
    ASSERT_NE(values, nullptr);
    ASSERT_EQ(values->values.size(), 3u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(values->values[0]).keyword, KeywordSolid);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(values->values[1]).keyword, KeywordNone);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(values->values[2]).keyword, KeywordOutset);

    Core::CSS::detail::TokenStream invalidStream("solid, none,");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    const auto invalid = parseListSeparatedBy<',', kOneOrMore, consumeLineStyle>(invalidRange);
    EXPECT_FALSE(invalid.has_value());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, ParsesLayeredShorthand) {
    Core::CSS::detail::TokenStream stream("solid, 2px");
    ValueRange range {stream, {0, stream.tokens().size()}};
    using Longhands = PropertyList<Property::BorderTopStyle, Property::BorderTopWidth>;

    const auto parsed = ShorthandPatternParser<Layered, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* styles = std::get_if<Core::CSS::List>(&(*parsed)[0].value);
    ASSERT_NE(styles, nullptr);
    ASSERT_EQ(styles->values.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(styles->values[0]).keyword, KeywordSolid);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(styles->values[1]).keyword, KeywordNone);
    const auto* widths = std::get_if<Core::CSS::List>(&(*parsed)[1].value);
    ASSERT_NE(widths, nullptr);
    ASSERT_EQ(widths->values.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(widths->values[0]).keyword, KeywordMedium);
    EXPECT_EQ(std::get<Core::CSS::Length>(widths->values[1]), Core::CSS::Px(2.f));

    Core::CSS::detail::TokenStream invalidStream("solid,");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<Layered, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, ParsesSpaceSeparatedShorthand) {
    using Longhands = PropertyList<Property::BorderTopStyle, Property::BorderRightStyle>;
    Core::CSS::detail::TokenStream stream("solid none");
    ValueRange range {stream, {0, stream.tokens().size()}};

    const auto parsed = ShorthandPatternParser<SpaceSeparated, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    EXPECT_EQ(*(*parsed)[0].property, Property::BorderTopStyle);
    EXPECT_EQ(std::get<Core::CSS::Keyword>((*parsed)[0].value).keyword, KeywordSolid);
    EXPECT_EQ(*(*parsed)[1].property, Property::BorderRightStyle);
    EXPECT_EQ(std::get<Core::CSS::Keyword>((*parsed)[1].value).keyword, KeywordNone);

    Core::CSS::List value;
    value.values.emplace_back(Core::CSS::Keyword {KeywordSolid});
    value.values.emplace_back(Core::CSS::Keyword {KeywordNone});
    ParsedProperties expanded;
    ASSERT_TRUE((ShorthandPatternParser<SpaceSeparated, Longhands>::expand(Value {std::move(value)}, expanded)));
    ASSERT_EQ(expanded.size(), 2u);
    EXPECT_EQ(*expanded[1].property, Property::BorderRightStyle);

    Core::CSS::detail::TokenStream invalidStream("solid");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<SpaceSeparated, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, ParsesSlashSeparatedShorthand) {
    using Longhands = PropertyList<Property::BorderTopWidth, Property::BorderTopStyle>;
    Core::CSS::detail::TokenStream stream("2px / solid");
    ValueRange range {stream, {0, stream.tokens().size()}};

    const auto parsed = ShorthandPatternParser<SlashSeparated, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    EXPECT_EQ(std::get<Core::CSS::Length>((*parsed)[0].value), Core::CSS::Px(2.f));
    EXPECT_EQ(std::get<Core::CSS::Keyword>((*parsed)[1].value).keyword, KeywordSolid);

    Core::CSS::List value;
    value.values.emplace_back(Core::CSS::Px(2.f));
    value.values.emplace_back(Core::CSS::Keyword {KeywordSolid});
    ParsedProperties expanded;
    ASSERT_TRUE((ShorthandPatternParser<SlashSeparated, Longhands>::expand(Value {std::move(value)}, expanded)));
    ASSERT_EQ(expanded.size(), 2u);
    EXPECT_EQ(*expanded[0].property, Property::BorderTopWidth);

    Core::CSS::detail::TokenStream invalidStream("2px solid");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<SlashSeparated, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, RollsBackExpansion) {
    using Longhands = PropertyList<Property::BorderRightStyle, Property::BorderTop>;
    Core::CSS::List invalidNestedValue;
    invalidNestedValue.values.emplace_back(Core::CSS::Keyword {KeywordSolid});
    const std::array values {
        ParsedLonghand {Property::BorderRightStyle, Value {Core::CSS::Keyword {KeywordSolid}}},
        ParsedLonghand {Property::BorderTop, Value {std::move(invalidNestedValue)}},
    };
    ParsedProperties expanded;
    expanded.emplace_back(Property::Opacity, Value {Core::CSS::Number {.5f}});

    EXPECT_FALSE(Longhands::expand(values, expanded));

    ASSERT_EQ(expanded.size(), 1u);
    ASSERT_TRUE(expanded.front().property.has_value());
    EXPECT_EQ(*expanded.front().property, Property::Opacity);
}

TEST(GeneratedStyle, ParsesCSSValueMultipliers) {
    Core::CSS::detail::TokenStream optionalStream("none");
    ValueRange optionalRange {optionalStream, {0, optionalStream.tokens().size()}};
    ASSERT_TRUE(parseOptional<consumeLineStyle>(optionalRange).has_value());
    EXPECT_EQ(optionalRange.range.begin, optionalRange.range.end);

    Core::CSS::detail::TokenStream starStream("");
    ValueRange starRange {starStream, {0, starStream.tokens().size()}};
    const auto star = parseStar<consumeLineStyle>(starRange);
    ASSERT_TRUE(star.has_value());
    EXPECT_TRUE(std::get<Core::CSS::List>(*star).values.empty());

    Core::CSS::detail::TokenStream plusStream("none solid");
    ValueRange plusRange {plusStream, {0, plusStream.tokens().size()}};
    const auto plus = parsePlus<consumeLineStyle>(plusRange);
    ASSERT_TRUE(plus.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*plus).values.size(), 2u);

    Core::CSS::detail::TokenStream exactStream("none solid");
    ValueRange exactRange {exactStream, {0, exactStream.tokens().size()}};
    const auto exact = parseExactly<2, consumeLineStyle>(exactRange);
    ASSERT_TRUE(exact.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*exact).values.size(), 2u);

    Core::CSS::detail::TokenStream hashStream("none, solid");
    ValueRange hashRange {hashStream, {0, hashStream.tokens().size()}};
    const auto hash = parseHash<consumeLineStyle>(hashRange);
    ASSERT_TRUE(hash.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*hash).values.size(), 2u);

    Core::CSS::detail::TokenStream hashExactStream("none, solid");
    ValueRange hashExactRange {hashExactStream, {0, hashExactStream.tokens().size()}};
    const auto hashExact = parseHash<2, consumeLineStyle>(hashExactRange);
    ASSERT_TRUE(hashExact.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*hashExact).values.size(), 2u);

    Core::CSS::detail::TokenStream hashRangeStream("none, solid, outset");
    ValueRange hashRangeRange {hashRangeStream, {0, hashRangeStream.tokens().size()}};
    const auto hashBounded = parseHash<{2, 3}, consumeLineStyle>(hashRangeRange);
    ASSERT_TRUE(hashBounded.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*hashBounded).values.size(), 3u);

    Core::CSS::detail::TokenStream requiredStream("");
    ValueRange requiredRange {requiredStream, {0, requiredStream.tokens().size()}};
    EXPECT_FALSE(parseRequired<parseStar<consumeLineStyle>>(requiredRange));
    EXPECT_EQ(requiredRange.range.begin, 0u);
}

TEST(GeneratedStyle, ParsesGenericGrammarCombinatorsTransactionally) {
    Core::CSS::detail::TokenStream legacyStream("legacy center");
    ValueRange legacyRange {legacyStream, {0, legacyStream.tokens().size()}};
    const auto legacy = parseJustifyItems(legacyRange);
    ASSERT_TRUE(legacy.has_value());
    ASSERT_TRUE(std::holds_alternative<Core::CSS::List>(*legacy));
    const auto& legacyValues = std::get<Core::CSS::List>(*legacy).values;
    ASSERT_EQ(legacyValues.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(legacyValues[0]).keyword, KeywordLegacy);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(legacyValues[1]).keyword, KeywordCenter);
    EXPECT_EQ(legacyRange.range.begin, legacyRange.range.end);

    Core::CSS::detail::TokenStream keywordStream("thin");
    ValueRange keywordRange {keywordStream, {0, keywordStream.tokens().size()}};
    const auto keyword = parseOneOf<consumeLengthPercentage<kNonnegative>, consumeKeyword<Keyword::Thin>, consumeKeyword<Keyword::Medium>,
        consumeKeyword<Keyword::Thick>>(keywordRange);
    ASSERT_TRUE(keyword.has_value());
    ASSERT_TRUE(std::holds_alternative<Core::CSS::Keyword>(*keyword));
    EXPECT_EQ(std::get<Core::CSS::Keyword>(*keyword).keyword, KeywordThin);
    EXPECT_EQ(keywordRange.range.begin, keywordRange.range.end);

    Core::CSS::detail::TokenStream invalidStream("not-a-line-style");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((parseOneOf<consumeKeyword<Keyword::Solid>, consumeKeyword<Keyword::Inset>>(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);

    Core::CSS::detail::TokenStream repeatedStream("none solid outset inset");
    ValueRange repeatedRange {repeatedStream, {0, repeatedStream.tokens().size()}};
    const auto repeated = parseRange<{1, 4}, consumeLineStyle>(repeatedRange);
    ASSERT_TRUE(repeated.has_value());
    ASSERT_TRUE(std::holds_alternative<Core::CSS::List>(*repeated));
    EXPECT_EQ(std::get<Core::CSS::List>(*repeated).values.size(), 4u);
    EXPECT_EQ(repeatedRange.range.begin, repeatedRange.range.end);

    Core::CSS::detail::TokenStream tooManyStream("none solid outset inset none");
    ValueRange tooManyRange {tooManyStream, {0, tooManyStream.tokens().size()}};
    const auto bounded = parseRange<{1, 4}, consumeLineStyle>(tooManyRange);
    ASSERT_TRUE(bounded.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*bounded).values.size(), 4u);
    EXPECT_NE(tooManyRange.range.begin, tooManyRange.range.end);

    Core::CSS::detail::TokenStream commaStream("red, blue");
    ValueRange commaRange {commaStream, {0, commaStream.tokens().size()}};
    const auto colors = parseListSeparatedBy<',', {1, kInfinite}, Core::CSS::detail::parseColor>(commaRange);
    ASSERT_TRUE(colors.has_value());
    ASSERT_TRUE(std::holds_alternative<Core::CSS::List>(*colors));
    EXPECT_EQ(std::get<Core::CSS::List>(*colors).values.size(), 2u);
    EXPECT_EQ(commaRange.range.begin, commaRange.range.end);

    Core::CSS::detail::TokenStream anyOrderStream("inset solid");
    ValueRange anyOrderRange {anyOrderStream, {0, anyOrderStream.tokens().size()}};
    const auto all = parseAllAnyOrder<consumeKeyword<Keyword::Solid>, consumeKeyword<Keyword::Inset>>(anyOrderRange);
    ASSERT_TRUE(all.has_value());
    const auto& allValues = std::get<Core::CSS::List>(*all).values;
    ASSERT_EQ(allValues.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(allValues[0]).keyword, KeywordSolid);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(allValues[1]).keyword, KeywordInset);

    Core::CSS::detail::TokenStream oneOrMoreStream("inset");
    ValueRange oneOrMoreRange {oneOrMoreStream, {0, oneOrMoreStream.tokens().size()}};
    const auto oneOrMore = parseOneOrMoreAnyOrder<consumeKeyword<Keyword::Solid>, consumeKeyword<Keyword::Inset>>(oneOrMoreRange);
    ASSERT_TRUE(oneOrMore.has_value());
    EXPECT_EQ(std::get<Core::CSS::List>(*oneOrMore).values.size(), 1u);

    Core::CSS::detail::TokenStream sequenceStream("2 / 50%");
    ValueRange sequenceRange {sequenceStream, {0, sequenceStream.tokens().size()}};
    const auto sequence = parseSequence<consumeNumber<kAnyRange>, consumeLiteral<'/'>, consumePercentage<kAnyRange>>(sequenceRange);
    ASSERT_TRUE(sequence.has_value());
    const auto& sequenceValues = std::get<Core::CSS::List>(*sequence).values;
    ASSERT_EQ(sequenceValues.size(), 2u);
    EXPECT_FLOAT_EQ(std::get<Core::CSS::Number>(sequenceValues[0]).value, 2.f);
    EXPECT_FLOAT_EQ(std::get<Core::CSS::Percentage>(sequenceValues[1]).value, 50.f);
    EXPECT_EQ(sequenceRange.range.begin, sequenceRange.range.end);
}

TEST(GeneratedStyle, ParsesFunctionAndBlockProductions) {
    Core::CSS::detail::TokenStream functionStream("CALC(2)");
    ValueRange functionRange {functionStream, {0, functionStream.tokens().size()}};
    const auto function = parseFunction<"calc", consumeNumber<kAnyRange>>(functionRange);
    ASSERT_TRUE(function.has_value());
    const auto* functionValue = std::get_if<std::shared_ptr<const Core::CSS::Function>>(&*function);
    ASSERT_NE(functionValue, nullptr);
    EXPECT_EQ((*functionValue)->name, "calc");
    ASSERT_EQ((*functionValue)->arguments.values.size(), 1u);
    EXPECT_FLOAT_EQ(std::get<Core::CSS::Number>((*functionValue)->arguments.values[0]).value, 2.f);
    EXPECT_EQ(functionRange.range.begin, functionRange.range.end);

    const auto expectBlock = []<Core::CSS::BlockType type>(const char* source) {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        const auto value = parseBlock<type, consumeNumber<kAnyRange>>(range);
        ASSERT_TRUE(value.has_value());
        const auto* block = std::get_if<std::shared_ptr<const Core::CSS::Block>>(&*value);
        ASSERT_NE(block, nullptr);
        EXPECT_EQ((*block)->type, type);
        ASSERT_EQ((*block)->values.values.size(), 1u);
        EXPECT_FLOAT_EQ(std::get<Core::CSS::Number>((*block)->values.values[0]).value, 2.f);
        EXPECT_EQ(range.range.begin, range.range.end);
    };
    expectBlock.template operator()<Core::CSS::BlockType::Parentheses>("(2)");
    expectBlock.template operator()<Core::CSS::BlockType::Brackets>("[2]");
    expectBlock.template operator()<Core::CSS::BlockType::Braces>("{2}");
}

TEST(GeneratedStyle, BorderRadius) {
    Core::CSS::detail::TokenStream stream("10px / 20%");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::BorderTopLeftRadius, range, parsed));
    ASSERT_EQ(range.range.begin, range.range.end);
    ASSERT_EQ(parsed.size(), 1u);
    const auto* values = std::get_if<Core::CSS::List>(&parsed[0].value);
    ASSERT_NE(values, nullptr);
    ASSERT_EQ(values->values.size(), 2u);
    const auto* horizontal = std::get_if<Core::CSS::LengthPercentage>(&values->values[0]);
    const auto* vertical = std::get_if<Core::CSS::LengthPercentage>(&values->values[1]);
    ASSERT_NE(horizontal, nullptr);
    ASSERT_NE(vertical, nullptr);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(Property::BorderTopLeftRadius, builderState, ApplyType::Value, &value));
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().horizontal.pixels, 10.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().horizontal.percent, 0.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().vertical.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().vertical.percent, 0.2f);

    Core::CSS::detail::TokenStream invalidStream("10px /");
    ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
    ParsedProperties invalidParsed;
    EXPECT_FALSE(parseProperty(Property::BorderTopLeftRadius, invalidRange, invalidParsed));
    EXPECT_TRUE(invalidParsed.empty());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyle, ParsesTextWrapShorthandInAnyOrder) {
    struct Case {
        const char* source;
        TextWrapMode mode;
        TextWrapStyle style;
    };
    constexpr Case cases[] = {
        {"wrap", TextWrapMode::Wrap, TextWrapStyle::Auto},
        {"pretty", TextWrapMode::Wrap, TextWrapStyle::Pretty},
        {"nowrap pretty", TextWrapMode::NoWrap, TextWrapStyle::Pretty},
        {"pretty nowrap", TextWrapMode::NoWrap, TextWrapStyle::Pretty},
    };

    for (const Case& test : cases) {
        Core::CSS::detail::TokenStream stream(test.source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        ASSERT_TRUE(parseProperty(Property::TextWrap, range, parsed));
        ASSERT_EQ(range.range.begin, range.range.end);
        ASSERT_EQ(parsed.size(), 2u);
        ASSERT_TRUE(parsed[0].property);
        ASSERT_TRUE(parsed[1].property);
        EXPECT_EQ(*parsed[0].property, Property::TextWrapMode);
        EXPECT_EQ(*parsed[1].property, Property::TextWrapStyle);

        ComputedStyle style;
        BuilderState builderState {style};
        for (const ParsedLonghand& longhand : parsed) {
            ASSERT_TRUE(longhand.property);
            const StyleValue value = longhand.value;
            ASSERT_TRUE(applyProperty(*longhand.property, builderState, ApplyType::Value, &value));
        }
        EXPECT_EQ(style.textWrapMode(), test.mode);
        EXPECT_EQ(style.textWrapStyle(), test.style);
    }

    for (const char* source : {"wrap wrap", "pretty pretty", "pretty unknown"}) {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(Property::TextWrap, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyle, Border) {
    for (const char* source : {"solid", "2px", "#112233ff", "2px solid #112233ff", "#112233ff 2px solid"}) {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        ASSERT_TRUE(parseProperty(Property::Border, range, parsed)) << source;
        EXPECT_EQ(range.range.begin, range.range.end) << source;
        ASSERT_EQ(parsed.size(), 17u) << source;
        EXPECT_EQ(*parsed[0].property, Property::BorderTopWidth);
        EXPECT_EQ(*parsed[1].property, Property::BorderRightWidth);
        EXPECT_EQ(*parsed[2].property, Property::BorderBottomWidth);
        EXPECT_EQ(*parsed[3].property, Property::BorderLeftWidth);
        EXPECT_EQ(*parsed[4].property, Property::BorderTopStyle);
        EXPECT_EQ(*parsed[7].property, Property::BorderLeftStyle);
        EXPECT_EQ(*parsed[8].property, Property::BorderTopColor);
        EXPECT_EQ(*parsed[11].property, Property::BorderLeftColor);
        EXPECT_EQ(*parsed[12].property, Property::BorderImageSource);
        EXPECT_EQ(*parsed[13].property, Property::BorderImageSlice);
        EXPECT_EQ(*parsed[14].property, Property::BorderImageWidth);
        EXPECT_EQ(*parsed[15].property, Property::BorderImageOutset);
        EXPECT_EQ(*parsed[16].property, Property::BorderImageRepeat);
        EXPECT_EQ(Core::CSS::propertyName(*parsed[12].property), "border-image-source");
        EXPECT_EQ(Core::CSS::propertyName(*parsed[13].property), "border-image-slice");
        EXPECT_EQ(Core::CSS::propertyName(*parsed[14].property), "border-image-width");
        EXPECT_EQ(Core::CSS::propertyName(*parsed[15].property), "border-image-outset");
        EXPECT_EQ(Core::CSS::propertyName(*parsed[16].property), "border-image-repeat");
    }

    Core::CSS::detail::TokenStream omittedStream("2px");
    ValueRange omittedRange {omittedStream, {0, omittedStream.tokens().size()}};
    ParsedProperties omitted;
    ASSERT_TRUE(parseProperty(Property::Border, omittedRange, omitted));
    const auto expectKeyword = [](const Value& value, KeywordName expected) {
        const auto* keyword = std::get_if<Core::CSS::Keyword>(&value);
        ASSERT_NE(keyword, nullptr);
        EXPECT_EQ(keyword->keyword, expected);
    };
    expectKeyword(omitted[8].value, KeywordCurrentColor);
    expectKeyword(omitted[4].value, KeywordNone);
    EXPECT_EQ(std::get<Core::CSS::Length>(omitted[0].value), Core::CSS::Px(2.f));

    Core::CSS::detail::TokenStream styleStream("solid");
    ValueRange styleRange {styleStream, {0, styleStream.tokens().size()}};
    ParsedProperties styleOnly;
    ASSERT_TRUE(parseProperty(Property::Border, styleRange, styleOnly));
    expectKeyword(styleOnly[4].value, KeywordSolid);
    expectKeyword(styleOnly[0].value, KeywordMedium);
    expectKeyword(styleOnly[8].value, KeywordCurrentColor);
    expectKeyword(styleOnly[12].value, KeywordNone);
    EXPECT_EQ(std::get<Core::CSS::Percentage>(styleOnly[13].value), Core::CSS::Percentage(100.f));
    EXPECT_EQ(std::get<Core::CSS::Number>(styleOnly[14].value), Core::CSS::Number(1.f));
    EXPECT_EQ(std::get<Core::CSS::Number>(styleOnly[15].value), Core::CSS::Number(0.f));
    expectKeyword(styleOnly[16].value, KeywordStretch);

    for (const char* source : {"solid inset", "2px 3px", "#112233ff #445566ff", "2px unknown"}) {
        Core::CSS::detail::TokenStream stream(source);
        ValueRange range {stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(Property::Border, range, parsed)) << source;
        EXPECT_TRUE(parsed.empty()) << source;
        EXPECT_EQ(range.range.begin, 0u) << source;
    }
}

TEST(GeneratedStyle, BorderImage) {
    Core::CSS::detail::TokenStream stream("url(images/frame.png) 30 fill / 4px / 2 round space");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::BorderImage, range, parsed));
    ASSERT_EQ(parsed.size(), 5u);
    EXPECT_EQ(*parsed[0].property, Property::BorderImageSource);
    EXPECT_EQ(*parsed[1].property, Property::BorderImageSlice);
    EXPECT_EQ(*parsed[2].property, Property::BorderImageWidth);
    EXPECT_EQ(*parsed[3].property, Property::BorderImageOutset);
    EXPECT_EQ(*parsed[4].property, Property::BorderImageRepeat);

    const auto* source = std::get_if<Core::CSS::String>(&parsed[0].value);
    ASSERT_NE(source, nullptr);
    EXPECT_EQ(source->value, "url(images/frame.png)");
    const auto* slice = std::get_if<Core::CSS::List>(&parsed[1].value);
    ASSERT_NE(slice, nullptr);
    ASSERT_EQ(slice->values.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::Keyword>(slice->values[1]).keyword, KeywordFill);

    for (const char* invalid : {"url(images/frame.png) 30 / /", "30 / 2 /", "30 / 2px / 3px / 4px"}) {
        Core::CSS::detail::TokenStream invalidStream(invalid);
        ValueRange invalidRange {invalidStream, {0, invalidStream.tokens().size()}};
        ParsedProperties rejected;
        EXPECT_FALSE(parseProperty(Property::BorderImage, invalidRange, rejected)) << invalid;
        EXPECT_TRUE(rejected.empty()) << invalid;
        EXPECT_EQ(invalidRange.range.begin, 0u) << invalid;
    }
}

TEST(GeneratedStyle, Gap) {
    Core::CSS::detail::TokenStream stream("4px 8px");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::Gap, range, parsed));
    ASSERT_EQ(parsed.size(), 2u);
    EXPECT_EQ(*parsed[0].property, Property::RowGap);
    EXPECT_EQ(*parsed[1].property, Property::ColumnGap);
    EXPECT_EQ(std::get<Core::CSS::LengthPercentage>(parsed[0].value), Core::CSS::LengthPercentage {Core::CSS::Px(4.f)});
    EXPECT_EQ(std::get<Core::CSS::LengthPercentage>(parsed[1].value), Core::CSS::LengthPercentage {Core::CSS::Px(8.f)});

    Core::CSS::detail::TokenStream singleStream("4px");
    ValueRange singleRange {singleStream, {0, singleStream.tokens().size()}};
    ParsedProperties single;
    ASSERT_TRUE(parseProperty(Property::Gap, singleRange, single));
    ASSERT_EQ(single.size(), 2u);
    EXPECT_EQ(std::get<Core::CSS::LengthPercentage>(single[0].value), Core::CSS::LengthPercentage {Core::CSS::Px(4.f)});
    EXPECT_EQ(std::get<Core::CSS::LengthPercentage>(single[1].value), Core::CSS::LengthPercentage {Core::CSS::Px(4.f)});
}

TEST(GeneratedStyle, AppliesGeneratedBorderSideColor) {
    Core::CSS::detail::TokenStream stream("2px outset #00ff00ff");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(Property::BorderRight, range, parsed));
    ASSERT_EQ(parsed.size(), 3u);
    EXPECT_EQ(*parsed[2].property, Property::BorderRightColor);
    const auto* color = std::get_if<Core::CSS::Color>(&parsed[2].value);
    ASSERT_NE(color, nullptr);
    const auto* solid = color->solidColor();
    ASSERT_NE(solid, nullptr);
    EXPECT_FLOAT_EQ(solid->g, 1.f);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue specified = parsed[2].value;
    ASSERT_TRUE(applyProperty(Property::BorderRightColor, builderState, ApplyType::Value, &specified));
    EXPECT_FLOAT_EQ(style.borderRightColor().resolvedColor().g, 1.f);
}

TEST(GeneratedStyle, ParsesAndAppliesAppearance) {
    Core::CSS::detail::TokenStream stream("base");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(Property::Appearance, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue parsedValue = parsed[0].value;
    EXPECT_TRUE(applyProperty(Property::Appearance, builderState, ApplyType::Value, &parsedValue));
    EXPECT_EQ(style.appearance(), Appearance::Base);

    EXPECT_TRUE(applyProperty(Property::Appearance, builderState, ApplyType::Initial));
    EXPECT_EQ(style.appearance(), Appearance::NoneValue);
}

TEST(GeneratedStyle, AppliesLineHeight) {
    ComputedStyle detached;
    BuilderState detachedState {detached};
    EXPECT_FALSE(applyProperty(Property::LineHeight, detachedState, ApplyType::Value));
    EXPECT_FALSE(applyProperty(Property::LineHeight, detachedState, ApplyType::Inherit));

    ComputedStyle parent;
    parent.setLineHeight(LineHeight {LineHeight::Number {2.f}});

    ComputedStyle style;
    BuilderState builderState {style, &parent};
    EXPECT_TRUE(applyProperty(Property::LineHeight, builderState, ApplyType::Inherit));
    EXPECT_EQ(style.lineHeight(), parent.lineHeight());

    EXPECT_TRUE(applyProperty(Property::LineHeight, builderState, ApplyType::Initial));
    EXPECT_TRUE(std::holds_alternative<Core::CSS::Keyword::Normal>(style.lineHeight().mValue));
}

TEST(GeneratedStyle, CopiesSharedStyleDataBeforeMutation) {
    ComputedStyle original;
    original.setLineHeight(LineHeight {LineHeight::Number {1.5f}});
    original.setFlexGrow(FlexGrow {2.f});
    original.setFlexWrap({FlexWrapMode::Wrap, true});
    original.setColorScheme(ColorScheme {false, false, {ColorSchemeMode::Dark}, {}});
    original.setMargin(RectEdges<MarginEdge> {MarginEdge::fromPixels(1.f), MarginEdge::automatic(), MarginEdge::fromPixels(3.f),
        MarginEdge::fromPixels(4.f)});
    original.setPadding(RectEdges<PaddingEdge> {PaddingEdge {5.f}, PaddingEdge {6.f}, PaddingEdge {7.f}, PaddingEdge {8.f}});

    ComputedStyle copy = original;
    copy.setLineHeight(LineHeight {LineHeight::Number {2.f}});
    copy.setFlexGrow(FlexGrow {3.f});
    copy.setFlexWrap({FlexWrapMode::WrapReverse, false});
    copy.setColorScheme(ColorScheme {false, false, {ColorSchemeMode::Light}, {}});
    copy.setMargin(RectEdges<MarginEdge> {});
    copy.setPadding(RectEdges<PaddingEdge> {});

    EXPECT_EQ(std::get<LineHeight::Number>(original.lineHeight().mValue).value, 1.5f);
    EXPECT_EQ(std::get<LineHeight::Number>(copy.lineHeight().mValue).value, 2.f);
    EXPECT_FLOAT_EQ(original.flexGrow().value, 2.f);
    EXPECT_FLOAT_EQ(copy.flexGrow().value, 3.f);
    const FlexWrap expectedOriginalWrap {FlexWrapMode::Wrap, true};
    const FlexWrap expectedCopyWrap {FlexWrapMode::WrapReverse, false};
    EXPECT_EQ(original.flexWrap(), expectedOriginalWrap);
    EXPECT_EQ(copy.flexWrap(), expectedCopyWrap);
    EXPECT_EQ(original.colorScheme().schemes, std::vector<ColorSchemeMode> {ColorSchemeMode::Dark});
    EXPECT_EQ(copy.colorScheme().schemes, std::vector<ColorSchemeMode> {ColorSchemeMode::Light});
    EXPECT_TRUE(original.margin().right.isAuto());
    EXPECT_EQ(original.margin().left.fixedPixels(), 4.f);
    EXPECT_EQ(copy.margin().left.fixedPixels(), 0.f);
    EXPECT_EQ(original.padding().right.pixels, 6.f);
    EXPECT_EQ(copy.padding().right.pixels, 0.f);
}

TEST(GeneratedStyle, ParsesAndAppliesFlexGrowthProperties) {
    Core::CSS::detail::TokenStream stream("2");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(Property::FlexGrow, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue parsedValue = parsed[0].value;
    EXPECT_TRUE(applyProperty(Property::FlexGrow, builderState, ApplyType::Value, &parsedValue));
    EXPECT_FLOAT_EQ(style.flexGrow().value, 2.f);

    EXPECT_TRUE(applyProperty(Property::FlexShrink, builderState, ApplyType::Initial));
    EXPECT_FLOAT_EQ(style.flexShrink().value, 1.f);
}

TEST(GeneratedStyle, ParsesAndAppliesOpacityValue) {
    Core::CSS::detail::TokenStream stream("50%");
    ValueRange range {stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(Property::Opacity, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* percentage = std::get_if<Core::CSS::Percentage>(&parsed[0].value);
    ASSERT_NE(percentage, nullptr);
    EXPECT_FLOAT_EQ(percentage->value, 50.f);

    ComputedStyle style;
    BuilderState builderState {style};
    const StyleValue parsedValue = parsed[0].value;
    EXPECT_TRUE(applyProperty(Property::Opacity, builderState, ApplyType::Value, &parsedValue));
    EXPECT_FLOAT_EQ(style.opacity().value, .5f);

    Core::CSS::detail::TokenStream outOfRangeStream("2");
    ValueRange outOfRange {outOfRangeStream, {0, outOfRangeStream.tokens().size()}};
    ParsedProperties outOfRangeParsed;
    ASSERT_TRUE(parseProperty(Property::Opacity, outOfRange, outOfRangeParsed));
    ASSERT_EQ(outOfRangeParsed.size(), 1u);
    const auto* number = std::get_if<Core::CSS::Number>(&outOfRangeParsed[0].value);
    ASSERT_NE(number, nullptr);
    EXPECT_FLOAT_EQ(number->value, 2.f);

    const StyleValue outOfRangeDeclaration = outOfRangeParsed[0].value;
    EXPECT_TRUE(applyProperty(Property::Opacity, builderState, ApplyType::Value, &outOfRangeDeclaration));
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);

    Core::CSS::detail::TokenStream percentageStream("150%");
    ValueRange percentageRange {percentageStream, {0, percentageStream.tokens().size()}};
    ParsedProperties outOfRangePercentageParsed;
    ASSERT_TRUE(parseProperty(Property::Opacity, percentageRange, outOfRangePercentageParsed));
    ASSERT_EQ(outOfRangePercentageParsed.size(), 1u);
    const auto* rawPercentage = std::get_if<Core::CSS::Percentage>(&outOfRangePercentageParsed[0].value);
    ASSERT_NE(rawPercentage, nullptr);
    EXPECT_FLOAT_EQ(rawPercentage->value, 150.f);

    const StyleValue outOfRangePercentageDeclaration = outOfRangePercentageParsed[0].value;
    EXPECT_TRUE(applyProperty(Property::Opacity, builderState, ApplyType::Value, &outOfRangePercentageDeclaration));
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);
}

TEST(GeneratedStyle, ParsesAndAppliesFlagBackedProperties) {
    ComputedStyle style;
    BuilderState builderState {style};

    Core::CSS::detail::TokenStream boxSizingStream("border-box");
    ValueRange boxSizingRange {boxSizingStream, {0, boxSizingStream.tokens().size()}};
    const std::optional<Value> boxSizingValue = parseBoxSizing(boxSizingRange);
    ASSERT_TRUE(boxSizingValue.has_value());
    const StyleValue boxSizingDeclaration = *boxSizingValue;
    EXPECT_TRUE(applyProperty(Property::BoxSizing, builderState, ApplyType::Value, &boxSizingDeclaration));
    EXPECT_EQ(style.boxSizing(), BoxSizing::BorderBox);
    EXPECT_TRUE(applyProperty(Property::BoxSizing, builderState, ApplyType::Initial));
    EXPECT_EQ(style.boxSizing(), BoxSizing::ContentBox);

    Core::CSS::detail::TokenStream positionStream("sticky");
    ValueRange positionRange {positionStream, {0, positionStream.tokens().size()}};
    const std::optional<Value> positionValue = parsePosition(positionRange);
    ASSERT_TRUE(positionValue.has_value());
    const StyleValue positionDeclaration = *positionValue;
    EXPECT_TRUE(applyProperty(Property::Position, builderState, ApplyType::Value, &positionDeclaration));
    EXPECT_EQ(style.position(), Position::Sticky);

    Core::CSS::detail::TokenStream visibilityStream("hidden");
    ValueRange visibilityRange {visibilityStream, {0, visibilityStream.tokens().size()}};
    const std::optional<Value> visibilityValue = parseVisibility(visibilityRange);
    ASSERT_TRUE(visibilityValue.has_value());
    const StyleValue visibilityDeclaration = *visibilityValue;
    EXPECT_TRUE(applyProperty(Property::Visibility, builderState, ApplyType::Value, &visibilityDeclaration));
    EXPECT_EQ(style.visibility(), Visibility::Hidden);

    ComputedStyle parent;
    BuilderState inheritState {style, &parent};
    parent.setVisibility(Visibility::Collapse);
    EXPECT_TRUE(applyProperty(Property::Visibility, inheritState, ApplyType::Inherit));
    EXPECT_EQ(style.visibility(), Visibility::Collapse);
}

TEST(GeneratedStyle, ParsesAndAppliesOverflowTextAndScrollbarProperties) {
    ComputedStyle style;
    BuilderState builderState {style};

    Core::CSS::detail::TokenStream overflowXStream("hidden");
    ValueRange overflowXRange {overflowXStream, {0, overflowXStream.tokens().size()}};
    const std::optional<Value> overflowXValue = parseOverflowX(overflowXRange);
    ASSERT_TRUE(overflowXValue.has_value());
    const StyleValue overflowXDeclaration = *overflowXValue;
    EXPECT_TRUE(applyProperty(Property::OverflowX, builderState, ApplyType::Value, &overflowXDeclaration));
    EXPECT_EQ(style.overflowX(), Overflow::Hidden);

    Core::CSS::detail::TokenStream overflowYStream("scroll");
    ValueRange overflowYRange {overflowYStream, {0, overflowYStream.tokens().size()}};
    const std::optional<Value> overflowYValue = parseOverflowY(overflowYRange);
    ASSERT_TRUE(overflowYValue.has_value());
    const StyleValue overflowYDeclaration = *overflowYValue;
    EXPECT_TRUE(applyProperty(Property::OverflowY, builderState, ApplyType::Value, &overflowYDeclaration));
    EXPECT_EQ(style.overflowY(), Overflow::Scroll);

    Core::CSS::detail::TokenStream scrollbarWidthStream("thin");
    ValueRange scrollbarWidthRange {scrollbarWidthStream, {0, scrollbarWidthStream.tokens().size()}};
    const std::optional<Value> scrollbarWidthValue = parseScrollbarWidth(scrollbarWidthRange);
    ASSERT_TRUE(scrollbarWidthValue.has_value());
    const StyleValue scrollbarWidthDeclaration = *scrollbarWidthValue;
    EXPECT_TRUE(applyProperty(Property::ScrollbarWidth, builderState, ApplyType::Value, &scrollbarWidthDeclaration));
    EXPECT_EQ(style.scrollbarWidth(), ScrollbarWidth::Thin);

    Core::CSS::detail::TokenStream textAlignStream("center");
    ValueRange textAlignRange {textAlignStream, {0, textAlignStream.tokens().size()}};
    const std::optional<Value> textAlignValue = parseTextAlign(textAlignRange);
    ASSERT_TRUE(textAlignValue.has_value());
    const StyleValue textAlignDeclaration = *textAlignValue;
    EXPECT_TRUE(applyProperty(Property::TextAlign, builderState, ApplyType::Value, &textAlignDeclaration));
    EXPECT_EQ(style.textAlign(), TextAlign::Center);

    Core::CSS::detail::TokenStream textOverflowStream("ellipsis-center");
    ValueRange textOverflowRange {textOverflowStream, {0, textOverflowStream.tokens().size()}};
    const std::optional<Value> textOverflowValue = parseTextOverflow(textOverflowRange);
    ASSERT_TRUE(textOverflowValue.has_value());
    const StyleValue textOverflowDeclaration = *textOverflowValue;
    EXPECT_TRUE(applyProperty(Property::TextOverflow, builderState, ApplyType::Value, &textOverflowDeclaration));
    EXPECT_EQ(style.textOverflow(), TextOverflow::EllipsisCenter);
}

TEST(GeneratedStyle, RejectsNegativeLineHeightWithoutConsumingInput) {
    Core::CSS::detail::TokenStream stream("-1");
    ValueRange range {stream, {0, stream.tokens().size()}};

    EXPECT_FALSE(parseLineHeight(range).has_value());
    EXPECT_EQ(range.range.begin, 0u);
}

TEST(GeneratedStyle, KeepsCSSLengthUnitsDistinct) {
    const Core::CSS::Length length = Core::CSS::Em(1.f);
    EXPECT_EQ(length.unit, Core::CSS::LengthUnit::Em);
    EXPECT_FLOAT_EQ(length.value, 1.f);

    const Value value = Core::CSS::LengthPercentage {length};
    const auto* lengthPercentage = std::get_if<Core::CSS::LengthPercentage>(&value);
    ASSERT_NE(lengthPercentage, nullptr);
    const auto* nestedLength = std::get_if<Core::CSS::Length>(lengthPercentage);
    ASSERT_NE(nestedLength, nullptr);
    EXPECT_EQ(nestedLength->unit, Core::CSS::LengthUnit::Em);
}

TEST(GeneratedStyle, PrimitiveParsersConsumeOneValueAndFollowingTrivia) {
    Core::CSS::detail::TokenStream stream(" \t1Em \n solid red");
    ValueRange range {stream, {0, stream.tokens().size()}};

    const std::optional<Core::CSS::Length> length = consumeLength<kNonnegative>(range);

    ASSERT_TRUE(length.has_value());
    EXPECT_EQ(length->unit, Core::CSS::LengthUnit::Em);
    EXPECT_FLOAT_EQ(length->value, 1.f);
    ASSERT_LT(range.range.begin, range.range.end);
    EXPECT_EQ(stream.text(range.range.begin), "solid");

    Core::CSS::detail::TokenStream percentageStream("50% 10px");
    ValueRange percentageRange {percentageStream, {0, percentageStream.tokens().size()}};
    const std::optional<Core::CSS::Percentage> percentage = consumePercentage<kAnyRange>(percentageRange);

    ASSERT_TRUE(percentage.has_value());
    EXPECT_FLOAT_EQ(percentage->value, 50.f);
    ASSERT_LT(percentageRange.range.begin, percentageRange.range.end);
    EXPECT_EQ(percentageStream.text(percentageRange.range.begin), "10px");
}

TEST(GeneratedStyle, PreservesPercentageValue) {
    Core::CSS::detail::TokenStream stream("50%");
    ValueRange range {stream, {0, stream.tokens().size()}};
    const std::optional<Core::CSS::LengthPercentage> parsed = consumeLengthPercentage<kNonnegative>(range);

    ASSERT_TRUE(parsed.has_value());
    const auto* percentage = std::get_if<Core::CSS::Percentage>(&*parsed);
    ASSERT_NE(percentage, nullptr);
    EXPECT_FLOAT_EQ(percentage->value, 50.f);
}

TEST(GeneratedStyle, ParsesEveryCSSLengthUnitInLengthPercentage) {
    using Check = bool (*)(const Core::CSS::Length&);
    struct LengthCase {
        std::string_view source;
        Check check;
    };
    constexpr LengthCase cases[] = {
        {"px", hasLengthUnit<Core::CSS::LengthUnit::Px>},
        {"em", hasLengthUnit<Core::CSS::LengthUnit::Em>},
        {"rem", hasLengthUnit<Core::CSS::LengthUnit::Rem>},
        {"ch", hasLengthUnit<Core::CSS::LengthUnit::Ch>},
        {"ex", hasLengthUnit<Core::CSS::LengthUnit::Ex>},
        {"cap", hasLengthUnit<Core::CSS::LengthUnit::Cap>},
        {"ic", hasLengthUnit<Core::CSS::LengthUnit::Ic>},
        {"lh", hasLengthUnit<Core::CSS::LengthUnit::Lh>},
        {"rlh", hasLengthUnit<Core::CSS::LengthUnit::Rlh>},
        {"vw", hasLengthUnit<Core::CSS::LengthUnit::Vw>},
        {"vh", hasLengthUnit<Core::CSS::LengthUnit::Vh>},
        {"vmin", hasLengthUnit<Core::CSS::LengthUnit::Vmin>},
        {"vmax", hasLengthUnit<Core::CSS::LengthUnit::Vmax>},
        {"vi", hasLengthUnit<Core::CSS::LengthUnit::Vi>},
        {"vb", hasLengthUnit<Core::CSS::LengthUnit::Vb>},
        {"svw", hasLengthUnit<Core::CSS::LengthUnit::Svw>},
        {"svh", hasLengthUnit<Core::CSS::LengthUnit::Svh>},
        {"svmin", hasLengthUnit<Core::CSS::LengthUnit::Svmin>},
        {"svmax", hasLengthUnit<Core::CSS::LengthUnit::Svmax>},
        {"svi", hasLengthUnit<Core::CSS::LengthUnit::Svi>},
        {"svb", hasLengthUnit<Core::CSS::LengthUnit::Svb>},
        {"lvw", hasLengthUnit<Core::CSS::LengthUnit::Lvw>},
        {"lvh", hasLengthUnit<Core::CSS::LengthUnit::Lvh>},
        {"lvmin", hasLengthUnit<Core::CSS::LengthUnit::Lvmin>},
        {"lvmax", hasLengthUnit<Core::CSS::LengthUnit::Lvmax>},
        {"lvi", hasLengthUnit<Core::CSS::LengthUnit::Lvi>},
        {"lvb", hasLengthUnit<Core::CSS::LengthUnit::Lvb>},
        {"dvw", hasLengthUnit<Core::CSS::LengthUnit::Dvw>},
        {"dvh", hasLengthUnit<Core::CSS::LengthUnit::Dvh>},
        {"dvmin", hasLengthUnit<Core::CSS::LengthUnit::Dvmin>},
        {"dvmax", hasLengthUnit<Core::CSS::LengthUnit::Dvmax>},
        {"dvi", hasLengthUnit<Core::CSS::LengthUnit::Dvi>},
        {"dvb", hasLengthUnit<Core::CSS::LengthUnit::Dvb>},
        {"cm", hasLengthUnit<Core::CSS::LengthUnit::Cm>},
        {"mm", hasLengthUnit<Core::CSS::LengthUnit::Mm>},
        {"q", hasLengthUnit<Core::CSS::LengthUnit::Q>},
        {"in", hasLengthUnit<Core::CSS::LengthUnit::In>},
        {"pt", hasLengthUnit<Core::CSS::LengthUnit::Pt>},
        {"pc", hasLengthUnit<Core::CSS::LengthUnit::Pc>},
    };

    for (const LengthCase& lengthCase : cases) {
        Core::CSS::detail::TokenStream stream(std::string("1") + std::string(lengthCase.source));
        ValueRange range {stream, {0, stream.tokens().size()}};
        const std::optional<Core::CSS::LengthPercentage> parsed = consumeLengthPercentage<kNonnegative>(range);

        SCOPED_TRACE(lengthCase.source);
        ASSERT_TRUE(parsed.has_value());
        const auto* length = std::get_if<Core::CSS::Length>(&*parsed);
        ASSERT_NE(length, nullptr);
        EXPECT_TRUE(lengthCase.check(*length));
        EXPECT_EQ(range.range.begin, range.range.end);
    }

    Core::CSS::detail::TokenStream stream("0");
    ValueRange range {stream, {0, stream.tokens().size()}};
    const std::optional<Core::CSS::LengthPercentage> parsed = consumeLengthPercentage<kNonnegative>(range);
    ASSERT_TRUE(parsed.has_value());
    const auto* length = std::get_if<Core::CSS::Length>(&*parsed);
    ASSERT_NE(length, nullptr);
    EXPECT_EQ(length->unit, Core::CSS::LengthUnit::Px);
}

TEST(LayoutGeometry, UsesZeroWidthForNoneBorderStyle) {
    ComputedStyle style;
    EXPECT_FLOAT_EQ(style.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(borderWidths(style), RectEdges<float> {});

    style.setBorderWidth({LineWidth {1.f}, LineWidth {2.f}, LineWidth {3.f}, LineWidth {4.f}});
    style.setBorderStyle({BorderStyle::Solid, BorderStyle::NoneValue, BorderStyle::Outset, BorderStyle::Inset});
    const RectEdges<float> expectedWidths {1.f, 0.f, 3.f, 4.f};
    EXPECT_EQ(borderWidths(style), expectedWidths);
}

TEST(LayoutGeometry, MarginEdges) {
    const RectEdges<MarginEdge> margin {MarginEdge::fromPixels(1.f), MarginEdge::automatic(), MarginEdge::fromPixels(3.f),
        MarginEdge::fromPixels(-4.f)};

    EXPECT_FLOAT_EQ(horizontalMargin(margin), -4.f);
    EXPECT_FLOAT_EQ(verticalMargin(margin), 4.f);
    EXPECT_EQ(horizontalAutoMarginCount(margin), 1);
    EXPECT_EQ(verticalAutoMarginCount(margin), 0);
}
} // namespace CoreTests
