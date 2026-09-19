/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"
#include "dom/elementinternal.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/elementfactory.h"
#include "html/floater.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "resource/elementdefinition.h"
#include "style/property.h"
#include "style/stylepass.h"
#include "text/metrics.h"

namespace {
using radia::ui::AppearanceMode;
using radia::ui::BoxSizing;
using radia::ui::Color;
using radia::ui::ColorScheme;
using radia::ui::ColorSchemeValue;
using radia::ui::ComputedStyle;
using radia::ui::ContentDistribution;
using radia::ui::ContentPosition;
using radia::ui::DimensionKeyword;
using radia::ui::DisplayMode;
using radia::ui::Element;
using radia::ui::ElementState;
using radia::ui::FixedTextMetrics;
using radia::ui::FlexDirection;
using radia::ui::FlexWrap;
using radia::ui::FontFamily;
using radia::ui::GradientKind;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::ItemPosition;
using radia::ui::LineHeight;
using radia::ui::OverflowAlignment;
using radia::ui::PointerEvents;
using radia::ui::PositionMode;
using radia::ui::StrokeCap;
using radia::ui::StylePass;
using radia::ui::StyleSheet;
using radia::ui::TextAlign;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::TextWrapStyle;
using radia::ui::VerticalAlign;
using radia::ui::Visibility;
using radia::ui::detail::CSSTokenKind;
using radia::ui::detail::CSSTokenStream;
using radia::ui::detail::ElementInternalAccess;
using radia::ui::detail::findStyleProperty;
using radia::ui::detail::HTMLElementFactory;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using radia::ui::detail::matchingBlock;
using radia::ui::detail::serializeCSSRange;
using radia::ui::detail::splitTopLevel;
using radia::ui::detail::stylePropertyBegin;
using radia::ui::detail::StylePropertyDefinition;
using radia::ui::detail::stylePropertyEnd;
using radia::ui::detail::tokenizeTopLevel;
using ::testing::Message;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    StylePass styles(stylesheet, FixedTextMetrics{});
    return styles.style(element);
}

Element& appendIcon(HTMLButtonElement& button, std::string name) {
    auto icon = makeElement<Element>("i");
    Element* result = icon.get();
    result->classList().add("i-" + name);
    button.append(std::move(icon));
    return *result;
}

} // namespace

TEST(StyleCompilerTest, ResolvesStructuralDivStyles) {
    constexpr char kDivStyles[] = "div.stack { display: flex; flex-direction: row; gap: 8px; padding: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDivStyles).ok());

    const auto* definition = radia::ui::findElementDefinition(radia::ui::HTMLTag::Div);
    ASSERT_NE(definition, nullptr);
    auto div = HTMLElementFactory::create("div");
    ASSERT_NE(div, nullptr);
    div->classList().add("stack");
    const ComputedStyle style = computedStyle(stylesheet, *div);
    EXPECT_EQ(style.display, DisplayMode::Flex);
    EXPECT_TRUE(style.displaySet);
    EXPECT_EQ(style.flexDirection, FlexDirection::Row);
    EXPECT_EQ(style.rowGap.fixedPixels(), 8.f);
    EXPECT_EQ(style.padding.top, 2.f);
}

TEST(StyleCompilerTest, ResolvesVisibility) {
    constexpr char kDisplayStyles[] = "panel.flex { display: flex; flex-direction: column; } "
                                      "panel.inline { display: inline; } panel.inline-flex { display: inline-flex; } "
                                      "panel.none { display: none; } "
                                      "panel.hidden { visibility: hidden; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDisplayStyles).ok());

    auto flex = makeElementValue<HTMLPanelElement>();
    flex.classList().add("flex");
    const ComputedStyle flexStyle = computedStyle(stylesheet, flex);
    EXPECT_EQ(flexStyle.display, DisplayMode::Flex);
    EXPECT_EQ(flexStyle.flexDirection, FlexDirection::Column);

    auto inlinePanel = makeElementValue<HTMLPanelElement>();
    inlinePanel.classList().add("inline");
    const ComputedStyle inlineStyle = computedStyle(stylesheet, inlinePanel);
    EXPECT_EQ(inlineStyle.display, DisplayMode::Inline);

    auto inlineFlexPanel = makeElementValue<HTMLPanelElement>();
    inlineFlexPanel.classList().add("inline-flex");
    EXPECT_EQ(computedStyle(stylesheet, inlineFlexPanel).display, DisplayMode::InlineFlex);

    auto none = makeElementValue<HTMLPanelElement>();
    none.classList().add("none");
    EXPECT_EQ(computedStyle(stylesheet, none).display, DisplayMode::NoneValue);

    auto hidden = makeElementValue<HTMLPanelElement>();
    hidden.classList().add("hidden");
    auto child = makeElement<HTMLLabelElement>("child");
    HTMLLabelElement* childPtr = child.get();
    hidden.append(std::move(child));
    EXPECT_EQ(computedStyle(stylesheet, *childPtr).visibility, Visibility::Hidden);
}

TEST(StyleCompilerTest, DecodesEscapedDeclarationKeywords) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { display: n\\6f ne; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).display, DisplayMode::NoneValue);
}

