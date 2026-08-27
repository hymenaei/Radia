/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
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
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "elements/elementinternal.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "../floater_test_helpers.h"
#include "layout/engine.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"

namespace {
using radia::ui::AlignItems;
using radia::ui::AlignSelf;
using radia::ui::AppearanceMode;
using radia::ui::ButtonElement;
using radia::ui::DisplayMode;
using radia::ui::Element;
using radia::ui::ElementState;
using radia::ui::FlexDirection;
using radia::ui::FloaterElement;
using radia::ui::FontFamily;
using radia::ui::IconElement;
using radia::ui::InputElement;
using radia::ui::JustifyContent;
using radia::ui::JustifySelf;
using radia::ui::LabelElement;
using radia::ui::PanelElement;
using radia::ui::PointerEvents;
using radia::ui::PositionMode;
using radia::ui::resolveElementStyle;
using radia::ui::StrokeCap;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::TextAlign;
using radia::ui::TextDecoration;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::VerticalAlign;
using radia::ui::Visibility;
using radia::ui::detail::ElementCompilerAccess;
using radia::ui::detail::matchingBlock;
using radia::ui::detail::splitTopLevel;
using radia::ui::detail::stylePropertyBegin;
using radia::ui::detail::StylePropertyDefinition;
using radia::ui::detail::stylePropertyEnd;
using radia::ui::detail::tokenizeTopLevel;
using ::testing::Message;

IconElement& appendIcon(ButtonElement& button, std::string name) {
    auto icon = std::make_unique<IconElement>(std::move(name));
    IconElement* result = icon.get();
    button.append(std::move(icon));
    return *result;
}

void loadCoreStylesheet(StyleSheet& stylesheet) {
    ASSERT_TRUE(stylesheet.loadRadia(std::string(radia::ui::defaultStylesheetSource()), std::string(radia::ui::kDefaultStylesheetResourceId)).ok());
}
} // namespace

TEST(StyleCompilerTest, ResolvesSelectorsByElementClassStateAndChild) {
    constexpr char kSelectorStyles[] = "button.primary:hover > icon { width: 17px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorStyles).ok());

    ButtonElement button;
    button.addClass("primary");
    ElementCompilerAccess::setState(button, ElementState::Hovered, true);
    IconElement& icon = appendIcon(button, "search");

    EXPECT_EQ(resolveElementStyle(stylesheet, icon).width.pixels(), 17.f);
}

TEST(StyleCompilerTest, ResolvesStructuralDivStyles) {
    constexpr char kDivStyles[] = "div.stack { display: flex; flex-direction: row; gap: 8px; padding: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDivStyles).ok());

    const auto* definition = radia::ui::findElementDefinition(radia::ui::Tag::Div);
    ASSERT_NE(definition, nullptr);
    auto div = definition->create();
    ASSERT_NE(div, nullptr);
    div->addClass("stack");
    const Style style = resolveElementStyle(stylesheet, *div);
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

    PanelElement flex;
    flex.addClass("flex");
    const Style flexStyle = resolveElementStyle(stylesheet, flex);
    EXPECT_EQ(flexStyle.display, DisplayMode::Flex);
    EXPECT_EQ(flexStyle.flexDirection, FlexDirection::Column);

    PanelElement inlinePanel;
    inlinePanel.addClass("inline");
    const Style inlineStyle = resolveElementStyle(stylesheet, inlinePanel);
    EXPECT_EQ(inlineStyle.display, DisplayMode::Inline);

    PanelElement none;
    none.addClass("none");
    EXPECT_EQ(resolveElementStyle(stylesheet, none).display, DisplayMode::NoneValue);

    PanelElement hidden;
    hidden.addClass("hidden");
    auto child = std::make_unique<LabelElement>("child");
    LabelElement* childPtr = child.get();
    hidden.append(std::move(child));
    EXPECT_EQ(resolveElementStyle(stylesheet, *childPtr).visibility, Visibility::Hidden);
}

