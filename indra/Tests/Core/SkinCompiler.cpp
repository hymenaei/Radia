/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/CSSPropertyParser.h>
#include <Core/CSSRules.h>
#include <Core/CSSTokenStream.h>
#include <Core/Color.h>
#include <Core/ElementInternal.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLElementFactory.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/HTMLInputElement.h>
#include <Core/HTMLLabelElement.h>
#include <Core/HTMLPanelElement.h>
#include <Core/LayoutEngine.h>
#include <Core/LayoutGeometry.h>
#include <Core/ResourceElementDefinition.h>
#include <Core/StylePass.h>
#include <Core/StyleProperty.h>
#include <Core/StyleSheet.h>
#include <Core/TextMeasurer.h>
#include <algorithm>
#include <cstdint>
#include <gtest/gtest.h>
#include <initializer_list>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "CSSProperties.h"
#include "ComputedStyleProperties.h"
#include "FloaterTestHelpers.h"

namespace {
using Core::Color;
using Core::Element;
using Core::FixedTextMeasurer;
using Core::HTMLButtonElement;
using Core::HTMLFloaterElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::CSS::KeywordName;
using Core::CSS::Property;
using Core::CSS::PseudoClass;
using Core::CSS::StyleSheet;
using Core::CSS::detail::matchingBlock;
using Core::CSS::detail::serializeRange;
using Core::CSS::detail::splitTopLevel;
using Core::CSS::detail::tokenizeTopLevel;
using Core::CSS::detail::TokenKind;
using Core::CSS::detail::TokenStream;
using Core::detail::ElementInternalAccess;
using Core::detail::HTMLElementFactory;
using Core::detail::makeElement;
using Core::detail::makeElementValue;
using Core::Layout::paddingPixels;
using Core::Style::Appearance;
using Core::Style::BorderImageRepeatMode;
using Core::Style::BorderImageValueUnit;
using Core::Style::BoxSizing;
using Core::Style::ColorScheme;
using Core::Style::ColorSchemeMode;
using Core::Style::ComputedStyle;
using Core::Style::ContentDistribution;
using Core::Style::ContentPosition;
using Core::Style::DimensionKeyword;
using Core::Style::Display;
using Core::Style::FlexDirection;
using Core::Style::FlexWrap;
using Core::Style::FlexWrapMode;
using Core::Style::FontFamilies;
using Core::Style::FontFamily;
using Core::Style::FontStyle;
using Core::Style::GenericFontFamily;
using Core::Style::GradientKind;
using Core::Style::ItemPosition;
using Core::Style::LineHeight;
using Core::Style::OverflowAlignment;
using Core::Style::Pass;
using Core::Style::PointerEvents;
using Core::Style::Position;
using Core::Style::StrokeCap;
using Core::Style::systemColorValue;
using Core::Style::TextAlign;
using Core::Style::TextOverflow;
using Core::Style::TextWrapMode;
using Core::Style::TextWrapStyle;
using Core::Style::VerticalAlign;
using Core::Style::Visibility;
using Core::Style::detail::findLegacyProperty;
using Core::Style::detail::legacyPropertyBegin;
using Core::Style::detail::legacyPropertyEnd;
using Core::Style::detail::PropertyDefinition;
using ::testing::Message;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    Pass styles(stylesheet, FixedTextMeasurer {});
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

TEST(StyleCompiler, ResolvesStructuralDivStyles) {
    constexpr char kDivStyles[] = "div.stack { display: flex; flex-direction: row; gap: 8px; padding: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDivStyles).ok());

    const auto* definition = Core::findElementDefinition(Core::HTMLTag::Div);
    ASSERT_NE(definition, nullptr);
    auto div = HTMLElementFactory::create("div");
    ASSERT_NE(div, nullptr);
    div->classList().add("stack");
    const ComputedStyle style = computedStyle(stylesheet, *div);
    EXPECT_EQ(style.display(), Display::Flex);
    EXPECT_TRUE(style.displaySet);
    EXPECT_EQ(style.flexDirection(), FlexDirection::Row);
    EXPECT_EQ(style.rowGap().fixedPixels(), 8.f);
    EXPECT_EQ(style.padding().top.pixels, 2.f);
}

TEST(StyleCompiler, ResolvesVisibility) {
    constexpr char kDisplayStyles[] = "panel.flex { display: flex; flex-direction: column; } "
                                      "panel.inline { display: inline; } panel.inline-flex { display: inline-flex; } "
                                      "panel.none { display: none; } "
                                      "panel.hidden { visibility: hidden; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDisplayStyles).ok());

    auto flex = makeElementValue<HTMLPanelElement>();
    flex.classList().add("flex");
    const ComputedStyle flexStyle = computedStyle(stylesheet, flex);
    EXPECT_EQ(flexStyle.display(), Display::Flex);
    EXPECT_EQ(flexStyle.flexDirection(), FlexDirection::Column);

    auto inlinePanel = makeElementValue<HTMLPanelElement>();
    inlinePanel.classList().add("inline");
    const ComputedStyle inlineStyle = computedStyle(stylesheet, inlinePanel);
    EXPECT_EQ(inlineStyle.display(), Display::Inline);

    auto inlineFlexPanel = makeElementValue<HTMLPanelElement>();
    inlineFlexPanel.classList().add("inline-flex");
    EXPECT_EQ(computedStyle(stylesheet, inlineFlexPanel).display(), Display::InlineFlex);

    auto none = makeElementValue<HTMLPanelElement>();
    none.classList().add("none");
    EXPECT_EQ(computedStyle(stylesheet, none).display(), Display::NoneValue);

    auto hidden = makeElementValue<HTMLPanelElement>();
    hidden.classList().add("hidden");
    auto child = makeElement<HTMLLabelElement>("child");
    HTMLLabelElement* childPtr = child.get();
    hidden.append(std::move(child));
    EXPECT_EQ(computedStyle(stylesheet, *childPtr).visibility(), Visibility::Hidden);
}

TEST(StyleCompiler, DecodesEscapedDeclarationKeywords) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { display: n\\6f ne; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, {}).display(), Display::NoneValue);
}