TEST(StyleCompilerTest, ParsesColorSchemeValues) {
    constexpr char kColorSchemeStyles[] = "panel { color-scheme: light; } panel.dark { color-scheme: dark; } "
                                          "panel.both { color-scheme: light dark; } panel.reset { color-scheme: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kColorSchemeStyles).ok());

    const ColorSchemeValue light = stylesheet.resolve("panel", "", {}, 0).colorScheme;
    const ColorSchemeValue dark = stylesheet.resolve("panel", "", {"dark"}, 0).colorScheme;
    const ColorSchemeValue both = stylesheet.resolve("panel", "", {"both"}, 0).colorScheme;
    const ColorSchemeValue reset = stylesheet.resolve("panel", "", {"reset"}, 0).colorScheme;
    ASSERT_FALSE(light.normal);
    ASSERT_EQ(light.schemes.size(), std::size_t{1});
    EXPECT_EQ(light.schemes.front(), ColorScheme::Light);
    ASSERT_FALSE(dark.normal);
    ASSERT_EQ(dark.schemes.size(), std::size_t{1});
    EXPECT_EQ(dark.schemes.front(), ColorScheme::Dark);
    ASSERT_FALSE(both.normal);
    ASSERT_EQ(both.schemes.size(), std::size_t{2});
    EXPECT_EQ(both.schemes[0], ColorScheme::Light);
    EXPECT_EQ(both.schemes[1], ColorScheme::Dark);
    EXPECT_TRUE(reset.normal);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { color-scheme: light light; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(invalid.resolve("panel", "", {}, 0).colorScheme.schemes, std::vector<ColorScheme>{ColorScheme::Light});

    const ColorSchemeValue normal = reset;
    EXPECT_TRUE(normal.normal);
    EXPECT_EQ(normal.used({ColorScheme::Light, std::nullopt, false}), ColorScheme::Light);
    EXPECT_EQ(normal.used({ColorScheme::Dark, std::nullopt, false}), ColorScheme::Dark);
    EXPECT_EQ(normal.used({std::nullopt, std::nullopt, false}), ColorScheme::Dark);
    EXPECT_EQ(both.used({ColorScheme::Light, std::nullopt, false}), ColorScheme::Light);
    EXPECT_EQ(both.used({ColorScheme::Dark, std::nullopt, false}), ColorScheme::Light);
    EXPECT_EQ(both.used({ColorScheme::Light, ColorScheme::Dark, false}), ColorScheme::Dark);

    StyleSheet custom;
    ASSERT_TRUE(custom.loadRadia("panel { color-scheme: only LIGHT custom-theme; }").ok());
    const ColorSchemeValue customValue = custom.resolve("panel", "", {}, 0).colorScheme;
    EXPECT_TRUE(customValue.only);
    EXPECT_EQ(customValue.schemes, std::vector<ColorScheme>{ColorScheme::Light});
    EXPECT_EQ(customValue.customIdentifiers, std::vector<std::string>{"custom-theme"});

    StyleSheet customOnly;
    ASSERT_TRUE(customOnly.loadRadia("panel { color-scheme: custom-theme; }").ok());
    const ColorSchemeValue customOnlyValue = customOnly.resolve("panel", "", {}, 0).colorScheme;
    EXPECT_EQ(customOnlyValue.used({ColorScheme::Dark, std::nullopt, false}), ColorScheme::Dark);

    StyleSheet invalidToken;
    const auto invalidTokenResult = invalidToken.loadRadia("panel { color-scheme: light url(theme); }");
    ASSERT_TRUE(invalidTokenResult.ok());
    ASSERT_FALSE(invalidTokenResult.warnings.empty());
    EXPECT_EQ(invalidTokenResult.warnings.front().code, "stylesheet.property.value_invalid");

    StyleSheet customKeywords;
    ASSERT_TRUE(customKeywords.loadRadia("panel { color-scheme: auto none; }").ok());
    EXPECT_EQ(customKeywords.resolve("panel", "", {}, 0).colorScheme.customIdentifiers, std::vector<std::string>({"auto", "none"}));
}

TEST(StyleCompilerTest, ParsesBoxSizingValues) {
    constexpr char kBoxSizingStyles[] = "panel { box-sizing: border-box; } panel.content { box-sizing: content-box; } "
                                        "panel.reset { box-sizing: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxSizingStyles).ok());

    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).boxSizing, BoxSizing::BorderBox);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content"}, 0).boxSizing, BoxSizing::ContentBox);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"reset"}, 0).boxSizing, BoxSizing::ContentBox);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { box-sizing: padding-box; }");
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompilerTest, ResolvesSchemeColors) {
    constexpr char kLightDarkStyles[] =
        "panel { color-scheme: light; background-color: light-dark(#ffffff, #000000); color: light-dark(#101010, #f0f0f0); "
        "border: 1px solid light-dark(#cccccc, #333333); } panel.dark { color-scheme: dark; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLightDarkStyles).ok());

    const ComputedStyle light = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_NEAR(light.backgroundColor.r, 1.f, 1.0e-4f);
    EXPECT_NEAR(light.color.r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(light.borderColor.r, 204.f / 255.f, 1.0e-4f);

    const ComputedStyle dark = stylesheet.resolve("panel", "", {"dark"}, 0);
    EXPECT_FLOAT_EQ(dark.backgroundColor.r, 0.f);
    EXPECT_NEAR(dark.color.r, 240.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(dark.borderColor.r, 51.f / 255.f, 1.0e-4f);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { color: light-dark(#fff); }");
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompilerTest, ResolvesStyleTokens) {
    constexpr char kTokenStyles[] = ":root { --accent: #204060ff; --space: 12px; } "
                                    "button { background-color: var(--accent); padding: var(--space); "
                                    "border-radius: 5px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTokenStyles).ok());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, 0);

    EXPECT_NEAR(style.backgroundColor.b, 96.f / 255.f, 1.0e-4f);
    EXPECT_EQ(style.padding.left, 12.f);
    EXPECT_EQ(style.borderRadius.topLeft.horizontal.pixels, 5.f);
    EXPECT_EQ(style.borderRadius.topLeft.vertical.pixels, 5.f);
}

TEST(StyleCompilerTest, ResolvesPercentageBorderRadius) {
    constexpr char kPercentageRadiusStyles[] = "input { border-radius: 100%; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kPercentageRadiusStyles).ok());
    const ComputedStyle style = stylesheet.resolve("input", "", {}, 0);

    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.resolve(20.f), 20.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.resolve(20.f), 20.f);
}

TEST(StyleCompilerTest, PreservesShorthandValues) {
    constexpr char kDeclarationOrderStyles[] = "button { width: 10px; size: 20px 30px; max-size: 15px 25px; width: 40px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDeclarationOrderStyles).ok());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, 0);

    EXPECT_EQ(style.width.pixels(), 40.f);
    EXPECT_EQ(style.height.pixels(), 20.f);
    ASSERT_TRUE(style.maxWidth.has_value());
    ASSERT_TRUE(style.maxHeight.has_value());
    EXPECT_EQ(style.maxWidth->pixels(), 25.f);
    EXPECT_EQ(style.maxHeight->pixels(), 15.f);
}

TEST(StyleCompilerTest, AppliesSelectorSpecificity) {
    constexpr char kSpecificityStyles[] = "button.primary { width: 30px; } button { width: 10px; } "
                                          "#save { width: 50px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSpecificityStyles).ok());
    const std::set<std::string> classes{"primary"};

    EXPECT_EQ(stylesheet.resolve("button", "save", classes, 0).width.pixels(), 50.f);
    EXPECT_EQ(stylesheet.resolve("button", "", classes, 0).width.pixels(), 30.f);
}

TEST(StyleCompilerTest, ResolvesNestedSelectors) {
    constexpr char kNestedStyles[] = "button { background-color: #101010ff; &:hover { background-color: #202020ff; } "
                                     "> i { size: 16px; } &:hover > i { stroke-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kNestedStyles).ok());
    const uint16_t hover = static_cast<uint16_t>(ElementState::Hovered) | static_cast<uint16_t>(ElementState::Default);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, hover).backgroundColor.r, 32.f / 255.f);

    auto button = makeElementValue<HTMLButtonElement>();
    ElementInternalAccess::setState(button, ElementState::Hovered, true);
    const ComputedStyle iconStyle = computedStyle(stylesheet, appendIcon(button, "search"));
    EXPECT_EQ(iconStyle.width.pixels(), 16.f);
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 3.f);
}

