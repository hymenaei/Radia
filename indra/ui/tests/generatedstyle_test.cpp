/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <array>
#include <gtest/gtest.h>
#include <optional>
#include <string_view>
#include <utility>
#include <variant>
#include "ComputedStyleProperties.h"
#include "CSSPropertyParsing.h"
#include "CSSValue.h"
#include "Geometry.h"
#include "style/computedstyle.h"
#include "style/property.h"

namespace radia::ui {
namespace {
template<CSS::LengthUnit Unit> bool hasLengthUnit(const CSS::Length& value) {
    return value.unit == Unit;
}

constexpr auto compileTimeBorder = shorthand<CSSProperty::Border>();
static_assert(compileTimeBorder.property == CSSProperty::Border);
static_assert(compileTimeBorder.properties.size() == 3);
} // namespace

TEST(GeneratedStyleTest, LooksUpShorthandAtRuntime) {
    const auto borderWidth = shorthand(CSSProperty::BorderWidth);

    ASSERT_TRUE(borderWidth.has_value());
    EXPECT_EQ(borderWidth->property, CSSProperty::BorderWidth);
    EXPECT_EQ(borderWidth->properties.size(), 4u);
    EXPECT_FALSE(shorthand(CSSProperty::Opacity).has_value());
}

TEST(GeneratedStyleTest, InitialValues) {
    const ComputedStyle style;

    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    EXPECT_TRUE(style.color().isSystemColor());
    EXPECT_FLOAT_EQ(style.borderTopWidth().pixels, 3.f);
    EXPECT_TRUE(style.borderTopColor().isCurrentColor());
    EXPECT_EQ(style.fontFamily(), FontFamilies{GenericFontFamily::SansSerif});
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

TEST(GeneratedStyleTest, Color) {
    ComputedStyle parent;
    parent.setColor(Color(0.f, 0.f, 1.f));
    ComputedStyle style;
    StyleBuilderState builderState{style, &parent};

    detail::CSSTokenStream stream("#ff0000");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(CSSProperty::Color, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);

    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(CSSProperty::Color, builderState, ApplyType::Value, &value));
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 0.f);

    ASSERT_TRUE(applyProperty(CSSProperty::Color, builderState, ApplyType::Initial));
    EXPECT_TRUE(style.color().isSystemColor());

    ASSERT_TRUE(applyProperty(CSSProperty::Color, builderState, ApplyType::Inherit));
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 1.f);
}

TEST(GeneratedStyleTest, SystemColors) {
    std::size_t systemColorCount = 0;
    for (const CSSKeywordDescriptor& keyword : cssKeywords) {
        if (!isSystemColorKeyword(keyword.keyword)) continue;
        ++systemColorCount;

        detail::CSSTokenStream stream(keyword.name);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(CSSProperty::Color, range, parsed)) << keyword.name;
        ASSERT_EQ(parsed.size(), 1u) << keyword.name;

        ComputedStyle style;
        StyleBuilderState builderState{style};
        const StyleValue value = parsed[0].value;
        ASSERT_TRUE(applyProperty(CSSProperty::Color, builderState, ApplyType::Value, &value)) << keyword.name;
        EXPECT_TRUE(style.color().isResolved()) << keyword.name;
    }
    EXPECT_EQ(systemColorCount, 19u);

    constexpr std::array deprecatedColors{
        "activeborder",   "activecaption",    "appworkspace",        "background",      "buttonhighlight",   "buttonshadow", "captiontext",
        "inactiveborder", "inactivecaption",  "inactivecaptiontext", "infobackground",  "infotext",          "menu",         "menutext",
        "scrollbar",      "threeddarkshadow", "threedface",          "threedhighlight", "threedlightshadow", "threedshadow", "window",
        "windowframe",    "windowtext",
    };
    for (const std::string_view name : deprecatedColors) {
        detail::CSSTokenStream stream(name);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        EXPECT_FALSE(parseProperty(CSSProperty::Color, range, parsed)) << name;
    }

    const auto lightDarkColor = [](std::string_view source, ColorSchemeMode scheme) -> std::optional<StyleColor> {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        const auto value = CSSValueDetail::consumeColor(range);
        EXPECT_TRUE(value.has_value());
        return value ? StyleColor::fromCSS(*value, scheme) : std::nullopt;
    };
    auto light = lightDarkColor("light-dark(CanvasText, Canvas)", ColorSchemeMode::Light);
    auto dark = lightDarkColor("light-dark(CanvasText, Canvas)", ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    light->resolve(Color{}, ColorSchemeMode::Light);
    dark->resolve(Color{}, ColorSchemeMode::Dark);
    EXPECT_FLOAT_EQ(light->resolvedColor().r, 0.f);
    EXPECT_NEAR(dark->resolvedColor().r, 28.f / 255.f, 1.0e-6f);

    ComputedStyle initialColor;
    initialColor.usedColorScheme = ColorSchemeMode::Dark;
    resolveStyleColors(initialColor, Color(0.f, 0.f, 0.f));
    EXPECT_NEAR(initialColor.color().resolvedColor().r, 245.f / 255.f, 1.0e-6f);
}

TEST(GeneratedStyleTest, NestedLightDark) {
    const auto parse = [](std::string_view source) -> std::optional<CSS::Color> {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        const auto color = CSSValueDetail::consumeColor(range);
        if (!color || CSSValueDetail::nextToken(range)) return std::nullopt;
        return color;
    };

    const auto value = parse("light-dark(light-dark(red, blue), light-dark(green, white))");
    ASSERT_TRUE(value.has_value());
    const auto light = StyleColor::fromCSS(*value, ColorSchemeMode::Light);
    const auto dark = StyleColor::fromCSS(*value, ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    const Color expectedLight{1.f, 0.f, 0.f};
    const Color expectedDark{1.f, 1.f, 1.f};
    EXPECT_EQ(light->resolvedColor(), expectedLight);
    EXPECT_EQ(dark->resolvedColor(), expectedDark);
    EXPECT_FALSE(parse("light-dark(red, light-dark(blue))").has_value());
}

TEST(GeneratedStyleTest, LightDarkCurrentColor) {
    detail::CSSTokenStream stream("light-dark(currentColor, red)");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    const auto value = CSSValueDetail::consumeColor(range);
    ASSERT_TRUE(value.has_value());

    const Color inherited{0.f, 1.f, 0.f};
    auto light = StyleColor::fromCSS(*value, ColorSchemeMode::Light);
    auto dark = StyleColor::fromCSS(*value, ColorSchemeMode::Dark);
    ASSERT_TRUE(light.has_value());
    ASSERT_TRUE(dark.has_value());
    EXPECT_TRUE(light->isCurrentColor());
    light->resolve(inherited, ColorSchemeMode::Light);
    dark->resolve(inherited, ColorSchemeMode::Dark);
    EXPECT_EQ(light->resolvedColor(), inherited);
    const Color expectedDark{1.f, 0.f, 0.f};
    EXPECT_EQ(dark->resolvedColor(), expectedDark);
}

TEST(GeneratedStyleTest, ParsesAndStoresLineHeight) {
    detail::CSSTokenStream stream("1.5");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const std::optional<CSSValue> value = parseLineHeight(range);

    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue parsedValue = *value;
    EXPECT_TRUE(applyProperty(CSSProperty::LineHeight, builderState, ApplyType::Value, &parsedValue));

    ASSERT_TRUE(std::holds_alternative<LineHeight::Number>(style.lineHeight().mValue));
    EXPECT_FLOAT_EQ(std::get<LineHeight::Number>(style.lineHeight().mValue).value, 1.5f);
}

TEST(GeneratedStyleTest, TextSpacing) {
    struct TestCase {
        CSSProperty property;
        std::string_view source;
        float pixels;
        float percent;
    };
    constexpr TestCase cases[] = {
        {CSSProperty::LetterSpacing, "-1.5px", -1.5f, 0.f},
        {CSSProperty::WordSpacing, "25%", 0.f, .25f},
        {CSSProperty::WordSpacing, "normal", 0.f, 0.f},
    };

    ComputedStyle initial;
    EXPECT_EQ(initial.letterSpacing(), ComputedStyle::initialLetterSpacing());
    EXPECT_EQ(initial.wordSpacing(), ComputedStyle::initialWordSpacing());

    for (const TestCase& test : cases) {
        detail::CSSTokenStream stream(test.source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(test.property, range, parsed));
        ASSERT_EQ(parsed.size(), 1u);

        ComputedStyle style;
        StyleBuilderState builderState{style};
        const StyleValue value = parsed[0].value;
        ASSERT_TRUE(applyProperty(test.property, builderState, ApplyType::Value, &value));
        if (test.property == CSSProperty::LetterSpacing) {
            EXPECT_FLOAT_EQ(style.letterSpacing().pixels, test.pixels);
            EXPECT_FLOAT_EQ(style.letterSpacing().percent, test.percent);
        } else {
            EXPECT_FLOAT_EQ(style.wordSpacing().pixels, test.pixels);
            EXPECT_FLOAT_EQ(style.wordSpacing().percent, test.percent);
        }
    }

    ComputedStyle parent;
    parent.setFontSize(20.f);
    parent.setWordSpacing(WordSpacing{0.f, .5f});
    ComputedStyle child;
    child.setFontSize(10.f);
    StyleBuilderState builderState{child, &parent};
    ASSERT_TRUE(applyProperty(CSSProperty::WordSpacing, builderState, ApplyType::Inherit));
    EXPECT_FLOAT_EQ(child.wordSpacing().percent, .5f);
    EXPECT_FLOAT_EQ(child.wordSpacing().resolve(child.fontSize()), 5.f);
}

TEST(GeneratedStyleTest, DispatchesGeneratedPropertyParser) {
    detail::CSSTokenStream stream("normal");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::LineHeight, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(parsed[0].property);
    EXPECT_EQ(*parsed[0].property, CSSProperty::LineHeight);
    ASSERT_TRUE(std::holds_alternative<CSS::Keyword>(parsed[0].value));
    EXPECT_EQ(std::get<CSS::Keyword>(parsed[0].value).id, CSSKeyword::Normal);
    EXPECT_EQ(range.range.begin, range.range.end);
}

TEST(GeneratedStyleTest, ParsesOrder) {
    detail::CSSTokenStream stream("-2");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::Order, range, parsed));
    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(std::holds_alternative<CSS::Number>(parsed[0].value));
    EXPECT_EQ(std::get<CSS::Number>(parsed[0].value).value, -2.f);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(CSSProperty::Order, builderState, ApplyType::Value, &value));
    EXPECT_EQ(style.order().value, -2);
}