TEST(StyleCompilerTest, UsesCssInitialAndDivDisplayDefaults) {
    StyleSheet stylesheet;
    const std::string source(radia::ui::defaultStylesheetSource());
    ASSERT_TRUE(stylesheet.loadRadia(source, std::string(radia::ui::kDefaultStylesheetResourceId)).ok());
    PanelElement panel;
    const auto* definition = radia::ui::findElementDefinition(radia::ui::Tag::Div);
    ASSERT_NE(definition, nullptr);
    auto div = definition->create();
    ASSERT_NE(div, nullptr);
    Element paragraph("p");

    const Style panelStyle = resolveElementStyle(stylesheet, panel);
    const Style divStyle = resolveElementStyle(stylesheet, *div);
    const Style paragraphStyle = resolveElementStyle(stylesheet, paragraph);
    EXPECT_EQ(panelStyle.display, DisplayMode::Inline);
    EXPECT_FALSE(panelStyle.displaySet);
    EXPECT_EQ(divStyle.display, DisplayMode::Block);
    EXPECT_TRUE(divStyle.displaySet);
    EXPECT_EQ(paragraphStyle.display, DisplayMode::Block);
    EXPECT_TRUE(paragraphStyle.displaySet);

    ASSERT_TRUE(
        stylesheet
            .loadRadiaLayers({
                radia::ui::StyleLayer{radia::ui::StyleOrigin::Default,
                                      radia::ui::ResourceLayer{std::string(radia::ui::kDefaultStylesheetResourceId), source}},
                radia::ui::StyleLayer{radia::ui::StyleOrigin::Skin, radia::ui::ResourceLayer{"test.css", "div.inline { display: inline; }"}},
            })
            .ok());
    div->addClass("inline");
    const Style authoredDivStyle = resolveElementStyle(stylesheet, *div);
    EXPECT_EQ(authoredDivStyle.display, DisplayMode::Inline);
    EXPECT_TRUE(authoredDivStyle.displaySet);
    EXPECT_EQ(resolveElementStyle(stylesheet, paragraph).display, DisplayMode::Block);
    EXPECT_TRUE(resolveElementStyle(stylesheet, paragraph).displaySet);
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
    EXPECT_EQ(style.borderRadius.topLeft.horizontal.pixels, 5.f);
    EXPECT_EQ(style.borderRadius.topLeft.vertical.pixels, 5.f);
}

TEST(StyleCompilerTest, ResolvesPercentageBorderRadius) {
    constexpr char kPercentageRadiusStyles[] = "input { border-radius: 100%; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kPercentageRadiusStyles).ok());
    const Style style = stylesheet.resolve("input", "", {}, 0);

    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.horizontal.resolve(20.f), 20.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.pixels, 0.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.percent, 1.f);
    EXPECT_FLOAT_EQ(style.borderRadius.topLeft.vertical.resolve(20.f), 20.f);
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
    const uint16_t hover = static_cast<uint16_t>(ElementState::Hovered) | static_cast<uint16_t>(ElementState::Default);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, hover).backgroundColor.r, 32.f / 255.f);

    ButtonElement button;
    ElementCompilerAccess::setState(button, ElementState::Hovered, true);
    const Style iconStyle = resolveElementStyle(stylesheet, appendIcon(button, "search"));
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

TEST(StyleCompilerTest, ParsesGridSelfAlignment) {
    constexpr char kGridAlignmentStyles[] = "input { justify-self: center; } button.start { justify-self: start; } "
                                             "button.end { justify-self: end; } label.stretch { justify-self: stretch; } "
                                             "label.auto { justify-self: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kGridAlignmentStyles).ok());

    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0).justifySelf, JustifySelf::Center);
    EXPECT_EQ(stylesheet.resolve("button", "", {"start"}, 0).justifySelf, JustifySelf::Start);
    EXPECT_EQ(stylesheet.resolve("button", "", {"end"}, 0).justifySelf, JustifySelf::End);
    EXPECT_EQ(stylesheet.resolve("label", "", {"stretch"}, 0).justifySelf, JustifySelf::Stretch);
    EXPECT_EQ(stylesheet.resolve("label", "", {"auto"}, 0).justifySelf, JustifySelf::Auto);
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