TEST(StyleCompilerTest, ParsesAlignmentEnums) {
    constexpr char kAlignmentStyles[] = "panel { display: flex; flex-direction: row; vertical-align: middle; pointer-events: none; } "
                                        "label { text-align: right; pointer-events: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kAlignmentStyles).ok());
    const ComputedStyle panel = stylesheet.resolve("panel", "", {}, 0);
    const ComputedStyle label = stylesheet.resolve("label", "", {}, 0);

    EXPECT_EQ(panel.display, DisplayMode::Flex);
    EXPECT_EQ(panel.pointerEvents, PointerEvents::NoneValue);
    EXPECT_EQ(label.textAlign, TextAlign::Right);
    EXPECT_EQ(panel.verticalAlign, VerticalAlign::Middle);
    EXPECT_EQ(label.verticalAlign, VerticalAlign::Top);
    EXPECT_EQ(label.pointerEvents, PointerEvents::Auto);
}

TEST(StyleCompilerTest, ParsesLogicalAlignment) {
    constexpr char kCrossAxisStyles[] = "label { text-align: start; } panel { align-items: end; } "
                                        "panel.normal { align-items: normal; } panel.flex-start { align-items: flex-start; } "
                                        "button { align-self: start; } button.auto { align-self: auto; } button.flex-end { align-self: flex-end; } "
                                        "panel.justify { justify-content: normal; } panel.justify-start { justify-content: start; } "
                                        "panel.justify-flex-start { justify-content: flex-start; } panel.justify-end { justify-content: end; } "
                                        "panel.justify-flex-end { justify-content: flex-end; } panel.content-start { align-content: start; } "
                                        "panel.content-flex-start { align-content: flex-start; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCrossAxisStyles).ok());

    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).textAlign, TextAlign::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).alignItems.position, ItemPosition::End);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).alignSelf.position, ItemPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"normal"}, 0).alignItems.position, ItemPosition::Normal);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"flex-start"}, 0).alignItems.position, ItemPosition::FlexStart);
    EXPECT_EQ(stylesheet.resolve("button", "", {"auto"}, 0).alignSelf.position, ItemPosition::Auto);
    EXPECT_EQ(stylesheet.resolve("button", "", {"flex-end"}, 0).alignSelf.position, ItemPosition::FlexEnd);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify"}, 0).justifyContent.position, ContentPosition::Normal);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-start"}, 0).justifyContent.position, ContentPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-flex-start"}, 0).justifyContent.position, ContentPosition::FlexStart);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-end"}, 0).justifyContent.position, ContentPosition::End);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-flex-end"}, 0).justifyContent.position, ContentPosition::FlexEnd);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content-start"}, 0).alignContent.position, ContentPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content-flex-start"}, 0).alignContent.position, ContentPosition::FlexStart);
}

TEST(StyleCompilerTest, ParsesGridSelfAlignment) {
    constexpr char kGridAlignmentStyles[] = "input { justify-self: center; } button.start { justify-self: start; } "
                                            "button.end { justify-self: end; } label.stretch { justify-self: stretch; } "
                                            "label.auto { justify-self: auto; } label.anchor { justify-self: anchor-center; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kGridAlignmentStyles).ok());

    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0).justifySelf.position, ItemPosition::Center);
    EXPECT_EQ(stylesheet.resolve("button", "", {"start"}, 0).justifySelf.position, ItemPosition::Start);
    EXPECT_EQ(stylesheet.resolve("button", "", {"end"}, 0).justifySelf.position, ItemPosition::End);
    EXPECT_EQ(stylesheet.resolve("label", "", {"stretch"}, 0).justifySelf.position, ItemPosition::Stretch);
    EXPECT_EQ(stylesheet.resolve("label", "", {"auto"}, 0).justifySelf.position, ItemPosition::Auto);
    EXPECT_EQ(stylesheet.resolve("label", "", {"anchor"}, 0).justifySelf.position, ItemPosition::AnchorCenter);
}

