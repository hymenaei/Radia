/**
 * @file compiler_test.cpp
 * @brief Tests RSL property compilation and shorthand expansion.
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
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "layout/engine.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"
#include "widgets/button.h"
#include "widgets/div.h"
#include "widgets/field.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace {
using radia::ui::AlignItems;
using radia::ui::AlignSelf;
using radia::ui::Button;
using radia::ui::Div;
using radia::ui::DisplayMode;
using radia::ui::Field;
using radia::ui::Floater;
using radia::ui::FlexDirection;
using radia::ui::FontFamily;
using radia::ui::Icon;
using radia::ui::JustifyContent;
using radia::ui::Label;
using radia::ui::Panel;
using radia::ui::PointerEvents;
using radia::ui::resolveWidgetStyle;
using radia::ui::StrokeCap;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Text;
using radia::ui::TextAlign;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::VerticalAlign;
using radia::ui::Visibility;
using radia::ui::WidgetState;
using radia::ui::detail::matchingBlock;
using radia::ui::detail::splitTopLevel;
using radia::ui::detail::stylePropertyBegin;
using radia::ui::detail::StylePropertyDefinition;
using radia::ui::detail::stylePropertyEnd;
using radia::ui::detail::tokenizeTopLevel;
using radia::ui::detail::WidgetCompilerAccess;
using ::testing::Message;
} // namespace

TEST(StyleCompilerTest, ResolvesSelectorsByElementClassStateAndChild) {
    constexpr char kSelectorStyles[] = "button.primary:hover > icon { width: 17px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorStyles).ok());

    Button button;
    button.addClass("primary");
    WidgetCompilerAccess::setState(button, WidgetState::Hovered, true);
    Icon& icon = button.setIcon("search");

    EXPECT_EQ(resolveWidgetStyle(stylesheet, icon).width.pixels(), 17.f);
}

TEST(StyleCompilerTest, ResolvesStructuralDivStyles) {
    constexpr char kDivStyles[] = "div.stack { display: flex; flex-direction: row; gap: 8px; padding: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDivStyles).ok());

    Div div;
    div.addClass("stack");
    const Style style = resolveWidgetStyle(stylesheet, div);
    EXPECT_EQ(style.display, DisplayMode::Flex);
    EXPECT_TRUE(style.displaySet);
    EXPECT_EQ(style.flexDirection, FlexDirection::Row);
    EXPECT_EQ(style.gap.fixedPixels(), 8.f);
    EXPECT_EQ(style.padding.top, 2.f);
}

TEST(StyleCompilerTest, ResolvesDisplayAndInheritedVisibility) {
    constexpr char kDisplayStyles[] = "panel.flex { display: flex; flex-direction: column; } "
                                      "panel.inline { display: inline; } panel.none { display: none; } "
                                      "panel.hidden { visibility: hidden; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDisplayStyles).ok());

    Panel flex;
    flex.addClass("flex");
    const Style flexStyle = resolveWidgetStyle(stylesheet, flex);
    EXPECT_EQ(flexStyle.display, DisplayMode::Flex);
    EXPECT_EQ(flexStyle.flexDirection, FlexDirection::Column);

    Panel inlinePanel;
    inlinePanel.addClass("inline");
    const Style inlineStyle = resolveWidgetStyle(stylesheet, inlinePanel);
    EXPECT_EQ(inlineStyle.display, DisplayMode::Inline);

    Panel none;
    none.addClass("none");
    EXPECT_EQ(resolveWidgetStyle(stylesheet, none).display, DisplayMode::NoneValue);

    Panel hidden;
    hidden.addClass("hidden");
    auto child = std::make_unique<Label>("child");
    Label* childPtr = child.get();
    hidden.addChild(std::move(child));
    EXPECT_EQ(resolveWidgetStyle(stylesheet, *childPtr).visibility, Visibility::Hidden);
}

TEST(StyleCompilerTest, UsesCssInitialAndDivDisplayDefaults) {
    StyleSheet stylesheet;
    Panel panel;
    Div div;
    Text paragraph;

    const Style panelStyle = resolveWidgetStyle(stylesheet, panel);
    const Style divStyle = resolveWidgetStyle(stylesheet, div);
    const Style paragraphStyle = resolveWidgetStyle(stylesheet, paragraph);
    EXPECT_EQ(panelStyle.display, DisplayMode::Inline);
    EXPECT_FALSE(panelStyle.displaySet);
    EXPECT_EQ(divStyle.display, DisplayMode::Block);
    EXPECT_FALSE(divStyle.displaySet);
    EXPECT_EQ(paragraphStyle.display, DisplayMode::Block);
    EXPECT_FALSE(paragraphStyle.displaySet);

    ASSERT_TRUE(stylesheet.loadRadia("div.inline { display: inline; }").ok());
    div.addClass("inline");
    const Style authoredDivStyle = resolveWidgetStyle(stylesheet, div);
    EXPECT_EQ(authoredDivStyle.display, DisplayMode::Inline);
    EXPECT_TRUE(authoredDivStyle.displaySet);
}

TEST(StyleCompilerTest, ResolvesColorAndLengthTokens) {
    constexpr char kTokenStyles[] = ":root { --accent: #204060ff; --space: 12px; } "
                                    "button { background-color: var(--accent); padding: var(--space); "
                                    "border-radius: 5px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTokenStyles).ok());
    const Style style = stylesheet.resolve("button", "", {}, 0);

    EXPECT_NEAR(style.backgroundColor.b, 96.f / 255.f, 1.0e-4f);
    EXPECT_EQ(style.padding.left, 12.f);
    EXPECT_EQ(style.borderRadius, 5.f);
}

TEST(StyleCompilerTest, UsesLaterDeclarationsWithoutDiscardingShorthandValues) {
    constexpr char kDeclarationOrderStyles[] = "button { width: 10px; size: 20px 30px; width: 40px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDeclarationOrderStyles).ok());
    const Style style = stylesheet.resolve("button", "", {}, 0);

    EXPECT_EQ(style.width.pixels(), 40.f);
    EXPECT_EQ(style.height.pixels(), 20.f);
}

TEST(StyleCompilerTest, AppliesIdAndClassSpecificityBeforeElementRules) {
    constexpr char kSpecificityStyles[] = "button.primary { width: 30px; } button { width: 10px; } "
                                          "#save { width: 50px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSpecificityStyles).ok());
    const std::set<std::string> classes{"primary"};

    EXPECT_EQ(stylesheet.resolve("button", "save", classes, 0).width.pixels(), 50.f);
    EXPECT_EQ(stylesheet.resolve("button", "", classes, 0).width.pixels(), 30.f);
}

TEST(StyleCompilerTest, ResolvesNestedStateAndChildSelectors) {
    constexpr char kNestedStyles[] = "button { background-color: #101010ff; &:hover { background-color: #202020ff; } "
                                     "> icon { size: 16px; } &:hover > icon { stroke-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kNestedStyles).ok());
    const uint8_t hover = static_cast<uint8_t>(WidgetState::Hovered) | static_cast<uint8_t>(WidgetState::Default);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, hover).backgroundColor.r, 32.f / 255.f);

    Button button;
    WidgetCompilerAccess::setState(button, WidgetState::Hovered, true);
    const Style iconStyle = resolveWidgetStyle(stylesheet, button.setIcon("search"));
    EXPECT_EQ(iconStyle.width.pixels(), 16.f);
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 3.f);
}

TEST(StyleCompilerTest, ParsesContainerAndTextAlignmentEnums) {
    constexpr char kAlignmentStyles[] = "panel { display: flex; flex-direction: row; vertical-align: middle; pointer-events: none; } "
                                        "label { text-align: right; pointer-events: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kAlignmentStyles).ok());
    const Style panel = stylesheet.resolve("panel", "", {}, 0);
    const Style label = stylesheet.resolve("label", "", {}, 0);

    EXPECT_EQ(panel.display, DisplayMode::Flex);
    EXPECT_EQ(panel.pointerEvents, PointerEvents::PassThrough);
    EXPECT_EQ(label.textAlign, TextAlign::Right);
    EXPECT_EQ(panel.verticalAlign, VerticalAlign::Middle);
    EXPECT_EQ(label.verticalAlign, VerticalAlign::Top);
    EXPECT_EQ(label.pointerEvents, PointerEvents::Auto);
}

TEST(StyleCompilerTest, ParsesLogicalTextAndCrossAxisAlignmentEnums) {
    constexpr char kCrossAxisStyles[] = "label { text-align: start; } panel { align-items: end; } "
                                        "panel.normal { align-items: normal; } button { align-self: start; } "
                                        "button.auto { align-self: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCrossAxisStyles).ok());

    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).textAlign, TextAlign::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).alignItems, AlignItems::End);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).alignSelf, AlignSelf::Start);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"normal"}, 0).alignItems, AlignItems::Normal);
    EXPECT_EQ(stylesheet.resolve("button", "", {"auto"}, 0).alignSelf, AlignSelf::Auto);
}

TEST(StyleCompilerTest, ParsesIndependentTypographyPropertiesAndVariableWeights) {
    constexpr char kTypographyStyles[] = "label#a { font-family: sans; font-size: 19px; "
                                         "font-weight: bold; font-style: italic; }";
    constexpr char kVariableWeightStyles[] = "label { font-weight: 525; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypographyStyles).ok());
    const Style label = stylesheet.resolve("label", "a", {}, 0);
    EXPECT_EQ(label.fontFamily, FontFamily::Sans);
    EXPECT_EQ(label.fontSize, 19.f);
    EXPECT_EQ(label.fontWeight, static_cast<U16>(700));
    EXPECT_TRUE(label.fontItalic);

    StyleSheet variableWeight;
    ASSERT_TRUE(variableWeight.loadRadia(kVariableWeightStyles).ok());
    EXPECT_EQ(variableWeight.resolve("label", "", {}, 0).fontWeight, static_cast<U16>(525));
}

TEST(StyleCompilerTest, RejectsInvalidTypographyForms) {
    struct InvalidTypographyCase {
        const char* name;
        const char* styles;
    };
    const InvalidTypographyCase cases[] = {
        {"pseudo font family", "label { font-family: sans-bold; }"},
        {"fractional weight", "label { font-weight: 525.5; }"},
        {"unit-bearing weight", "label { font-weight: 700px; }"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid typography case: " << test.name);
        StyleSheet stylesheet;
        EXPECT_FALSE(stylesheet.loadRadia(test.styles).ok());
    }
}

TEST(StyleCompilerTest, ParsesBordersAndSvgStrokeProperties) {
    constexpr char kBorderStyles[] = "button { border: 1px #112233ff; border-width: 2px 3px; "
                                     "border-color: #ffffffff; } "
                                     "button > icon { stroke: 4px #abcdef88; stroke-linecap: square; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBorderStyles).ok());
    const Style buttonStyle = stylesheet.resolve("button", "", {}, 0);
    EXPECT_EQ(buttonStyle.borderWidth.top, 2.f);
    EXPECT_EQ(buttonStyle.borderWidth.right, 3.f);
    EXPECT_EQ(buttonStyle.borderColor.r, 1.f);

    Button button;
    const Style iconStyle = resolveWidgetStyle(stylesheet, button.setIcon("search"));
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 4.f);
    EXPECT_EQ(iconStyle.svgStrokeCap, StrokeCap::Square);
}

TEST(StyleCompilerTest, RejectsUnsupportedDisplayValuesWithoutCommittingThem) {
    constexpr char kUnsupportedDisplayStyles[] = "panel { display: grid; } panel#bad { display: sideways; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnsupportedDisplayStyles, "test.radia");

    ASSERT_FALSE(result.ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).display, DisplayMode::Inline);
    EXPECT_EQ(stylesheet.resolve("panel", "bad", {}, 0).display, DisplayMode::Inline);
    EXPECT_TRUE(result.warnings.empty());
    ASSERT_EQ(result.errors.size(), std::size_t(2));
    EXPECT_EQ(result.errors.front().source, "test.radia");
}

TEST(StyleCompilerTest, AppliesSelectorListsChildRulesAndStates) {
    constexpr char kSelectorListStyles[] = "button, switch { height: 32px; } button > icon { width: 14px; } "
                                           "button:disabled { opacity: .5; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorListStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).height.pixels(), 32.f);
    EXPECT_EQ(stylesheet.resolve("switch", "", {}, 0).height.pixels(), 32.f);

    Button button;
    EXPECT_EQ(resolveWidgetStyle(stylesheet, button.setIcon("search")).width.pixels(), 14.f);
    const uint8_t disabled = static_cast<uint8_t>(WidgetState::Disabled);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, disabled).opacity, .5f);
}

TEST(StyleCompilerTest, ParsesFlexItemShorthands) {
    constexpr char kFlexItemStyles[] = "panel { padding: 1px 2px 3px 4px; min-width: 20px; min-height: 10px; gap: 7px; "
                                       "flex: 2 3 40%; order: -2; } "
                                       "panel.auto { flex: auto; } panel.none { flex: none; } "
                                       "panel.one { flex: 4; } panel.two { flex: 5 6; } panel.basis { flex: 10px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFlexItemStyles).ok());
    const Style style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(style.padding.top, 1.f);
    EXPECT_EQ(style.padding.right, 2.f);
    EXPECT_EQ(style.padding.bottom, 3.f);
    EXPECT_EQ(style.padding.left, 4.f);
    ASSERT_TRUE(style.minWidth.has_value());
    EXPECT_EQ(style.minWidth->pixels, 20.f);
    EXPECT_EQ(style.gap.fixedPixels(), 7.f);
    EXPECT_EQ(style.flexGrow, 2.f);
    EXPECT_EQ(style.flexShrink, 3.f);
    EXPECT_NEAR(style.flexBasis.resolve(0.f, 200.f), 80.f, 1.0e-4f);
    EXPECT_EQ(style.order, -2);

    const Style automatic = stylesheet.resolve("panel", "", {"auto"}, 0);
    EXPECT_EQ(automatic.flexGrow, 1.f);
    EXPECT_EQ(automatic.flexShrink, 1.f);
    EXPECT_TRUE(automatic.flexBasis.isAuto());

    const Style none = stylesheet.resolve("panel", "", {"none"}, 0);
    EXPECT_EQ(none.flexGrow, 0.f);
    EXPECT_EQ(none.flexShrink, 0.f);
    EXPECT_TRUE(none.flexBasis.isAuto());

    const Style one = stylesheet.resolve("panel", "", {"one"}, 0);
    EXPECT_EQ(one.flexGrow, 4.f);
    EXPECT_EQ(one.flexBasis.resolve(1.f), 0.f);

    const Style two = stylesheet.resolve("panel", "", {"two"}, 0);
    EXPECT_EQ(two.flexGrow, 5.f);
    EXPECT_EQ(two.flexShrink, 6.f);

    const Style basis = stylesheet.resolve("panel", "", {"basis"}, 0);
    EXPECT_EQ(basis.flexGrow, 1.f);
    EXPECT_EQ(basis.flexBasis.resolve(0.f), 10.f);
}

TEST(StyleCompilerTest, RejectsLegacyAndUnitBearingFlexProperties) {
    struct InvalidFlowCase {
        const char* name;
        const char* styles;
    };
    const InvalidFlowCase cases[] = {
        {"legacy grow property", "panel { grow: 1; }"},
        {"unit-bearing flex-grow", "panel { flex-grow: 1px; }"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid layout property case: " << test.name);
        StyleSheet stylesheet;
        EXPECT_FALSE(stylesheet.loadRadia(test.styles).ok());
    }
}

TEST(StyleCompilerTest, ProvidesStableStyleDefaults) {
    const Style style;

    EXPECT_EQ(style.display, DisplayMode::Inline);
    EXPECT_FALSE(style.displaySet);
    EXPECT_EQ(style.justifyContent, JustifyContent::Start);
    EXPECT_EQ(style.alignItems, AlignItems::Normal);
    EXPECT_EQ(style.alignSelf, AlignSelf::Auto);
    EXPECT_EQ(style.flexGrow, 0.f);
    EXPECT_EQ(style.flexShrink, 1.f);
    EXPECT_TRUE(style.flexBasis.isAuto());
    EXPECT_EQ(style.order, 0);
    EXPECT_FALSE(style.gap.isAuto());
    EXPECT_EQ(style.gap.fixedPixels(), 0.f);
    EXPECT_EQ(style.pointerEvents, PointerEvents::Default);
    EXPECT_EQ(style.fontFamily, FontFamily::Sans);
    EXPECT_EQ(style.verticalAlign, VerticalAlign::Top);
    EXPECT_EQ(style.backgroundColor.a, 0.f);
}

TEST(StyleCompilerTest, AppliesIntrinsicButtonLayoutAndAuthoredOverrides) {
    constexpr char kButtonOverrideStyles[] = "button { display: flex; flex-direction: column; justify-content: end; vertical-align: top; }";

    StyleSheet stylesheet;
    Button button;
    const Style defaults = resolveWidgetStyle(stylesheet, button);
    EXPECT_EQ(defaults.display, DisplayMode::Flex);
    EXPECT_EQ(defaults.flexDirection, FlexDirection::Row);
    EXPECT_EQ(defaults.justifyContent, JustifyContent::Center);
    EXPECT_EQ(defaults.verticalAlign, VerticalAlign::Middle);

    ASSERT_TRUE(stylesheet.loadRadia(kButtonOverrideStyles).ok());
    const Style authored = resolveWidgetStyle(stylesheet, button);
    EXPECT_EQ(authored.display, DisplayMode::Flex);
    EXPECT_EQ(authored.flexDirection, FlexDirection::Column);
    EXPECT_EQ(authored.justifyContent, JustifyContent::End);
    EXPECT_EQ(authored.verticalAlign, VerticalAlign::Top);
}

TEST(StyleCompilerTest, AppliesIntrinsicFloaterAndFieldAlignment) {
    constexpr char kFieldOverrideStyles[] = "field { vertical-align: bottom; }";

    StyleSheet stylesheet;
    Floater floater;
    Panel* customHeader = nullptr;
    for (const auto& child : floater.header()->children())
        if (child->part() == "header::custom") customHeader = static_cast<Panel*>(child.get());
    ASSERT_NE(customHeader, nullptr);
    EXPECT_EQ(resolveWidgetStyle(stylesheet, *floater.header()).verticalAlign, VerticalAlign::Middle);
    EXPECT_EQ(resolveWidgetStyle(stylesheet, *customHeader).verticalAlign, VerticalAlign::Middle);

    Field field;
    EXPECT_EQ(resolveWidgetStyle(stylesheet, field).verticalAlign, VerticalAlign::Middle);
    ASSERT_TRUE(stylesheet.loadRadia(kFieldOverrideStyles).ok());
    EXPECT_EQ(resolveWidgetStyle(stylesheet, field).verticalAlign, VerticalAlign::Bottom);
}

TEST(StyleCompilerTest, ParsesTextPresentationAndFontShorthands) {
    constexpr char kTextPresentationStyles[] = "panel { letter-spacing: 50%; word-spacing: 25%; text-wrap: nowrap; } "
                                               "p { text-overflow: ellipsis-center; } "
                                               "label { font: italic 525 17px/21px sans; } "
                                               "label.reset { font-style: italic; font-weight: bold; "
                                               "line-height: 30px; font: 12px sans; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTextPresentationStyles).ok());

    auto parent = std::make_unique<Panel>();
    auto text = std::make_unique<Text>("inventory item");
    Text* child = text.get();
    parent->addChild(std::move(text));
    const Style inherited = resolveWidgetStyle(stylesheet, *child);
    EXPECT_EQ(inherited.letterSpacing.percent, .5f);
    EXPECT_EQ(inherited.wordSpacing.percent, .25f);
    EXPECT_EQ(inherited.textWrap, TextWrap::NoWrap);
    EXPECT_EQ(inherited.textOverflow, TextOverflow::EllipsisCenter);

    const Style shorthand = stylesheet.resolve("label", "", {}, 0);
    EXPECT_TRUE(shorthand.fontItalic);
    EXPECT_EQ(shorthand.fontWeight, static_cast<U16>(525));
    EXPECT_EQ(shorthand.fontSize, 17.f);
    ASSERT_TRUE(shorthand.lineHeight.has_value());
    EXPECT_EQ(shorthand.lineHeight->pixels, 21.f);

    const Style reset = stylesheet.resolve("label", "", {"reset"}, 0);
    EXPECT_FALSE(reset.fontItalic);
    EXPECT_EQ(reset.fontWeight, static_cast<U16>(400));
    EXPECT_FALSE(reset.lineHeight.has_value());
}

TEST(StyleCompilerTest, RejectsInvalidFontAndTextOverflowValues) {
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
        EXPECT_FALSE(stylesheet.loadRadia(test.styles).ok());
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
        EXPECT_FALSE(stylesheet.loadRadia(styles, "nonfinite-edge.radia").ok());
    }
}

TEST(StyleCompilerTest, TokenizesTopLevelValuesWithoutSplittingNestedSyntax) {
    const std::vector<std::string> tokens = tokenizeTopLevel("italic 17px/21px sans", true);
    ASSERT_EQ(tokens.size(), std::size_t(5));
    EXPECT_EQ(tokens[2], "/");
    EXPECT_TRUE(tokenizeTopLevel("var(--accent", true).empty());
    EXPECT_TRUE(splitTopLevel("rgb(1, 2)), blue", ',').empty());
}

TEST(StyleCompilerTest, FindsNestedStyleBlocksAndRejectsUnclosedBlocks) {
    const std::string kNestedStyles = "button { icon { width: 1px; } }";
    const std::size_t open = kNestedStyles.find('{');
    ASSERT_NE(open, std::string::npos);
    const std::optional<std::size_t> close = matchingBlock(kNestedStyles, open);

    ASSERT_TRUE(close.has_value());
    EXPECT_EQ(*close, kNestedStyles.size() - 1);
    EXPECT_FALSE(matchingBlock("button {", 7).has_value());
}

TEST(StyleCompilerTest, KeepsStylePropertyRegistryCompleteAndConsistent) {
    const std::set<std::string_view> shorthandNames{"font", "flex", "min-size", "overflow"};
    std::set<std::string_view> names;
    for (const StylePropertyDefinition* property = stylePropertyBegin(); property != stylePropertyEnd(); ++property) {
        SCOPED_TRACE(Message() << "style property: " << property->name);
        EXPECT_TRUE(names.insert(property->name).second);
        EXPECT_NE(property->compile, nullptr);
        EXPECT_EQ(property->apply == nullptr, shorthandNames.count(property->name) != 0);
    }

    EXPECT_EQ(names.size(), std::size_t(55));
}