TEST(StyleCompiler, ParsesColorSchemes) {
    constexpr char kColorSchemeStyles[] = "panel { color-scheme: light; } panel.dark { color-scheme: dark; } "
                                          "panel.both { color-scheme: light dark; } panel.reset { color-scheme: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kColorSchemeStyles).ok());

    const ColorScheme light = stylesheet.resolve("panel", "", {}, {}).colorScheme();
    const ColorScheme dark = stylesheet.resolve("panel", "", {"dark"}, {}).colorScheme();
    const ColorScheme both = stylesheet.resolve("panel", "", {"both"}, {}).colorScheme();
    const ColorScheme reset = stylesheet.resolve("panel", "", {"reset"}, {}).colorScheme();
    ASSERT_FALSE(light.normal);
    ASSERT_EQ(light.schemes.size(), std::size_t {1});
    EXPECT_EQ(light.schemes.front(), ColorSchemeMode::Light);
    ASSERT_FALSE(dark.normal);
    ASSERT_EQ(dark.schemes.size(), std::size_t {1});
    EXPECT_EQ(dark.schemes.front(), ColorSchemeMode::Dark);
    ASSERT_FALSE(both.normal);
    ASSERT_EQ(both.schemes.size(), std::size_t {2});
    EXPECT_EQ(both.schemes[0], ColorSchemeMode::Light);
    EXPECT_EQ(both.schemes[1], ColorSchemeMode::Dark);
    EXPECT_TRUE(reset.normal);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { color-scheme: light light; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(invalid.resolve("panel", "", {}, {}).colorScheme().schemes, std::vector<ColorSchemeMode> {ColorSchemeMode::Light});

    const ColorScheme normal = reset;
    EXPECT_TRUE(normal.normal);
    EXPECT_EQ(normal.used({ColorSchemeMode::Light, std::nullopt, false}), ColorSchemeMode::Light);
    EXPECT_EQ(normal.used({ColorSchemeMode::Dark, std::nullopt, false}), ColorSchemeMode::Dark);
    EXPECT_EQ(normal.used({std::nullopt, std::nullopt, false}), ColorSchemeMode::Dark);
    EXPECT_EQ(both.used({ColorSchemeMode::Light, std::nullopt, false}), ColorSchemeMode::Light);
    EXPECT_EQ(both.used({ColorSchemeMode::Dark, std::nullopt, false}), ColorSchemeMode::Light);
    EXPECT_EQ(both.used({ColorSchemeMode::Light, ColorSchemeMode::Dark, false}), ColorSchemeMode::Dark);

    StyleSheet custom;
    ASSERT_TRUE(custom.loadRadia("panel { color-scheme: only LIGHT custom-theme; }").ok());
    const ColorScheme customValue = custom.resolve("panel", "", {}, {}).colorScheme();
    EXPECT_TRUE(customValue.only);
    EXPECT_EQ(customValue.schemes, std::vector<ColorSchemeMode> {ColorSchemeMode::Light});
    EXPECT_EQ(customValue.customIdentifiers, std::vector<std::string> {"custom-theme"});

    StyleSheet customOnly;
    ASSERT_TRUE(customOnly.loadRadia("panel { color-scheme: custom-theme; }").ok());
    const ColorScheme customOnlyValue = customOnly.resolve("panel", "", {}, {}).colorScheme();
    EXPECT_EQ(customOnlyValue.used({ColorSchemeMode::Dark, std::nullopt, false}), ColorSchemeMode::Dark);

    StyleSheet invalidToken;
    const auto invalidTokenResult = invalidToken.loadRadia("panel { color-scheme: light url(theme); }");
    ASSERT_TRUE(invalidTokenResult.ok());
    ASSERT_FALSE(invalidTokenResult.warnings.empty());
    EXPECT_EQ(invalidTokenResult.warnings.front().code, "stylesheet.property.value_invalid");

    StyleSheet customKeywords;
    ASSERT_TRUE(customKeywords.loadRadia("panel { color-scheme: auto none; }").ok());
    EXPECT_EQ(customKeywords.resolve("panel", "", {}, {}).colorScheme().customIdentifiers, std::vector<std::string>({"auto", "none"}));
}

TEST(StyleCompiler, ParsesBoxSizingValues) {
    constexpr char kBoxSizingStyles[] = "panel { box-sizing: border-box; } panel.content { box-sizing: content-box; } "
                                        "panel.reset { box-sizing: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxSizingStyles).ok());

    EXPECT_EQ(stylesheet.resolve("panel", "", {}, {}).boxSizing(), BoxSizing::BorderBox);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content"}, {}).boxSizing(), BoxSizing::ContentBox);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"reset"}, {}).boxSizing(), BoxSizing::ContentBox);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { box-sizing: padding-box; }");
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompiler, ResolvesSchemeColors) {
    constexpr char kLightDarkStyles[] =
        "panel { background-color: light-dark(#ffffff, #000000); color: light-dark(#101010, #f0f0f0); "
        "border: 1px solid light-dark(#cccccc, #333333); color-scheme: light; } panel.dark { color-scheme: dark; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLightDarkStyles).ok());

    const ComputedStyle light = stylesheet.resolve("panel", "", {}, {});
    EXPECT_NEAR(light.backgroundColor().resolvedColor().r, 1.f, 1.0e-4f);
    EXPECT_NEAR(light.color().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(light.borderColor().top.resolvedColor().r, 204.f / 255.f, 1.0e-4f);

    const ComputedStyle dark = stylesheet.resolve("panel", "", {"dark"}, {});
    EXPECT_FLOAT_EQ(dark.backgroundColor().resolvedColor().r, 0.f);
    EXPECT_NEAR(dark.color().resolvedColor().r, 240.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(dark.borderColor().top.resolvedColor().r, 51.f / 255.f, 1.0e-4f);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("panel { color: light-dark(#fff); }");
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompiler, ResolvesStyleTokens) {
    constexpr char kTokenStyles[] = ":root { --accent: #204060ff; --space: 12px; } "
                                    "button { background-color: var(--accent); padding: var(--space); "
                                    "border-radius: 5px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTokenStyles).ok());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, {});

    EXPECT_NEAR(style.backgroundColor().resolvedColor().b, 96.f / 255.f, 1.0e-4f);
    EXPECT_EQ(style.padding().left.pixels, 12.f);
    EXPECT_EQ(style.borderRadius().topLeft.horizontal.pixels, 5.f);
    EXPECT_EQ(style.borderRadius().topLeft.vertical.pixels, 5.f);
}

TEST(StyleCompiler, ResolvesPercentageBorderRadius) {
    constexpr char kPercentageRadiusStyles[] = "input { border-radius: 100%; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kPercentageRadiusStyles).ok());
    const ComputedStyle style = stylesheet.resolve("input", "", {}, {});

    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.horizontal.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.horizontal.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.horizontal.resolve(20.f), 20.f);
    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.vertical.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.vertical.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius().topLeft.vertical.resolve(20.f), 20.f);
}

TEST(StyleCompiler, ResolvesSlashSeparatedCornerRadiusLonghand) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("input { border-top-left-radius: 10px / 20%; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle style = stylesheet.resolve("input", "", {}, {});
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().horizontal.pixels, 10.f);
    EXPECT_FLOAT_EQ(style.borderTopLeftRadius().vertical.percent, 0.2f);
}

TEST(StyleCompiler, PreservesShorthandValues) {
    constexpr char kDeclarationOrderStyles[] = "button { width: 10px; size: 20px 30px; max-size: 15px 25px; width: 40px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDeclarationOrderStyles).ok());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, {});

    EXPECT_EQ(style.width().pixels(), 40.f);
    EXPECT_EQ(style.height().pixels(), 20.f);
    ASSERT_TRUE(style.maxWidth().has_value());
    ASSERT_TRUE(style.maxHeight().has_value());
    EXPECT_EQ(style.maxWidth()->pixels(), 25.f);
    EXPECT_EQ(style.maxHeight()->pixels(), 15.f);
}

TEST(StyleCompiler, AppliesSelectorSpecificity) {
    constexpr char kSpecificityStyles[] = "button.primary { width: 30px; } button { width: 10px; } "
                                          "#save { width: 50px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSpecificityStyles).ok());
    const std::set<std::string> classes {"primary"};

    EXPECT_EQ(stylesheet.resolve("button", "save", classes, {}).width().pixels(), 50.f);
    EXPECT_EQ(stylesheet.resolve("button", "", classes, {}).width().pixels(), 30.f);
}

TEST(StyleCompiler, ResolvesNestedSelectors) {
    constexpr char kNestedStyles[] = "button { background-color: #101010ff; &:hover { background-color: #202020ff; } "
                                     "> i { size: 16px; } &:hover > i { stroke-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kNestedStyles).ok());
    const std::initializer_list<PseudoClass> hover {PseudoClass::Hover};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, hover).backgroundColor().resolvedColor().r, 32.f / 255.f);

    auto button = makeElementValue<HTMLButtonElement>();
    ElementInternalAccess::setHovered(button, true);
    const ComputedStyle iconStyle = computedStyle(stylesheet, appendIcon(button, "search"));
    EXPECT_EQ(iconStyle.width().pixels(), 16.f);
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 3.f);
}

TEST(StyleCompiler, ParsesAlignmentEnums) {
    constexpr char kAlignmentStyles[] = "panel { display: flex; flex-direction: row; vertical-align: middle; pointer-events: none; } "
                                        "label { text-align: right; pointer-events: auto; } label.length { vertical-align: 4px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kAlignmentStyles).ok());
    const ComputedStyle panel = stylesheet.resolve("panel", "", {}, {});
    const ComputedStyle label = stylesheet.resolve("label", "", {}, {});
    const ComputedStyle length = stylesheet.resolve("label", "", {"length"}, {});

    EXPECT_EQ(panel.display(), Display::Flex);
    EXPECT_EQ(panel.pointerEvents(), PointerEvents::NoneValue);
    EXPECT_EQ(label.textAlign(), TextAlign::Right);
    EXPECT_EQ(panel.verticalAlign().value, VerticalAlign::Middle);
    EXPECT_EQ(label.verticalAlign().value, VerticalAlign::Baseline);
    EXPECT_EQ(length.verticalAlign().value, VerticalAlign::Length);
    EXPECT_EQ(length.verticalAlign().offset.pixels, 4.f);
    EXPECT_EQ(label.pointerEvents(), PointerEvents::Auto);
}