TEST(StyleCompilerTest, ParsesStandardsLayoutExtensions) {
    constexpr char kStyles[] = "panel { flex-flow: row-reverse wrap-reverse; justify-content: safe center; align-items: last baseline; "
                               "justify-items: unsafe self-end; align-content: stretch; } "
                               "panel.place { place-content: safe center space-between; place-items: center start; place-self: end safe center; } "
                               "label { position: sticky; width: min-content; height: fit-content; flex-basis: max-content; text-wrap: wrap balance; "
                               "vertical-align: 25%; color-scheme: only light custom-theme; } label.pretty { text-wrap-style: pretty; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle panel = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(panel.flexDirection, FlexDirection::RowReverse);
    EXPECT_EQ(panel.flexWrap, FlexWrap::WrapReverse);
    EXPECT_FALSE(panel.flexWrapBalance);
    EXPECT_EQ(panel.justifyContent.position, ContentPosition::Center);
    EXPECT_EQ(panel.justifyContent.overflow, OverflowAlignment::Safe);
    EXPECT_EQ(panel.alignItems.position, ItemPosition::LastBaseline);
    EXPECT_EQ(panel.alignItems.overflow, OverflowAlignment::Default);
    EXPECT_EQ(panel.justifyItems.position, ItemPosition::SelfEnd);
    EXPECT_EQ(panel.justifyItems.overflow, OverflowAlignment::Unsafe);

    const ComputedStyle place = stylesheet.resolve("panel", "", {"place"}, 0);
    EXPECT_EQ(place.alignContent.position, ContentPosition::Center);
    EXPECT_EQ(place.alignContent.overflow, OverflowAlignment::Safe);
    EXPECT_EQ(place.justifyContent.distribution, ContentDistribution::SpaceBetween);
    EXPECT_EQ(place.alignItems.position, ItemPosition::Center);
    EXPECT_EQ(place.justifyItems.position, ItemPosition::Start);
    EXPECT_EQ(place.alignSelf.position, ItemPosition::End);
    EXPECT_EQ(place.justifySelf.position, ItemPosition::Center);
    EXPECT_EQ(place.justifySelf.overflow, OverflowAlignment::Safe);

    StyleSheet invalidBaseline;
    const auto invalidBaselineResult = invalidBaseline.loadRadia("panel { align-items: last-baseline; } ");
    ASSERT_TRUE(invalidBaselineResult.ok());
    EXPECT_FALSE(invalidBaselineResult.warnings.empty());
    EXPECT_EQ(invalidBaseline.resolve("panel", "", {}, 0).alignItems.position, ItemPosition::Normal);

    const ComputedStyle label = stylesheet.resolve("label", "", {}, 0);
    EXPECT_EQ(label.position, PositionMode::Sticky);
    ASSERT_TRUE(label.width.isIntrinsic());
    EXPECT_EQ(label.width.intrinsicKeyword(), DimensionKeyword::MinContent);
    EXPECT_EQ(label.height.intrinsicKeyword(), DimensionKeyword::FitContent);
    EXPECT_EQ(label.flexBasis.intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(label.textWrap, TextWrap::Wrap);
    EXPECT_EQ(label.textWrapStyle, TextWrapStyle::Balance);
    EXPECT_EQ(stylesheet.resolve("label", "", {"pretty"}, 0).textWrapStyle, TextWrapStyle::Pretty);
    EXPECT_EQ(label.verticalAlign, VerticalAlign::Percentage);
    EXPECT_TRUE(label.colorScheme.only);
    ASSERT_EQ(label.colorScheme.schemes.size(), std::size_t{1});
    EXPECT_EQ(label.colorScheme.schemes.front(), ColorScheme::Light);
    ASSERT_EQ(label.colorScheme.customIdentifiers.size(), std::size_t{1});
    EXPECT_EQ(label.colorScheme.customIdentifiers.front(), "custom-theme");

    StyleSheet balanced;
    ASSERT_TRUE(balanced
                    .loadRadia(".wrap { flex-wrap: wrap balance; } .reverse { flex-wrap: wrap-reverse balance; } .alone { flex-wrap: balance; } "
                               ".flow { flex-flow: row wrap balance; } .flow-alone { flex-flow: balance; }")
                    .ok());
    const ComputedStyle wrap = balanced.resolve("panel", "", {"wrap"}, 0);
    EXPECT_EQ(wrap.flexWrap, FlexWrap::Wrap);
    EXPECT_TRUE(wrap.flexWrapBalance);
    const ComputedStyle reverse = balanced.resolve("panel", "", {"reverse"}, 0);
    EXPECT_EQ(reverse.flexWrap, FlexWrap::WrapReverse);
    EXPECT_TRUE(reverse.flexWrapBalance);
    const ComputedStyle alone = balanced.resolve("panel", "", {"alone"}, 0);
    EXPECT_EQ(alone.flexWrap, FlexWrap::Wrap);
    EXPECT_TRUE(alone.flexWrapBalance);
    const ComputedStyle flow = balanced.resolve("panel", "", {"flow"}, 0);
    EXPECT_EQ(flow.flexDirection, FlexDirection::Row);
    EXPECT_EQ(flow.flexWrap, FlexWrap::Wrap);
    EXPECT_TRUE(flow.flexWrapBalance);
    const ComputedStyle flowAlone = balanced.resolve("panel", "", {"flow-alone"}, 0);
    EXPECT_EQ(flowAlone.flexWrap, FlexWrap::Wrap);
    EXPECT_TRUE(flowAlone.flexWrapBalance);

    StyleSheet intrinsicSizes;
    ASSERT_TRUE(intrinsicSizes
                    .loadRadia("label { size: min-content max-content; min-size: fit-content min-content; "
                               "max-size: max-content fit-content; } button { min-width: max-content; max-height: min-content; }")
                    .ok());
    const ComputedStyle sized = intrinsicSizes.resolve("label", "", {}, 0);
    EXPECT_EQ(sized.height.intrinsicKeyword(), DimensionKeyword::MinContent);
    EXPECT_EQ(sized.width.intrinsicKeyword(), DimensionKeyword::MaxContent);
    ASSERT_TRUE(sized.minHeight.has_value());
    ASSERT_TRUE(sized.minWidth.has_value());
    EXPECT_EQ(sized.minHeight->intrinsicKeyword(), DimensionKeyword::FitContent);
    EXPECT_EQ(sized.minWidth->intrinsicKeyword(), DimensionKeyword::MinContent);
    ASSERT_TRUE(sized.maxHeight.has_value());
    ASSERT_TRUE(sized.maxWidth.has_value());
    EXPECT_EQ(sized.maxHeight->intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(sized.maxWidth->intrinsicKeyword(), DimensionKeyword::FitContent);
    const ComputedStyle longhands = intrinsicSizes.resolve("button", "", {}, 0);
    ASSERT_TRUE(longhands.minWidth.has_value());
    ASSERT_TRUE(longhands.maxHeight.has_value());
    EXPECT_EQ(longhands.minWidth->intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(longhands.maxHeight->intrinsicKeyword(), DimensionKeyword::MinContent);

    StyleSheet contentBasis;
    ASSERT_TRUE(contentBasis.loadRadia("label { flex-basis: content; }").ok());
    EXPECT_EQ(contentBasis.resolve("label", "", {}, 0).flexBasis.intrinsicKeyword(), DimensionKeyword::Content);

    StyleSheet invalidContent;
    ASSERT_TRUE(invalidContent.loadRadia("label { width: content; height: content; }").ok());
    EXPECT_TRUE(invalidContent.resolve("label", "", {}, 0).width.isAuto());
    EXPECT_TRUE(invalidContent.resolve("label", "", {}, 0).height.isAuto());
}

TEST(StyleCompilerTest, ParsesTypographyProperties) {
    constexpr char kTypographyStyles[] = "label#a { font-family: sans-serif; font-size: 19px; "
                                         "font-weight: bold; font-style: italic; }";
    constexpr char kVariableWeightStyles[] = "label { font-weight: 525; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypographyStyles).ok());
    const ComputedStyle label = stylesheet.resolve("label", "a", {}, 0);
    EXPECT_EQ(label.fontFamily, FontFamily::SansSerif);
    EXPECT_EQ(label.fontSize, 19.f);
    EXPECT_EQ(label.fontWeight, static_cast<U16>(700));
    EXPECT_TRUE(label.fontItalic);

    StyleSheet variableWeight;
    ASSERT_TRUE(variableWeight.loadRadia(kVariableWeightStyles).ok());
    EXPECT_EQ(variableWeight.resolve("label", "", {}, 0).fontWeight, static_cast<U16>(525));
}

TEST(StyleCompilerTest, ExpandsInitialValues) {
    constexpr char kInitialStyles[] = "panel { display: flex; margin: 4px; padding: 5px; size: 20px 30px; min-size: 2px 3px; "
                                      "flex: 2 3 4px; overflow: hidden; font: italic 700 21px/25px sans-serif; color: #abcdef; } "
                                      "panel.reset { display: initial; margin: initial; padding: initial; size: initial; min-size: initial; "
                                      "flex: initial; overflow: initial; font: initial; color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    auto reset = makeElementValue<HTMLPanelElement>();
    reset.classList().add("reset");
    const ComputedStyle style = computedStyle(stylesheet, reset);

    EXPECT_EQ(style.display, DisplayMode::Inline);
    EXPECT_TRUE(style.displaySet);
    EXPECT_EQ(style.margin.horizontal(), 0.f);
    EXPECT_EQ(style.padding.horizontal(), 0.f);
    EXPECT_TRUE(style.width.isAuto());
    EXPECT_TRUE(style.height.isAuto());
    EXPECT_FALSE(style.minWidth.has_value());
    EXPECT_FALSE(style.minHeight.has_value());
    EXPECT_EQ(style.flexGrow, 0.f);
    EXPECT_EQ(style.flexShrink, 1.f);
    EXPECT_TRUE(style.flexBasis.isAuto());
    EXPECT_EQ(style.overflowX, radia::ui::Overflow::Visible);
    EXPECT_EQ(style.overflowY, radia::ui::Overflow::Visible);
    EXPECT_FALSE(style.fontItalic);
    EXPECT_EQ(style.fontWeight, static_cast<U16>(400));
    EXPECT_EQ(style.fontSize, 13.f);
    EXPECT_EQ(style.lineHeight.kind, LineHeight::Kind::Normal);
    EXPECT_EQ(style.fontFamily, FontFamily::SansSerif);
    EXPECT_FLOAT_EQ(style.color.r, 0.f);
    EXPECT_FLOAT_EQ(style.color.g, 0.f);
    EXPECT_FLOAT_EQ(style.color.b, 0.f);
}

TEST(StyleCompilerTest, InitialOverridesInheritedValue) {
    constexpr char kInitialStyles[] = "panel { font-size: 22px; } label { font-size: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>("label");
    HTMLLabelElement* labelPtr = label.get();
    panel.append(std::move(label));

    EXPECT_EQ(computedStyle(stylesheet, panel).fontSize, 22.f);
    EXPECT_EQ(computedStyle(stylesheet, *labelPtr).fontSize, 13.f);
}

TEST(StyleCompilerTest, RejectsInvalidTypographyForms) {
    struct InvalidTypographyCase {
        const char* name;
        const char* styles;
    };
    const InvalidTypographyCase cases[] = {
        {"pseudo font family", "label { font-family: sans-bold; }"},
        {"legacy generic family", "label { font-family: sans; }"},
        {"fractional weight", "label { font-weight: 525.5; }"},
        {"unit-bearing weight", "label { font-weight: 700px; }"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid typography case: " << test.name);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.styles);
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleCompilerTest, ParsesBorderProperties) {
    constexpr char kBorderStyles[] = "button { border: 1px solid #112233ff; border-width: 2px 3px; "
                                     "border-color: #ffffffff; } "
                                     "button > i { stroke: #abcdef88; stroke-width: 4px; stroke-linecap: square; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBorderStyles).ok());
    const ComputedStyle buttonStyle = stylesheet.resolve("button", "", {}, 0);
    EXPECT_EQ(buttonStyle.borderWidth.top, 2.f);
    EXPECT_EQ(buttonStyle.borderWidth.right, 3.f);
    EXPECT_EQ(buttonStyle.borderColor.r, 1.f);

    auto button = makeElementValue<HTMLButtonElement>();
    const ComputedStyle iconStyle = computedStyle(stylesheet, appendIcon(button, "search"));
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 4.f);
    EXPECT_EQ(iconStyle.svgStrokeCap, StrokeCap::Square);
    EXPECT_FLOAT_EQ(iconStyle.strokeColor.r, 171.f / 255.f);
    EXPECT_FLOAT_EQ(iconStyle.strokeColor.g, 205.f / 255.f);
    EXPECT_FLOAT_EQ(iconStyle.strokeColor.b, 239.f / 255.f);
    EXPECT_FLOAT_EQ(iconStyle.strokeColor.a, 136.f / 255.f);
}

TEST(StyleCompilerTest, SplitsCommentSeparatedBorderValues) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button { border: 2px/**/solid #112233ff; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, 0);
    EXPECT_EQ(style.borderWidth.top, 2.f);
    EXPECT_EQ(style.borderStyle, radia::ui::BorderStyle::Solid);
}

TEST(StyleCompilerTest, RejectsStrokeShorthandAndLegacyColorProperty) {
    StyleSheet stylesheet;
    const auto shorthand = stylesheet.loadRadia("i { stroke: 4px #abcdef; }");
    ASSERT_TRUE(shorthand.ok());
    ASSERT_FALSE(shorthand.warnings.empty());
    EXPECT_EQ(shorthand.warnings.front().code, "stylesheet.property.value_invalid");

    const auto legacy = stylesheet.loadRadia("i { stroke-color: #abcdef; }");
    ASSERT_TRUE(legacy.ok());
    ASSERT_FALSE(legacy.warnings.empty());
    EXPECT_EQ(legacy.warnings.front().code, "stylesheet.property.unknown");
}

TEST(StyleCompilerTest, ResolvesGradientStroke) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { stroke: linear-gradient(to right, #ff0000, #0000ff); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_TRUE(style.strokeGradient.has_value());
    EXPECT_EQ(style.strokeGradient->kind, GradientKind::Linear);
    EXPECT_EQ(style.strokeGradient->stops.size(), 2U);
}

TEST(StyleCompilerTest, ResolvesGridSwitchStyles) {
    constexpr char kGridSwitchStyles[] = "input.basic-switch { appearance: base; display: inline-grid; position: relative; }"
                                         "input.basic-switch::slider-track { grid-area: 1 / 1; box-shadow: 0 0 5px rgb(0, 0, 0, .3); }"
                                         "input.basic-switch::slider-fill { width: 37px; }"
                                         "input.basic-switch::slider-thumb { grid-area: 1/1; translate: 22px 0; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kGridSwitchStyles).ok());

    auto switchInput = makeElementValue<HTMLInputElement>();
    switchInput.type("checkbox").switchMode(true);
    switchInput.classList().add("basic-switch");
    ASSERT_NE(switchInput.sliderTrack(), nullptr);
    ASSERT_NE(switchInput.sliderThumb(), nullptr);
    const ComputedStyle owner = computedStyle(stylesheet, switchInput);
    const ComputedStyle track = stylesheet.resolvePseudoElement(switchInput, "slider-track");
    const ComputedStyle fill = stylesheet.resolvePseudoElement(switchInput, "slider-fill");
    const ComputedStyle thumb = stylesheet.resolvePseudoElement(switchInput, "slider-thumb");

    EXPECT_EQ(owner.appearance, AppearanceMode::Base);
    EXPECT_EQ(owner.display, DisplayMode::InlineGrid);
    EXPECT_EQ(owner.position, PositionMode::Relative);
    ASSERT_TRUE(track.gridArea.has_value());
    EXPECT_EQ(track.gridArea->row, 1);
    EXPECT_EQ(track.gridArea->column, 1);
    ASSERT_EQ(track.shadows.size(), std::size_t(1));
    EXPECT_NEAR(track.shadows.front().blur, 5.f, 1.0e-4f);
    EXPECT_EQ(fill.width.pixels(), 37.f);
    EXPECT_EQ(thumb.translate.x, 22.f);
    EXPECT_EQ(thumb.translate.y, 0.f);
}

TEST(StyleCompilerTest, ResolvesSwitchFillSelector) {
    constexpr char kSwitchFillStyles[] = "input[type=\"checkbox\"][switch]::slider-fill { width: 37px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSwitchFillStyles).ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-fill").width.pixels(), 37.f);

    input.type("radio");
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-fill").width.resolve(0.f), 0.f);
}

TEST(StyleCompilerTest, ResolvesCheckmarkStyles) {
    constexpr char kCheckmarkStyles[] =
        "input[type=checkbox]::checkmark { content: \"\\2713\" / \"\"; width: 10px; height: 10px; border-radius: 2px; }"
        "input[type=radio]::checkmark { width: 8px; height: 8px; border-width: 1px; }";
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCheckmarkStyles).ok());

    auto checkbox = makeElementValue<HTMLInputElement>();
    checkbox.type("checkbox");
    auto radio = makeElementValue<HTMLInputElement>();
    radio.type("radio");

    const ComputedStyle checkboxMark = stylesheet.resolvePseudoElement(checkbox, "checkmark");
    const ComputedStyle radioMark = stylesheet.resolvePseudoElement(radio, "checkmark");
    EXPECT_EQ(checkboxMark.width.pixels(), 10.f);
    EXPECT_EQ(checkboxMark.height.pixels(), 10.f);
    ASSERT_TRUE(checkboxMark.content.has_value());
    EXPECT_EQ(*checkboxMark.content, "\xE2\x9C\x93");
    EXPECT_EQ(checkboxMark.borderRadius.topLeft.horizontal.pixels, 2.f);
    EXPECT_EQ(radioMark.width.pixels(), 8.f);
    EXPECT_EQ(radioMark.height.pixels(), 8.f);
    EXPECT_EQ(radioMark.borderWidth.top, 1.f);
}

TEST(StyleCompilerTest, RejectsUnsupportedDisplay) {
    constexpr char kUnsupportedDisplayStyles[] = "panel { display: sideways; } panel#bad { display: sideways; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnsupportedDisplayStyles, "test.css");

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).display, DisplayMode::Inline);
    EXPECT_EQ(stylesheet.resolve("panel", "bad", {}, 0).display, DisplayMode::Inline);
    EXPECT_TRUE(result.errors.empty());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings.front().source, "test.css");
}

