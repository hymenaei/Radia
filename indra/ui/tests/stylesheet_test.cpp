/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstdint>
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <utility>
#include <vector>
#include "css/stylesheet.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "resource/elementdefinition.h"
#include "style/stylepass.h"
#include "text/metrics.h"

namespace {
using radia::ui::AccentColor;
using radia::ui::ColorScheme;
using radia::ui::ComputedStyle;
using radia::ui::CursorStyle;
using radia::ui::Document;
using radia::ui::EffectKind;
using radia::ui::Element;
using radia::ui::ElementState;
using radia::ui::FixedTextMetrics;
using radia::ui::FontFamily;
using radia::ui::GradientKind;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLIconElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::LayoutDirection;
using radia::ui::Overflow;
using radia::ui::PointerEvents;
using radia::ui::RadialGradientShape;
using radia::ui::ResourceLayer;
using radia::ui::ScrollbarGutter;
using radia::ui::ScrollbarMode;
using radia::ui::ScrollbarWidth;
using radia::ui::StyleLayer;
using radia::ui::StyleOrigin;
using radia::ui::StylePass;
using radia::ui::StyleSheet;
using radia::ui::TextAlign;
using radia::ui::VerticalAlign;
using radia::ui::detail::ElementInternalAccess;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using ::testing::Message;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    StylePass styles(stylesheet, FixedTextMetrics{});
    return styles.style(element);
}

HTMLIconElement& appendIcon(HTMLButtonElement& button, std::string name) {
    auto icon = makeElement<HTMLIconElement>(std::move(name));
    HTMLIconElement* result = icon.get();
    button.append(std::move(icon));
    return *result;
}

constexpr char kColorTokenStyles[] = ":root { --accent: hsl(120 100% 50%); --ink: rgb(255, 0, 0, 50%); } "
                                     "button { background-color: var(--accent); color: var(--ink); "
                                     "font-size: 17px; } "
                                     "label { color: #00ff00ff; font-size: 29px; }";
} // namespace

TEST(StyleSheetTest, ResolvesColorTokensAndInheritance) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kColorTokenStyles).ok());
    const ComputedStyle button = stylesheet.resolve("button", "", {}, 0);
    EXPECT_NEAR(button.backgroundColor.g, 1.f, 1.0e-4f);
    auto control = makeElementValue<HTMLButtonElement>();
    auto labelElement = makeElement<Element>("span");
    Element* label = labelElement.get();
    labelElement->textContent("Inherited");
    control.append(std::move(labelElement));
    EXPECT_NEAR(computedStyle(stylesheet, *label).color.a, .5f, 1.0e-4f);
    EXPECT_EQ(computedStyle(stylesheet, *label).fontSize, 17.f);
    auto standalone = makeElementValue<HTMLLabelElement>("Standalone");
    EXPECT_EQ(computedStyle(stylesheet, standalone).fontSize, 29.f);
}

TEST(StyleSheetTest, TreatsRootAsDocumentRootSelector) {
    constexpr char kRootStyles[] = ":root { --root-width: 24px; color-scheme: light; width: var(--root-width); min-width: 18px; "
                                   "color: #204060ff; } "
                                   ".root { width: 30px; } :root > label { width: 17px; } label { width: 9px; }";

    StyleSheet stylesheet;
    const auto loadResult = stylesheet.loadRadia(kRootStyles);
    ASSERT_TRUE(loadResult.ok()) << (loadResult.errors.empty() ? std::string() : loadResult.errors.front().message);

    auto rootOwner = makeElement<HTMLPanelElement>();
    rootOwner->addClass("root");
    Document document(std::move(rootOwner));
    auto labelOwner = makeElement<HTMLLabelElement>();
    HTMLLabelElement* label = labelOwner.get();
    document.documentElement()->append(std::move(labelOwner));

    const ComputedStyle rootStyle = computedStyle(stylesheet, *document.documentElement());
    EXPECT_EQ(rootStyle.colorScheme, ColorScheme::Light);
    EXPECT_EQ(rootStyle.width.pixels(), 30.f);
    ASSERT_TRUE(rootStyle.minWidth.has_value());
    EXPECT_EQ(rootStyle.minWidth->pixels, 18.f);

    const ComputedStyle labelStyle = computedStyle(stylesheet, *label);
    EXPECT_EQ(labelStyle.width.pixels(), 17.f);
    EXPECT_EQ(labelStyle.colorScheme, ColorScheme::Light);
    EXPECT_NEAR(labelStyle.color.r, 32.f / 255.f, 1.0e-4f);
    EXPECT_FALSE(labelStyle.minWidth.has_value());

    auto standaloneRoot = makeElementValue<HTMLPanelElement>();
    EXPECT_EQ(computedStyle(stylesheet, standaloneRoot).colorScheme, ColorScheme::Light);
}

TEST(StyleSheetTest, StartsWithoutImplicitCoreRules) {
    StyleSheet stylesheet;
    const ComputedStyle paragraph = stylesheet.resolve("p", "", {}, 0);

    EXPECT_FALSE(paragraph.displaySet);
    EXPECT_EQ(paragraph.fontWeight, static_cast<U16>(400));
}

TEST(StyleSheetTest, PreservesLiveStylesheetAfterInvalidColorCandidate) {
    constexpr char kInvalidColorStyles[] = "input { background-color: ##invalid; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kColorTokenStyles).ok());

    const auto invalid = stylesheet.loadRadia(kInvalidColorStyles, "invalid.css");

    ASSERT_FALSE(invalid.ok());
    ASSERT_FALSE(invalid.errors.empty());
    EXPECT_EQ(invalid.errors.front().code, "stylesheet.property.value_invalid");
    EXPECT_NEAR(stylesheet.resolve("button", "", {}, 0).backgroundColor.g, 1.f, 1.0e-4f);
}

TEST(StyleSheetTest, ParsesMarginPaddingAndGapShorthands) {
    constexpr char kBoxSpacingStyles[] = "panel { margin: 1px auto 3px -4px; padding: 5px 6px; gap: 7px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxSpacingStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(style.margin.top.fixedPixels(), 1.f);
    EXPECT_TRUE(style.margin.right.isAuto());
    EXPECT_EQ(style.margin.bottom.fixedPixels(), 3.f);
    EXPECT_EQ(style.margin.left.fixedPixels(), -4.f);
    EXPECT_FALSE(style.margin.left.isAuto());
    EXPECT_EQ(style.padding.left, 6.f);
    EXPECT_EQ(style.gap.fixedPixels(), 7.f);
    EXPECT_FALSE(style.gap.isAuto());
}

TEST(StyleSheetTest, MatchesFocusStatesWithoutConfusingFocusVisible) {
    constexpr char kFocusStateStyles[] = "button { border-width: 1px; &:focus { opacity: .8; } "
                                         "&:focus-visible { border-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFocusStateStyles).ok());
    const uint8_t focused = static_cast<uint8_t>(ElementState::Focused);
    const uint16_t focusVisible = ElementState::Focused | ElementState::FocusVisible;
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).opacity, .8f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).borderWidth.top, 1.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth.top, 3.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, static_cast<uint8_t>(ElementState::FocusVisible)).borderWidth.top, 1.f);
}

TEST(StyleSheetTest, AcceptsElementlessPseudoClassSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(":focus-visible { border-width: 2px; }").ok());

    const uint16_t focusVisible = ElementState::Focused | ElementState::FocusVisible;
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth.top, 2.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, static_cast<uint8_t>(ElementState::Focused)).borderWidth.top, 0.f);
}