TEST(StyleCompiler, ParsesLogicalAlignment) {
    constexpr char kCrossAxisStyles[] =
        "label { text-align: start; } panel { align-items: end; } "
        "panel.normal { align-items: normal; } panel.flex-start { align-items: flex-start; } "
        "button { align-self: start; } button.auto { align-self: auto; } button.flex-end { align-self: flex-end; } "
        "button.safe-normal { align-self: safe normal; } "
        "panel.justify { justify-content: normal; } panel.justify-start { justify-content: start; } "
        "panel.justify-flex-start { justify-content: flex-start; } panel.justify-end { justify-content: end; } "
        "panel.justify-flex-end { justify-content: flex-end; } panel.content-start { align-content: start; } "
        "panel.content-flex-start { align-content: flex-start; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCrossAxisStyles).ok());

    EXPECT_EQ(stylesheet.resolve("label", "", {}, {}).textAlign(), TextAlign::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, {}).alignItems().position, ItemPosition::End);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {}).alignSelf().position, ItemPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"normal"}, {}).alignItems().position, ItemPosition::Normal);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"flex-start"}, {}).alignItems().position, ItemPosition::FlexStart);
    EXPECT_EQ(stylesheet.resolve("button", "", {"auto"}, {}).alignSelf().position, ItemPosition::Auto);
    EXPECT_EQ(stylesheet.resolve("button", "", {"flex-end"}, {}).alignSelf().position, ItemPosition::FlexEnd);
    EXPECT_EQ(stylesheet.resolve("button", "", {"safe-normal"}, {}).alignSelf().position, ItemPosition::Normal);
    EXPECT_EQ(stylesheet.resolve("button", "", {"safe-normal"}, {}).alignSelf().overflow, OverflowAlignment::Safe);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify"}, {}).justifyContent().position, ContentPosition::Normal);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-start"}, {}).justifyContent().position, ContentPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-flex-start"}, {}).justifyContent().position, ContentPosition::FlexStart);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-end"}, {}).justifyContent().position, ContentPosition::End);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"justify-flex-end"}, {}).justifyContent().position, ContentPosition::FlexEnd);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content-start"}, {}).alignContent().position, ContentPosition::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"content-flex-start"}, {}).alignContent().position, ContentPosition::FlexStart);
}

TEST(StyleCompiler, ParsesGridSelfAlignment) {
    constexpr char kGridAlignmentStyles[] = "input { justify-self: center; } button.start { justify-self: start; } "
                                            "button.end { justify-self: end; } label.stretch { justify-self: stretch; } "
                                            "label.auto { justify-self: auto; } label.anchor { justify-self: anchor-center; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kGridAlignmentStyles).ok());

    EXPECT_EQ(stylesheet.resolve("input", "", {}, {}).justifySelf().position, ItemPosition::Center);
    EXPECT_EQ(stylesheet.resolve("button", "", {"start"}, {}).justifySelf().position, ItemPosition::Start);
    EXPECT_EQ(stylesheet.resolve("button", "", {"end"}, {}).justifySelf().position, ItemPosition::End);
    EXPECT_EQ(stylesheet.resolve("label", "", {"stretch"}, {}).justifySelf().position, ItemPosition::Stretch);
    EXPECT_EQ(stylesheet.resolve("label", "", {"auto"}, {}).justifySelf().position, ItemPosition::Auto);
    EXPECT_EQ(stylesheet.resolve("label", "", {"anchor"}, {}).justifySelf().position, ItemPosition::AnchorCenter);
}

TEST(StyleCompiler, AlignSelfAnchor) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button { align-self: anchor-center; }").ok());

    EXPECT_EQ(stylesheet.resolve("button", "", {}, {}).alignSelf().position, ItemPosition::AnchorCenter);
}

TEST(StyleCompiler, JustifyItemsLegacy) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia(".legacy-parent { justify-items: legacy center; } .legacy-child { justify-items: legacy; } "
                       ".normal-parent { justify-items: center; } .normal-child { justify-items: legacy; }")
            .ok());

    auto legacyParent = makeElement<HTMLPanelElement>();
    legacyParent->classList().add("legacy-parent");
    auto legacyChild = makeElement<HTMLLabelElement>();
    legacyChild->classList().add("legacy-child");
    HTMLLabelElement* legacyChildElement = legacyChild.get();
    legacyParent->append(std::move(legacyChild));

    const ComputedStyle legacyParentStyle = computedStyle(stylesheet, *legacyParent);
    EXPECT_TRUE(legacyParentStyle.justifyItems().legacy);
    EXPECT_EQ(legacyParentStyle.justifyItems().position, ItemPosition::Center);
    const ComputedStyle inherited = computedStyle(stylesheet, *legacyChildElement);
    EXPECT_TRUE(inherited.justifyItems().legacy);
    EXPECT_EQ(inherited.justifyItems().position, ItemPosition::Center);

    auto normalParent = makeElement<HTMLPanelElement>();
    normalParent->classList().add("normal-parent");
    auto normalChild = makeElement<HTMLLabelElement>();
    normalChild->classList().add("normal-child");
    HTMLLabelElement* normalChildElement = normalChild.get();
    normalParent->append(std::move(normalChild));

    const ComputedStyle normal = computedStyle(stylesheet, *normalChildElement);
    EXPECT_FALSE(normal.justifyItems().legacy);
    EXPECT_EQ(normal.justifyItems().position, ItemPosition::Normal);

    StyleSheet shorthand;
    ASSERT_TRUE(shorthand.loadRadia("panel { place-items: safe center legacy; }").ok());
    const ComputedStyle place = shorthand.resolve("panel", "", {}, {});
    EXPECT_EQ(place.alignItems().position, ItemPosition::Center);
    EXPECT_EQ(place.alignItems().overflow, OverflowAlignment::Safe);
    EXPECT_FALSE(place.justifyItems().legacy);
    EXPECT_EQ(place.justifyItems().position, ItemPosition::Normal);
}