TEST(StyleCompilerTest, AppliesSelectorRules) {
    constexpr char kSelectorListStyles[] = "button, input { height: 32px; } button > i { width: 14px; } "
                                           "button:disabled { opacity: .5; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorListStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).height.pixels(), 32.f);
    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0).height.pixels(), 32.f);

    auto button = makeElementValue<HTMLButtonElement>();
    EXPECT_EQ(computedStyle(stylesheet, appendIcon(button, "search")).width.pixels(), 14.f);
    const uint16_t disabled = static_cast<uint16_t>(ElementState::Disabled);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, disabled).opacity, .5f);
}

TEST(StyleCompilerTest, ParsesFlexItemShorthands) {
    constexpr char kFlexItemStyles[] = "panel { padding: 1px 2px 3px 4px; min-width: 20px; min-height: 10px; gap: 7px; "
                                       "flex: 2 3 40%; order: -2; } "
                                       "panel.auto { flex: auto; } panel.none { flex: none; } "
                                       "panel.one { flex: 4; } panel.two { flex: 5 6; } panel.basis { flex: 10px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFlexItemStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(style.padding.top, 1.f);
    EXPECT_EQ(style.padding.right, 2.f);
    EXPECT_EQ(style.padding.bottom, 3.f);
    EXPECT_EQ(style.padding.left, 4.f);
    ASSERT_TRUE(style.minWidth.has_value());
    EXPECT_EQ(style.minWidth->pixels(), 20.f);
    EXPECT_EQ(style.rowGap.fixedPixels(), 7.f);
    EXPECT_EQ(style.flexGrow, 2.f);
    EXPECT_EQ(style.flexShrink, 3.f);
    EXPECT_NEAR(style.flexBasis.resolve(0.f, 200.f), 80.f, 1.0e-4f);
    EXPECT_EQ(style.order, -2);

    const ComputedStyle automatic = stylesheet.resolve("panel", "", {"auto"}, 0);
    EXPECT_EQ(automatic.flexGrow, 1.f);
    EXPECT_EQ(automatic.flexShrink, 1.f);
    EXPECT_TRUE(automatic.flexBasis.isAuto());

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"}, 0);
    EXPECT_EQ(none.flexGrow, 0.f);
    EXPECT_EQ(none.flexShrink, 0.f);
    EXPECT_TRUE(none.flexBasis.isAuto());

    const ComputedStyle one = stylesheet.resolve("panel", "", {"one"}, 0);
    EXPECT_EQ(one.flexGrow, 4.f);
    EXPECT_EQ(one.flexBasis.resolve(1.f), 0.f);

    const ComputedStyle two = stylesheet.resolve("panel", "", {"two"}, 0);
    EXPECT_EQ(two.flexGrow, 5.f);
    EXPECT_EQ(two.flexShrink, 6.f);

    const ComputedStyle basis = stylesheet.resolve("panel", "", {"basis"}, 0);
    EXPECT_EQ(basis.flexGrow, 1.f);
    EXPECT_EQ(basis.flexBasis.resolve(0.f), 10.f);
}