TEST(StyleSheetTest, MatchesDirectionAlongsideCheckedState) {
    constexpr char kDirectionStyles[] = "input { &:dir(rtl):checked::slider-thumb { translate: -22px 0; } "
                                        "&:dir(ltr):checked::slider-thumb { translate: 22px 0; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDirectionStyles).ok());
    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true).checked(true);

    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb", LayoutDirection::RightToLeft).translate.x, -22.f);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb", LayoutDirection::LeftToRight).translate.x, 22.f);
    input.checked(false);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb", LayoutDirection::RightToLeft).translate.x, 0.f);
}

TEST(StyleSheetTest, PreservesLiveStylesheetAfterUnknownProperty) {
    constexpr char kInitialStyles[] = "button { width: 12px; }";
    constexpr char kInvalidStyles[] = "button { width: 99px; unknown-property: 1; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInitialStyles).ok());
    const auto failed = stylesheet.loadRadia(kInvalidStyles, "candidate.css");

    ASSERT_FALSE(failed.ok());
    ASSERT_FALSE(failed.errors.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).width.pixels(), 12.f);
    EXPECT_EQ(failed.errors.front().code, "stylesheet.property.unknown");
    EXPECT_EQ(failed.errors.front().source, "candidate.css");
}

TEST(StyleSheetTest, RejectsInvalidTokenValues) {
    constexpr char kInvalidTokenStyles[] = ":root { --bad: nonsense; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kInvalidTokenStyles, "tokens.css");

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "stylesheet.token.value_invalid");
    EXPECT_EQ(result.errors.front().source, "tokens.css");
}

TEST(StyleSheetTest, RejectsReferencesToMissingTokens) {
    constexpr char kMissingTokenStyles[] = "button { width: var(--missing); }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kMissingTokenStyles, "missing-token.css");

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().source, "missing-token.css");
}

TEST(StyleSheetTest, MatchesChildSelectorsUsingOwnerClassAndState) {
    constexpr char kChildOwnerStyles[] = "button.primary > icon { width: 10px; } "
                                         "button.primary:hover > icon { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kChildOwnerStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.addClass("primary");
    HTMLIconElement& icon = appendIcon(button, "search");

    EXPECT_EQ(computedStyle(stylesheet, icon).width.pixels(), 10.f);
    ElementInternalAccess::setState(button, ElementState::Hovered, true);
    EXPECT_EQ(computedStyle(stylesheet, icon).width.pixels(), 18.f);
}

TEST(StyleSheetTest, MatchesInteractivePartStateIndependentlyFromOwner) {
    constexpr char kInteractivePartStyles[] = "floater > head > close { width: 10px; } floater > head > close:hover { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInteractivePartStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, true);
    auto* closeButton = floater.closeButton();
    ASSERT_NE(closeButton, nullptr);

    ElementInternalAccess::setState(floater, ElementState::Hovered, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width.pixels(), 10.f);
    ElementInternalAccess::setState(*closeButton, ElementState::Hovered, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width.pixels(), 18.f);
}

TEST(StyleSheetTest, ParsesCursorValuesAndPreservesPriorValueAfterFailure) {
    constexpr char kCursorStyles[] = "button { cursor: pointer; } #horizontal { cursor: e-resize; } "
                                     "#diagonal { cursor: sw-resize; } #grab { cursor: grab; } "
                                     "#grabbing { cursor: grabbing; }";
    constexpr char kInvalidCursorStyles[] = "button { cursor: teleport; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCursorStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).cursor, CursorStyle::Pointer);
    EXPECT_EQ(stylesheet.resolve("panel", "horizontal", {}, 0).cursor, CursorStyle::EastWestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "diagonal", {}, 0).cursor, CursorStyle::NortheastSouthwestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "grab", {}, 0).cursor, CursorStyle::Grab);
    EXPECT_EQ(stylesheet.resolve("panel", "grabbing", {}, 0).cursor, CursorStyle::Grabbing);
    EXPECT_NE(stylesheet.resolve("panel", "grab", {}, 0).cursor, stylesheet.resolve("panel", "grabbing", {}, 0).cursor);

    const auto invalid = stylesheet.loadRadia(kInvalidCursorStyles, "cursor.css");
    ASSERT_FALSE(invalid.ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).cursor, CursorStyle::Pointer);
}

TEST(StyleSheetTest, CompilesTargetSpecificRulesWithoutWarnings) {
    constexpr char kRelevantAndIrrelevantStyles[] =
        "input { padding: 4px; "
        "&:checked::slider-thumb { order: 1; } } "
        "input::slider-track { display: flex; } input::slider-thumb { border-radius: 10px; } label { font-size: 13px; } "
        "panel { display: flex; flex-direction: row; font-size: 14px; } input { display: flex; flex-direction: row; } "
        "label { gap: 2px; align-items: center; } "
        "icon { font-size: 13px; } "
        "button > label { stroke-width: 2px; } "
        ".copy { font-size: 13px; order: 1; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kRelevantAndIrrelevantStyles);

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
}

TEST(StyleSheetTest, SelectsSwitchInputsByAttribute) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[switch] { width: 40px; }").ok());

    auto genericInput = makeElementValue<HTMLInputElement>();
    auto switchInput = makeElementValue<HTMLInputElement>();
    switchInput.type("checkbox").switchMode(true);
    EXPECT_EQ(computedStyle(stylesheet, genericInput).width.pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, switchInput).width.pixels(), 40.f);
}

TEST(StyleSheetTest, SelectsRadioInputsByNameAttribute) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width.pixels(), 40.f);
}

TEST(StyleSheetTest, ClearsNamePresenceWhenRadioNameIsRemoved) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name] { width: 20px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width.pixels(), 40.f);

    input.name("");
    EXPECT_FALSE(input.hasAttribute("name"));
    EXPECT_EQ(computedStyle(stylesheet, input).width.pixels(), 10.f);
}

TEST(StyleSheetTest, SelectsIndeterminateInputs) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:indeterminate { opacity: .5; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox");
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity, 1.f);
    input.indeterminate(true);
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity, .5f);
}