TEST(StyleCompiler, ParsesStandardsLayoutExtensions) {
    constexpr char kStyles[] =
        "panel { flex-flow: row-reverse wrap-reverse; justify-content: safe center; align-items: last baseline; "
        "justify-items: unsafe self-end; align-content: stretch; } "
        "panel.place { place-content: safe center space-between; place-items: center start; place-self: end safe center; } "
        "panel.baseline { place-content: first baseline; } "
        "label { position: sticky; width: min-content; height: fit-content; flex-basis: max-content; text-wrap: wrap balance; "
        "vertical-align: 25%; color-scheme: only light custom-theme; } label.pretty { text-wrap-style: pretty; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle panel = stylesheet.resolve("panel", "", {}, {});
    EXPECT_EQ(panel.flexDirection(), FlexDirection::RowReverse);
    EXPECT_EQ(panel.flexWrap().mode, FlexWrapMode::WrapReverse);
    EXPECT_FALSE(panel.flexWrap().balance);
    EXPECT_EQ(panel.justifyContent().position, ContentPosition::Center);
    EXPECT_EQ(panel.justifyContent().overflow, OverflowAlignment::Safe);
    EXPECT_EQ(panel.alignItems().position, ItemPosition::LastBaseline);
    EXPECT_EQ(panel.alignItems().overflow, OverflowAlignment::Default);
    EXPECT_EQ(panel.justifyItems().position, ItemPosition::SelfEnd);
    EXPECT_EQ(panel.justifyItems().overflow, OverflowAlignment::Unsafe);

    const ComputedStyle place = stylesheet.resolve("panel", "", {"place"}, {});
    EXPECT_EQ(place.alignContent().position, ContentPosition::Center);
    EXPECT_EQ(place.alignContent().overflow, OverflowAlignment::Safe);
    EXPECT_EQ(place.justifyContent().distribution, ContentDistribution::SpaceBetween);
    EXPECT_EQ(place.alignItems().position, ItemPosition::Center);
    EXPECT_EQ(place.justifyItems().position, ItemPosition::Start);
    EXPECT_EQ(place.alignSelf().position, ItemPosition::End);
    EXPECT_EQ(place.justifySelf().position, ItemPosition::Center);
    EXPECT_EQ(place.justifySelf().overflow, OverflowAlignment::Safe);

    const ComputedStyle baseline = stylesheet.resolve("panel", "", {"baseline"}, {});
    EXPECT_EQ(baseline.alignContent().position, ContentPosition::Baseline);
    EXPECT_EQ(baseline.justifyContent().position, ContentPosition::Start);

    StyleSheet invalidBaseline;
    const auto invalidBaselineResult = invalidBaseline.loadRadia("panel { align-items: last-baseline; } ");
    ASSERT_TRUE(invalidBaselineResult.ok());
    EXPECT_FALSE(invalidBaselineResult.warnings.empty());
    EXPECT_EQ(invalidBaseline.resolve("panel", "", {}, {}).alignItems().position, ItemPosition::Normal);

    const ComputedStyle label = stylesheet.resolve("label", "", {}, {});
    EXPECT_EQ(label.position(), Position::Sticky);
    ASSERT_TRUE(label.width().isIntrinsic());
    EXPECT_EQ(label.width().intrinsicKeyword(), DimensionKeyword::MinContent);
    EXPECT_EQ(label.height().intrinsicKeyword(), DimensionKeyword::FitContent);
    EXPECT_EQ(label.flexBasis().intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(label.textWrapMode(), TextWrapMode::Wrap);
    EXPECT_EQ(label.textWrapStyle(), TextWrapStyle::Balance);
    EXPECT_EQ(stylesheet.resolve("label", "", {"pretty"}, {}).textWrapStyle(), TextWrapStyle::Pretty);
    EXPECT_EQ(label.verticalAlign().value, VerticalAlign::Percentage);
    EXPECT_EQ(label.verticalAlign().offset.percent, .25f);
    EXPECT_TRUE(label.colorScheme().only);
    ASSERT_EQ(label.colorScheme().schemes.size(), std::size_t {1});
    EXPECT_EQ(label.colorScheme().schemes.front(), ColorSchemeMode::Light);
    ASSERT_EQ(label.colorScheme().customIdentifiers.size(), std::size_t {1});
    EXPECT_EQ(label.colorScheme().customIdentifiers.front(), "custom-theme");

    StyleSheet balanced;
    ASSERT_TRUE(balanced
            .loadRadia(".wrap { flex-wrap: wrap balance; } .reverse { flex-wrap: wrap-reverse balance; } .alone { flex-wrap: balance; } "
                       ".backwards { flex-wrap: balance wrap; } .flow { flex-flow: row wrap balance; } "
                       ".flow-alone { flex-flow: balance; } .flow-reversed { flex-flow: balance wrap-reverse column-reverse; }")
            .ok());
    const ComputedStyle wrap = balanced.resolve("panel", "", {"wrap"}, {});
    EXPECT_EQ(wrap.flexWrap().mode, FlexWrapMode::Wrap);
    EXPECT_TRUE(wrap.flexWrap().balance);
    const ComputedStyle reverse = balanced.resolve("panel", "", {"reverse"}, {});
    EXPECT_EQ(reverse.flexWrap().mode, FlexWrapMode::WrapReverse);
    EXPECT_TRUE(reverse.flexWrap().balance);
    const ComputedStyle alone = balanced.resolve("panel", "", {"alone"}, {});
    EXPECT_EQ(alone.flexWrap().mode, FlexWrapMode::Wrap);
    EXPECT_TRUE(alone.flexWrap().balance);
    const ComputedStyle backwards = balanced.resolve("panel", "", {"backwards"}, {});
    EXPECT_EQ(backwards.flexWrap().mode, FlexWrapMode::Wrap);
    EXPECT_TRUE(backwards.flexWrap().balance);
    const ComputedStyle flow = balanced.resolve("panel", "", {"flow"}, {});
    EXPECT_EQ(flow.flexDirection(), FlexDirection::Row);
    EXPECT_EQ(flow.flexWrap().mode, FlexWrapMode::Wrap);
    EXPECT_TRUE(flow.flexWrap().balance);
    const ComputedStyle flowAlone = balanced.resolve("panel", "", {"flow-alone"}, {});
    EXPECT_EQ(flowAlone.flexWrap().mode, FlexWrapMode::Wrap);
    EXPECT_TRUE(flowAlone.flexWrap().balance);
    const ComputedStyle flowReversed = balanced.resolve("panel", "", {"flow-reversed"}, {});
    EXPECT_EQ(flowReversed.flexDirection(), FlexDirection::ColumnReverse);
    EXPECT_EQ(flowReversed.flexWrap().mode, FlexWrapMode::WrapReverse);
    EXPECT_TRUE(flowReversed.flexWrap().balance);

    StyleSheet intrinsicSizes;
    ASSERT_TRUE(intrinsicSizes
            .loadRadia("label { size: min-content max-content; min-size: fit-content min-content; "
                       "max-size: max-content fit-content; } button { min-width: max-content; max-height: min-content; }")
            .ok());
    const ComputedStyle sized = intrinsicSizes.resolve("label", "", {}, {});
    EXPECT_EQ(sized.height().intrinsicKeyword(), DimensionKeyword::MinContent);
    EXPECT_EQ(sized.width().intrinsicKeyword(), DimensionKeyword::MaxContent);
    ASSERT_TRUE(sized.minHeight().has_value());
    ASSERT_TRUE(sized.minWidth().has_value());
    EXPECT_EQ(sized.minHeight()->intrinsicKeyword(), DimensionKeyword::FitContent);
    EXPECT_EQ(sized.minWidth()->intrinsicKeyword(), DimensionKeyword::MinContent);
    ASSERT_TRUE(sized.maxHeight().has_value());
    ASSERT_TRUE(sized.maxWidth().has_value());
    EXPECT_EQ(sized.maxHeight()->intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(sized.maxWidth()->intrinsicKeyword(), DimensionKeyword::FitContent);
    const ComputedStyle longhands = intrinsicSizes.resolve("button", "", {}, {});
    ASSERT_TRUE(longhands.minWidth().has_value());
    ASSERT_TRUE(longhands.maxHeight().has_value());
    EXPECT_EQ(longhands.minWidth()->intrinsicKeyword(), DimensionKeyword::MaxContent);
    EXPECT_EQ(longhands.maxHeight()->intrinsicKeyword(), DimensionKeyword::MinContent);

    StyleSheet contentBasis;
    ASSERT_TRUE(contentBasis.loadRadia("label { flex-basis: content; }").ok());
    EXPECT_EQ(contentBasis.resolve("label", "", {}, {}).flexBasis().intrinsicKeyword(), DimensionKeyword::Content);

    StyleSheet invalidContent;
    ASSERT_TRUE(invalidContent.loadRadia("label { width: content; height: content; }").ok());
    EXPECT_TRUE(invalidContent.resolve("label", "", {}, {}).width().isAuto());
    EXPECT_TRUE(invalidContent.resolve("label", "", {}, {}).height().isAuto());
}

TEST(StyleCompiler, DimensionConstraints) {
    StyleSheet valid;
    const auto validResult = valid.loadRadia("label { min-width: auto; max-width: none; }");
    ASSERT_TRUE(validResult.ok());
    EXPECT_TRUE(validResult.warnings.empty());
    EXPECT_FALSE(valid.resolve("label", "", {}, {}).minWidth().has_value());
    EXPECT_FALSE(valid.resolve("label", "", {}, {}).maxWidth().has_value());

    StyleSheet invalid;
    const auto invalidResult = invalid.loadRadia("label { min-width: none; max-width: auto; }");
    ASSERT_TRUE(invalidResult.ok());
    EXPECT_EQ(invalidResult.warnings.size(), 2u);
}

TEST(StyleCompiler, OutlineOffset) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { font-size: 12px; outline-offset: -.5em; }").ok());
    EXPECT_EQ(stylesheet.resolve("label", "", {}, {}).outline().offset.pixels, -6.f);

    StyleSheet invalid;
    const auto result = invalid.loadRadia("label { outline-offset: 50%; }");
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.warnings.size(), 1u);
}

TEST(StyleCompiler, EdgePercents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { margin: 12%; padding: 25%; }").ok());
    const ComputedStyle style = stylesheet.resolve("label", "", {}, {});
    EXPECT_FLOAT_EQ(style.margin().top.resolve(200.f), 24.f);
    EXPECT_FLOAT_EQ(style.padding().top.resolve(200.f), 50.f);
}

TEST(StyleCompiler, Typography) {
    constexpr char kTypographyStyles[] = "label#a { font-family: sans-serif; font-size: 19px; "
                                         "font-weight: bold; font-style: italic; } label#oblique { font: oblique 19px sans-serif; }";
    constexpr char kVariableWeightStyles[] = "label { font-weight: 525.5; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypographyStyles).ok());
    const ComputedStyle label = stylesheet.resolve("label", "a", {}, {});
    EXPECT_EQ(label.fontFamily(), FontFamilies {GenericFontFamily::SansSerif});
    EXPECT_EQ(label.fontSize(), 19.f);
    EXPECT_EQ(label.fontWeight().value, 700.f);
    EXPECT_EQ(label.fontStyle(), FontStyle::Italic);
    const ComputedStyle oblique = stylesheet.resolve("label", "oblique", {}, {});
    EXPECT_EQ(oblique.fontStyle(), FontStyle::Oblique);

    StyleSheet variableWeight;
    ASSERT_TRUE(variableWeight.loadRadia(kVariableWeightStyles).ok());
    EXPECT_EQ(variableWeight.resolve("label", "", {}, {}).fontWeight().value, 525.5f);
}