TEST(GeneratedStyleTest, RejectsNonIntegerOrder) {
    for (const char* source : {"1.0", "1e0", "1px"}) {
        SCOPED_TRACE(source);
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(CSSProperty::Order, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyleTest, ParsesQuotedPropertyReference) {
    detail::CSSTokenStream stream("nowrap");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    ParsedProperties parsed;
    ASSERT_TRUE(parseProperty(CSSProperty::TextWrapMode, range, parsed));

    ASSERT_EQ(parsed.size(), 1u);
    ASSERT_TRUE(std::holds_alternative<CSS::Keyword>(parsed[0].value));
    EXPECT_EQ(std::get<CSS::Keyword>(parsed[0].value).id, CSSKeyword::Nowrap);
    EXPECT_EQ(range.range.begin, range.range.end);

    detail::CSSTokenStream invalidStream("unknown");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    ParsedProperties invalidParsed;
    EXPECT_FALSE(parseProperty(CSSProperty::TextWrapMode, invalidRange, invalidParsed));
    EXPECT_TRUE(invalidParsed.empty());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, GroupsBorderProperties) {
    ComputedStyle style;
    style.setBorderWidth({LineWidth{1.f}, LineWidth{2.f}, LineWidth{3.f}, LineWidth{4.f}});
    style.setBorderStyle({BorderStyle::Solid, BorderStyle::Outset, BorderStyle::Inset, BorderStyle::NoneValue});
    style.setBorderColor({StyleColor::fromColor(Color(1.f, 0.f, 0.f)), StyleColor::fromColor(Color(0.f, 1.f, 0.f)),
                          StyleColor::fromColor(Color(0.f, 0.f, 1.f)), StyleColor::fromColor(Color(1.f, 1.f, 0.f))});

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

    const RectEdges<StyleColor> colors = style.borderColor();
    EXPECT_FLOAT_EQ(colors.top.resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(colors.right.resolvedColor().g, 1.f);
    EXPECT_FLOAT_EQ(colors.bottom.resolvedColor().b, 1.f);
    EXPECT_FLOAT_EQ(colors.left.resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(colors.left.resolvedColor().g, 1.f);
}

TEST(GeneratedStyleTest, ParsesGeneratedBorderStyleRectEdges) {
    struct Case {
        const char* source;
        std::array<CSSKeyword, 4> expected;
    };
    const Case cases[] = {
        {"solid", {CSSKeyword::Solid, CSSKeyword::Solid, CSSKeyword::Solid, CSSKeyword::Solid}},
        {"none solid", {CSSKeyword::NoneValue, CSSKeyword::Solid, CSSKeyword::NoneValue, CSSKeyword::Solid}},
        {"none solid outset", {CSSKeyword::NoneValue, CSSKeyword::Solid, CSSKeyword::Outset, CSSKeyword::Solid}},
        {"none solid outset inset", {CSSKeyword::NoneValue, CSSKeyword::Solid, CSSKeyword::Outset, CSSKeyword::Inset}},
    };
    constexpr std::array properties{
        CSSProperty::BorderTopStyle,
        CSSProperty::BorderRightStyle,
        CSSProperty::BorderBottomStyle,
        CSSProperty::BorderLeftStyle,
    };

    for (const Case& test : cases) {
        detail::CSSTokenStream stream(test.source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        ASSERT_TRUE(parseProperty(CSSProperty::BorderStyle, range, parsed)) << test.source;
        EXPECT_EQ(range.range.begin, range.range.end) << test.source;
        ASSERT_EQ(parsed.size(), properties.size());
        for (std::size_t index = 0; index < parsed.size(); ++index) {
            ASSERT_TRUE(parsed[index].property);
            EXPECT_EQ(*parsed[index].property, properties[index]);
            const auto* keyword = std::get_if<CSS::Keyword>(&parsed[index].value);
            ASSERT_NE(keyword, nullptr);
            EXPECT_EQ(keyword->id, test.expected[index]);
        }
    }

    for (const char* source : {"", "solid none inset outset hidden", "solid invalid"}) {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;
        EXPECT_FALSE(parseProperty(CSSProperty::BorderStyle, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyleTest, ParsesCommaSeparatedGeneratedList) {
    detail::CSSTokenStream stream("solid, none, outset");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const auto parsed = parseListSeparatedBy<',', OneOrMore, consumeLineStyle>(range);
    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* values = std::get_if<CSS::List>(&*parsed);
    ASSERT_NE(values, nullptr);
    ASSERT_EQ(values->values.size(), 3u);
    EXPECT_EQ(std::get<CSS::Keyword>(values->values[0]).id, CSSKeyword::Solid);
    EXPECT_EQ(std::get<CSS::Keyword>(values->values[1]).id, CSSKeyword::NoneValue);
    EXPECT_EQ(std::get<CSS::Keyword>(values->values[2]).id, CSSKeyword::Outset);

    detail::CSSTokenStream invalidStream("solid, none,");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    const auto invalid = parseListSeparatedBy<',', OneOrMore, consumeLineStyle>(invalidRange);
    EXPECT_FALSE(invalid.has_value());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, Layered) {
    detail::CSSTokenStream stream("solid, 2px");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    using Longhands = PropertyList<CSSProperty::BorderTopStyle, CSSProperty::BorderTopWidth>;

    const auto parsed = ShorthandPatternParser<Layered, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* styles = std::get_if<CSS::List>(&(*parsed)[0].value);
    ASSERT_NE(styles, nullptr);
    ASSERT_EQ(styles->values.size(), 2u);
    EXPECT_EQ(std::get<CSS::Keyword>(styles->values[0]).id, CSSKeyword::Solid);
    EXPECT_EQ(std::get<CSS::Keyword>(styles->values[1]).id, CSSKeyword::NoneValue);
    const auto* widths = std::get_if<CSS::List>(&(*parsed)[1].value);
    ASSERT_NE(widths, nullptr);
    ASSERT_EQ(widths->values.size(), 2u);
    EXPECT_EQ(std::get<CSS::Keyword>(widths->values[0]).id, CSSKeyword::Medium);
    EXPECT_EQ(std::get<CSS::Length>(widths->values[1]), CSS::Px(2.f));

    detail::CSSTokenStream invalidStream("solid,");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<Layered, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, SpaceSeparated) {
    using Longhands = PropertyList<CSSProperty::BorderTopStyle, CSSProperty::BorderRightStyle>;
    detail::CSSTokenStream stream("solid none");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const auto parsed = ShorthandPatternParser<SpaceSeparated, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    EXPECT_EQ(*(*parsed)[0].property, CSSProperty::BorderTopStyle);
    EXPECT_EQ(std::get<CSS::Keyword>((*parsed)[0].value).id, CSSKeyword::Solid);
    EXPECT_EQ(*(*parsed)[1].property, CSSProperty::BorderRightStyle);
    EXPECT_EQ(std::get<CSS::Keyword>((*parsed)[1].value).id, CSSKeyword::NoneValue);

    CSS::List value;
    value.values.emplace_back(CSS::Keyword{CSSKeyword::Solid});
    value.values.emplace_back(CSS::Keyword{CSSKeyword::NoneValue});
    ParsedProperties expanded;
    ASSERT_TRUE((ShorthandPatternParser<SpaceSeparated, Longhands>::expand(CSSValue{std::move(value)}, expanded)));
    ASSERT_EQ(expanded.size(), 2u);
    EXPECT_EQ(*expanded[1].property, CSSProperty::BorderRightStyle);

    detail::CSSTokenStream invalidStream("solid");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<SpaceSeparated, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, SlashSeparated) {
    using Longhands = PropertyList<CSSProperty::BorderTopWidth, CSSProperty::BorderTopStyle>;
    detail::CSSTokenStream stream("2px / solid");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const auto parsed = ShorthandPatternParser<SlashSeparated, Longhands>::parse(range);

    ASSERT_TRUE(parsed.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    EXPECT_EQ(std::get<CSS::Length>((*parsed)[0].value), CSS::Px(2.f));
    EXPECT_EQ(std::get<CSS::Keyword>((*parsed)[1].value).id, CSSKeyword::Solid);

    CSS::List value;
    value.values.emplace_back(CSS::Px(2.f));
    value.values.emplace_back(CSS::Keyword{CSSKeyword::Solid});
    ParsedProperties expanded;
    ASSERT_TRUE((ShorthandPatternParser<SlashSeparated, Longhands>::expand(CSSValue{std::move(value)}, expanded)));
    ASSERT_EQ(expanded.size(), 2u);
    EXPECT_EQ(*expanded[0].property, CSSProperty::BorderTopWidth);

    detail::CSSTokenStream invalidStream("2px solid");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((ShorthandPatternParser<SlashSeparated, Longhands>::parse(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, RollsBackExpansion) {
    using Longhands = PropertyList<CSSProperty::BorderRightStyle, CSSProperty::BorderTop>;
    CSS::List invalidNestedValue;
    invalidNestedValue.values.emplace_back(CSS::Keyword{CSSKeyword::Solid});
    const std::array values{
        ParsedLonghand{CSSProperty::BorderRightStyle, CSSValue{CSS::Keyword{CSSKeyword::Solid}}},
        ParsedLonghand{CSSProperty::BorderTop, CSSValue{std::move(invalidNestedValue)}},
    };
    ParsedProperties expanded;
    expanded.emplace_back(CSSProperty::Opacity, CSSValue{CSS::Number{.5f}});

    EXPECT_FALSE(Longhands::expand(values, expanded));

    ASSERT_EQ(expanded.size(), 1u);
    ASSERT_TRUE(expanded.front().property.has_value());
    EXPECT_EQ(*expanded.front().property, CSSProperty::Opacity);
}

TEST(GeneratedStyleTest, ParsesCSSValueMultipliers) {
    detail::CSSTokenStream optionalStream("none");
    CSSValueRange optionalRange{optionalStream, {0, optionalStream.tokens().size()}};
    ASSERT_TRUE(parseOptional<consumeLineStyle>(optionalRange).has_value());
    EXPECT_EQ(optionalRange.range.begin, optionalRange.range.end);

    detail::CSSTokenStream starStream("");
    CSSValueRange starRange{starStream, {0, starStream.tokens().size()}};
    const auto star = parseStar<consumeLineStyle>(starRange);
    ASSERT_TRUE(star.has_value());
    EXPECT_TRUE(std::get<CSS::List>(*star).values.empty());

    detail::CSSTokenStream plusStream("none solid");
    CSSValueRange plusRange{plusStream, {0, plusStream.tokens().size()}};
    const auto plus = parsePlus<consumeLineStyle>(plusRange);
    ASSERT_TRUE(plus.has_value());
    EXPECT_EQ(std::get<CSS::List>(*plus).values.size(), 2u);

    detail::CSSTokenStream exactStream("none solid");
    CSSValueRange exactRange{exactStream, {0, exactStream.tokens().size()}};
    const auto exact = parseExactly<2, consumeLineStyle>(exactRange);
    ASSERT_TRUE(exact.has_value());
    EXPECT_EQ(std::get<CSS::List>(*exact).values.size(), 2u);

    detail::CSSTokenStream hashStream("none, solid");
    CSSValueRange hashRange{hashStream, {0, hashStream.tokens().size()}};
    const auto hash = parseHash<consumeLineStyle>(hashRange);
    ASSERT_TRUE(hash.has_value());
    EXPECT_EQ(std::get<CSS::List>(*hash).values.size(), 2u);

    detail::CSSTokenStream hashExactStream("none, solid");
    CSSValueRange hashExactRange{hashExactStream, {0, hashExactStream.tokens().size()}};
    const auto hashExact = parseHash<2, consumeLineStyle>(hashExactRange);
    ASSERT_TRUE(hashExact.has_value());
    EXPECT_EQ(std::get<CSS::List>(*hashExact).values.size(), 2u);

    detail::CSSTokenStream hashRangeStream("none, solid, outset");
    CSSValueRange hashRangeRange{hashRangeStream, {0, hashRangeStream.tokens().size()}};
    const auto hashBounded = parseHash<{2, 3}, consumeLineStyle>(hashRangeRange);
    ASSERT_TRUE(hashBounded.has_value());
    EXPECT_EQ(std::get<CSS::List>(*hashBounded).values.size(), 3u);

    detail::CSSTokenStream requiredStream("");
    CSSValueRange requiredRange{requiredStream, {0, requiredStream.tokens().size()}};
    EXPECT_FALSE(parseRequired<parseStar<consumeLineStyle>>(requiredRange));
    EXPECT_EQ(requiredRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, ParsesGenericGrammarCombinatorsTransactionally) {
    detail::CSSTokenStream legacyStream("legacy center");
    CSSValueRange legacyRange{legacyStream, {0, legacyStream.tokens().size()}};
    const auto legacy = parseJustifyItems(legacyRange);
    ASSERT_TRUE(legacy.has_value());
    ASSERT_TRUE(std::holds_alternative<CSS::List>(*legacy));
    const auto& legacyValues = std::get<CSS::List>(*legacy).values;
    ASSERT_EQ(legacyValues.size(), 2u);
    EXPECT_EQ(std::get<CSS::Keyword>(legacyValues[0]).id, CSSKeyword::Legacy);
    EXPECT_EQ(std::get<CSS::Keyword>(legacyValues[1]).id, CSSKeyword::Center);
    EXPECT_EQ(legacyRange.range.begin, legacyRange.range.end);

    detail::CSSTokenStream keywordStream("thin");
    CSSValueRange keywordRange{keywordStream, {0, keywordStream.tokens().size()}};
    const auto keyword = parseOneOf<consumeLengthPercentage<Nonnegative>, consumeKeyword<CSSKeyword::Thin>, consumeKeyword<CSSKeyword::Medium>,
                                    consumeKeyword<CSSKeyword::Thick>>(keywordRange);
    ASSERT_TRUE(keyword.has_value());
    ASSERT_TRUE(std::holds_alternative<CSS::Keyword>(*keyword));
    EXPECT_EQ(std::get<CSS::Keyword>(*keyword).id, CSSKeyword::Thin);
    EXPECT_EQ(keywordRange.range.begin, keywordRange.range.end);

    detail::CSSTokenStream invalidStream("not-a-line-style");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    EXPECT_FALSE((parseOneOf<consumeKeyword<CSSKeyword::Solid>, consumeKeyword<CSSKeyword::Inset>>(invalidRange)));
    EXPECT_EQ(invalidRange.range.begin, 0u);

    detail::CSSTokenStream repeatedStream("none solid outset inset");
    CSSValueRange repeatedRange{repeatedStream, {0, repeatedStream.tokens().size()}};
    const auto repeated = parseRange<{1, 4}, consumeLineStyle>(repeatedRange);
    ASSERT_TRUE(repeated.has_value());
    ASSERT_TRUE(std::holds_alternative<CSS::List>(*repeated));
    EXPECT_EQ(std::get<CSS::List>(*repeated).values.size(), 4u);
    EXPECT_EQ(repeatedRange.range.begin, repeatedRange.range.end);

    detail::CSSTokenStream tooManyStream("none solid outset inset none");
    CSSValueRange tooManyRange{tooManyStream, {0, tooManyStream.tokens().size()}};
    const auto bounded = parseRange<{1, 4}, consumeLineStyle>(tooManyRange);
    ASSERT_TRUE(bounded.has_value());
    EXPECT_EQ(std::get<CSS::List>(*bounded).values.size(), 4u);
    EXPECT_NE(tooManyRange.range.begin, tooManyRange.range.end);

    detail::CSSTokenStream commaStream("red, blue");
    CSSValueRange commaRange{commaStream, {0, commaStream.tokens().size()}};
    const auto colors = parseListSeparatedBy<',', {1, Infinite}, CSSValueDetail::consumeColor>(commaRange);
    ASSERT_TRUE(colors.has_value());
    ASSERT_TRUE(std::holds_alternative<CSS::List>(*colors));
    EXPECT_EQ(std::get<CSS::List>(*colors).values.size(), 2u);
    EXPECT_EQ(commaRange.range.begin, commaRange.range.end);

    detail::CSSTokenStream anyOrderStream("inset solid");
    CSSValueRange anyOrderRange{anyOrderStream, {0, anyOrderStream.tokens().size()}};
    const auto all = parseAllAnyOrder<consumeKeyword<CSSKeyword::Solid>, consumeKeyword<CSSKeyword::Inset>>(anyOrderRange);
    ASSERT_TRUE(all.has_value());
    const auto& allValues = std::get<CSS::List>(*all).values;
    ASSERT_EQ(allValues.size(), 2u);
    EXPECT_EQ(std::get<CSS::Keyword>(allValues[0]).id, CSSKeyword::Solid);
    EXPECT_EQ(std::get<CSS::Keyword>(allValues[1]).id, CSSKeyword::Inset);

    detail::CSSTokenStream oneOrMoreStream("inset");
    CSSValueRange oneOrMoreRange{oneOrMoreStream, {0, oneOrMoreStream.tokens().size()}};
    const auto oneOrMore = parseOneOrMoreAnyOrder<consumeKeyword<CSSKeyword::Solid>, consumeKeyword<CSSKeyword::Inset>>(oneOrMoreRange);
    ASSERT_TRUE(oneOrMore.has_value());
    EXPECT_EQ(std::get<CSS::List>(*oneOrMore).values.size(), 1u);

    detail::CSSTokenStream sequenceStream("2 / 50%");
    CSSValueRange sequenceRange{sequenceStream, {0, sequenceStream.tokens().size()}};
    const auto sequence = parseSequence<consumeNumber<AnyRange>, consumeLiteral<'/'>, consumePercentage<AnyRange>>(sequenceRange);
    ASSERT_TRUE(sequence.has_value());
    const auto& sequenceValues = std::get<CSS::List>(*sequence).values;
    ASSERT_EQ(sequenceValues.size(), 2u);
    EXPECT_FLOAT_EQ(std::get<CSS::Number>(sequenceValues[0]).value, 2.f);
    EXPECT_FLOAT_EQ(std::get<CSS::Percentage>(sequenceValues[1]).value, 50.f);
    EXPECT_EQ(sequenceRange.range.begin, sequenceRange.range.end);
}

TEST(GeneratedStyleTest, ParsesFunctionAndBlockProductions) {
    detail::CSSTokenStream functionStream("CALC(2)");
    CSSValueRange functionRange{functionStream, {0, functionStream.tokens().size()}};
    const auto function = parseFunction<"calc", consumeNumber<AnyRange>>(functionRange);
    ASSERT_TRUE(function.has_value());
    const auto* functionValue = std::get_if<std::shared_ptr<const CSS::Function>>(&*function);
    ASSERT_NE(functionValue, nullptr);
    EXPECT_EQ((*functionValue)->name, "calc");
    ASSERT_EQ((*functionValue)->arguments.values.size(), 1u);
    EXPECT_FLOAT_EQ(std::get<CSS::Number>((*functionValue)->arguments.values[0]).value, 2.f);
    EXPECT_EQ(functionRange.range.begin, functionRange.range.end);

    const auto expectBlock = []<CSS::BlockType type>(const char* source) {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        const auto value = parseBlock<type, consumeNumber<AnyRange>>(range);
        ASSERT_TRUE(value.has_value());
        const auto* block = std::get_if<std::shared_ptr<const CSS::Block>>(&*value);
        ASSERT_NE(block, nullptr);
        EXPECT_EQ((*block)->type, type);
        ASSERT_EQ((*block)->values.values.size(), 1u);
        EXPECT_FLOAT_EQ(std::get<CSS::Number>((*block)->values.values[0]).value, 2.f);
        EXPECT_EQ(range.range.begin, range.range.end);
    };
    expectBlock.template operator()<CSS::BlockType::Parentheses>("(2)");
    expectBlock.template operator()<CSS::BlockType::Brackets>("[2]");
    expectBlock.template operator()<CSS::BlockType::Braces>("{2}");
}

TEST(GeneratedStyleTest, BorderRadius) {
    detail::CSSTokenStream stream("10px / 20%");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::BorderTopLeftRadius, range, parsed));
    ASSERT_EQ(range.range.begin, range.range.end);
    ASSERT_EQ(parsed.size(), 1u);
    const auto* values = std::get_if<CSS::List>(&parsed[0].value);
    ASSERT_NE(values, nullptr);
    ASSERT_EQ(values->values.size(), 2u);
    const auto* horizontal = std::get_if<CSS::LengthPercentage>(&values->values[0]);
    const auto* vertical = std::get_if<CSS::LengthPercentage>(&values->values[1]);
    ASSERT_NE(horizontal, nullptr);
    ASSERT_NE(vertical, nullptr);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue value = parsed[0].value;
    ASSERT_TRUE(applyProperty(CSSProperty::BorderTopLeftRadius, builderState, ApplyType::Value, &value));
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().horizontal.pixels, 10.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().horizontal.percent, 0.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().vertical.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().vertical.percent, 0.2f);

    detail::CSSTokenStream invalidStream("10px /");
    CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
    ParsedProperties invalidParsed;
    EXPECT_FALSE(parseProperty(CSSProperty::BorderTopLeftRadius, invalidRange, invalidParsed));
    EXPECT_TRUE(invalidParsed.empty());
    EXPECT_EQ(invalidRange.range.begin, 0u);
}

TEST(GeneratedStyleTest, ParsesTextWrapShorthandInAnyOrder) {
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
        detail::CSSTokenStream stream(test.source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        ASSERT_TRUE(parseProperty(CSSProperty::TextWrap, range, parsed));
        ASSERT_EQ(range.range.begin, range.range.end);
        ASSERT_EQ(parsed.size(), 2u);
        ASSERT_TRUE(parsed[0].property);
        ASSERT_TRUE(parsed[1].property);
        EXPECT_EQ(*parsed[0].property, CSSProperty::TextWrapMode);
        EXPECT_EQ(*parsed[1].property, CSSProperty::TextWrapStyle);

        ComputedStyle style;
        StyleBuilderState builderState{style};
        for (const ParsedLonghand& longhand : parsed) {
            ASSERT_TRUE(longhand.property);
            const StyleValue value = longhand.value;
            ASSERT_TRUE(applyProperty(*longhand.property, builderState, ApplyType::Value, &value));
        }
        EXPECT_EQ(style.textWrapMode(), test.mode);
        EXPECT_EQ(style.textWrapStyle(), test.style);
    }

    for (const char* source : {"wrap wrap", "pretty pretty", "pretty unknown"}) {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(CSSProperty::TextWrap, range, parsed));
        EXPECT_TRUE(parsed.empty());
        EXPECT_EQ(range.range.begin, 0u);
    }
}

TEST(GeneratedStyleTest, Border) {
    for (const char* source : {"solid", "2px", "#112233ff", "2px solid #112233ff", "#112233ff 2px solid"}) {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        ASSERT_TRUE(parseProperty(CSSProperty::Border, range, parsed)) << source;
        EXPECT_EQ(range.range.begin, range.range.end) << source;
        ASSERT_EQ(parsed.size(), 17u) << source;
        EXPECT_EQ(*parsed[0].property, CSSProperty::BorderTopWidth);
        EXPECT_EQ(*parsed[1].property, CSSProperty::BorderRightWidth);
        EXPECT_EQ(*parsed[2].property, CSSProperty::BorderBottomWidth);
        EXPECT_EQ(*parsed[3].property, CSSProperty::BorderLeftWidth);
        EXPECT_EQ(*parsed[4].property, CSSProperty::BorderTopStyle);
        EXPECT_EQ(*parsed[7].property, CSSProperty::BorderLeftStyle);
        EXPECT_EQ(*parsed[8].property, CSSProperty::BorderTopColor);
        EXPECT_EQ(*parsed[11].property, CSSProperty::BorderLeftColor);
        EXPECT_EQ(*parsed[12].property, CSSProperty::BorderImageSource);
        EXPECT_EQ(*parsed[13].property, CSSProperty::BorderImageSlice);
        EXPECT_EQ(*parsed[14].property, CSSProperty::BorderImageWidth);
        EXPECT_EQ(*parsed[15].property, CSSProperty::BorderImageOutset);
        EXPECT_EQ(*parsed[16].property, CSSProperty::BorderImageRepeat);
        EXPECT_EQ(cssPropertyName(*parsed[12].property), "border-image-source");
        EXPECT_EQ(cssPropertyName(*parsed[13].property), "border-image-slice");
        EXPECT_EQ(cssPropertyName(*parsed[14].property), "border-image-width");
        EXPECT_EQ(cssPropertyName(*parsed[15].property), "border-image-outset");
        EXPECT_EQ(cssPropertyName(*parsed[16].property), "border-image-repeat");
    }

    detail::CSSTokenStream omittedStream("2px");
    CSSValueRange omittedRange{omittedStream, {0, omittedStream.tokens().size()}};
    ParsedProperties omitted;
    ASSERT_TRUE(parseProperty(CSSProperty::Border, omittedRange, omitted));
    const auto expectKeyword = [](const CSSValue& value, CSSKeyword expected) {
        const auto* keyword = std::get_if<CSS::Keyword>(&value);
        ASSERT_NE(keyword, nullptr);
        EXPECT_EQ(keyword->id, expected);
    };
    expectKeyword(omitted[8].value, CSSKeyword::CurrentColor);
    expectKeyword(omitted[4].value, CSSKeyword::NoneValue);
    EXPECT_EQ(std::get<CSS::Length>(omitted[0].value), CSS::Px(2.f));

    detail::CSSTokenStream styleStream("solid");
    CSSValueRange styleRange{styleStream, {0, styleStream.tokens().size()}};
    ParsedProperties styleOnly;
    ASSERT_TRUE(parseProperty(CSSProperty::Border, styleRange, styleOnly));
    expectKeyword(styleOnly[4].value, CSSKeyword::Solid);
    expectKeyword(styleOnly[0].value, CSSKeyword::Medium);
    expectKeyword(styleOnly[8].value, CSSKeyword::CurrentColor);
    expectKeyword(styleOnly[12].value, CSSKeyword::NoneValue);
    EXPECT_EQ(std::get<CSS::Percentage>(styleOnly[13].value), CSS::Percentage(100.f));
    EXPECT_EQ(std::get<CSS::Number>(styleOnly[14].value), CSS::Number(1.f));
    EXPECT_EQ(std::get<CSS::Number>(styleOnly[15].value), CSS::Number(0.f));
    expectKeyword(styleOnly[16].value, CSSKeyword::Stretch);

    for (const char* source : {"solid inset", "2px 3px", "#112233ff #445566ff", "2px unknown"}) {
        detail::CSSTokenStream stream(source);
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        ParsedProperties parsed;

        EXPECT_FALSE(parseProperty(CSSProperty::Border, range, parsed)) << source;
        EXPECT_TRUE(parsed.empty()) << source;
        EXPECT_EQ(range.range.begin, 0u) << source;
    }
}

TEST(GeneratedStyleTest, BorderImage) {
    detail::CSSTokenStream stream("url(images/frame.png) 30 fill / 4px / 2 round space");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::BorderImage, range, parsed));
    ASSERT_EQ(parsed.size(), 5u);
    EXPECT_EQ(*parsed[0].property, CSSProperty::BorderImageSource);
    EXPECT_EQ(*parsed[1].property, CSSProperty::BorderImageSlice);
    EXPECT_EQ(*parsed[2].property, CSSProperty::BorderImageWidth);
    EXPECT_EQ(*parsed[3].property, CSSProperty::BorderImageOutset);
    EXPECT_EQ(*parsed[4].property, CSSProperty::BorderImageRepeat);

    const auto* source = std::get_if<CSS::String>(&parsed[0].value);
    ASSERT_NE(source, nullptr);
    EXPECT_EQ(source->value, "url(images/frame.png)");
    const auto* slice = std::get_if<CSS::List>(&parsed[1].value);
    ASSERT_NE(slice, nullptr);
    ASSERT_EQ(slice->values.size(), 2u);
    EXPECT_EQ(std::get<CSS::Keyword>(slice->values[1]).id, CSSKeyword::Fill);

    for (const char* invalid : {"url(images/frame.png) 30 / /", "30 / 2 /", "30 / 2px / 3px / 4px"}) {
        detail::CSSTokenStream invalidStream(invalid);
        CSSValueRange invalidRange{invalidStream, {0, invalidStream.tokens().size()}};
        ParsedProperties rejected;
        EXPECT_FALSE(parseProperty(CSSProperty::BorderImage, invalidRange, rejected)) << invalid;
        EXPECT_TRUE(rejected.empty()) << invalid;
        EXPECT_EQ(invalidRange.range.begin, 0u) << invalid;
    }
}

TEST(GeneratedStyleTest, Gap) {
    detail::CSSTokenStream stream("4px 8px");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::Gap, range, parsed));
    ASSERT_EQ(parsed.size(), 2u);
    EXPECT_EQ(*parsed[0].property, CSSProperty::RowGap);
    EXPECT_EQ(*parsed[1].property, CSSProperty::ColumnGap);
    EXPECT_EQ(std::get<CSS::LengthPercentage>(parsed[0].value), CSS::LengthPercentage{CSS::Px(4.f)});
    EXPECT_EQ(std::get<CSS::LengthPercentage>(parsed[1].value), CSS::LengthPercentage{CSS::Px(8.f)});

    detail::CSSTokenStream singleStream("4px");
    CSSValueRange singleRange{singleStream, {0, singleStream.tokens().size()}};
    ParsedProperties single;
    ASSERT_TRUE(parseProperty(CSSProperty::Gap, singleRange, single));
    ASSERT_EQ(single.size(), 2u);
    EXPECT_EQ(std::get<CSS::LengthPercentage>(single[0].value), CSS::LengthPercentage{CSS::Px(4.f)});
    EXPECT_EQ(std::get<CSS::LengthPercentage>(single[1].value), CSS::LengthPercentage{CSS::Px(4.f)});
}

TEST(GeneratedStyleTest, AppliesGeneratedBorderSideColor) {
    detail::CSSTokenStream stream("2px outset #00ff00ff");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    ParsedProperties parsed;

    ASSERT_TRUE(parseProperty(CSSProperty::BorderRight, range, parsed));
    ASSERT_EQ(parsed.size(), 3u);
    EXPECT_EQ(*parsed[2].property, CSSProperty::BorderRightColor);
    const auto* color = std::get_if<CSS::Color>(&parsed[2].value);
    ASSERT_NE(color, nullptr);
    const auto* solid = color->solidColor();
    ASSERT_NE(solid, nullptr);
    EXPECT_FLOAT_EQ(solid->g, 1.f);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue specified = parsed[2].value;
    ASSERT_TRUE(applyProperty(CSSProperty::BorderRightColor, builderState, ApplyType::Value, &specified));
    EXPECT_FLOAT_EQ(style.borderRightColor().resolvedColor().g, 1.f);
}

TEST(GeneratedStyleTest, ParsesAndAppliesAppearance) {
    detail::CSSTokenStream stream("base");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const std::optional<CSSValue> value = parseAppearance(range);

    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue parsedValue = *value;
    EXPECT_TRUE(applyProperty(CSSProperty::Appearance, builderState, ApplyType::Value, &parsedValue));
    EXPECT_EQ(style.appearance(), Appearance::Base);

    EXPECT_TRUE(applyProperty(CSSProperty::Appearance, builderState, ApplyType::Initial));
    EXPECT_EQ(style.appearance(), Appearance::NoneValue);
}

TEST(GeneratedStyleTest, AppliesLineHeight) {
    ComputedStyle detached;
    StyleBuilderState detachedState{detached};
    EXPECT_FALSE(applyProperty(CSSProperty::LineHeight, detachedState, ApplyType::Value));
    EXPECT_FALSE(applyProperty(CSSProperty::LineHeight, detachedState, ApplyType::Inherit));

    ComputedStyle parent;
    parent.setLineHeight(LineHeight{LineHeight::Number{2.f}});

    ComputedStyle style;
    StyleBuilderState builderState{style, &parent};
    EXPECT_TRUE(applyProperty(CSSProperty::LineHeight, builderState, ApplyType::Inherit));
    EXPECT_EQ(style.lineHeight(), parent.lineHeight());

    EXPECT_TRUE(applyProperty(CSSProperty::LineHeight, builderState, ApplyType::Initial));
    EXPECT_TRUE(std::holds_alternative<CSS::Keyword::Normal>(style.lineHeight().mValue));
}

TEST(GeneratedStyleTest, CopiesSharedStyleDataBeforeMutation) {
    ComputedStyle original;
    original.setLineHeight(LineHeight{LineHeight::Number{1.5f}});
    original.setFlexGrow(FlexGrow{2.f});
    original.setFlexWrap({FlexWrapMode::Wrap, true});
    original.setColorScheme(ColorScheme{false, false, {ColorSchemeMode::Dark}, {}});
    original.setMargin(
        RectEdges<MarginEdge>{MarginEdge::fromPixels(1.f), MarginEdge::automatic(), MarginEdge::fromPixels(3.f), MarginEdge::fromPixels(4.f)});
    original.setPadding(RectEdges<PaddingEdge>{PaddingEdge{5.f}, PaddingEdge{6.f}, PaddingEdge{7.f}, PaddingEdge{8.f}});

    ComputedStyle copy = original;
    copy.setLineHeight(LineHeight{LineHeight::Number{2.f}});
    copy.setFlexGrow(FlexGrow{3.f});
    copy.setFlexWrap({FlexWrapMode::WrapReverse, false});
    copy.setColorScheme(ColorScheme{false, false, {ColorSchemeMode::Light}, {}});
    copy.setMargin(RectEdges<MarginEdge>{});
    copy.setPadding(RectEdges<PaddingEdge>{});

    EXPECT_EQ(std::get<LineHeight::Number>(original.lineHeight().mValue).value, 1.5f);
    EXPECT_EQ(std::get<LineHeight::Number>(copy.lineHeight().mValue).value, 2.f);
    EXPECT_FLOAT_EQ(original.flexGrow().value, 2.f);
    EXPECT_FLOAT_EQ(copy.flexGrow().value, 3.f);
    const FlexWrap expectedOriginalWrap{FlexWrapMode::Wrap, true};
    const FlexWrap expectedCopyWrap{FlexWrapMode::WrapReverse, false};
    EXPECT_EQ(original.flexWrap(), expectedOriginalWrap);
    EXPECT_EQ(copy.flexWrap(), expectedCopyWrap);
    EXPECT_EQ(original.colorScheme().schemes, std::vector<ColorSchemeMode>{ColorSchemeMode::Dark});
    EXPECT_EQ(copy.colorScheme().schemes, std::vector<ColorSchemeMode>{ColorSchemeMode::Light});
    EXPECT_TRUE(original.margin().right.isAuto());
    EXPECT_EQ(original.margin().left.fixedPixels(), 4.f);
    EXPECT_EQ(copy.margin().left.fixedPixels(), 0.f);
    EXPECT_EQ(original.padding().right.pixels, 6.f);
    EXPECT_EQ(copy.padding().right.pixels, 0.f);
}

TEST(GeneratedStyleTest, ParsesAndAppliesFlexGrowthProperties) {
    detail::CSSTokenStream stream("2");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    const std::optional<CSSValue> value = parseFlexGrow(range);

    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue parsedValue = *value;
    EXPECT_TRUE(applyProperty(CSSProperty::FlexGrow, builderState, ApplyType::Value, &parsedValue));
    EXPECT_FLOAT_EQ(style.flexGrow().value, 2.f);

    EXPECT_TRUE(applyProperty(CSSProperty::FlexShrink, builderState, ApplyType::Initial));
    EXPECT_FLOAT_EQ(style.flexShrink().value, 1.f);
}

TEST(GeneratedStyleTest, ParsesAndAppliesOpacityValue) {
    detail::CSSTokenStream stream("50%");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    const std::optional<CSSValue> value = parseOpacity(range);

    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(range.range.begin, range.range.end);
    const auto* percentage = std::get_if<CSS::Percentage>(&*value);
    ASSERT_NE(percentage, nullptr);
    EXPECT_FLOAT_EQ(percentage->value, 50.f);

    ComputedStyle style;
    StyleBuilderState builderState{style};
    const StyleValue parsedValue = *value;
    EXPECT_TRUE(applyProperty(CSSProperty::Opacity, builderState, ApplyType::Value, &parsedValue));
    EXPECT_FLOAT_EQ(style.opacity().value, .5f);

    detail::CSSTokenStream outOfRangeStream("2");
    CSSValueRange outOfRange{outOfRangeStream, {0, outOfRangeStream.tokens().size()}};
    const std::optional<CSSValue> outOfRangeValue = parseOpacity(outOfRange);
    ASSERT_TRUE(outOfRangeValue.has_value());
    const auto* number = std::get_if<CSS::Number>(&*outOfRangeValue);
    ASSERT_NE(number, nullptr);
    EXPECT_FLOAT_EQ(number->value, 2.f);

    const StyleValue outOfRangeDeclaration = *outOfRangeValue;
    EXPECT_TRUE(applyProperty(CSSProperty::Opacity, builderState, ApplyType::Value, &outOfRangeDeclaration));
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);

    detail::CSSTokenStream percentageStream("150%");
    CSSValueRange percentageRange{percentageStream, {0, percentageStream.tokens().size()}};
    const std::optional<CSSValue> outOfRangePercentage = parseOpacity(percentageRange);
    ASSERT_TRUE(outOfRangePercentage.has_value());
    const auto* rawPercentage = std::get_if<CSS::Percentage>(&*outOfRangePercentage);
    ASSERT_NE(rawPercentage, nullptr);
    EXPECT_FLOAT_EQ(rawPercentage->value, 150.f);

    const StyleValue outOfRangePercentageDeclaration = *outOfRangePercentage;
    EXPECT_TRUE(applyProperty(CSSProperty::Opacity, builderState, ApplyType::Value, &outOfRangePercentageDeclaration));
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);
}

TEST(GeneratedStyleTest, ParsesAndAppliesFlagBackedProperties) {
    ComputedStyle style;
    StyleBuilderState builderState{style};

    detail::CSSTokenStream boxSizingStream("border-box");
    CSSValueRange boxSizingRange{boxSizingStream, {0, boxSizingStream.tokens().size()}};
    const std::optional<CSSValue> boxSizingValue = parseBoxSizing(boxSizingRange);
    ASSERT_TRUE(boxSizingValue.has_value());
    const StyleValue boxSizingDeclaration = *boxSizingValue;
    EXPECT_TRUE(applyProperty(CSSProperty::BoxSizing, builderState, ApplyType::Value, &boxSizingDeclaration));
    EXPECT_EQ(style.boxSizing(), BoxSizing::BorderBox);
    EXPECT_TRUE(applyProperty(CSSProperty::BoxSizing, builderState, ApplyType::Initial));
    EXPECT_EQ(style.boxSizing(), BoxSizing::ContentBox);

    detail::CSSTokenStream positionStream("sticky");
    CSSValueRange positionRange{positionStream, {0, positionStream.tokens().size()}};
    const std::optional<CSSValue> positionValue = parsePosition(positionRange);
    ASSERT_TRUE(positionValue.has_value());
    const StyleValue positionDeclaration = *positionValue;
    EXPECT_TRUE(applyProperty(CSSProperty::Position, builderState, ApplyType::Value, &positionDeclaration));
    EXPECT_EQ(style.position(), Position::Sticky);

    detail::CSSTokenStream visibilityStream("hidden");
    CSSValueRange visibilityRange{visibilityStream, {0, visibilityStream.tokens().size()}};
    const std::optional<CSSValue> visibilityValue = parseVisibility(visibilityRange);
    ASSERT_TRUE(visibilityValue.has_value());
    const StyleValue visibilityDeclaration = *visibilityValue;
    EXPECT_TRUE(applyProperty(CSSProperty::Visibility, builderState, ApplyType::Value, &visibilityDeclaration));
    EXPECT_EQ(style.visibility(), Visibility::Hidden);

    ComputedStyle parent;
    StyleBuilderState inheritState{style, &parent};
    parent.setVisibility(Visibility::Collapse);
    EXPECT_TRUE(applyProperty(CSSProperty::Visibility, inheritState, ApplyType::Inherit));
    EXPECT_EQ(style.visibility(), Visibility::Collapse);
}

TEST(GeneratedStyleTest, ParsesAndAppliesOverflowTextAndScrollbarProperties) {
    ComputedStyle style;
    StyleBuilderState builderState{style};

    detail::CSSTokenStream overflowXStream("hidden");
    CSSValueRange overflowXRange{overflowXStream, {0, overflowXStream.tokens().size()}};
    const std::optional<CSSValue> overflowXValue = parseOverflowX(overflowXRange);
    ASSERT_TRUE(overflowXValue.has_value());
    const StyleValue overflowXDeclaration = *overflowXValue;
    EXPECT_TRUE(applyProperty(CSSProperty::OverflowX, builderState, ApplyType::Value, &overflowXDeclaration));
    EXPECT_EQ(style.overflowX(), Overflow::Hidden);

    detail::CSSTokenStream overflowYStream("scroll");
    CSSValueRange overflowYRange{overflowYStream, {0, overflowYStream.tokens().size()}};
    const std::optional<CSSValue> overflowYValue = parseOverflowY(overflowYRange);
    ASSERT_TRUE(overflowYValue.has_value());
    const StyleValue overflowYDeclaration = *overflowYValue;
    EXPECT_TRUE(applyProperty(CSSProperty::OverflowY, builderState, ApplyType::Value, &overflowYDeclaration));
    EXPECT_EQ(style.overflowY(), Overflow::Scroll);

    detail::CSSTokenStream scrollbarWidthStream("thin");
    CSSValueRange scrollbarWidthRange{scrollbarWidthStream, {0, scrollbarWidthStream.tokens().size()}};
    const std::optional<CSSValue> scrollbarWidthValue = parseScrollbarWidth(scrollbarWidthRange);
    ASSERT_TRUE(scrollbarWidthValue.has_value());
    const StyleValue scrollbarWidthDeclaration = *scrollbarWidthValue;
    EXPECT_TRUE(applyProperty(CSSProperty::ScrollbarWidth, builderState, ApplyType::Value, &scrollbarWidthDeclaration));
    EXPECT_EQ(style.scrollbarWidth(), ScrollbarWidth::Thin);

    detail::CSSTokenStream textAlignStream("center");
    CSSValueRange textAlignRange{textAlignStream, {0, textAlignStream.tokens().size()}};
    const std::optional<CSSValue> textAlignValue = parseTextAlign(textAlignRange);
    ASSERT_TRUE(textAlignValue.has_value());
    const StyleValue textAlignDeclaration = *textAlignValue;
    EXPECT_TRUE(applyProperty(CSSProperty::TextAlign, builderState, ApplyType::Value, &textAlignDeclaration));
    EXPECT_EQ(style.textAlign(), TextAlign::Center);

    detail::CSSTokenStream textOverflowStream("ellipsis-center");
    CSSValueRange textOverflowRange{textOverflowStream, {0, textOverflowStream.tokens().size()}};
    const std::optional<CSSValue> textOverflowValue = parseTextOverflow(textOverflowRange);
    ASSERT_TRUE(textOverflowValue.has_value());
    const StyleValue textOverflowDeclaration = *textOverflowValue;
    EXPECT_TRUE(applyProperty(CSSProperty::TextOverflow, builderState, ApplyType::Value, &textOverflowDeclaration));
    EXPECT_EQ(style.textOverflow(), TextOverflow::EllipsisCenter);
}

TEST(GeneratedStyleTest, RejectsNegativeLineHeightWithoutConsumingInput) {
    detail::CSSTokenStream stream("-1");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    EXPECT_FALSE(parseLineHeight(range).has_value());
    EXPECT_EQ(range.range.begin, 0u);
}

TEST(GeneratedStyleTest, KeepsCSSLengthUnitsDistinct) {
    const CSS::Length length = CSS::Em(1.f);
    EXPECT_EQ(length.unit, CSS::LengthUnit::Em);
    EXPECT_FLOAT_EQ(length.value, 1.f);

    const CSSValue value = CSS::LengthPercentage{length};
    const auto* lengthPercentage = std::get_if<CSS::LengthPercentage>(&value);
    ASSERT_NE(lengthPercentage, nullptr);
    const auto* nestedLength = std::get_if<CSS::Length>(lengthPercentage);
    ASSERT_NE(nestedLength, nullptr);
    EXPECT_EQ(nestedLength->unit, CSS::LengthUnit::Em);
}

TEST(GeneratedStyleTest, PrimitiveParsersConsumeOneValueAndFollowingTrivia) {
    detail::CSSTokenStream stream(" \t1Em \n solid red");
    CSSValueRange range{stream, {0, stream.tokens().size()}};

    const std::optional<CSS::Length> length = consumeLength<Nonnegative>(range);

    ASSERT_TRUE(length.has_value());
    EXPECT_EQ(length->unit, CSS::LengthUnit::Em);
    EXPECT_FLOAT_EQ(length->value, 1.f);
    ASSERT_LT(range.range.begin, range.range.end);
    EXPECT_EQ(stream.text(range.range.begin), "solid");

    detail::CSSTokenStream percentageStream("50% 10px");
    CSSValueRange percentageRange{percentageStream, {0, percentageStream.tokens().size()}};
    const std::optional<CSS::Percentage> percentage = consumePercentage<AnyRange>(percentageRange);

    ASSERT_TRUE(percentage.has_value());
    EXPECT_FLOAT_EQ(percentage->value, 50.f);
    ASSERT_LT(percentageRange.range.begin, percentageRange.range.end);
    EXPECT_EQ(percentageStream.text(percentageRange.range.begin), "10px");
}

TEST(GeneratedStyleTest, PreservesPercentageValue) {
    detail::CSSTokenStream stream("50%");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    const std::optional<CSS::LengthPercentage> parsed = consumeLengthPercentage<Nonnegative>(range);

    ASSERT_TRUE(parsed.has_value());
    const auto* percentage = std::get_if<CSS::Percentage>(&*parsed);
    ASSERT_NE(percentage, nullptr);
    EXPECT_FLOAT_EQ(percentage->value, 50.f);
}

TEST(GeneratedStyleTest, ParsesEveryCSSLengthUnitInLengthPercentage) {
    using Check = bool (*)(const CSS::Length&);
    struct LengthCase {
        std::string_view source;
        Check check;
    };
    constexpr LengthCase cases[] = {
        {"px", hasLengthUnit<CSS::LengthUnit::Px>},       {"em", hasLengthUnit<CSS::LengthUnit::Em>},
        {"rem", hasLengthUnit<CSS::LengthUnit::Rem>},     {"ch", hasLengthUnit<CSS::LengthUnit::Ch>},
        {"ex", hasLengthUnit<CSS::LengthUnit::Ex>},       {"cap", hasLengthUnit<CSS::LengthUnit::Cap>},
        {"ic", hasLengthUnit<CSS::LengthUnit::Ic>},       {"lh", hasLengthUnit<CSS::LengthUnit::Lh>},
        {"rlh", hasLengthUnit<CSS::LengthUnit::Rlh>},     {"vw", hasLengthUnit<CSS::LengthUnit::Vw>},
        {"vh", hasLengthUnit<CSS::LengthUnit::Vh>},       {"vmin", hasLengthUnit<CSS::LengthUnit::Vmin>},
        {"vmax", hasLengthUnit<CSS::LengthUnit::Vmax>},   {"vi", hasLengthUnit<CSS::LengthUnit::Vi>},
        {"vb", hasLengthUnit<CSS::LengthUnit::Vb>},       {"svw", hasLengthUnit<CSS::LengthUnit::Svw>},
        {"svh", hasLengthUnit<CSS::LengthUnit::Svh>},     {"svmin", hasLengthUnit<CSS::LengthUnit::Svmin>},
        {"svmax", hasLengthUnit<CSS::LengthUnit::Svmax>}, {"svi", hasLengthUnit<CSS::LengthUnit::Svi>},
        {"svb", hasLengthUnit<CSS::LengthUnit::Svb>},     {"lvw", hasLengthUnit<CSS::LengthUnit::Lvw>},
        {"lvh", hasLengthUnit<CSS::LengthUnit::Lvh>},     {"lvmin", hasLengthUnit<CSS::LengthUnit::Lvmin>},
        {"lvmax", hasLengthUnit<CSS::LengthUnit::Lvmax>}, {"lvi", hasLengthUnit<CSS::LengthUnit::Lvi>},
        {"lvb", hasLengthUnit<CSS::LengthUnit::Lvb>},     {"dvw", hasLengthUnit<CSS::LengthUnit::Dvw>},
        {"dvh", hasLengthUnit<CSS::LengthUnit::Dvh>},     {"dvmin", hasLengthUnit<CSS::LengthUnit::Dvmin>},
        {"dvmax", hasLengthUnit<CSS::LengthUnit::Dvmax>}, {"dvi", hasLengthUnit<CSS::LengthUnit::Dvi>},
        {"dvb", hasLengthUnit<CSS::LengthUnit::Dvb>},     {"cm", hasLengthUnit<CSS::LengthUnit::Cm>},
        {"mm", hasLengthUnit<CSS::LengthUnit::Mm>},       {"q", hasLengthUnit<CSS::LengthUnit::Q>},
        {"in", hasLengthUnit<CSS::LengthUnit::In>},       {"pt", hasLengthUnit<CSS::LengthUnit::Pt>},
        {"pc", hasLengthUnit<CSS::LengthUnit::Pc>},
    };

    for (const LengthCase& lengthCase : cases) {
        detail::CSSTokenStream stream(std::string("1") + std::string(lengthCase.source));
        CSSValueRange range{stream, {0, stream.tokens().size()}};
        const std::optional<CSS::LengthPercentage> parsed = consumeLengthPercentage<Nonnegative>(range);

        SCOPED_TRACE(lengthCase.source);
        ASSERT_TRUE(parsed.has_value());
        const auto* length = std::get_if<CSS::Length>(&*parsed);
        ASSERT_NE(length, nullptr);
        EXPECT_TRUE(lengthCase.check(*length));
        EXPECT_EQ(range.range.begin, range.range.end);
    }

    detail::CSSTokenStream stream("0");
    CSSValueRange range{stream, {0, stream.tokens().size()}};
    const std::optional<CSS::LengthPercentage> parsed = consumeLengthPercentage<Nonnegative>(range);
    ASSERT_TRUE(parsed.has_value());
    const auto* length = std::get_if<CSS::Length>(&*parsed);
    ASSERT_NE(length, nullptr);
    EXPECT_EQ(length->unit, CSS::LengthUnit::Px);
}

TEST(LayoutGeometryTest, UsesZeroWidthForNoneBorderStyle) {
    ComputedStyle style;
    EXPECT_FLOAT_EQ(style.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(borderWidths(style), RectEdges<float>{});

    style.setBorderWidth({LineWidth{1.f}, LineWidth{2.f}, LineWidth{3.f}, LineWidth{4.f}});
    style.setBorderStyle({BorderStyle::Solid, BorderStyle::NoneValue, BorderStyle::Outset, BorderStyle::Inset});
    const RectEdges<float> expectedWidths{1.f, 0.f, 3.f, 4.f};
    EXPECT_EQ(borderWidths(style), expectedWidths);
}

TEST(LayoutGeometryTest, MarginEdges) {
    const RectEdges<MarginEdge> margin{MarginEdge::fromPixels(1.f), MarginEdge::automatic(), MarginEdge::fromPixels(3.f),
                                       MarginEdge::fromPixels(-4.f)};

    EXPECT_FLOAT_EQ(horizontalMargin(margin), -4.f);
    EXPECT_FLOAT_EQ(verticalMargin(margin), 4.f);
    EXPECT_EQ(horizontalAutoMarginCount(margin), 1);
    EXPECT_EQ(verticalAutoMarginCount(margin), 0);
}
} // namespace radia::ui