TEST(StyleSheetTest, RejectsInvalidSelectorsAndPropertyValues) {
    struct InvalidRuleCase {
        const char* source;
        const char* diagnostic;
    };

    const InvalidRuleCase cases[] = {
        {"button::label { stroke-width: 2px; }", "stylesheet.selector.pseudo_element_unknown"},
        {"input::missing { width: 10px; }", "stylesheet.selector.pseudo_element_unknown"},
        {"panel { align-items: sideways; }", "stylesheet.property.value_invalid"},
        {"button { align-self: sideways; }", "stylesheet.property.value_invalid"},
        {"input { justify-self: sideways; }", "stylesheet.property.value_invalid"},
        {"label { text-align: middle; }", "stylesheet.property.value_invalid"},
        {"panel { vertical-align: center; }", "stylesheet.property.value_invalid"},
        {"mystery { width: 10px; }", "stylesheet.selector.element_unknown"},
        {"label:cheked { opacity: .5; }", "stylesheet.selector.state_unknown"},
        {"button { --local: 2px; }", "stylesheet.token.root_required"},
        {"label { order: 1.5; }", "stylesheet.property.value_invalid"},
        {"label { order: 1px; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-mode: native; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-width: wide; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-gutter: stable auto; }", "stylesheet.property.value_invalid"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid CSS: " << test.source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.source, "contract.css");

        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST(StyleSheetTest, RejectsPseudoElementPseudoClasses) {
    constexpr char kPseudoElementStateStyles[] = "input::slider-thumb:checked { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kPseudoElementStateStyles, "contract.css");

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheetTest, RejectsNestedPseudoElements) {
    constexpr char kNestedPseudoElementStyles[] = "input::slider-track::slider-fill { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kNestedPseudoElementStyles, "contract.css");

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheetTest, InheritsOnlyInheritablePropertiesAndAllowsOverrides) {
    constexpr char kInheritedStyles[] = "panel { font-family: sans; font-size: 19px; font-weight: bold; "
                                        "font-style: italic; line-height: 23px; color: #204060ff; "
                                        "accent-color: #102030ff; "
                                        "text-align: center; vertical-align: bottom; cursor: grab; opacity: .5; "
                                        "pointer-events: none; background-color: #ffffffff; } "
                                        "label#override { font-size: 11px; cursor: default; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInheritedStyles).ok());

    auto parent = makeElement<HTMLPanelElement>();
    auto inherited = makeElement<HTMLLabelElement>("Inherited");
    HTMLLabelElement* inheritedLabel = inherited.get();
    parent->append(std::move(inherited));
    auto overridden = makeElement<HTMLLabelElement>("Overridden");
    HTMLLabelElement* overriddenLabel = overridden.get();
    overridden->setId("override");
    parent->append(std::move(overridden));

    const ComputedStyle inheritedStyle = computedStyle(stylesheet, *inheritedLabel);
    EXPECT_EQ(inheritedStyle.fontFamily, FontFamily::Sans);
    EXPECT_EQ(inheritedStyle.fontSize, 19.f);
    EXPECT_EQ(inheritedStyle.fontWeight, static_cast<U16>(700));
    EXPECT_TRUE(inheritedStyle.fontItalic);
    ASSERT_TRUE(inheritedStyle.lineHeight.has_value());
    EXPECT_EQ(inheritedStyle.lineHeight->pixels, 23.f);
    EXPECT_NEAR(inheritedStyle.color.b, 96.f / 255.f, 1.0e-4f);
    EXPECT_EQ(inheritedStyle.accentColor.kind, AccentColor::Kind::Color);
    EXPECT_NEAR(inheritedStyle.accentColor.color.r, 16.f / 255.f, 1.0e-4f);
    EXPECT_EQ(inheritedStyle.textAlign, TextAlign::Center);
    EXPECT_EQ(inheritedStyle.verticalAlign, VerticalAlign::Top);
    EXPECT_EQ(inheritedStyle.cursor, CursorStyle::Grab);
    EXPECT_EQ(inheritedStyle.opacity, 1.f);
    EXPECT_EQ(inheritedStyle.pointerEvents, PointerEvents::Default);
    EXPECT_EQ(inheritedStyle.backgroundColor.a, 0.f);

    const ComputedStyle overriddenStyle = computedStyle(stylesheet, *overriddenLabel);
    EXPECT_EQ(overriddenStyle.fontSize, 11.f);
    EXPECT_EQ(overriddenStyle.cursor, CursorStyle::Default);
}

TEST(StyleSheetTest, ResolvesCSSWideInheritanceKeywords) {
    constexpr char kStyles[] = "panel { width: 42px; display: block; color: #204060ff; font-size: 19px; } "
                               "label.explicit-inherit { width: inherit; display: inherit; color: inherit; } "
                               "label.unset-inherited { color: unset; font-size: unset; } "
                               "label.unset-initial { width: unset; } "
                               "label.late-inherit { width: 7px; width: inherit; } "
                               "label.late-value { width: inherit; width: 9px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    auto parent = makeElementValue<HTMLPanelElement>();
    auto explicitInherit = makeElement<HTMLLabelElement>();
    explicitInherit->addClass("explicit-inherit");
    HTMLLabelElement* explicitInheritPtr = explicitInherit.get();
    parent.append(std::move(explicitInherit));
    auto unsetInherited = makeElement<HTMLLabelElement>();
    unsetInherited->addClass("unset-inherited");
    HTMLLabelElement* unsetInheritedPtr = unsetInherited.get();
    parent.append(std::move(unsetInherited));
    auto unsetInitial = makeElement<HTMLLabelElement>();
    unsetInitial->addClass("unset-initial");
    HTMLLabelElement* unsetInitialPtr = unsetInitial.get();
    parent.append(std::move(unsetInitial));
    auto lateInherit = makeElement<HTMLLabelElement>();
    lateInherit->addClass("late-inherit");
    HTMLLabelElement* lateInheritPtr = lateInherit.get();
    parent.append(std::move(lateInherit));
    auto lateValue = makeElement<HTMLLabelElement>();
    lateValue->addClass("late-value");
    HTMLLabelElement* lateValuePtr = lateValue.get();
    parent.append(std::move(lateValue));

    const ComputedStyle parentStyle = computedStyle(stylesheet, parent);
    EXPECT_EQ(parentStyle.width.pixels(), 42.f);
    EXPECT_EQ(parentStyle.display, radia::ui::DisplayMode::Block);
    EXPECT_NEAR(parentStyle.color.r, 32.f / 255.f, 1.0e-4f);
    EXPECT_EQ(parentStyle.fontSize, 19.f);

    const ComputedStyle explicitInheritStyle = computedStyle(stylesheet, *explicitInheritPtr);
    EXPECT_EQ(explicitInheritStyle.width.pixels(), 42.f);
    EXPECT_EQ(explicitInheritStyle.display, radia::ui::DisplayMode::Block);
    EXPECT_NEAR(explicitInheritStyle.color.r, parentStyle.color.r, 1.0e-4f);

    const ComputedStyle unsetInheritedStyle = computedStyle(stylesheet, *unsetInheritedPtr);
    EXPECT_NEAR(unsetInheritedStyle.color.r, parentStyle.color.r, 1.0e-4f);
    EXPECT_EQ(unsetInheritedStyle.fontSize, parentStyle.fontSize);

    const ComputedStyle unsetInitialStyle = computedStyle(stylesheet, *unsetInitialPtr);
    EXPECT_TRUE(unsetInitialStyle.width.isAuto());

    EXPECT_EQ(computedStyle(stylesheet, *lateInheritPtr).width.pixels(), 42.f);
    EXPECT_EQ(computedStyle(stylesheet, *lateValuePtr).width.pixels(), 9.f);
}

TEST(StyleSheetTest, ParsesOverflowShorthandAndRejectsUnsupportedValues) {
    constexpr char kOverflowStyles[] = "panel { overflow: scroll auto; } "
                                       "#single { overflow: auto; } "
                                       "#longhand { overflow-x: auto; overflow-y: scroll; } "
                                       "#vertical { overflow-x: visible; overflow-y: hidden; } "
                                       "#horizontal { overflow-x: hidden; overflow-y: visible; } "
                                       "#initial { overflow: scroll; overflow-x: initial; overflow-y: initial; }";
    constexpr char kInvalidOverflowStyles[] = "panel { overflow: clip; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kOverflowStyles).ok());
    const ComputedStyle split = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(split.overflowX, Overflow::Scroll);
    EXPECT_EQ(split.overflowY, Overflow::Auto);
    const ComputedStyle single = stylesheet.resolve("panel", "single", {}, 0);
    EXPECT_EQ(single.overflowX, Overflow::Auto);
    EXPECT_EQ(single.overflowY, Overflow::Auto);
    const ComputedStyle longhand = stylesheet.resolve("panel", "longhand", {}, 0);
    EXPECT_EQ(longhand.overflowX, Overflow::Auto);
    EXPECT_EQ(longhand.overflowY, Overflow::Scroll);
    const ComputedStyle vertical = stylesheet.resolve("panel", "vertical", {}, 0);
    EXPECT_EQ(vertical.overflowX, Overflow::Auto);
    EXPECT_EQ(vertical.overflowY, Overflow::Hidden);
    const ComputedStyle horizontal = stylesheet.resolve("panel", "horizontal", {}, 0);
    EXPECT_EQ(horizontal.overflowX, Overflow::Hidden);
    EXPECT_EQ(horizontal.overflowY, Overflow::Auto);
    const ComputedStyle initial = stylesheet.resolve("panel", "initial", {}, 0);
    EXPECT_EQ(initial.overflowX, Overflow::Visible);
    EXPECT_EQ(initial.overflowY, Overflow::Visible);

    const auto invalid = stylesheet.loadRadia(kInvalidOverflowStyles, "overflow.css");
    ASSERT_FALSE(invalid.ok());
    ASSERT_FALSE(invalid.errors.empty());
    EXPECT_EQ(invalid.errors.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleSheetTest, ParsesEllipticalBorderRadiusShorthand) {
    constexpr char kStyles[] = "panel { border-radius: 10px 100px / 120px; } "
                               "#expanded { border-radius: 10px 20px 30px / 40px 50px 60px 70px; } "
                               "#mirrored { border-radius: 10%; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle split = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(split.borderRadius.topLeft.horizontal.pixels, 10.f);
    EXPECT_EQ(split.borderRadius.topRight.horizontal.pixels, 100.f);
    EXPECT_EQ(split.borderRadius.bottomRight.horizontal.pixels, 10.f);
    EXPECT_EQ(split.borderRadius.bottomLeft.horizontal.pixels, 100.f);
    EXPECT_EQ(split.borderRadius.topLeft.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius.topRight.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius.bottomRight.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius.bottomLeft.vertical.pixels, 120.f);

    const ComputedStyle expanded = stylesheet.resolve("panel", "expanded", {}, 0);
    EXPECT_EQ(expanded.borderRadius.topLeft.horizontal.pixels, 10.f);
    EXPECT_EQ(expanded.borderRadius.topRight.horizontal.pixels, 20.f);
    EXPECT_EQ(expanded.borderRadius.bottomRight.horizontal.pixels, 30.f);
    EXPECT_EQ(expanded.borderRadius.bottomLeft.horizontal.pixels, 20.f);
    EXPECT_EQ(expanded.borderRadius.topLeft.vertical.pixels, 40.f);
    EXPECT_EQ(expanded.borderRadius.topRight.vertical.pixels, 50.f);
    EXPECT_EQ(expanded.borderRadius.bottomRight.vertical.pixels, 60.f);
    EXPECT_EQ(expanded.borderRadius.bottomLeft.vertical.pixels, 70.f);

    const ComputedStyle mirrored = stylesheet.resolve("panel", "mirrored", {}, 0);
    EXPECT_NEAR(mirrored.borderRadius.topLeft.horizontal.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius.topLeft.vertical.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius.bottomLeft.horizontal.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius.bottomLeft.vertical.percent, .1f, 1.0e-6f);
}

TEST(StyleSheetTest, RejectsMalformedEllipticalBorderRadiusShorthand) {
    constexpr char kInvalidStyles[] = "panel { border-radius: 1px /; }";
    constexpr char kMultipleSlashStyles[] = "panel { border-radius: 1px / 2px / 3px; }";
    constexpr char kTooManyValuesStyles[] = "panel { border-radius: 1px 2px 3px 4px 5px; }";

    for (const char* source : {kInvalidStyles, kMultipleSlashStyles, kTooManyValuesStyles}) {
        SCOPED_TRACE(Message() << "malformed border-radius CSS: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "border-radius.css");
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleSheetTest, ParsesScrollbarPolicyProperties) {
    constexpr char kScrollbarStyles[] =
        "panel { scrollbar-mode: overlay; scrollbar-width: thin; scrollbar-gutter: stable both-edges; scrollbar-color: #112233 #445566; } "
        "#classic { scrollbar-mode: classic; scrollbar-width: none; scrollbar-gutter: stable; } "
        "#initial { scrollbar-mode: overlay; scrollbar-mode: initial; scrollbar-width: thin; scrollbar-width: initial; "
        "scrollbar-gutter: stable both-edges; scrollbar-gutter: initial; scrollbar-color: #112233 #445566; scrollbar-color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kScrollbarStyles).ok());

    const ComputedStyle overlay = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(overlay.scrollbarMode, ScrollbarMode::Overlay);
    EXPECT_TRUE(overlay.scrollbarModeSet);
    EXPECT_EQ(overlay.scrollbarWidth, ScrollbarWidth::Thin);
    EXPECT_EQ(overlay.scrollbarGutter, ScrollbarGutter::StableBothEdges);
    EXPECT_FALSE(overlay.scrollbarColor.automatic);
    EXPECT_NEAR(overlay.scrollbarColor.thumb.r, 0x11 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor.thumb.g, 0x22 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor.track.b, 0x66 / 255.f, 1.0e-6f);

    const ComputedStyle classic = stylesheet.resolve("panel", "classic", {}, 0);
    EXPECT_EQ(classic.scrollbarMode, ScrollbarMode::Classic);
    EXPECT_TRUE(classic.scrollbarModeSet);
    EXPECT_EQ(classic.scrollbarWidth, ScrollbarWidth::NoneValue);
    EXPECT_EQ(classic.scrollbarGutter, ScrollbarGutter::Stable);

    const ComputedStyle initial = stylesheet.resolve("panel", "initial", {}, 0);
    EXPECT_EQ(initial.scrollbarMode, ScrollbarMode::Classic);
    EXPECT_TRUE(initial.scrollbarModeSet);
    EXPECT_EQ(initial.scrollbarWidth, ScrollbarWidth::Auto);
    EXPECT_EQ(initial.scrollbarGutter, ScrollbarGutter::Auto);
    EXPECT_TRUE(initial.scrollbarColor.automatic);
}

TEST(StyleSheetTest, ParsesAccentColorValues) {
    constexpr char kAccentStyles[] = "panel { accent-color: #12345678; } #current { accent-color: currentcolor; } "
                                     "#automatic { accent-color: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kAccentStyles).ok());

    const ComputedStyle color = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(color.accentColor.kind, AccentColor::Kind::Color);
    EXPECT_NEAR(color.accentColor.color.r, 0x12 / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.accentColor.color.a, 0x78 / 255.f, 1.0e-6f);

    EXPECT_EQ(stylesheet.resolve("panel", "current", {}, 0).accentColor.kind, AccentColor::Kind::CurrentColor);
    EXPECT_EQ(stylesheet.resolve("panel", "automatic", {}, 0).accentColor.kind, AccentColor::Kind::Auto);
}

TEST(StyleSheetTest, InheritsColorSchemeAndAllowsOverride) {
    constexpr char kColorSchemeStyles[] = "panel { color-scheme: light; } input.dark { color-scheme: dark; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kColorSchemeStyles).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto inheritedInput = makeElement<HTMLInputElement>();
    HTMLInputElement* inheritedInputPtr = inheritedInput.get();
    inheritedInputPtr->type("checkbox");
    panel.append(std::move(inheritedInput));

    auto overriddenInput = makeElement<HTMLInputElement>();
    HTMLInputElement* overriddenInputPtr = overriddenInput.get();
    overriddenInputPtr->type("checkbox").addClass("dark");
    panel.append(std::move(overriddenInput));

    EXPECT_EQ(computedStyle(stylesheet, *inheritedInputPtr).colorScheme, ColorScheme::Light);
    EXPECT_EQ(computedStyle(stylesheet, *overriddenInputPtr).colorScheme, ColorScheme::Dark);
}

TEST(StyleSheetTest, ResolvesLightDarkAfterColorSchemeInheritance) {
    constexpr char kLightDarkStyles[] = ":root { color-scheme: light; } panel.dark { color-scheme: dark; } "
                                        "label { color: light-dark(#101010, #f0f0f0); }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLightDarkStyles).ok());

    auto lightPanel = makeElementValue<HTMLPanelElement>();
    auto lightLabel = makeElement<HTMLLabelElement>("light");
    HTMLLabelElement* lightLabelPtr = lightLabel.get();
    lightPanel.append(std::move(lightLabel));

    auto darkPanel = makeElementValue<HTMLPanelElement>();
    darkPanel.addClass("dark");
    auto darkLabel = makeElement<HTMLLabelElement>("dark");
    HTMLLabelElement* darkLabelPtr = darkLabel.get();
    darkPanel.append(std::move(darkLabel));

    EXPECT_NEAR(computedStyle(stylesheet, *lightLabelPtr).color.r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(computedStyle(stylesheet, *darkLabelPtr).color.r, 240.f / 255.f, 1.0e-4f);
}

TEST(StyleSheetTest, ProjectsMinimizedFloaterStateIntoHeadStyles) {
    constexpr char kMinimizedFloaterStyles[] = "floater > head { border-width: 0px 0px 1px; } "
                                               "floater:minimized > head { border-width: 0px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMinimizedFloaterStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, false, true);
    Element* head = floater.head();
    ASSERT_NE(head, nullptr);

    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth.bottom, 1.f);
    floater.setMinimized(true);
    EXPECT_TRUE(floater.hasState(ElementState::Minimized));
    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth.bottom, 0.f);
    floater.setMinimized(false);
    EXPECT_FALSE(floater.hasState(ElementState::Minimized));
    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth.bottom, 1.f);
}

TEST(StyleSheetTest, ParsesTypedLengthsAndAutomaticDimensions) {
    constexpr char kTypedLengthStyles[] = "panel { width: 40px; min-width: 20px; left: -8px; line-height: 18px; }";
    constexpr char kAutoDimensionStyles[] = "panel { width: 40px; height: 20px; width: auto; height: auto; } "
                                            "button { size: auto; } icon { size: auto 16px; }";
    constexpr char kAutoGapStyles[] = "panel { gap: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypedLengthStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_FALSE(style.width.isAuto());
    EXPECT_EQ(style.width.pixels(), 40.f);
    ASSERT_TRUE(style.minWidth.has_value());
    EXPECT_EQ(style.minWidth->pixels, 20.f);
    ASSERT_TRUE(style.left.has_value());
    EXPECT_EQ(style.left->pixels, -8.f);
    ASSERT_TRUE(style.lineHeight.has_value());
    EXPECT_EQ(style.lineHeight->pixels, 18.f);

    ASSERT_TRUE(stylesheet.loadRadia(kAutoDimensionStyles).ok());
    const ComputedStyle automatic = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_TRUE(automatic.width.isAuto());
    EXPECT_TRUE(automatic.height.isAuto());
    const ComputedStyle automaticSize = stylesheet.resolve("button", "", {}, 0);
    EXPECT_TRUE(automaticSize.width.isAuto());
    EXPECT_TRUE(automaticSize.height.isAuto());
    const ComputedStyle mixedSize = stylesheet.resolve("icon", "", {}, 0);
    EXPECT_TRUE(mixedSize.height.isAuto());
    EXPECT_EQ(mixedSize.width.pixels(), 16.f);

    ASSERT_TRUE(stylesheet.loadRadia(kAutoGapStyles).ok());
    const ComputedStyle automaticGap = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_TRUE(automaticGap.gap.isAuto());
    EXPECT_EQ(automaticGap.gap.fixedPixels(), 0.f);
}

TEST(StyleSheetTest, MatchesStructuralSelectorsAndCombinators) {
    constexpr char kStructuralStyles[] = "* { opacity: .8; } panel.root > label { width: 10px; } "
                                         "panel.root label { height: 11px; } panel.root { "
                                         "> label.direct { min-width: 20%; } "
                                         "& > label.direct { right: 5%; } label.nested { min-height: 25%; } "
                                         "& label.nested { bottom: 10%; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStructuralStyles).ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.addClass("root");
    auto direct = makeElement<HTMLLabelElement>("direct");
    direct->addClass("direct");
    HTMLLabelElement* directLabel = direct.get();
    root.append(std::move(direct));

    auto container = makeElement<HTMLPanelElement>();
    auto nested = makeElement<HTMLLabelElement>("nested");
    nested->addClass("nested");
    HTMLLabelElement* nestedLabel = nested.get();
    container->append(std::move(nested));
    root.append(std::move(container));

    const ComputedStyle directStyle = computedStyle(stylesheet, *directLabel);
    EXPECT_EQ(directStyle.opacity, .8f);
    EXPECT_EQ(directStyle.width.pixels(), 10.f);
    EXPECT_EQ(directStyle.height.pixels(), 11.f);
    ASSERT_TRUE(directStyle.minWidth.has_value());
    EXPECT_TRUE(directStyle.minWidth->isPercentage());
    EXPECT_NEAR(directStyle.minWidth->percent, .2f, 1.0e-4f);
    ASSERT_TRUE(directStyle.right.has_value());
    EXPECT_NEAR(directStyle.right->percent, .05f, 1.0e-4f);

    const ComputedStyle nestedStyle = computedStyle(stylesheet, *nestedLabel);
    EXPECT_TRUE(nestedStyle.width.isAuto());
    EXPECT_EQ(nestedStyle.height.pixels(), 11.f);
    ASSERT_TRUE(nestedStyle.minHeight.has_value());
    EXPECT_NEAR(nestedStyle.minHeight->percent, .25f, 1.0e-4f);
    ASSERT_TRUE(nestedStyle.bottom.has_value());
    EXPECT_NEAR(nestedStyle.bottom->percent, .1f, 1.0e-4f);
}

TEST(StyleSheetTest, ParsesGradientsEffectsShadowsAndOutlines) {
    constexpr char kBoxEffectStyles[] = "panel { background-color: linear-gradient(to right, #ff0000ff, "
                                        "rgb(0, 255, 0, 50%) 75%, #0000ffff); "
                                        "box-shadow: 1px 2px #11223344, 3px 4px 5px 6px "
                                        "rgb(10, 20, 30, 40%) inset; outline-offset: 3px; "
                                        "outline: light-dark(#abcdef88, #12345688) solid 2px; } panel.light { color-scheme: light; } "
                                        "label { outline: 1px dashed #ffffffff; }";
    constexpr char kBlurEffectStyles[] = "panel { effect: background-blur(to bottom, 0px 25%, 16px 75%), "
                                         "layer-blur(4px); } button { effect: layer-blur(to right, "
                                         "0px 50%, 4px 50%); } label { effect: none; }";
    constexpr char kGradientStyles[] = "panel { background-color: radial-gradient(circle at 25% 75%, "
                                       "#ffffffff, #00000000 80%); border-width: 3px; border-color: "
                                       "conic-gradient(from 45deg at top left, #ff0000ff 0deg 90deg, "
                                       "#0000ffff 100%); } button { border: 2px "
                                       "repeating-linear-gradient(90deg, #ffffffff 0%, #000000ff 20%); } "
                                       "input { background-color: repeating-radial-gradient(ellipse at center, "
                                       "#ffffffff 0%, #000000ff 25%); } floater { background-color: "
                                       "repeating-conic-gradient(from .25turn, #ffffffff 0deg 30deg, "
                                       "#000000ff 60deg); } panel.solid { background-color: #112233ff; "
                                       "border-color: #445566ff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxEffectStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_TRUE(style.backgroundGradient.has_value());
    EXPECT_EQ(style.backgroundGradient->kind, GradientKind::Linear);
    EXPECT_EQ(style.backgroundGradient->angleDegrees, 90.f);
    EXPECT_EQ(style.backgroundGradient->stops.size(), std::size_t(3));
    EXPECT_NEAR(style.backgroundGradient->stops[1].position, .75f, 1.0e-4f);
    EXPECT_NEAR(style.backgroundGradient->stops[1].color.a, .5f, 1.0e-4f);
    EXPECT_EQ(style.shadows.size(), std::size_t(2));
    EXPECT_EQ(style.shadows[0].blur, 0.f);
    EXPECT_FALSE(style.shadows[0].inset);
    EXPECT_EQ(style.shadows[1].spread, 6.f);
    EXPECT_TRUE(style.shadows[1].inset);
    EXPECT_EQ(style.outline.width, 2.f);
    EXPECT_EQ(style.outline.offset, 3.f);
    EXPECT_NEAR(style.outline.color.r, 18.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {"light"}, 0).outline.color.r, 171.f / 255.f, 1.0e-4f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).outline.offset, 0.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).outline.style, radia::ui::OutlineStyle::Dashed);

    ASSERT_TRUE(stylesheet.loadRadia(kBlurEffectStyles).ok());
    const ComputedStyle effects = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_EQ(effects.effects.size(), std::size_t(2));
    EXPECT_EQ(effects.effects[0].kind, EffectKind::BackgroundBlur);
    EXPECT_EQ(effects.effects[0].startRadius, 0.f);
    EXPECT_EQ(effects.effects[0].endRadius, 16.f);
    EXPECT_NEAR(effects.effects[0].startPosition, .25f, 1.0e-4f);
    EXPECT_NEAR(effects.effects[0].endPosition, .75f, 1.0e-4f);
    EXPECT_EQ(effects.effects[0].angleDegrees, 180.f);
    EXPECT_EQ(effects.effects[1].kind, EffectKind::LayerBlur);
    EXPECT_EQ(effects.effects[1].endRadius, 4.f);
    const ComputedStyle buttonEffects = stylesheet.resolve("button", "", {}, 0);
    ASSERT_FALSE(buttonEffects.effects.empty());
    EXPECT_TRUE(buttonEffects.effects[0].progressive());
    EXPECT_TRUE(stylesheet.resolve("label", "", {}, 0).effects.empty());

    ASSERT_TRUE(stylesheet.loadRadia(kGradientStyles).ok());
    const ComputedStyle radial = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_TRUE(radial.backgroundGradient.has_value());
    EXPECT_EQ(radial.backgroundGradient->kind, GradientKind::Radial);
    EXPECT_EQ(radial.backgroundGradient->radialShape, RadialGradientShape::Circle);
    EXPECT_NEAR(radial.backgroundGradient->center.x, .25f, 1.0e-4f);
    EXPECT_NEAR(radial.backgroundGradient->center.y, .25f, 1.0e-4f);
    ASSERT_TRUE(radial.borderGradient.has_value());
    EXPECT_EQ(radial.borderGradient->kind, GradientKind::Conic);
    EXPECT_EQ(radial.borderGradient->stops.size(), std::size_t(3));
    EXPECT_EQ(radial.borderWidth.top, 3.f);
    const ComputedStyle repeatingBorder = stylesheet.resolve("button", "", {}, 0);
    ASSERT_TRUE(repeatingBorder.borderGradient.has_value());
    EXPECT_TRUE(repeatingBorder.borderGradient->repeating);
    EXPECT_EQ(repeatingBorder.borderWidth.left, 2.f);
    const ComputedStyle repeatingRadial = stylesheet.resolve("input", "", {}, 0);
    ASSERT_TRUE(repeatingRadial.backgroundGradient.has_value());
    EXPECT_EQ(repeatingRadial.backgroundGradient->kind, GradientKind::Radial);
    EXPECT_TRUE(repeatingRadial.backgroundGradient->repeating);
    const ComputedStyle repeatingConic = stylesheet.resolve("floater", "", {}, 0);
    ASSERT_TRUE(repeatingConic.backgroundGradient.has_value());
    EXPECT_EQ(repeatingConic.backgroundGradient->kind, GradientKind::Conic);
    EXPECT_TRUE(repeatingConic.backgroundGradient->repeating);
    EXPECT_EQ(repeatingConic.backgroundGradient->angleDegrees, 90.f);
    const ComputedStyle solidOverride = stylesheet.resolve("panel", "", {"solid"}, 0);
    EXPECT_FALSE(solidOverride.backgroundGradient.has_value());
    EXPECT_FALSE(solidOverride.borderGradient.has_value());
    EXPECT_NEAR(solidOverride.borderColor.g, 85.f / 255.f, 1.0e-4f);
}

TEST(StyleSheetTest, RejectsInvalidBoxEffects) {
    const char* invalidSources[] = {
        "panel { background-color: linear-gradient(#fff); }",
        "panel { background-color: radial-gradient(square, #fff, #000); }",
        "panel { border-color: conic-gradient(from nowhere, #fff, #000); }",
        "panel { border: 1px repeating-linear-gradient(#fff 20%, #000 20%); }",
        "panel { background: #ffffffff; }",
        "panel { box-shadow: 0 0 -1px #000; }",
        "panel { background-color: ButtonFace; }",
        "panel { border-color: ButtonBorder; }",
        "panel { border: 1px ButtonBorder; }",
        "panel { color: ButtonText; }",
        "panel { outline: 10% #000; }",
        "panel { outline: 1px 2px #000; }",
        "panel { outline-offset: 0%; }",
        "panel { outline: 1px dashed solid #000; }",
        "panel { outline: #000 auto 1px; }",
        "panel { outline: -focus-ring-color auto 1px; }",
        "panel { effect: blur(4px); }",
        "panel { effect: layer-blur(to bottom, 4px); }",
        "panel { effect: background-blur(to nowhere, 0px 0%, 4px 100%); }",
        "panel { effect: background-blur(to bottom, 0px, 4px 100%); }",
        "panel { effect: background-blur(to bottom, 0px 75%, 4px 25%); }",
        "panel { effect: layer-blur(-1px); }",
        "panel { effect: layer-blur(2px), background-blur(4px); }",
        "panel { effect: background-blur(2px) layer-blur(4px); }",
    };
    constexpr char kLargeBlurStyles[] = "panel { effect: layer-blur(64px); }";

    for (const char* source : invalidSources) {
        SCOPED_TRACE(Message() << "invalid box effect: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "effects.css");
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
    }

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLargeBlurStyles, "large-effect.css").ok());
}