TEST(StyleCompiler, ExpandsInitialValues) {
    using enum Core::CSS::KeywordName;
    constexpr char kInitialStyles[] =
        "panel { display: flex; margin: 4px; padding: 5px; size: 20px 30px; min-size: 2px 3px; "
        "flex: 2 3 4px; overflow: hidden; font: italic 700 21px/25px sans-serif; color: #abcdef; } "
        "panel.reset { display: initial; margin: initial; padding: initial; size: initial; min-size: initial; "
        "flex: initial; overflow: initial; font: initial; color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    auto reset = makeElementValue<HTMLPanelElement>();
    reset.classList().add("reset");
    const ComputedStyle style = computedStyle(stylesheet, reset);

    EXPECT_EQ(style.display(), Display::Inline);
    EXPECT_TRUE(style.displaySet);
    EXPECT_EQ(Core::Layout::horizontalMargin(style.margin()), 0.f);
    EXPECT_EQ(paddingPixels(style).horizontal(), 0.f);
    EXPECT_TRUE(style.width().isAuto());
    EXPECT_TRUE(style.height().isAuto());
    EXPECT_FALSE(style.minWidth().has_value());
    EXPECT_FALSE(style.minHeight().has_value());
    EXPECT_EQ(style.flexGrow().value, 0.f);
    EXPECT_EQ(style.flexShrink().value, 1.f);
    EXPECT_TRUE(style.flexBasis().isAuto());
    EXPECT_EQ(style.overflowX(), Core::Style::Overflow::Visible);
    EXPECT_EQ(style.overflowY(), Core::Style::Overflow::Visible);
    EXPECT_EQ(style.fontStyle(), FontStyle::Normal);
    EXPECT_EQ(style.fontWeight().value, 400.f);
    EXPECT_EQ(style.fontSize(), 13.f);
    EXPECT_TRUE(std::holds_alternative<Core::CSS::Keyword::Normal>(style.lineHeight().mValue));
    EXPECT_EQ(style.fontFamily(), FontFamilies {GenericFontFamily::SansSerif});
    EXPECT_EQ(style.color().resolvedColor(), systemColorValue(KeywordCanvasText, style.usedColorScheme));
}

TEST(StyleCompiler, BoxInherit) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("panel.parent { margin: 1px 2px 3px 4px; padding: 5px 6px 7px 8px; } "
                       "label.child { margin: inherit; padding: inherit; }")
            .ok());

    auto parent = makeElementValue<HTMLPanelElement>();
    parent.classList().add("parent");
    auto child = makeElement<HTMLLabelElement>("child");
    child->classList().add("child");
    HTMLLabelElement* childElement = child.get();
    parent.append(std::move(child));

    const ComputedStyle style = computedStyle(stylesheet, *childElement);
    EXPECT_EQ(style.margin().top.fixedPixels(), 1.f);
    EXPECT_EQ(style.margin().right.fixedPixels(), 2.f);
    EXPECT_EQ(style.margin().bottom.fixedPixels(), 3.f);
    EXPECT_EQ(style.margin().left.fixedPixels(), 4.f);
    EXPECT_EQ(style.padding().top.pixels, 5.f);
    EXPECT_EQ(style.padding().right.pixels, 6.f);
    EXPECT_EQ(style.padding().bottom.pixels, 7.f);
    EXPECT_EQ(style.padding().left.pixels, 8.f);
}

TEST(StyleCompiler, InitialOverridesInheritedValue) {
    constexpr char kInitialStyles[] = "panel { font-size: 22px; } label { font-size: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>("label");
    HTMLLabelElement* labelPtr = label.get();
    panel.append(std::move(label));

    EXPECT_EQ(computedStyle(stylesheet, panel).fontSize(), 22.f);
    EXPECT_EQ(computedStyle(stylesheet, *labelPtr).fontSize(), 13.f);
}

TEST(StyleCompiler, AcceptsAuthoredFontFamilyNames) {
    for (const char* family : {"sans-bold", "sans"}) {
        SCOPED_TRACE(family);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(std::string("label { font-family: ") + family + "; }");
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result.warnings.empty());
        const ComputedStyle style = stylesheet.resolve("label", "", {}, {});
        const FontFamilies& computedFamilies = style.fontFamily();
        ASSERT_EQ(computedFamilies.size(), std::size_t {1});
        EXPECT_EQ(std::get<std::string>(computedFamilies.front()), family);
    }
}

TEST(StyleCompiler, RejectsInvalidTypographyForms) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("label { font-weight: bold; font-weight: 1001; }");
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_FLOAT_EQ(stylesheet.resolve("label", "", {}, {}).fontWeight().value, 700.f);
}

TEST(StyleCompiler, ParsesBorderProperties) {
    constexpr char kBorderStyles[] = "button { border: 1px solid #112233ff; border-width: 2px 3px; "
                                     "border-color: #ffffffff; } "
                                     "button > i { stroke: #abcdef88; stroke-width: 4px; stroke-linecap: square; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kBorderStyles);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle buttonStyle = stylesheet.resolve("button", "", {}, {});
    EXPECT_EQ(buttonStyle.borderWidth().top.pixels, 2.f);
    EXPECT_EQ(buttonStyle.borderWidth().right.pixels, 3.f);
    EXPECT_EQ(buttonStyle.borderColor().top.resolvedColor().r, 1.f);

    auto button = makeElementValue<HTMLButtonElement>();
    const ComputedStyle iconStyle = computedStyle(stylesheet, appendIcon(button, "search"));
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 4.f);
    EXPECT_EQ(iconStyle.svgStrokeCap, StrokeCap::Square);
    const auto& strokeColor = std::get<0>(iconStyle.stroke);
    EXPECT_FLOAT_EQ(strokeColor.resolvedColor().r, 171.f / 255.f);
    EXPECT_FLOAT_EQ(strokeColor.resolvedColor().g, 205.f / 255.f);
    EXPECT_FLOAT_EQ(strokeColor.resolvedColor().b, 239.f / 255.f);
    EXPECT_FLOAT_EQ(strokeColor.resolvedColor().a, 136.f / 255.f);
}

TEST(StyleCompiler, BorderImage) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(
        "button { border: 6px solid #112233ff; border-image: linear-gradient(red, blue) 30 fill / 4px / 2 round space; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());

    const ComputedStyle imageStyle = stylesheet.resolve("button", "", {}, {});
    ASSERT_NE(imageStyle.borderImageSource().gradient(), nullptr);
    EXPECT_TRUE(imageStyle.borderImageSlice().fill);
    EXPECT_EQ(imageStyle.borderImageSlice().edges.top.value, 30.f);
    EXPECT_FALSE(imageStyle.borderImageSlice().edges.top.percentage);
    EXPECT_EQ(imageStyle.borderImageWidth().edges.top.unit, BorderImageValueUnit::Length);
    EXPECT_FLOAT_EQ(imageStyle.borderImageWidth().edges.top.value, 4.f);
    EXPECT_EQ(imageStyle.borderImageOutset().edges.top.value, 2.f);
    EXPECT_TRUE(imageStyle.borderImageOutset().edges.top.multiplier);
    EXPECT_EQ(imageStyle.borderImageRepeat().horizontal, BorderImageRepeatMode::Round);
    EXPECT_EQ(imageStyle.borderImageRepeat().vertical, BorderImageRepeatMode::Space);

    ASSERT_TRUE(
        stylesheet.loadRadia("button { border-image-source: url(images/frame.png); border-image-slice: 20; border: 1px solid; }").ok());
    const ComputedStyle resetStyle = stylesheet.resolve("button", "", {}, {});
    EXPECT_TRUE(resetStyle.borderImageSource().value.index() == 0);
    EXPECT_TRUE(resetStyle.borderImageSlice().fill == false);
    EXPECT_TRUE(resetStyle.borderImageSlice().edges.top.percentage);
    EXPECT_FLOAT_EQ(resetStyle.borderImageSlice().edges.top.value, 100.f);
    EXPECT_EQ(resetStyle.borderImageWidth().edges.top.unit, BorderImageValueUnit::Number);
    EXPECT_FLOAT_EQ(resetStyle.borderImageWidth().edges.top.value, 1.f);
    EXPECT_TRUE(resetStyle.borderImageOutset().edges.top.multiplier);
    EXPECT_FLOAT_EQ(resetStyle.borderImageOutset().edges.top.value, 0.f);
    EXPECT_EQ(resetStyle.borderImageRepeat().horizontal, BorderImageRepeatMode::Stretch);
    EXPECT_EQ(resetStyle.borderImageRepeat().vertical, BorderImageRepeatMode::Stretch);

    StyleSheet invalid;
    const auto invalidResult =
        invalid.loadRadia("button { border-image-source: url(images/frame.png); border-image-source: linear-gradient(red, 20px); }");
    ASSERT_TRUE(invalidResult.ok());
    ASSERT_FALSE(invalidResult.warnings.empty());
    EXPECT_EQ(invalid.resolve("button", "", {}, {}).borderImageSource().value.index(), 1u);
}