TEST(StyleCompilerTest, RejectsUnitBearingFlexGrow) {
    constexpr char kUnitBearingFlexGrow[] = "panel { flex-grow: 1px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnitBearingFlexGrow);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompilerTest, ProvidesStableStyleDefaults) {
    const ComputedStyle style;

    EXPECT_EQ(style.appearance, AppearanceMode::NoneValue);
    EXPECT_EQ(style.display, DisplayMode::Inline);
    EXPECT_FALSE(style.displaySet);
    EXPECT_EQ(style.justifyContent.position, ContentPosition::Normal);
    EXPECT_EQ(style.alignItems.position, ItemPosition::Normal);
    EXPECT_EQ(style.alignSelf.position, ItemPosition::Auto);
    EXPECT_EQ(style.alignContent.position, ContentPosition::Normal);
    EXPECT_EQ(style.flexWrap, FlexWrap::Nowrap);
    EXPECT_FALSE(style.flexWrapBalance);
    EXPECT_FALSE(style.maxWidth.has_value());
    EXPECT_FALSE(style.maxHeight.has_value());
    EXPECT_EQ(style.flexGrow, 0.f);
    EXPECT_EQ(style.flexShrink, 1.f);
    EXPECT_TRUE(style.flexBasis.isAuto());
    EXPECT_EQ(style.order, 0);
    EXPECT_EQ(style.rowGap.fixedPixels(), 0.f);
    EXPECT_EQ(style.pointerEvents, PointerEvents::Auto);
    EXPECT_EQ(style.fontFamily, FontFamily::SansSerif);
    EXPECT_EQ(style.verticalAlign, VerticalAlign::Top);
    EXPECT_EQ(style.backgroundColor.a, 0.f);
}

TEST(StyleCompilerTest, PreservesAuthoredFloaterParts) {
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, true, true);
    ASSERT_NE(floater.head(), nullptr);
    ASSERT_NE(floater.body(), nullptr);
    ASSERT_NE(floater.closeButton(), nullptr);
    ASSERT_NE(floater.minimizeButton(), nullptr);
    EXPECT_EQ(floater.head()->elementName(), "head");
    EXPECT_EQ(floater.closeButton()->elementName(), "close");
    EXPECT_EQ(floater.minimizeButton()->elementName(), "minimize");
    EXPECT_TRUE(floater.closeButton()->focusable());
    EXPECT_TRUE(floater.minimizeButton()->focusable());
}