TEST(StyleSheetTest, ParsesMinSizeShorthandAndRejectsInvalidValues) {
    constexpr char kMinSizeStyles[] = "panel.one { min-size: 24px; } panel.two { min-size: 30% 80px; } "
                                      "panel.longhand-after { min-size: 10px 20px; min-width: 40px; } "
                                      "panel.shorthand-after { min-height: 5px; min-size: 12px 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMinSizeStyles).ok());

    const ComputedStyle one = stylesheet.resolve("panel", "", {"one"}, 0);
    ASSERT_TRUE(one.minHeight.has_value());
    ASSERT_TRUE(one.minWidth.has_value());
    EXPECT_EQ(one.minHeight->pixels, 24.f);
    EXPECT_EQ(one.minWidth->pixels, 24.f);

    const ComputedStyle two = stylesheet.resolve("panel", "", {"two"}, 0);
    ASSERT_TRUE(two.minHeight.has_value());
    ASSERT_TRUE(two.minWidth.has_value());
    EXPECT_NEAR(two.minHeight->percent, .3f, 1.0e-4f);
    EXPECT_EQ(two.minWidth->pixels, 80.f);

    const ComputedStyle longhandAfter = stylesheet.resolve("panel", "", {"longhand-after"}, 0);
    ASSERT_TRUE(longhandAfter.minWidth.has_value());
    ASSERT_TRUE(longhandAfter.minHeight.has_value());
    EXPECT_EQ(longhandAfter.minWidth->pixels, 40.f);
    EXPECT_EQ(longhandAfter.minHeight->pixels, 10.f);

    const ComputedStyle shorthandAfter = stylesheet.resolve("panel", "", {"shorthand-after"}, 0);
    ASSERT_TRUE(shorthandAfter.minHeight.has_value());
    ASSERT_TRUE(shorthandAfter.minWidth.has_value());
    EXPECT_EQ(shorthandAfter.minHeight->pixels, 12.f);
    EXPECT_EQ(shorthandAfter.minWidth->pixels, 18.f);

    struct InvalidMinSizeCase {
        const char* name;
        const char* styles;
    };
    const InvalidMinSizeCase invalidCases[] = {
        {"auto", "panel { min-size: auto; }"},
        {"negative length", "panel { min-size: -1px; }"},
        {"too many values", "panel { min-size: 1px 2px 3px; }"},
    };
    for (const auto& test : invalidCases) {
        SCOPED_TRACE(Message() << "invalid min-size case: " << test.name);
        const auto result = stylesheet.loadRadia(test.styles, "min-size.css");
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, "stylesheet.property.value_invalid");
    }

    const ComputedStyle preservedStyle = stylesheet.resolve("panel", "", {"one"}, 0);
    ASSERT_TRUE(preservedStyle.minWidth.has_value());
    EXPECT_EQ(preservedStyle.minWidth->pixels, 24.f);
}