TEST(StyleCompiler, ParsesBorderSideShorthands) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("input { border-top: 1px solid #ff0000ff; border-right: 2px outset #00ff00ff; "
                                             "border-bottom: 3px inset #0000ffff; border-left: 4px none #ffffffff; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());

    const ComputedStyle style = stylesheet.resolve("input", "", {}, {});
    EXPECT_FLOAT_EQ(style.borderTopWidth().pixels, 1.f);
    EXPECT_EQ(style.borderTopStyle(), Core::Style::BorderStyle::Solid);
    EXPECT_FLOAT_EQ(style.borderTopColor().resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(style.borderRightWidth().pixels, 2.f);
    EXPECT_EQ(style.borderRightStyle(), Core::Style::BorderStyle::Outset);
    EXPECT_FLOAT_EQ(style.borderRightColor().resolvedColor().g, 1.f);
    EXPECT_FLOAT_EQ(style.borderBottomWidth().pixels, 3.f);
    EXPECT_EQ(style.borderBottomStyle(), Core::Style::BorderStyle::Inset);
    EXPECT_FLOAT_EQ(style.borderBottomColor().resolvedColor().b, 1.f);
    EXPECT_FLOAT_EQ(style.borderLeftWidth().pixels, 4.f);
    EXPECT_EQ(style.borderLeftStyle(), Core::Style::BorderStyle::NoneValue);
    EXPECT_FLOAT_EQ(style.borderLeftColor().resolvedColor().r, 1.f);
}

TEST(StyleCompiler, PreservesIndependentBorderSideColors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("panel { border-top-color: #112233; border-right-color: #445566; "
                       "border-bottom-color: #778899; border-left-color: #aabbcc; }")
            .ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {}, {});
    EXPECT_NEAR(style.borderTopColor().resolvedColor().r, 17.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(style.borderRightColor().resolvedColor().g, 85.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(style.borderBottomColor().resolvedColor().b, 153.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(style.borderLeftColor().resolvedColor().r, 170.f / 255.f, 1.0e-6f);
}

TEST(StyleCompiler, PreservesExtendedSrgbBorderColorFromOklab) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { border-top-color: oklab(.7 .4 .2); }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {}, {});
    const auto& color = style.borderTopColor();
    EXPECT_GT(color.resolvedColor().r, 1.f);
    EXPECT_LT(color.resolvedColor().g, 0.f);
    EXPECT_LT(color.resolvedColor().b, 0.f);
}

TEST(StyleCompiler, ParsesBorderWidthKeywordsAndAbsoluteUnits) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("button { border-width: thin medium thick; } panel { border-width: 1in 2.54cm 25.4mm 101.6q; } "
                       "label { border-width: 72pt 6pc; }")
            .ok());

    const ComputedStyle initial;
    EXPECT_FLOAT_EQ(initial.borderWidth().top.pixels, 3.f);

    const ComputedStyle keywords = stylesheet.resolve("button", "", {}, {});
    EXPECT_FLOAT_EQ(keywords.borderWidth().top.pixels, 1.f);
    EXPECT_FLOAT_EQ(keywords.borderWidth().right.pixels, 3.f);
    EXPECT_FLOAT_EQ(keywords.borderWidth().bottom.pixels, 5.f);
    EXPECT_FLOAT_EQ(keywords.borderWidth().left.pixels, 3.f);

    const ComputedStyle absolute = stylesheet.resolve("panel", "", {}, {});
    EXPECT_FLOAT_EQ(absolute.borderWidth().top.pixels, 96.f);
    EXPECT_FLOAT_EQ(absolute.borderWidth().right.pixels, 96.f);
    EXPECT_FLOAT_EQ(absolute.borderWidth().bottom.pixels, 96.f);
    EXPECT_FLOAT_EQ(absolute.borderWidth().left.pixels, 96.f);

    const ComputedStyle pointsAndPicas = stylesheet.resolve("label", "", {}, {});
    EXPECT_FLOAT_EQ(pointsAndPicas.borderWidth().top.pixels, 96.f);
    EXPECT_FLOAT_EQ(pointsAndPicas.borderWidth().right.pixels, 96.f);
}

TEST(StyleCompiler, SplitsCommentSeparatedBorderValues) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button { border: 2px/**/solid #112233ff; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle style = stylesheet.resolve("button", "", {}, {});
    EXPECT_EQ(style.borderWidth().top.pixels, 2.f);
    EXPECT_EQ(style.borderStyle().top, Core::Style::BorderStyle::Solid);
}

TEST(StyleCompiler, RejectsStrokeShorthandAndLegacyColorProperty) {
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

TEST(StyleCompiler, ResolvesGradientStroke) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { stroke: linear-gradient(to right, #ff0000, #0000ff); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, {});
    const auto& strokeImage = std::get<1>(style.stroke);
    ASSERT_NE(strokeImage.gradient(), nullptr);
    EXPECT_EQ(strokeImage.gradient()->kind, GradientKind::Linear);
    EXPECT_EQ(strokeImage.gradient()->stops.size(), 2U);
}

TEST(StyleCompiler, ResolvesGridSwitchStyles) {
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

    EXPECT_EQ(owner.appearance(), Appearance::Base);
    EXPECT_EQ(owner.display(), Display::InlineGrid);
    EXPECT_EQ(owner.position(), Position::Relative);
    ASSERT_TRUE(track.gridArea.has_value());
    EXPECT_EQ(track.gridArea->row, 1);
    EXPECT_EQ(track.gridArea->column, 1);
    ASSERT_TRUE(thumb.gridArea.has_value());
    EXPECT_EQ(thumb.gridArea->row, 1);
    EXPECT_EQ(thumb.gridArea->column, 1);
    ASSERT_EQ(track.boxShadow().size(), std::size_t(1));
    EXPECT_NEAR(track.boxShadow().front().blur, 5.f, 1.0e-4f);
    EXPECT_EQ(fill.width().pixels(), 37.f);
    EXPECT_EQ(thumb.translate().x.pixels, 22.f);
    EXPECT_EQ(thumb.translate().y.pixels, 0.f);
}

TEST(StyleCompiler, Translate) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("label { translate: none; } label.single { font-size: 20px; translate: 1em; } "
                       "label.zero { translate: 0px; } "
                       "label.pair { translate: -4px 7px; } label.percent { translate: 25% 50%; } "
                       "label.depth { translate: 1px 2px 3px; }")
            .ok());

    const ComputedStyle none = stylesheet.resolve("label", "", {}, {});
    const ComputedStyle single = stylesheet.resolve("label", "", {"single"}, {});
    const ComputedStyle zero = stylesheet.resolve("label", "", {"zero"}, {});
    const ComputedStyle pair = stylesheet.resolve("label", "", {"pair"}, {});
    const ComputedStyle percent = stylesheet.resolve("label", "", {"percent"}, {});
    const ComputedStyle depth = stylesheet.resolve("label", "", {"depth"}, {});
    EXPECT_TRUE(none.translate().isNone);
    EXPECT_FALSE(zero.translate().isNone);
    EXPECT_EQ(none.translate().x.pixels, 0.f);
    EXPECT_EQ(none.translate().y.pixels, 0.f);
    EXPECT_EQ(single.translate().x.pixels, 20.f);
    EXPECT_EQ(single.translate().y.pixels, 0.f);
    EXPECT_EQ(pair.translate().x.pixels, -4.f);
    EXPECT_EQ(pair.translate().y.pixels, 7.f);
    EXPECT_EQ(percent.translate().x.percent, .25f);
    EXPECT_EQ(percent.translate().y.percent, .5f);
    EXPECT_EQ(depth.translate().z.pixels, 3.f);
}

TEST(StyleCompiler, ResolvesSwitchFillSelector) {
    constexpr char kSwitchFillStyles[] = "input[type=\"checkbox\"][switch]::slider-fill { width: 37px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSwitchFillStyles).ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-fill").width().pixels(), 37.f);

    input.type("radio");
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-fill").width().resolve(0.f), 0.f);
}

TEST(StyleCompiler, ResolvesCheckmarkStyles) {
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
    EXPECT_EQ(checkboxMark.width().pixels(), 10.f);
    EXPECT_EQ(checkboxMark.height().pixels(), 10.f);
    ASSERT_TRUE(checkboxMark.content.has_value());
    EXPECT_EQ(*checkboxMark.content, "\xE2\x9C\x93");
    EXPECT_EQ(checkboxMark.borderRadius().topLeft.horizontal.pixels, 2.f);
    EXPECT_EQ(radioMark.width().pixels(), 8.f);
    EXPECT_EQ(radioMark.height().pixels(), 8.f);
    EXPECT_EQ(radioMark.borderWidth().top.pixels, 1.f);
}