TEST(StyleCompilerTest, SuppliesUserAgentDecorationForSemanticTextElements) {
    StyleSheet stylesheet;
    loadCoreStylesheet(stylesheet);
    EXPECT_EQ(stylesheet.resolve("s", "", {}, 0).textDecoration, TextDecoration::LineThrough);
    EXPECT_EQ(stylesheet.resolve("del", "", {}, 0).textDecoration, TextDecoration::LineThrough);
    EXPECT_EQ(stylesheet.resolve("u", "", {}, 0).textDecoration, TextDecoration::Underline);
    EXPECT_EQ(stylesheet.resolve("ins", "", {}, 0).textDecoration, TextDecoration::Underline);
    EXPECT_EQ(stylesheet.resolve("strong", "", {}, 0).fontWeight, static_cast<U16>(700));
    EXPECT_TRUE(stylesheet.resolve("em", "", {}, 0).fontItalic);
    EXPECT_TRUE(stylesheet.resolve("cite", "", {}, 0).fontItalic);
    EXPECT_TRUE(stylesheet.resolve("dfn", "", {}, 0).fontItalic);
    EXPECT_EQ(stylesheet.resolve("small", "", {}, 0).fontSize, 11.f);
    EXPECT_NEAR(stylesheet.resolve("mark", "", {}, 0).backgroundColor.r, 1.f, 1.0e-4f);
    EXPECT_NEAR(stylesheet.resolve("mark", "", {}, 0).backgroundColor.g, 245.f / 255.f, 1.0e-4f);
}

TEST(StyleCompilerTest, SuppliesCoreSwitchStructure) {
    StyleSheet stylesheet;
    loadCoreStylesheet(stylesheet);

    InputElement switchInput;
    switchInput.type("checkbox").switchMode(true);
    const Style owner = resolveElementStyle(stylesheet, switchInput);

    EXPECT_EQ(owner.appearance, AppearanceMode::Auto);
    EXPECT_EQ(owner.display, DisplayMode::Inline);
    EXPECT_TRUE(owner.displaySet);
    EXPECT_TRUE(resolveElementStyle(stylesheet, *switchInput.track()).width.isAuto());
    EXPECT_EQ(resolveElementStyle(stylesheet, *switchInput.thumb()).order, 0);
}

TEST(StyleCompilerTest, AppliesInitialToLonghandsAndShorthands) {
    constexpr char kInitialStyles[] = "panel { display: flex; margin: 4px; padding: 5px; size: 20px 30px; min-size: 2px 3px; "
                                      "flex: 2 3 4px; overflow: hidden; font: italic 700 21px/25px sans; text-color: #abcdef; } "
                                      "panel.reset { display: initial; margin: initial; padding: initial; size: initial; min-size: initial; "
                                      "flex: initial; overflow: initial; font: initial; text-color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    PanelElement reset;
    reset.addClass("reset");
    const Style style = resolveElementStyle(stylesheet, reset);

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
    EXPECT_FALSE(style.lineHeight.has_value());
    EXPECT_EQ(style.fontFamily, FontFamily::Sans);
    EXPECT_FLOAT_EQ(style.textColor.r, 0.f);
    EXPECT_FLOAT_EQ(style.textColor.g, 0.f);
    EXPECT_FLOAT_EQ(style.textColor.b, 0.f);
}

TEST(StyleCompilerTest, InitialOverridesInheritedValue) {
    constexpr char kInitialStyles[] = "panel { font-size: 22px; } label { font-size: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    PanelElement panel;
    auto label = std::make_unique<LabelElement>("label");
    LabelElement* labelPtr = label.get();
    panel.append(std::move(label));

    EXPECT_EQ(resolveElementStyle(stylesheet, panel).fontSize, 22.f);
    EXPECT_EQ(resolveElementStyle(stylesheet, *labelPtr).fontSize, 13.f);
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

    ButtonElement button;
    const Style iconStyle = resolveElementStyle(stylesheet, appendIcon(button, "search"));
    ASSERT_TRUE(iconStyle.svgStrokeWidth.has_value());
    EXPECT_EQ(iconStyle.svgStrokeWidth->pixels, 4.f);
    EXPECT_EQ(iconStyle.svgStrokeCap, StrokeCap::Square);
}