TEST(StyleSheetTest, CopiesAndMovesStylesheetsWithoutSharingState) {
    constexpr char kOriginalStyles[] = "panel { width: 10px; }";
    constexpr char kReplacementStyles[] = "panel { width: 20px; }";

    StyleSheet original;
    ASSERT_TRUE(original.loadRadia(kOriginalStyles, "original.css").ok());
    const std::uint64_t copiedGeneration = original.generation();

    StyleSheet copy = original;
    ASSERT_TRUE(original.loadRadia(kReplacementStyles, "replacement.css").ok());
    EXPECT_EQ(copy.resolve("panel", "", {}, 0).width.pixels(), 10.f);
    EXPECT_EQ(copy.generation(), copiedGeneration);
    EXPECT_EQ(original.resolve("panel", "", {}, 0).width.pixels(), 20.f);

    StyleSheet assigned;
    assigned = copy;
    EXPECT_EQ(assigned.resolve("panel", "", {}, 0).width.pixels(), 10.f);

    StyleSheet moved = std::move(assigned);
    EXPECT_EQ(moved.resolve("panel", "", {}, 0).width.pixels(), 10.f);
}

TEST(StyleSheetTest, MergesStyleLayersTransactionally) {
    constexpr char kBaseLayerStyles[] = "panel { width: 10px; height: 30px; }";
    constexpr char kDerivedLayerStyles[] = "panel { width: 20px; }";
    constexpr char kMalformedLayerStyles[] = "not a rule";

    StyleSheet stylesheet;
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Skin, {"base/skin.css", kBaseLayerStyles}},
        {StyleOrigin::Skin, {"derived/skin.css", kDerivedLayerStyles}},
    };
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(resolved.width.pixels(), 20.f);
    EXPECT_EQ(resolved.height.pixels(), 30.f);

    const auto malformed = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"base/skin.css", kBaseLayerStyles}},
        {StyleOrigin::Skin, {"derived/skin.css", kMalformedLayerStyles}},
    });
    ASSERT_FALSE(malformed.ok());
    ASSERT_FALSE(malformed.errors.empty());
    EXPECT_EQ(malformed.errors.front().source, "derived/skin.css");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 20.f);
}