TEST(StyleCompilerTest, ParsesTextShorthands) {
    constexpr char kTextPresentationStyles[] = "panel { letter-spacing: 50%; word-spacing: 25%; text-wrap: nowrap; } "
                                               "p { text-overflow: ellipsis-center; } "
                                               "label { font: italic 525 17px/21px sans-serif; } "
                                               "label.reset { font-style: italic; font-weight: bold; "
                                               "line-height: 30px; font: 12px sans-serif; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTextPresentationStyles).ok());

    auto parent = makeElement<HTMLPanelElement>();
    auto text = makeElement<Element>("p");
    Element* child = text.get();
    child->textContent("inventory item");
    parent->append(std::move(text));
    const ComputedStyle inherited = computedStyle(stylesheet, *child);
    EXPECT_EQ(inherited.letterSpacing.percent, .5f);
    EXPECT_EQ(inherited.wordSpacing.percent, .25f);
    EXPECT_EQ(inherited.textWrap, TextWrap::NoWrap);
    EXPECT_EQ(inherited.textOverflow, TextOverflow::EllipsisCenter);

    const ComputedStyle shorthand = stylesheet.resolve("label", "", {}, 0);
    EXPECT_TRUE(shorthand.fontItalic);
    EXPECT_EQ(shorthand.fontWeight, static_cast<U16>(525));
    EXPECT_EQ(shorthand.fontSize, 17.f);
    EXPECT_EQ(shorthand.lineHeight.kind, LineHeight::Kind::Length);
    EXPECT_EQ(shorthand.lineHeight.value, 21.f);

    const ComputedStyle reset = stylesheet.resolve("label", "", {"reset"}, 0);
    EXPECT_FALSE(reset.fontItalic);
    EXPECT_EQ(reset.fontWeight, static_cast<U16>(400));
    EXPECT_EQ(reset.lineHeight.kind, LineHeight::Kind::Normal);
}

TEST(StyleCompilerTest, RejectsInvalidTextValues) {
    struct InvalidTextStyleCase {
        const char* name;
        const char* styles;
    };
    const InvalidTextStyleCase cases[] = {
        {"unsupported text overflow", "p { text-overflow: middle; }"},
        {"font shorthand without family", "p { font: 13px; }"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid text style case: " << test.name);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.styles);
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleCompilerTest, TreatsNormalWordSpacingAsZero) {
    constexpr char kNormalWordSpacingStyles[] = "p { word-spacing: normal; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kNormalWordSpacingStyles).ok());
    EXPECT_EQ(stylesheet.resolve("p", "", {}, 0).wordSpacing.pixels, 0.f);
}

TEST(StyleCompilerTest, RejectsNonFiniteEdgeValues) {
    struct NonFiniteValueCase {
        const char* property;
        const char* value;
    };
    // clang-format off
    constexpr NonFiniteValueCase cases[] = {
        {"padding", "nan 2px 3px 4px"},
        {"padding", "1px inf 3px 4px"},
        {"padding", "1px 2px 3px nan"},
        {"padding", "-nan 2px 3px 4px"},
        {"padding", "1px 2px -nan 4px"},
        {"padding", "1px 2px 3px -inf"},
        {"padding", "1px 2px -inf 4px"},
        {"padding", "1px -inf 3px 4px"},
        {"border-width", "nan 2px 3px 4px"},
        {"border-width", "1px inf 3px 4px"},
        {"border-width", "1px 2px 3px nan"},
        {"border-width", "-nan 2px 3px 4px"},
        {"border-width", "1px 2px -nan 4px"},
        {"border-width", "1px 2px 3px -inf"},
        {"border-width", "1px 2px -inf 4px"},
        {"border-width", "1px -inf 3px 4px"},
    };
    // clang-format on

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "non-finite " << test.property << " value: " << test.value);
        const std::string styles = std::string("panel { ") + test.property + ": " + test.value + "; }";
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(styles, "nonfinite-edge.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleCompilerTest, PreservesNestedSyntax) {
    const std::vector<std::string> tokens = tokenizeTopLevel("italic 17px/21px sans-serif", true);
    ASSERT_EQ(tokens.size(), std::size_t(5));
    EXPECT_EQ(tokens[2], "/");
    const std::vector<std::string> commentSeparated = tokenizeTopLevel("1px/**/solid");
    ASSERT_EQ(commentSeparated.size(), std::size_t(2));
    EXPECT_EQ(commentSeparated[0], "1px");
    EXPECT_EQ(commentSeparated[1], "solid");
    EXPECT_TRUE(tokenizeTopLevel("var(--accent", true).empty());
    EXPECT_TRUE(splitTopLevel("rgb(1, 2)), blue", ',').empty());
}

TEST(StyleCompilerTest, TokenizesCSSStructuralKinds) {
    const std::string source = R"(<!-- panel { value: rgb(1, fn("}")), url(icon\)name.svg); /* } */ text: "}"; } -->)";
    const CSSTokenStream stream(source);
    std::vector<CSSTokenKind> kinds;
    std::size_t openIndex = 0;
    std::size_t closeIndex = 0;
    std::size_t urlIndex = 0;
    for (std::size_t index = 0; index < stream.tokens().size(); ++index) {
        const auto kind = stream.tokens()[index].kind;
        if (kind != CSSTokenKind::Whitespace) kinds.push_back(kind);
        if (kind == CSSTokenKind::OpenBrace) openIndex = index;
        if (kind == CSSTokenKind::CloseBrace) closeIndex = index;
        if (kind == CSSTokenKind::Url) urlIndex = index;
    }

    EXPECT_EQ(kinds.front(), CSSTokenKind::CDO);
    EXPECT_EQ(kinds.back(), CSSTokenKind::CDC);
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), CSSTokenKind::Colon), kinds.end());
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), CSSTokenKind::Semicolon), kinds.end());
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), CSSTokenKind::Comma), kinds.end());
    EXPECT_EQ(stream.text(urlIndex), "url(icon\\)name.svg)");
    ASSERT_NE(openIndex, std::size_t(0));
    ASSERT_NE(closeIndex, std::size_t(0));
    EXPECT_EQ(stream.tokens()[openIndex].matching, closeIndex);
    EXPECT_EQ(stream.tokens()[closeIndex].begin, source.rfind('}'));
    EXPECT_EQ(matchingBlock(source, stream.tokens()[openIndex].begin), stream.tokens()[closeIndex].begin);
}