TEST(StyleCompilerTest, ResolvesGridSwitchPresentationProperties) {
    constexpr char kGridSwitchStyles[] =
        "input.basic-switch { appearance: none; display: inline-grid; position: relative; }"
        "input.basic-switch::track { grid-area: 1 / 1; box-shadow: 0 0 5px rgb(0, 0, 0, .3); }"
        "input.basic-switch::thumb { grid-area: 1/1; translate: 22px 0; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kGridSwitchStyles).ok());

    InputElement switchInput;
    switchInput.type("checkbox").switchMode(true).addClass("basic-switch");
    const Style owner = resolveElementStyle(stylesheet, switchInput);
    const Style track = resolveElementStyle(stylesheet, *switchInput.track());
    const Style thumb = resolveElementStyle(stylesheet, *switchInput.thumb());

    EXPECT_EQ(owner.appearance, AppearanceMode::Unstyled);
    EXPECT_EQ(owner.display, DisplayMode::InlineGrid);
    EXPECT_EQ(owner.position, PositionMode::Relative);
    ASSERT_TRUE(track.gridArea.has_value());
    EXPECT_EQ(track.gridArea->row, 1);
    EXPECT_EQ(track.gridArea->column, 1);
    ASSERT_EQ(track.shadows.size(), std::size_t(1));
    EXPECT_NEAR(track.shadows.front().blur, 5.f, 1.0e-4f);
    EXPECT_EQ(thumb.translate.x, 22.f);
    EXPECT_EQ(thumb.translate.y, 0.f);
}

TEST(StyleCompilerTest, RejectsUnsupportedDisplayValuesWithoutCommittingThem) {
    constexpr char kUnsupportedDisplayStyles[] = "panel { display: sideways; } panel#bad { display: sideways; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kUnsupportedDisplayStyles, "test.css");

    ASSERT_FALSE(result.ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).display, DisplayMode::Inline);
    EXPECT_EQ(stylesheet.resolve("panel", "bad", {}, 0).display, DisplayMode::Inline);
    EXPECT_TRUE(result.warnings.empty());
    ASSERT_EQ(result.errors.size(), std::size_t(2));
    EXPECT_EQ(result.errors.front().source, "test.css");
}

TEST(StyleCompilerTest, AppliesSelectorListsChildRulesAndStates) {
    constexpr char kSelectorListStyles[] = "button, input { height: 32px; } button > icon { width: 14px; } "
                                           "button:disabled { opacity: .5; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kSelectorListStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).height.pixels(), 32.f);
    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0).height.pixels(), 32.f);

    ButtonElement button;
    EXPECT_EQ(resolveElementStyle(stylesheet, appendIcon(button, "search")).width.pixels(), 14.f);
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

TEST(StyleCompilerTest, RejectsUnitBearingFlexGrow) {
    constexpr char kUnitBearingFlexGrow[] = "panel { flex-grow: 1px; }";

    StyleSheet stylesheet;
    EXPECT_FALSE(stylesheet.loadRadia(kUnitBearingFlexGrow).ok());
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
    loadCoreStylesheet(stylesheet);
    ButtonElement button;
    const Style defaults = resolveElementStyle(stylesheet, button);
    EXPECT_EQ(defaults.display, DisplayMode::Flex);
    EXPECT_EQ(defaults.flexDirection, FlexDirection::Row);
    EXPECT_EQ(defaults.justifyContent, JustifyContent::Center);
    EXPECT_EQ(defaults.verticalAlign, VerticalAlign::Middle);

    ASSERT_TRUE(stylesheet.loadRadia(kButtonOverrideStyles).ok());
    const Style authored = resolveElementStyle(stylesheet, button);
    EXPECT_EQ(authored.display, DisplayMode::Flex);
    EXPECT_EQ(authored.flexDirection, FlexDirection::Column);
    EXPECT_EQ(authored.justifyContent, JustifyContent::End);
    EXPECT_EQ(authored.verticalAlign, VerticalAlign::Top);
}

TEST(StyleCompilerTest, KeepsFloaterHeadAndControlsAsAuthoredElements) {
    StyleSheet stylesheet;
    loadCoreStylesheet(stylesheet);
    FloaterElement floater;
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

TEST(StyleCompilerTest, ParsesTextPresentationAndFontShorthands) {
    constexpr char kTextPresentationStyles[] = "panel { letter-spacing: 50%; word-spacing: 25%; text-wrap: nowrap; } "
                                               "p { text-overflow: ellipsis-center; } "
                                               "label { font: italic 525 17px/21px sans; } "
                                               "label.reset { font-style: italic; font-weight: bold; "
                                               "line-height: 30px; font: 12px sans; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTextPresentationStyles).ok());

    auto parent = std::make_unique<PanelElement>();
    auto text = std::make_unique<Element>("p");
    Element* child = text.get();
    child->textContent("inventory item");
    parent->append(std::move(text));
    const Style inherited = resolveElementStyle(stylesheet, *child);
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
        EXPECT_FALSE(stylesheet.loadRadia(styles, "nonfinite-edge.css").ok());
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

    EXPECT_EQ(names.size(), std::size_t(66));
}