TEST(StyleSheetTest, SkinOriginOverridesMoreSpecificDefaultRule) {
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Skin, {"skin.css", "panel { width: 20px; }"}},
        {StyleOrigin::Default, {"defaults.css", "panel.primary { width: 10px; }"}},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"primary"}, 0).width.pixels(), 20.f);
}

TEST(StyleSheetTest, RestrictsInternalAlignmentToDefaultStylesheet) {
    StyleSheet stylesheet;
    const auto skinOnly = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(skinOnly.ok());
    ASSERT_EQ(skinOnly.warnings.size(), std::size_t(1));
    EXPECT_EQ(skinOnly.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_FALSE(stylesheet.resolve("button", "", {}, 0).alignContentBlockCenter);

    const auto defaultAndSkin = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: normal; }"}},
        {StyleOrigin::Default, {"defaults.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(defaultAndSkin.ok());
    ASSERT_EQ(defaultAndSkin.warnings.size(), std::size_t(1));
    EXPECT_EQ(defaultAndSkin.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_TRUE(stylesheet.resolve("button", "", {}, 0).alignContentBlockCenter);
}

TEST(StyleSheetTest, ResolvesRecursiveImportsAndRecordsDependencies) {
    constexpr char kEntrypointStyles[] = "@import \"components/panel.css\";\n"
                                         ":root { --panel-width: 12px; }\n"
                                         "panel { width: var(--panel-width); }\n"
                                         "panel { width: 30px; }";
    constexpr char kPanelModule[] = "@import \"../foundation/sizes.css\";\n"
                                    "panel { width: var(--panel-width); height: var(--panel-height); }";
    constexpr char kFoundationModuleStyles[] = ":root { --panel-height: 18px; }";

    StyleSheet stylesheet;
    ResourceLayer layer{"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"foundation/sizes.css", kFoundationModuleStyles},
        {"components/panel.css", kPanelModule},
    };

    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}}).ok());
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(resolved.width.pixels(), 30.f);
    EXPECT_EQ(resolved.height.pixels(), 18.f);

    const auto& dependencies = stylesheet.dependencies();
    ASSERT_TRUE(dependencies.contains("theme/main.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/panel.css"));
    ASSERT_TRUE(dependencies.contains("theme/components/panel.css"));
    EXPECT_TRUE(dependencies.at("theme/components/panel.css").contains("theme/foundation/sizes.css"));
}