TEST(StyleCompilerTest, HandlesUnicodeCodePointsAndCSSValueEscapes) {
    const CSSTokenStream functionStream(R"(r\67 b(1, 2, 3))");
    const auto function = radia::ui::detail::parseCSSFunction(functionStream, {0, functionStream.tokens().size()});
    ASSERT_TRUE(function.has_value());
    EXPECT_EQ(function->name, "rgb");
    EXPECT_EQ(serializeCSSRange(functionStream, function->body), "1, 2, 3");

    const CSSTokenStream dimensionStream(R"(17p\78)");
    const auto dimension = radia::ui::detail::parseCSSDimension(dimensionStream, {0, dimensionStream.tokens().size()});
    ASSERT_TRUE(dimension.has_value());
    EXPECT_EQ(dimension->number, "17");
    EXPECT_EQ(dimension->unit, "px");
    EXPECT_EQ(radia::ui::detail::normalizeCSSKeyword(R"(to t\6f p)"), "to top");

    const std::string urlSource = std::string("url(icon") + std::string("\xC2\x80", 2) + ")";
    const CSSTokenStream urlStream(urlSource);
    ASSERT_EQ(urlStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(urlStream.tokens().front().kind, CSSTokenKind::BadUrl);

    EXPECT_EQ(radia::ui::detail::cssSourcePosition("éx", 2), (std::pair<std::size_t, std::size_t>{1, 2}));
}

TEST(StyleCompilerTest, PreservesTokenBoundariesAcrossComments) {
    const CSSTokenStream stream("foo/**/bar");

    ASSERT_EQ(stream.tokens().size(), std::size_t(2));
    EXPECT_EQ(stream.tokens()[0].kind, CSSTokenKind::Ident);
    EXPECT_EQ(stream.tokens()[1].kind, CSSTokenKind::Ident);
    EXPECT_TRUE(stream.tokens()[1].precededByComment);
    EXPECT_EQ(serializeCSSRange(stream, {0, stream.tokens().size()}), "foo/**/bar");
    EXPECT_EQ(radia::ui::detail::normalizeCSSKeyword("to/**/right"), "to right");
}

TEST(StyleCompilerTest, PreservesCSSParseErrorTokensAtEOF) {
    const CSSTokenStream stringStream("\"unterminated");
    ASSERT_EQ(stringStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(stringStream.tokens().front().kind, CSSTokenKind::String);

    const CSSTokenStream urlStream("url(icon.svg");
    ASSERT_EQ(urlStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(urlStream.tokens().front().kind, CSSTokenKind::Url);

    const CSSTokenStream escapedURLStream(R"(u\72l(icon.svg))");
    ASSERT_EQ(escapedURLStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(escapedURLStream.tokens().front().kind, CSSTokenKind::Url);

    const CSSTokenStream escapedCRLFURLStream("u\\72\r\nl(icon.svg)");
    ASSERT_EQ(escapedCRLFURLStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(escapedCRLFURLStream.tokens().front().kind, CSSTokenKind::Url);
    EXPECT_EQ(radia::ui::detail::decodeCSSIdentifier("u\\72\r\nl"), "url");
    const auto decodedString = radia::ui::detail::decodeCSSString("\"a\\31\r\nb\"");
    ASSERT_TRUE(decodedString.has_value());
    EXPECT_EQ(*decodedString, "a1b");

    std::string nulSource = "\"A";
    nulSource.push_back('\0');
    nulSource += "B\"";
    const CSSTokenStream nulStream(nulSource);
    ASSERT_EQ(nulStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(nulStream.tokens().front().kind, CSSTokenKind::String);
    EXPECT_EQ(nulStream.text(0),
              "\"A\xEF\xBF\xBD"
              "B\"");

    const std::string surrogateSource = std::string("A") + "\xED\xA0\x80" + "B";
    const CSSTokenStream surrogateStream(surrogateSource);
    ASSERT_EQ(surrogateStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(surrogateStream.tokens().front().kind, CSSTokenKind::Ident);
    EXPECT_EQ(surrogateStream.text(0),
              "A\xEF\xBF\xBD"
              "B");
}

TEST(StyleCompilerTest, ConsumesInvalidURLRemnants) {
    const CSSTokenStream stream("url(foo\\");

    ASSERT_EQ(stream.tokens().size(), std::size_t(1));
    EXPECT_EQ(stream.tokens().front().kind, CSSTokenKind::BadUrl);
    EXPECT_EQ(stream.tokens().front().end, stream.source().size());
}

TEST(StyleCompilerTest, ReconsumesNewlineAfterBadString) {
    const CSSTokenStream stream("\"unterminated\nwidth: 1px;");
    ASSERT_GE(stream.tokens().size(), std::size_t(4));
    EXPECT_EQ(stream.tokens()[0].kind, CSSTokenKind::BadString);
    EXPECT_EQ(stream.tokens()[0].end, std::size_t(13));
    EXPECT_EQ(stream.tokens()[1].kind, CSSTokenKind::Whitespace);
    EXPECT_EQ(stream.tokens()[1].begin, std::size_t(13));
    EXPECT_EQ(stream.tokens()[2].kind, CSSTokenKind::Ident);
    EXPECT_EQ(stream.text(2), "width");
}

TEST(StyleCompilerTest, RejectsUnclosedStyleBlocks) {
    const std::string kNestedStyles = "button { i { width: 1px; } }";
    const std::size_t open = kNestedStyles.find('{');
    ASSERT_NE(open, std::string::npos);
    const std::optional<std::size_t> close = matchingBlock(kNestedStyles, open);

    ASSERT_TRUE(close.has_value());
    EXPECT_EQ(*close, kNestedStyles.size() - 1);
    EXPECT_FALSE(matchingBlock("button {", 7).has_value());

    const std::string lineBreakStyles = "button\r\n{ width: 1px; }";
    const std::size_t lineBreakOpen = lineBreakStyles.find('{');
    ASSERT_NE(lineBreakOpen, std::string::npos);
    EXPECT_EQ(matchingBlock(lineBreakStyles, lineBreakOpen), lineBreakStyles.rfind('}'));
}

TEST(StyleCompilerTest, KeepsPropertyRegistryValid) {
    const std::set<std::string_view> shorthandNames{"background", "flex",     "flex-flow",     "font",        "gap",        "mask", "max-size",
                                                    "min-size",   "overflow", "place-content", "place-items", "place-self", "size", "text-wrap"};
    std::set<std::string_view> names;
    for (const StylePropertyDefinition* property = stylePropertyBegin(); property != stylePropertyEnd(); ++property) {
        SCOPED_TRACE(Message() << "style property: " << property->name);
        const bool shorthand = shorthandNames.count(property->name) != 0;
        EXPECT_TRUE(names.insert(property->name).second);
        EXPECT_NE(property->compile, nullptr);
        EXPECT_EQ(property->apply == nullptr, shorthand);
        EXPECT_EQ(property->reset == nullptr, shorthand);
        EXPECT_EQ(property->longhands.empty(), !shorthand);
        for (const std::string_view longhand : property->longhands) EXPECT_NE(findStyleProperty(longhand), nullptr);
    }
}