TEST(StyleCompiler, RejectsUnsupportedDisplay) {
    constexpr char kUnsupportedDisplayStyles[] = "panel { display: sideways; } panel#bad { display: sideways; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnsupportedDisplayStyles, "test.css");

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, {}).display(), Display::Inline);
    EXPECT_EQ(stylesheet.resolve("panel", "bad", {}, {}).display(), Display::Inline);
    EXPECT_TRUE(result.errors.empty());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings.front().source, "test.css");
}

TEST(StyleCompiler, AppliesSelectorRules) {
    constexpr char kSelectorListStyles[] = "button, input { height: 32px; } button > i { width: 14px; } "
                                           "button:disabled { opacity: .5; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorListStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {}).height().pixels(), 32.f);
    EXPECT_EQ(stylesheet.resolve("input", "", {}, {}).height().pixels(), 32.f);

    auto button = makeElementValue<HTMLButtonElement>();
    EXPECT_EQ(computedStyle(stylesheet, appendIcon(button, "search")).width().pixels(), 14.f);
    const std::initializer_list<PseudoClass> disabled {PseudoClass::Disabled};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, disabled).opacity().value, .5f);
}

TEST(StyleCompiler, Flex) {
    constexpr char kFlexItemStyles[] = "panel { padding: 1px 2px 3px 4px; min-width: 20px; min-height: 10px; gap: 7px; "
                                       "flex: 2 3 40%; order: -2; } "
                                       "panel.auto { flex: auto; } panel.none { flex: none; } "
                                       "panel.one { flex: 4; } panel.two { flex: 5 6; } panel.basis { flex: 10px; } "
                                       "panel.basis-first { flex: 40% 5 6; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFlexItemStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, {});
    EXPECT_EQ(style.padding().top.pixels, 1.f);
    EXPECT_EQ(style.padding().right.pixels, 2.f);
    EXPECT_EQ(style.padding().bottom.pixels, 3.f);
    EXPECT_EQ(style.padding().left.pixels, 4.f);
    ASSERT_TRUE(style.minWidth().has_value());
    EXPECT_EQ(style.minWidth()->pixels(), 20.f);
    EXPECT_EQ(style.rowGap().fixedPixels(), 7.f);
    EXPECT_EQ(style.flexGrow().value, 2.f);
    EXPECT_EQ(style.flexShrink().value, 3.f);
    EXPECT_NEAR(style.flexBasis().resolve(0.f, 200.f), 80.f, 1.0e-4f);
    EXPECT_EQ(style.order().value, -2);

    const ComputedStyle automatic = stylesheet.resolve("panel", "", {"auto"}, {});
    EXPECT_EQ(automatic.flexGrow().value, 1.f);
    EXPECT_EQ(automatic.flexShrink().value, 1.f);
    EXPECT_TRUE(automatic.flexBasis().isAuto());

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"}, {});
    EXPECT_EQ(none.flexGrow().value, 0.f);
    EXPECT_EQ(none.flexShrink().value, 0.f);
    EXPECT_TRUE(none.flexBasis().isAuto());

    const ComputedStyle one = stylesheet.resolve("panel", "", {"one"}, {});
    EXPECT_EQ(one.flexGrow().value, 4.f);
    EXPECT_TRUE(one.flexBasis().isPercentage());
    EXPECT_EQ(one.flexBasis().resolve(1.f), 0.f);

    const ComputedStyle two = stylesheet.resolve("panel", "", {"two"}, {});
    EXPECT_EQ(two.flexGrow().value, 5.f);
    EXPECT_EQ(two.flexShrink().value, 6.f);

    const ComputedStyle basis = stylesheet.resolve("panel", "", {"basis"}, {});
    EXPECT_EQ(basis.flexGrow().value, 1.f);
    EXPECT_EQ(basis.flexBasis().resolve(0.f), 10.f);

    const ComputedStyle basisFirst = stylesheet.resolve("panel", "", {"basis-first"}, {});
    EXPECT_EQ(basisFirst.flexGrow().value, 5.f);
    EXPECT_EQ(basisFirst.flexShrink().value, 6.f);
    EXPECT_NEAR(basisFirst.flexBasis().resolve(0.f, 200.f), 80.f, 1.0e-4f);
}

TEST(StyleCompiler, RejectsUnitBearingFlexGrow) {
    constexpr char kUnitBearingFlexGrow[] = "panel { flex-grow: 1px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnitBearingFlexGrow);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleCompiler, ProvidesStableStyleDefaults) {
    const ComputedStyle style;

    EXPECT_EQ(style.appearance(), Appearance::NoneValue);
    EXPECT_EQ(style.display(), Display::Inline);
    EXPECT_FALSE(style.displaySet);
    EXPECT_EQ(style.justifyContent().position, ContentPosition::Normal);
    EXPECT_EQ(style.alignItems().position, ItemPosition::Normal);
    EXPECT_EQ(style.alignSelf().position, ItemPosition::Auto);
    EXPECT_EQ(style.alignContent().position, ContentPosition::Normal);
    EXPECT_EQ(style.flexWrap().mode, FlexWrapMode::Nowrap);
    EXPECT_FALSE(style.flexWrap().balance);
    EXPECT_FALSE(style.maxWidth().has_value());
    EXPECT_FALSE(style.maxHeight().has_value());
    EXPECT_EQ(style.flexGrow().value, 0.f);
    EXPECT_EQ(style.flexShrink().value, 1.f);
    EXPECT_TRUE(style.flexBasis().isAuto());
    EXPECT_EQ(style.order().value, 0);
    EXPECT_EQ(style.rowGap().fixedPixels(), 0.f);
    EXPECT_EQ(style.pointerEvents(), PointerEvents::Auto);
    EXPECT_EQ(style.fontFamily(), FontFamilies {GenericFontFamily::SansSerif});
    EXPECT_EQ(style.verticalAlign().value, VerticalAlign::Baseline);
    EXPECT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
}

TEST(StyleCompiler, PreservesAuthoredFloaterParts) {
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater, true, true);
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

TEST(StyleCompiler, ParsesTextShorthands) {
    constexpr char kTextPresentationStyles[] = "panel { letter-spacing: 50%; word-spacing: 25%; text-wrap: nowrap; } "
                                               "p { text-overflow: ellipsis-center; font-width: 75%; } "
                                               "label { font: condensed italic 525.5 17px/21px sans-serif; } "
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
    EXPECT_EQ(inherited.letterSpacing().percent, .5f);
    EXPECT_EQ(inherited.wordSpacing().percent, .25f);
    EXPECT_EQ(inherited.textWrapMode(), TextWrapMode::NoWrap);
    EXPECT_EQ(inherited.textOverflow(), TextOverflow::EllipsisCenter);
    EXPECT_EQ(inherited.fontWidth().percentage, 75.f);

    const ComputedStyle shorthand = stylesheet.resolve("label", "", {}, {});
    EXPECT_EQ(shorthand.fontStyle(), FontStyle::Italic);
    EXPECT_EQ(shorthand.fontWeight().value, 525.5f);
    EXPECT_EQ(shorthand.fontWidth().percentage, 75.f);
    EXPECT_EQ(shorthand.fontSize(), 17.f);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(shorthand.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(shorthand.lineHeight().mValue).pixels, 21.f);

    const ComputedStyle reset = stylesheet.resolve("label", "", {"reset"}, {});
    EXPECT_EQ(reset.fontStyle(), FontStyle::Normal);
    EXPECT_EQ(reset.fontWeight().value, 400.f);
    EXPECT_EQ(reset.fontWidth().percentage, 100.f);
    EXPECT_TRUE(std::holds_alternative<Core::CSS::Keyword::Normal>(reset.lineHeight().mValue));
}