TEST(StyleSheetTest, RejectsImportFailuresAndPreservesLiveStylesheet) {
    constexpr char kBaselineStyles[] = "panel { width: 44px; }";
    constexpr char kMissingImport[] = "\n@import \"missing.css\";";
    constexpr char kCycleImport[] = "@import \"cycle.css\";";
    constexpr char kCycleModule[] = "@import \"main.css\";";
    constexpr char kTraversalImport[] = "@import \"../outside.css\";";
    constexpr char kMalformedImport[] = "@import \"broken.css\";";
    constexpr char kMalformedModule[] = "panel { width: ; }";
    constexpr char kLateImport[] = "panel { width: 1px; } @import \"late.css\";";
    constexpr char kLateModule[] = "panel { height: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBaselineStyles).ok());

    auto layer = [](std::string source) {
        ResourceLayer result{"theme/main.css", std::move(source)};
        result.entrypoint = "main.css";
        return result;
    };

    auto missing = layer(kMissingImport);
    const auto missingResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, missing}});
    ASSERT_FALSE(missingResult.ok());
    ASSERT_FALSE(missingResult.errors.empty());
    EXPECT_EQ(missingResult.errors.front().code, "stylesheet.import.missing");
    EXPECT_EQ(missingResult.errors.front().line, std::size_t(2));

    auto cycle = layer(kCycleImport);
    cycle.modules["cycle.css"] = kCycleModule;
    const auto cycleResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, cycle}});
    ASSERT_FALSE(cycleResult.ok());
    ASSERT_FALSE(cycleResult.errors.empty());
    EXPECT_EQ(cycleResult.errors.front().code, "stylesheet.import.cycle");

    const auto traversalResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer(kTraversalImport)}});
    ASSERT_FALSE(traversalResult.ok());
    ASSERT_FALSE(traversalResult.errors.empty());
    EXPECT_EQ(traversalResult.errors.front().code, "stylesheet.import.path_invalid");

    auto malformed = layer(kMalformedImport);
    malformed.modules["broken.css"] = kMalformedModule;
    const auto malformedResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, malformed}});
    ASSERT_FALSE(malformedResult.ok());
    ASSERT_FALSE(malformedResult.errors.empty());
    EXPECT_EQ(malformedResult.errors.front().source, "theme/broken.css");
    EXPECT_NE(malformedResult.errors.front().message.find("main.css -> broken.css"), std::string::npos);

    auto late = layer(kLateImport);
    late.modules["late.css"] = kLateModule;
    const auto lateResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, late}});
    ASSERT_FALSE(lateResult.ok());
    ASSERT_FALSE(lateResult.errors.empty());
    EXPECT_EQ(lateResult.errors.front().code, "stylesheet.import.order");

    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 44.f);
}