TEST(StyleCompiler, RejectsInvalidTextValues) {
    struct InvalidTextStyleCase {
        const char* name;
        const char* styles;
    };
    const InvalidTextStyleCase cases[] = {
        {"unsupported text overflow", "p { text-overflow: middle; }"},
        {"font shorthand without family", "p { font: 13px; }"},
        {"font width percentage in shorthand", "p { font: 75% 13px sans-serif; }"},
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

TEST(StyleCompiler, TreatsNormalWordSpacingAsZero) {
    constexpr char kNormalWordSpacingStyles[] = "p { word-spacing: normal; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kNormalWordSpacingStyles).ok());
    EXPECT_EQ(stylesheet.resolve("p", "", {}, {}).wordSpacing().pixels, 0.f);
}

TEST(StyleCompiler, RejectsInvalidEdges) {
    struct NonFiniteValueCase {
        const char* property;
        const char* value;
    };
    // clang-format off
    constexpr NonFiniteValueCase cases[] = {
        {"padding", "nan 2px 3px 4px"},
        {"padding", "-1px 2px 3px 4px"},
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

TEST(StyleCompiler, PreservesNestedSyntax) {
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

TEST(StyleCompiler, TokenizesCSSStructuralKinds) {
    const std::string source = R"(<!-- panel { value: rgb(1, fn("}")), url(icon\)name.svg); /* } */ text: "}"; } -->)";
    const TokenStream stream(source);
    std::vector<TokenKind> kinds;
    std::size_t openIndex = 0;
    std::size_t closeIndex = 0;
    std::size_t urlIndex = 0;
    for (std::size_t index = 0; index < stream.tokens().size(); ++index) {
        const auto kind = stream.tokens()[index].kind;
        if (kind != TokenKind::Whitespace)
            kinds.push_back(kind);
        if (kind == TokenKind::OpenBrace)
            openIndex = index;
        if (kind == TokenKind::CloseBrace)
            closeIndex = index;
        if (kind == TokenKind::Url)
            urlIndex = index;
    }

    EXPECT_EQ(kinds.front(), TokenKind::CDO);
    EXPECT_EQ(kinds.back(), TokenKind::CDC);
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), TokenKind::Colon), kinds.end());
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), TokenKind::Semicolon), kinds.end());
    EXPECT_NE(std::find(kinds.begin(), kinds.end(), TokenKind::Comma), kinds.end());
    EXPECT_EQ(stream.text(urlIndex), "url(icon\\)name.svg)");
    ASSERT_NE(openIndex, std::size_t(0));
    ASSERT_NE(closeIndex, std::size_t(0));
    EXPECT_EQ(stream.tokens()[openIndex].matching, closeIndex);
    EXPECT_EQ(stream.tokens()[closeIndex].begin, source.rfind('}'));
    EXPECT_EQ(matchingBlock(source, stream.tokens()[openIndex].begin), stream.tokens()[closeIndex].begin);
}

TEST(StyleCompiler, HandlesUnicodeCodePointsAndCSSValueEscapes) {
    const TokenStream functionStream(R"(r\67 b(1, 2, 3))");
    const auto function = Core::CSS::detail::parseFunction(functionStream, {0, functionStream.tokens().size()});
    ASSERT_TRUE(function.has_value());
    EXPECT_EQ(function->name, "rgb");
    EXPECT_EQ(serializeRange(functionStream, function->body), "1, 2, 3");

    const TokenStream dimensionStream(R"(17p\78)");
    const auto dimension = Core::CSS::detail::parseDimension(dimensionStream, {0, dimensionStream.tokens().size()});
    ASSERT_TRUE(dimension.has_value());
    EXPECT_EQ(dimension->number, "17");
    EXPECT_EQ(dimension->unit, "px");
    EXPECT_EQ(Core::CSS::detail::normalizeKeyword(R"(to t\6f p)"), "to top");

    const std::string urlSource = std::string("url(icon") + std::string("\xC2\x80", 2) + ")";
    const TokenStream urlStream(urlSource);
    ASSERT_EQ(urlStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(urlStream.tokens().front().kind, TokenKind::BadUrl);

    EXPECT_EQ(Core::CSS::detail::sourcePosition("éx", 2), (std::pair<std::size_t, std::size_t> {1, 2}));
}

TEST(StyleCompiler, PreservesTokenBoundariesAcrossComments) {
    const TokenStream stream("foo/**/bar");

    ASSERT_EQ(stream.tokens().size(), std::size_t(2));
    EXPECT_EQ(stream.tokens()[0].kind, TokenKind::Ident);
    EXPECT_EQ(stream.tokens()[1].kind, TokenKind::Ident);
    EXPECT_TRUE(stream.tokens()[1].precededByComment);
    EXPECT_EQ(serializeRange(stream, {0, stream.tokens().size()}), "foo/**/bar");
    EXPECT_EQ(Core::CSS::detail::normalizeKeyword("to/**/right"), "to right");
}

TEST(StyleCompiler, PreservesCSSParseErrorTokensAtEOF) {
    const TokenStream stringStream("\"unterminated");
    ASSERT_EQ(stringStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(stringStream.tokens().front().kind, TokenKind::String);

    const TokenStream urlStream("url(icon.svg");
    ASSERT_EQ(urlStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(urlStream.tokens().front().kind, TokenKind::Url);

    const TokenStream escapedURLStream(R"(u\72l(icon.svg))");
    ASSERT_EQ(escapedURLStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(escapedURLStream.tokens().front().kind, TokenKind::Url);

    const TokenStream escapedCRLFURLStream("u\\72\r\nl(icon.svg)");
    ASSERT_EQ(escapedCRLFURLStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(escapedCRLFURLStream.tokens().front().kind, TokenKind::Url);
    EXPECT_EQ(Core::CSS::detail::decodeIdentifier("u\\72\r\nl"), "url");
    const auto decodedString = Core::CSS::detail::decodeString("\"a\\31\r\nb\"");
    ASSERT_TRUE(decodedString.has_value());
    EXPECT_EQ(*decodedString, "a1b");

    std::string nulSource = "\"A";
    nulSource.push_back('\0');
    nulSource += "B\"";
    const TokenStream nulStream(nulSource);
    ASSERT_EQ(nulStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(nulStream.tokens().front().kind, TokenKind::String);
    EXPECT_EQ(nulStream.text(0),
        "\"A\xEF\xBF\xBD"
        "B\"");

    const std::string surrogateSource = std::string("A") + "\xED\xA0\x80" + "B";
    const TokenStream surrogateStream(surrogateSource);
    ASSERT_EQ(surrogateStream.tokens().size(), std::size_t(1));
    EXPECT_EQ(surrogateStream.tokens().front().kind, TokenKind::Ident);
    EXPECT_EQ(surrogateStream.text(0),
        "A\xEF\xBF\xBD"
        "B");
}

TEST(StyleCompiler, ConsumesInvalidURLRemnants) {
    const TokenStream stream("url(foo\\");

    ASSERT_EQ(stream.tokens().size(), std::size_t(1));
    EXPECT_EQ(stream.tokens().front().kind, TokenKind::BadUrl);
    EXPECT_EQ(stream.tokens().front().end, stream.source().size());
}

TEST(StyleCompiler, ReconsumesNewlineAfterBadString) {
    const TokenStream stream("\"unterminated\nwidth: 1px;");
    ASSERT_GE(stream.tokens().size(), std::size_t(4));
    EXPECT_EQ(stream.tokens()[0].kind, TokenKind::BadString);
    EXPECT_EQ(stream.tokens()[0].end, std::size_t(13));
    EXPECT_EQ(stream.tokens()[1].kind, TokenKind::Whitespace);
    EXPECT_EQ(stream.tokens()[1].begin, std::size_t(13));
    EXPECT_EQ(stream.tokens()[2].kind, TokenKind::Ident);
    EXPECT_EQ(stream.text(2), "width");
}

TEST(StyleCompiler, RejectsUnclosedStyleBlocks) {
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

TEST(StyleCompiler, KeepsPropertyRegistryValid) {
    std::set<std::string_view> names;
    for (const PropertyDefinition* property = legacyPropertyBegin(); property != legacyPropertyEnd(); ++property) {
        const std::optional<Property> cssProperty = Core::CSS::findProperty(property->name);
        bool isShorthand = false;
        if (cssProperty) {
            const std::optional<Core::Style::ShorthandDescriptor> descriptor = Core::Style::shorthand(*cssProperty);
            isShorthand = descriptor.has_value();
            if (descriptor) {
                EXPECT_FALSE(descriptor->properties.empty());
                for (const Property longhand : descriptor->properties)
                    EXPECT_TRUE(Core::CSS::findProperty(Core::CSS::propertyName(longhand)).has_value());
            }
            if (!property->set && !property->initial)
                continue;
        }
        SCOPED_TRACE(Message() << "style property: " << property->name);
        EXPECT_TRUE(names.insert(property->name).second);
        EXPECT_NE(property->compile, nullptr);
        EXPECT_EQ(property->set == nullptr, isShorthand);
        EXPECT_EQ(property->initial == nullptr, isShorthand);
    }

    EXPECT_EQ(findLegacyProperty("line-height"), nullptr);
}