TEST(StyleSheetTest, NormalizesSelectorNamesAndUsesCSSIdentifierSyntax) {
    constexpr char kMixedCaseSelector[] = "BuTtOn { width: 23px; }";
    constexpr char kUnderscoreSelector[] = "button#bad_id { width: 29px; }";
    constexpr char kEscapedIdSelector[] = "button#bad\\.id { width: 31px; }";
    constexpr char kEscapedColonSelector[] = "button#bad\\:id { width: 37px; }";
    constexpr char kHexEscapedIdSelector[] = "button#\\31 23\\:bad\\.id { width: 41px; }";
    constexpr char kInvalidIdSelector[] = "button#bad@id { width: 1px; }";
    constexpr char kInvalidPseudoElementSelector[] = "floater::head.part { width: 1px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMixedCaseSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).width.pixels(), 23.f);

    ASSERT_TRUE(stylesheet.loadRadia(kUnderscoreSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad_id", {}, 0).width.pixels(), 29.f);

    ASSERT_TRUE(stylesheet.loadRadia(kEscapedIdSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad.id", {}, 0).width.pixels(), 31.f);

    ASSERT_TRUE(stylesheet.loadRadia(kEscapedColonSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad:id", {}, 0).width.pixels(), 37.f);

    ASSERT_TRUE(stylesheet.loadRadia(kHexEscapedIdSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "123:bad.id", {}, 0).width.pixels(), 41.f);

    const auto invalidId = stylesheet.loadRadia(kInvalidIdSelector);
    ASSERT_FALSE(invalidId.ok());
    ASSERT_FALSE(invalidId.errors.empty());
    EXPECT_EQ(invalidId.errors.front().code, "stylesheet.selector.id_invalid");

    const auto invalidPseudoElement = stylesheet.loadRadia(kInvalidPseudoElementSelector);
    ASSERT_FALSE(invalidPseudoElement.ok());
    ASSERT_FALSE(invalidPseudoElement.errors.empty());
    EXPECT_EQ(invalidPseudoElement.errors.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheetTest, ResolvesNestedInlineKbdSelectors) {
    constexpr char kKbdStyles[] = "kbd { padding: 1px; border-radius: 4px; > kbd { padding: 2px; "
                                  "border-radius: 3px; } } p > kbd { gap: 5px; }";
    constexpr char kRejectedKbdPseudoElement[] = "kbd::key { padding: 1px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kKbdStyles).ok());

    auto owner = makeElementValue<Element>("p");
    const ComputedStyle chord = stylesheet.resolveInline(owner, "kbd");
    const ComputedStyle key = stylesheet.resolveInline(owner, "kbd", {"kbd"});
    EXPECT_EQ(chord.padding.left, 1.f);
    EXPECT_EQ(chord.gap.fixedPixels(), 5.f);
    EXPECT_EQ(key.padding.left, 2.f);
    EXPECT_EQ(key.borderRadius.topLeft.horizontal.pixels, 3.f);
    EXPECT_EQ(key.borderRadius.topLeft.vertical.pixels, 3.f);
    EXPECT_NE(key.gap.fixedPixels(), 5.f);

    const auto rejectedPseudoElement = stylesheet.loadRadia(kRejectedKbdPseudoElement);
    ASSERT_FALSE(rejectedPseudoElement.ok());
    ASSERT_FALSE(rejectedPseudoElement.errors.empty());
    EXPECT_EQ(rejectedPseudoElement.errors.front().code, "stylesheet.selector.pseudo_element_unknown");
}

TEST(StyleSheetTest, PreservesSourceOrderAcrossNestedRules) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button { color: #ff0000ff; & { color: #00ff00ff; } color: #0000ffff; }").ok());

    const ComputedStyle style = stylesheet.resolve("button", "", {}, 0);
    EXPECT_FLOAT_EQ(style.color.r, 0.f);
    EXPECT_FLOAT_EQ(style.color.g, 0.f);
    EXPECT_FLOAT_EQ(style.color.b, 1.f);
}

TEST(StyleSheetTest, ReportsEachSharedImportFailure) {
    constexpr char kEntrypointImports[] = "@import \"branch-a.css\"; @import \"branch-b.css\";";
    constexpr char kBranchA[] = "@import \"shared.css\"; panel { width: 10px; }";
    constexpr char kBranchB[] = "@import \"shared.css\"; panel { height: 20px; }";
    constexpr char kSharedFailure[] = "panel { unknown-property: 1; }";

    StyleSheet stylesheet;
    ResourceLayer layer{"theme/main.css", kEntrypointImports};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"branch-a.css", kBranchA},
        {"branch-b.css", kBranchB},
        {"shared.css", kSharedFailure},
    };

    const auto result = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}});
    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), std::size_t(2));
    EXPECT_EQ(result.errors[0].source, "theme/shared.css");
    EXPECT_EQ(result.errors[1].source, "theme/shared.css");
}

TEST(StyleSheetTest, MarksStateBorderChangesAsLayoutAffecting) {
    constexpr char kStateBorderStyles[] = "fieldset { border: 1px #ffffff; } "
                                          "fieldset:hover { border: 4px #ffffff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStateBorderStyles).ok());
    EXPECT_TRUE(stylesheet.stateAffectsLayout(ElementState::Hovered));
}

TEST(StyleSheetTest, MarksStateAppearanceChangesAsLayoutAffecting) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:hover { appearance: none; }").ok());
    EXPECT_TRUE(stylesheet.stateAffectsLayout(ElementState::Hovered));
}
