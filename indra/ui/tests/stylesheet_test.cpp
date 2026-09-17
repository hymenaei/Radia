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
#include <variant>
#include <vector>
#include "css/rules.h"
#include "css/stylesheet.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "resource/elementdefinition.h"
#include "style/stylepass.h"
#include "text/metrics.h"

namespace {
using radia::ui::AccentColor;
using radia::ui::BackgroundAttachment;
using radia::ui::BackgroundBox;
using radia::ui::BackgroundRepeat;
using radia::ui::BackgroundSizeMode;
using radia::ui::BlurFilter;
using radia::ui::Color;
using radia::ui::ColorScheme;
using radia::ui::ComputedStyle;
using radia::ui::CursorStyle;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::ElementState;
using radia::ui::FixedTextMetrics;
using radia::ui::FontFamily;
using radia::ui::Gradient;
using radia::ui::GradientKind;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::LayoutDirection;
using radia::ui::LinearBlurFilter;
using radia::ui::LineHeight;
using radia::ui::MaskComposite;
using radia::ui::MaskLayer;
using radia::ui::MaskMode;
using radia::ui::MaskType;
using radia::ui::Overflow;
using radia::ui::PointerEvents;
using radia::ui::RadialGradientShape;
using radia::ui::ResourceLayer;
using radia::ui::ScrollbarGutter;
using radia::ui::ScrollbarWidth;
using radia::ui::SelectorCombinator;
using radia::ui::StyleLayer;
using radia::ui::StyleOrigin;
using radia::ui::StylePass;
using radia::ui::StyleRule;
using radia::ui::StyleSheet;
using radia::ui::TextAlign;
using radia::ui::TextDecoration;
using radia::ui::VerticalAlign;
using radia::ui::Visibility;
using radia::ui::detail::ElementInternalAccess;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
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

constexpr char kColorTokenStyles[] = ":root { --accent: hsl(120 100% 50%); --ink: rgb(255, 0, 0, 50%); } "
                                     "button { background-color: var(--accent); color: var(--ink); "
                                     "font-size: 17px; } "
                                     "label { color: #00ff00ff; font-size: 29px; }";
} // namespace

TEST(StyleSheetTest, ResolvesInheritedColors) {
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
    rootOwner->classList().add("root");
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

TEST(StyleSheetTest, ScopesStructuralSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(":root { width: 24px; } .outer .inner { opacity: .5; } .inner > label { width: 13px; }").ok());

    auto outer = makeElementValue<HTMLPanelElement>();
    outer.classList().add("outer");
    auto resource = makeElement<HTMLPanelElement>();
    resource->classList().add("inner");
    ElementInternalAccess::setIdScopeRoot(*resource);
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    resource->append(std::move(label));
    outer.append(std::move(resource));

    const ComputedStyle resourceStyle = computedStyle(stylesheet, *outer.children().front());
    EXPECT_EQ(resourceStyle.width.pixels(), 24.f);
    EXPECT_EQ(resourceStyle.opacity, 1.f);

    const ComputedStyle labelStyle = computedStyle(stylesheet, *labelPointer);
    EXPECT_EQ(labelStyle.width.pixels(), 13.f);
}

TEST(StyleSheetTest, StartsWithoutImplicitCoreRules) {
    StyleSheet stylesheet;
    const ComputedStyle paragraph = stylesheet.resolve("p", "", {}, 0);

    EXPECT_FALSE(paragraph.displaySet);
    EXPECT_EQ(paragraph.fontWeight, static_cast<U16>(400));
}

TEST(StyleSheetTest, KeepsValidDeclarationsAroundInvalidColor) {
    constexpr char kInvalidColorStyles[] = "input { background-color: ##invalid; width: 8px; }";

    StyleSheet stylesheet;
    const auto invalid = stylesheet.loadRadia(kInvalidColorStyles, "invalid.css");

    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(invalid.warnings.front().source, "invalid.css");
    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0).width.pixels(), 8.f);
}

TEST(StyleSheetTest, ParsesCommentSeparatedColorComponents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { color: rgb(1/**/2/**/3); }").ok());

    const Color color = stylesheet.resolve("panel", "", {}, 0).color;
    EXPECT_NEAR(color.r, 1.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.g, 2.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.b, 3.f / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, ParsesBoxShorthands) {
    constexpr char kBoxSpacingStyles[] = "panel { margin: 1px auto 3px -4px; padding: 5px 6px; gap: 7px 11px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxSpacingStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(style.margin.top.fixedPixels(), 1.f);
    EXPECT_TRUE(style.margin.right.isAuto());
    EXPECT_EQ(style.margin.bottom.fixedPixels(), 3.f);
    EXPECT_EQ(style.margin.left.fixedPixels(), -4.f);
    EXPECT_FALSE(style.margin.left.isAuto());
    EXPECT_EQ(style.padding.left, 6.f);
    EXPECT_EQ(style.rowGap.fixedPixels(), 7.f);
    EXPECT_EQ(style.columnGap.fixedPixels(), 11.f);
}

TEST(StyleSheetTest, ParsesGapLonghands) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { row-gap: 13px; column-gap: 17px; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(style.rowGap.fixedPixels(), 13.f);
    EXPECT_EQ(style.columnGap.fixedPixels(), 17.f);
}

TEST(StyleSheetTest, AcceptsUnitlessShadowZero) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { box-shadow: 0 0 0 #123456; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_EQ(style.shadows.size(), std::size_t(1));
    EXPECT_EQ(style.shadows.front().horizontal, 0.f);
    EXPECT_EQ(style.shadows.front().vertical, 0.f);
    EXPECT_EQ(style.shadows.front().blur, 0.f);
}

TEST(StyleSheetTest, DistinguishesFocusStates) {
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

TEST(StyleSheetTest, AcceptsElementlessPseudos) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(":focus-visible { border-width: 2px; }").ok());

    const uint16_t focusVisible = ElementState::Focused | ElementState::FocusVisible;
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth.top, 2.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, static_cast<uint8_t>(ElementState::Focused)).borderWidth.top, 0.f);
}

TEST(StyleSheetTest, MatchesDirectionState) {
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

TEST(StyleSheetTest, KeepsValidDeclarationsAroundUnknown) {
    constexpr char kInvalidStyles[] = "button { width: 99px; unknown-property: 1; height: 7px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kInvalidStyles, "candidate.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).width.pixels(), 99.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).height.pixels(), 7.f);
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.unknown");
    EXPECT_EQ(result.warnings.front().source, "candidate.css");
}

TEST(StyleSheetTest, RejectsInvalidDeclarationPropertyNames) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":root { --bad name: #ff0000; } panel { width: 13px; }", "property-name.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 13.f);
}

TEST(StyleSheetTest, AcceptsArbitraryCustomPropertyValues) {
    constexpr char kCustomPropertyStyles[] = ":root { --bad: nonsense; --empty:; --number: .1; --case-token: MiXeD; } "
                                             "panel.empty { width: var(--empty, 12px); } panel.fallback { width: var(--missing,); } "
                                             "panel.boundary { opacity: var(--number)0; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kCustomPropertyStyles, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"empty"}, 0).width.isAuto());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"fallback"}, 0).width.isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"boundary"}, 0).opacity, 1.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).customProperties.at("--case-token").source, "MiXeD");
}

TEST(StyleSheetTest, RejectsReferencesToMissingTokens) {
    constexpr char kMissingTokenStyles[] = "button { width: var(--missing); }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kMissingTokenStyles, "missing-token.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("button", "", {}, 0).width.isAuto());
}

TEST(StyleSheetTest, InvalidCustomPropertySubstitutionUsesUnsetValue) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { width: 24px; opacity: .5; } label.invalid { width: var(--missing); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    label.classList().add("invalid");

    const ComputedStyle style = computedStyle(stylesheet, label);
    EXPECT_TRUE(style.width.isAuto());
    EXPECT_EQ(style.opacity, .5f);
}

TEST(StyleSheetTest, ResolvesCustomPropertyFallbacksAndCycles) {
    constexpr char kStyles[] = ":root { --inherited-width: 24px; --cycle-a: var(--cycle-b); --cycle-b: var(--cycle-a); color: #123456; } "
                               "label { width: var(--inherited-width); opacity: .5; } "
                               "label.fallback { height: var(--missing-height, 12px); min-width: var(--cycle-a, 8px); } "
                               "label.invalid { height: var(--cycle-a); }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    auto root = makeElementValue<HTMLPanelElement>();
    auto fallback = makeElement<HTMLLabelElement>();
    fallback->classList().add("fallback");
    HTMLLabelElement* fallbackPointer = fallback.get();
    root.append(std::move(fallback));
    auto invalid = makeElement<HTMLLabelElement>();
    invalid->classList().add("invalid");
    HTMLLabelElement* invalidPointer = invalid.get();
    root.append(std::move(invalid));

    const ComputedStyle fallbackStyle = computedStyle(stylesheet, *fallbackPointer);
    EXPECT_EQ(fallbackStyle.width.pixels(), 24.f);
    EXPECT_EQ(fallbackStyle.height.pixels(), 12.f);
    ASSERT_TRUE(fallbackStyle.minWidth.has_value());
    EXPECT_EQ(fallbackStyle.minWidth->pixels, 8.f);
    EXPECT_NEAR(fallbackStyle.color.r, 0x12 / 255.f, 1.0e-4f);
    EXPECT_EQ(fallbackStyle.opacity, .5f);

    const ComputedStyle invalidStyle = computedStyle(stylesheet, *invalidPointer);
    EXPECT_TRUE(invalidStyle.height.isAuto());
    EXPECT_EQ(invalidStyle.width.pixels(), 24.f);
    EXPECT_EQ(invalidStyle.opacity, .5f);
}

TEST(StyleSheetTest, DoesNotUseFallbackInsideCustomPropertyCycle) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --a: var(--b, 2px); --b: var(--a, 3px); width: var(--a, 7px); height: var(--a); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width.pixels(), 7.f);
    EXPECT_TRUE(style.height.isAuto());
}

TEST(StyleSheetTest, TreatsInitialCustomPropertyAsGuaranteedInvalid) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --token: initial; width: var(--token, 17px); height: var(--token); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width.pixels(), 17.f);
    EXPECT_TRUE(style.height.isAuto());
}

TEST(StyleSheetTest, MatchesChildSelectors) {
    constexpr char kChildOwnerStyles[] = "button.primary > i { width: 10px; } "
                                         "button.primary:hover > i { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kChildOwnerStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("primary");
    Element& icon = appendIcon(button, "search");

    EXPECT_EQ(computedStyle(stylesheet, icon).width.pixels(), 10.f);
    ElementInternalAccess::setState(button, ElementState::Hovered, true);
    EXPECT_EQ(computedStyle(stylesheet, icon).width.pixels(), 18.f);
}

TEST(StyleSheetTest, IgnoresCommentsAroundChildCombinators) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button /* comment */ > i { width: 19px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    Element& icon = appendIcon(button, "search");
    EXPECT_EQ(computedStyle(stylesheet, icon).width.pixels(), 19.f);
}

TEST(StyleSheetTest, MatchesSiblingSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label + button { width: 10px; } label ~ input { height: 12px; }").ok());

    auto parent = makeElement<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* buttonPointer = button.get();
    auto spacer = makeElement<HTMLPanelElement>();
    auto input = makeElement<HTMLInputElement>();
    HTMLInputElement* inputPointer = input.get();
    parent->append(std::move(label));
    parent->append(std::move(button));
    parent->append(std::move(spacer));
    parent->append(std::move(input));

    EXPECT_EQ(computedStyle(stylesheet, *buttonPointer).width.pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, *inputPointer).height.pixels(), 12.f);
}

TEST(StyleSheetTest, BacktracksMixedSelectorAncestors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label.lead ~ panel.branch label.target { width: 33px; }").ok());

    auto root = makeElement<HTMLPanelElement>();
    auto lead = makeElement<HTMLLabelElement>();
    lead->classList().add("lead");
    root->append(std::move(lead));

    auto outerBranch = makeElement<HTMLPanelElement>();
    outerBranch->classList().add("branch");
    auto innerBranch = makeElement<HTMLPanelElement>();
    innerBranch->classList().add("branch");
    auto target = makeElement<HTMLLabelElement>();
    target->classList().add("target");
    HTMLLabelElement* targetPointer = target.get();
    innerBranch->append(std::move(target));
    outerBranch->append(std::move(innerBranch));
    root->append(std::move(outerBranch));

    const ComputedStyle style = computedStyle(stylesheet, *targetPointer);
    ASSERT_FALSE(style.width.isAuto());
    EXPECT_EQ(style.width.pixels(), 33.f);
}

TEST(StyleSheetTest, InvalidatesFollowingSiblingSelectorsOnMutation) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("label.lead + button { width: 10px; } label#lead + button { height: 14px; } "
                               "label[data-lead] ~ input { height: 12px; }")
                    .ok());

    auto parent = makeElement<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* buttonPointer = button.get();
    auto spacer = makeElement<HTMLPanelElement>();
    auto input = makeElement<HTMLInputElement>();
    HTMLInputElement* inputPointer = input.get();
    parent->append(std::move(label));
    parent->append(std::move(button));
    parent->append(std::move(spacer));
    parent->append(std::move(input));

    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_TRUE(styles.style(*buttonPointer).width.isAuto());
    EXPECT_TRUE(styles.style(*buttonPointer).height.isAuto());
    EXPECT_TRUE(styles.style(*inputPointer).height.isAuto());

    labelPointer->classList().add("lead");
    EXPECT_EQ(styles.style(*buttonPointer).width.pixels(), 10.f);
    labelPointer->classList().remove("lead");
    EXPECT_TRUE(styles.style(*buttonPointer).width.isAuto());

    labelPointer->setId("lead");
    EXPECT_EQ(styles.style(*buttonPointer).height.pixels(), 14.f);
    labelPointer->setId("");
    EXPECT_TRUE(styles.style(*buttonPointer).height.isAuto());

    labelPointer->setAttribute("data-lead", "true");
    EXPECT_EQ(styles.style(*inputPointer).height.pixels(), 12.f);
    labelPointer->removeAttribute("data-lead");
    EXPECT_TRUE(styles.style(*inputPointer).height.isAuto());

    auto inserted = makeElement<HTMLLabelElement>();
    inserted->classList().add("lead");
    auto* insertedPointer = buttonPointer->before(std::move(inserted));
    ASSERT_NE(insertedPointer, nullptr);
    EXPECT_EQ(styles.style(*buttonPointer).width.pixels(), 10.f);
    auto removed = insertedPointer->remove();
    ASSERT_NE(removed, nullptr);
    EXPECT_TRUE(styles.style(*buttonPointer).width.isAuto());
}

TEST(StyleSheetTest, ComparesSpecificityAsCSS) {
    constexpr char kStyles[] = "button.a.b.c.d.e.f.g.h.i.j { width: 10px; } button#save { width: 20px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.setId("save");
    for (const char* className : {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"}) button.classList().add(className);

    EXPECT_EQ(computedStyle(stylesheet, button).width.pixels(), 20.f);
}

TEST(StyleSheetTest, CountsRepeatedPseudoSpecificity) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button:hover:hover { width: 10px; } button:hover { width: 20px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    ElementInternalAccess::setState(button, ElementState::Hovered, true);
    EXPECT_EQ(computedStyle(stylesheet, button).width.pixels(), 10.f);
}

TEST(StyleSheetTest, MatchesCompoundClassesAndStates) {
    constexpr char kStyles[] = "button.primary.priority:hover:focus { width: 23px; } button.primary { width: 7px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("primary").add("priority");
    ElementInternalAccess::setState(button, ElementState::Hovered, true);
    ElementInternalAccess::setState(button, ElementState::Focused, true);

    EXPECT_EQ(computedStyle(stylesheet, button).width.pixels(), 23.f);
    ElementInternalAccess::setState(button, ElementState::Focused, false);
    EXPECT_EQ(computedStyle(stylesheet, button).width.pixels(), 7.f);
}

TEST(StyleSheetTest, ParsesColumnCombinator) {
    const StyleRule rule = radia::ui::detail::parseSelector("label || button");

    ASSERT_EQ(rule.selectors.size(), std::size_t(2));
    ASSERT_EQ(rule.combinators.size(), std::size_t(1));
    EXPECT_EQ(rule.combinators.front(), SelectorCombinator::Column);

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("label || button { width: 10px; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
}

TEST(StyleSheetTest, MatchesCaseInsensitivePseudoStatesAndRootTokens) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":/**/ROOT { --accent: #123456; } button:HOVER { width: 18px; } panel { color: var(--accent); }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, static_cast<uint16_t>(ElementState::Hovered)).width.pixels(), 18.f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {}, 0).color.r, 0x12 / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, DecodesEscapedSelectorKeywords) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button:h\\6f ver { width: 18px; } input:dir(r\\74 l) { height: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, static_cast<uint16_t>(ElementState::Hovered)).width.pixels(), 18.f);
    EXPECT_EQ(stylesheet.resolve("input", "", {}, 0, LayoutDirection::RightToLeft).height.pixels(), 13.f);
}

TEST(StyleSheetTest, SeparatesPartState) {
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

TEST(StyleSheetTest, MatchesPseudoElementState) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input::slider-thumb { width: 10px; } input::slider-thumb:hover { width: 18px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true);

    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb", LayoutDirection::LeftToRight).width.pixels(), 10.f);
    ElementInternalAccess::setState(input, ElementState::Hovered, true);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb", LayoutDirection::LeftToRight).width.pixels(), 18.f);
}

TEST(StyleSheetTest, PreservesCursorOnFailure) {
    constexpr char kCursorStyles[] = "button { cursor: pointer; } #horizontal { cursor: e-resize; } "
                                     "#diagonal { cursor: sw-resize; } #grab { cursor: grab; } "
                                     "#grabbing { cursor: grabbing; }";
    constexpr char kInvalidCursorStyles[] = "button { cursor: teleport; cursor: pointer; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCursorStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).cursor, CursorStyle::Pointer);
    EXPECT_EQ(stylesheet.resolve("panel", "horizontal", {}, 0).cursor, CursorStyle::EastWestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "diagonal", {}, 0).cursor, CursorStyle::NortheastSouthwestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "grab", {}, 0).cursor, CursorStyle::Grab);
    EXPECT_EQ(stylesheet.resolve("panel", "grabbing", {}, 0).cursor, CursorStyle::Grabbing);
    EXPECT_NE(stylesheet.resolve("panel", "grab", {}, 0).cursor, stylesheet.resolve("panel", "grabbing", {}, 0).cursor);

    const auto invalid = stylesheet.loadRadia(kInvalidCursorStyles, "cursor.css");
    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(stylesheet.resolve("button", "", {}, 0).cursor, CursorStyle::Pointer);
}

TEST(StyleSheetTest, CompilesTargetRules) {
    constexpr char kRelevantAndIrrelevantStyles[] =
        "input { padding: 4px; "
        "&:checked::slider-thumb { order: 1; } } "
        "input::slider-track { display: flex; } input::slider-thumb { border-radius: 10px; } label { font-size: 13px; } "
        "panel { display: flex; flex-direction: row; font-size: 14px; } input { display: flex; flex-direction: row; } "
        "label { gap: 2px; align-items: center; } "
        "i { font-size: 13px; } "
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

TEST(StyleSheetTest, ClearsRemovedRadioName) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name] { width: 20px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width.pixels(), 40.f);

    input.name("");
    EXPECT_FALSE(input.hasAttribute("name"));
    EXPECT_EQ(computedStyle(stylesheet, input).width.pixels(), 10.f);
}

TEST(StyleSheetTest, RefreshesSerializedDisabledAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input[disabled=one] { width: 10px; } input[disabled=two] { width: 20px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("disabled", "one");
    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_EQ(styles.style(input).width.pixels(), 10.f);

    input.setAttribute("disabled", "two");
    EXPECT_EQ(styles.style(input).width.pixels(), 20.f);
}

TEST(StyleSheetTest, MatchesCSSAttributeOperators) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel[data-value=\"Alpha-beta gamma\"] { width: 10px; } "
                                             "panel[data-value^=alpha i] { height: 11px; } "
                                             "panel[data-value$=GAMMA i] { left: 12px; } "
                                             "panel[data-value*=PHA-B i] { right: 13px; } "
                                             "panel[data-value~=gamma] { top: 14px; } "
                                             "panel[data-value|=alpha i] { bottom: 15px; } "
                                             "panel[data-value=alpha s] { opacity: .2; } "
                                             "panel[type=text] { flex-grow: 1; } "
                                             "panel[type=text i] { flex-shrink: 2; } "
                                             "panel[type=text s] { order: 3; } "
                                             "i[class^=\"i-\"] { opacity: .5; }");
    ASSERT_TRUE(result.ok()) << (result.errors.empty() ? "" : result.errors.front().message);

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setAttribute("data-value", "Alpha-beta gamma");
    EXPECT_EQ(computedStyle(stylesheet, panel).width.pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).height.pixels(), 11.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).left->pixels, 12.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).right->pixels, 13.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).top->pixels, 14.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).bottom->pixels, 15.f);
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, panel).opacity, 1.f);
    panel.setAttribute("type", "TeXt");
    ComputedStyle mixedType = computedStyle(stylesheet, panel);
    EXPECT_EQ(mixedType.flexGrow, 1.f);
    EXPECT_EQ(mixedType.flexShrink, 2.f);
    EXPECT_EQ(mixedType.order, 0);

    panel.setAttribute("type", "text");
    mixedType = computedStyle(stylesheet, panel);
    EXPECT_EQ(mixedType.flexGrow, 1.f);
    EXPECT_EQ(mixedType.flexShrink, 2.f);
    EXPECT_EQ(mixedType.order, 3);

    auto icon = makeElement<Element>("i");
    icon->classList().add("i-search");
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, *icon).opacity, .5f);
}

TEST(StyleSheetTest, RequiresExplicitUniversalForAttributeSelectors) {
    StyleSheet invalid;
    const auto invalidResult = invalid.loadRadia("[hidden] { width: 11px; }");
    ASSERT_TRUE(invalidResult.ok());
    ASSERT_FALSE(invalidResult.warnings.empty());
    EXPECT_EQ(invalidResult.warnings.front().code, "stylesheet.selector.target_required");

    StyleSheet universal;
    ASSERT_TRUE(universal.loadRadia("*[hidden] { width: 11px; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setAttribute("hidden");
    EXPECT_EQ(computedStyle(universal, panel).width.pixels(), 11.f);

    StyleSheet universalPseudo;
    const auto pseudoResult = universalPseudo.loadRadia("*::unknown { width: 12px; }");
    ASSERT_TRUE(pseudoResult.ok());
    ASSERT_FALSE(pseudoResult.warnings.empty());
    EXPECT_EQ(pseudoResult.warnings.front().code, "stylesheet.selector.target_required");
}

TEST(StyleSheetTest, DecodesEscapedAttributeSelectorDelimiters) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(R"(i[data=foo\]] { width: 12px; })").ok());

    auto icon = makeElement<Element>("i");
    icon->setAttribute("data", "foo]");
    EXPECT_EQ(computedStyle(stylesheet, *icon).width.pixels(), 12.f);
}

TEST(StyleSheetTest, InvalidatesDynamicAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel[hidden] { width: 11px; }").ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_TRUE(styles.style(panel).width.isAuto());
    EXPECT_TRUE(styles.style(panel).height.isAuto());

    panel.setAttribute("hidden");
    ASSERT_FALSE(styles.style(panel).width.isAuto());
    EXPECT_EQ(styles.style(panel).width.pixels(), 11.f);
    panel.removeAttribute("hidden");
    EXPECT_TRUE(styles.style(panel).width.isAuto());

    panel.setVisibility(Visibility::Hidden);
    EXPECT_FALSE(panel.hasAttribute("visibility"));
    panel.setVisibility(Visibility::Visible);
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

TEST(StyleSheetTest, SkipsInvalidRules) {
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
        {"button { &:cheked { opacity: .5; } }", "stylesheet.selector.state_unknown"},
        {"label { order: 1.5; }", "stylesheet.property.value_invalid"},
        {"label { order: 1px; }", "stylesheet.property.value_invalid"},
        {"label:is(button) { order: 1; }", "stylesheet.selector.function_unsupported"},
        {"panel { scrollbar-width: wide; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-gutter: stable auto; }", "stylesheet.property.value_invalid"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid CSS: " << test.source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.source, "contract.css");

        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, test.diagnostic);
    }
}

TEST(StyleSheetTest, SkipsMultiplePseudoElementStates) {
    constexpr char kPseudoElementStateStyles[] = "input::slider-thumb:hover:checked { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kPseudoElementStateStyles, "contract.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheetTest, SkipsNestedPseudoElements) {
    constexpr char kNestedPseudoElementStyles[] = "input::slider-track::slider-fill { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kNestedPseudoElementStyles, "contract.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheetTest, InheritsAllowedProperties) {
    constexpr char kInheritedStyles[] = "panel { font-family: sans-serif; font-size: 19px; font-weight: bold; "
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
    EXPECT_EQ(inheritedStyle.fontFamily, FontFamily::SansSerif);
    EXPECT_EQ(inheritedStyle.fontSize, 19.f);
    EXPECT_EQ(inheritedStyle.fontWeight, static_cast<U16>(700));
    EXPECT_TRUE(inheritedStyle.fontItalic);
    EXPECT_EQ(inheritedStyle.lineHeight.kind, LineHeight::Kind::Length);
    EXPECT_EQ(inheritedStyle.lineHeight.value, 23.f);
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

TEST(StyleSheetTest, OmitsBorderStyleByDefault) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { border: 3px #123456; } panel.none { border-style: none; border-width: 4px; }").ok());

    const ComputedStyle omitted = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(omitted.borderWidth.top, 3.f);
    EXPECT_EQ(omitted.borderStyle, radia::ui::BorderStyle::NoneValue);

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"}, 0);
    EXPECT_EQ(none.borderWidth.top, 4.f);
    EXPECT_EQ(none.borderStyle, radia::ui::BorderStyle::NoneValue);
}

TEST(StyleSheetTest, PropagatesTextDecorationWithoutInheritingProperty) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("panel { text-decoration: underline; } label.explicit { text-decoration: line-through; } "
                               "label.inherit { text-decoration: underline; text-decoration: inherit; }")
                    .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto inherited = makeElement<HTMLLabelElement>();
    HTMLLabelElement* inheritedPointer = inherited.get();
    panel.append(std::move(inherited));
    auto explicitDecoration = makeElement<HTMLLabelElement>();
    explicitDecoration->classList().add("explicit");
    HTMLLabelElement* explicitPointer = explicitDecoration.get();
    panel.append(std::move(explicitDecoration));

    const ComputedStyle parentStyle = computedStyle(stylesheet, panel);
    EXPECT_EQ(parentStyle.textDecoration, TextDecoration::Underline);
    EXPECT_EQ(parentStyle.textDecorationPropagation, TextDecoration::Underline);

    const ComputedStyle inheritedStyle = computedStyle(stylesheet, *inheritedPointer);
    EXPECT_EQ(inheritedStyle.textDecoration, TextDecoration::NoneValue);
    EXPECT_EQ(inheritedStyle.textDecorationPropagation, TextDecoration::Underline);

    auto inheritedDecoration = makeElement<HTMLLabelElement>();
    inheritedDecoration->classList().add("inherit");
    HTMLLabelElement* inheritedDecorationPointer = inheritedDecoration.get();
    panel.append(std::move(inheritedDecoration));
    const ComputedStyle explicitInheritanceStyle = computedStyle(stylesheet, *inheritedDecorationPointer);
    EXPECT_EQ(explicitInheritanceStyle.textDecoration, TextDecoration::Underline);

    const ComputedStyle explicitStyle = computedStyle(stylesheet, *explicitPointer);
    const auto both =
        static_cast<TextDecoration>(static_cast<unsigned>(TextDecoration::Underline) | static_cast<unsigned>(TextDecoration::LineThrough));
    EXPECT_EQ(explicitStyle.textDecoration, TextDecoration::LineThrough);
    EXPECT_EQ(explicitStyle.textDecorationPropagation, both);
}

TEST(StyleSheetTest, ResolvesRelativeFontWeights) {
    constexpr char kStyles[] = "panel.w400 { font-weight: 400; } panel.w550 { font-weight: 550; } panel.w1000 { font-weight: 1000; } "
                               "label.bolder { font-weight: bolder; } label.lighter { font-weight: lighter; }";
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const auto childWeight = [&](const char* parentClass, const char* childClass) {
        auto parent = makeElementValue<HTMLPanelElement>();
        parent.classList().add(parentClass);
        auto child = makeElement<HTMLLabelElement>("Child");
        HTMLLabelElement* childPointer = child.get();
        childPointer->classList().add(childClass);
        parent.append(std::move(child));
        return computedStyle(stylesheet, *childPointer).fontWeight;
    };

    EXPECT_EQ(childWeight("w400", "bolder"), static_cast<U16>(700));
    EXPECT_EQ(childWeight("w400", "lighter"), static_cast<U16>(100));
    EXPECT_EQ(childWeight("w550", "bolder"), static_cast<U16>(900));
    EXPECT_EQ(childWeight("w550", "lighter"), static_cast<U16>(400));
    EXPECT_EQ(childWeight("w1000", "bolder"), static_cast<U16>(1000));
    EXPECT_EQ(childWeight("w1000", "lighter"), static_cast<U16>(700));
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
    explicitInherit->classList().add("explicit-inherit");
    HTMLLabelElement* explicitInheritPtr = explicitInherit.get();
    parent.append(std::move(explicitInherit));
    auto unsetInherited = makeElement<HTMLLabelElement>();
    unsetInherited->classList().add("unset-inherited");
    HTMLLabelElement* unsetInheritedPtr = unsetInherited.get();
    parent.append(std::move(unsetInherited));
    auto unsetInitial = makeElement<HTMLLabelElement>();
    unsetInitial->classList().add("unset-initial");
    HTMLLabelElement* unsetInitialPtr = unsetInitial.get();
    parent.append(std::move(unsetInitial));
    auto lateInherit = makeElement<HTMLLabelElement>();
    lateInherit->classList().add("late-inherit");
    HTMLLabelElement* lateInheritPtr = lateInherit.get();
    parent.append(std::move(lateInherit));
    auto lateValue = makeElement<HTMLLabelElement>();
    lateValue->classList().add("late-value");
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

TEST(StyleSheetTest, ParsesOverflowShorthand) {
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
    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleSheetTest, ParsesBorderRadius) {
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

TEST(StyleSheetTest, SkipsMalformedBorderRadius) {
    constexpr char kInvalidStyles[] = "panel { border-radius: 1px /; }";
    constexpr char kMultipleSlashStyles[] = "panel { border-radius: 1px / 2px / 3px; }";
    constexpr char kTooManyValuesStyles[] = "panel { border-radius: 1px 2px 3px 4px 5px; }";

    for (const char* source : {kInvalidStyles, kMultipleSlashStyles, kTooManyValuesStyles}) {
        SCOPED_TRACE(Message() << "malformed border-radius CSS: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "border-radius.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleSheetTest, ParsesScrollbarCSSProperties) {
    constexpr char kScrollbarStyles[] = "panel { scrollbar-width: thin; scrollbar-gutter: stable both-edges; scrollbar-color: #112233 #445566; } "
                                        "#classic { scrollbar-width: none; scrollbar-gutter: stable; } "
                                        "#initial { scrollbar-width: initial; scrollbar-gutter: initial; scrollbar-color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kScrollbarStyles).ok());

    const ComputedStyle overlay = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(overlay.scrollbarWidth, ScrollbarWidth::Thin);
    EXPECT_EQ(overlay.scrollbarGutter, ScrollbarGutter::StableBothEdges);
    EXPECT_FALSE(overlay.scrollbarColor.automatic);
    EXPECT_NEAR(overlay.scrollbarColor.thumb.r, 0x11 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor.thumb.g, 0x22 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor.track.b, 0x66 / 255.f, 1.0e-6f);

    const ComputedStyle classic = stylesheet.resolve("panel", "classic", {}, 0);
    EXPECT_EQ(classic.scrollbarWidth, ScrollbarWidth::NoneValue);
    EXPECT_EQ(classic.scrollbarGutter, ScrollbarGutter::Stable);

    const ComputedStyle initial = stylesheet.resolve("panel", "initial", {}, 0);
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

TEST(StyleSheetTest, OverridesColorScheme) {
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
    overriddenInputPtr->type("checkbox");
    overriddenInputPtr->classList().add("dark");
    panel.append(std::move(overriddenInput));

    EXPECT_EQ(computedStyle(stylesheet, *inheritedInputPtr).colorScheme, ColorScheme::Light);
    EXPECT_EQ(computedStyle(stylesheet, *overriddenInputPtr).colorScheme, ColorScheme::Dark);
}

TEST(StyleSheetTest, ResolvesInheritedSchemeColors) {
    constexpr char kLightDarkStyles[] = ":root { color-scheme: light; } panel.dark { color-scheme: dark; } "
                                        "label { color: light-dark(#101010, #f0f0f0); }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLightDarkStyles).ok());

    auto lightPanel = makeElementValue<HTMLPanelElement>();
    auto lightLabel = makeElement<HTMLLabelElement>("light");
    HTMLLabelElement* lightLabelPtr = lightLabel.get();
    lightPanel.append(std::move(lightLabel));

    auto darkPanel = makeElementValue<HTMLPanelElement>();
    darkPanel.classList().add("dark");
    auto darkLabel = makeElement<HTMLLabelElement>("dark");
    HTMLLabelElement* darkLabelPtr = darkLabel.get();
    darkPanel.append(std::move(darkLabel));

    EXPECT_NEAR(computedStyle(stylesheet, *lightLabelPtr).color.r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(computedStyle(stylesheet, *darkLabelPtr).color.r, 240.f / 255.f, 1.0e-4f);
}

TEST(StyleSheetTest, ProjectsMinimizedState) {
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

TEST(StyleSheetTest, ParsesTypedDimensions) {
    constexpr char kTypedLengthStyles[] = "panel { width: 40px; min-width: 20px; min-height: 10; left: -8px; line-height: 18px; }";
    constexpr char kAutoDimensionStyles[] = "panel { width: 40px; height: 20px; width: auto; height: auto; } "
                                            "button { size: auto; } i { size: auto 16px; }";
    constexpr char kUnitlessLineHeightStyles[] = "panel { font-size: 20px; line-height: 1.5; min-width: auto; min-height: auto; }";
    constexpr char kAutoGapStyles[] = "panel { gap: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypedLengthStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_FALSE(style.width.isAuto());
    EXPECT_EQ(style.width.pixels(), 40.f);
    ASSERT_TRUE(style.minWidth.has_value());
    EXPECT_EQ(style.minWidth->pixels, 20.f);
    ASSERT_TRUE(style.minHeight.has_value());
    EXPECT_EQ(style.minHeight->pixels, 10.f);
    ASSERT_TRUE(style.left.has_value());
    EXPECT_EQ(style.left->pixels, -8.f);
    EXPECT_EQ(style.lineHeight.kind, LineHeight::Kind::Length);
    EXPECT_EQ(style.lineHeight.value, 18.f);

    StyleSheet unitlessStylesheet;
    ASSERT_TRUE(unitlessStylesheet.loadRadia(kUnitlessLineHeightStyles).ok());
    const ComputedStyle unitless = unitlessStylesheet.resolve("panel", "", {}, 0);
    EXPECT_EQ(unitless.lineHeight.kind, LineHeight::Kind::Number);
    EXPECT_EQ(unitless.lineHeight.value, 1.5f);
    EXPECT_FALSE(unitless.minWidth.has_value());
    EXPECT_FALSE(unitless.minHeight.has_value());

    ASSERT_TRUE(stylesheet.loadRadia(kAutoDimensionStyles).ok());
    const ComputedStyle automatic = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_TRUE(automatic.width.isAuto());
    EXPECT_TRUE(automatic.height.isAuto());
    const ComputedStyle automaticSize = stylesheet.resolve("button", "", {}, 0);
    EXPECT_TRUE(automaticSize.width.isAuto());
    EXPECT_TRUE(automaticSize.height.isAuto());
    const ComputedStyle mixedSize = stylesheet.resolve("i", "", {}, 0);
    EXPECT_TRUE(mixedSize.height.isAuto());
    EXPECT_EQ(mixedSize.width.pixels(), 16.f);

    const auto invalidGap = stylesheet.loadRadia(kAutoGapStyles);
    ASSERT_TRUE(invalidGap.ok());
    ASSERT_FALSE(invalidGap.warnings.empty());
    EXPECT_EQ(invalidGap.warnings.front().code, "stylesheet.property.value_invalid");

    const auto percentageGap = stylesheet.loadRadia("panel { gap: 10%; row-gap: 20%; column-gap: 30%; } panel.zero { gap: 0%; }");
    ASSERT_TRUE(percentageGap.ok());
    ASSERT_EQ(percentageGap.warnings.size(), std::size_t(4));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).rowGap.fixedPixels(), 0.f);
}

TEST(StyleSheetTest, MatchesStructuralSelectors) {
    constexpr char kStructuralStyles[] = "* { opacity: .8; } panel.root > label { width: 10px; } "
                                         "panel.root label { height: 11px; } panel.root { "
                                         "> label.direct { min-width: 20%; } "
                                         "& > label.direct { right: 5%; } label.nested { min-height: 25%; } "
                                         "& label.nested { bottom: 10%; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStructuralStyles).ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.classList().add("root");
    auto direct = makeElement<HTMLLabelElement>("direct");
    direct->classList().add("direct");
    HTMLLabelElement* directLabel = direct.get();
    root.append(std::move(direct));

    auto container = makeElement<HTMLPanelElement>();
    auto nested = makeElement<HTMLLabelElement>("nested");
    nested->classList().add("nested");
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

TEST(StyleSheetTest, ParsesFilterOperations) {
    constexpr char kBoxEffectStyles[] = "panel { background-color: linear-gradient(to/**/right, #ff0000ff, "
                                        "rgb(0, 255, 0, 50%) 75%, #0000ffff); "
                                        "box-shadow: 1px 2px #11223344, 3px 4px 5px 6px "
                                        "rgb(10, 20, 30, 40%) inset; outline-offset: 3px; "
                                        "outline: light-dark(#abcdef88, #12345688) solid 2px; } panel.light { color-scheme: light; } "
                                        "label { outline: 1px dashed #ffffffff; }";
    constexpr char kFilterStyles[] = "panel { backdrop-filter: linear-blur(to bottom, 0px 25%, 16px 75%); "
                                     "filter: linear-blur(to right, 2px 25%, 6px 75%) blur(4px); } "
                                     "button { filter: linear-blur(8px); } label { --blur-radius: 4px; "
                                     "filter: blur(var(--blur-radius)); backdrop-filter: none; }";
    constexpr char kGradientStyles[] = "panel { background-color: radial-gradient(circle at 25% 75%, "
                                       "#ffffffff, #00000000 80%); border-width: 3px; border-color: "
                                       "conic-gradient(from 45deg at top left, #ff0000ff 0deg 90deg, "
                                       "#0000ffff 100%); } button { border: 2px solid "
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

    ASSERT_TRUE(stylesheet.loadRadia(kFilterStyles).ok());
    const ComputedStyle filters = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_EQ(filters.backdropFilter.size(), std::size_t(1));
    const LinearBlurFilter* backdrop = std::get_if<LinearBlurFilter>(&filters.backdropFilter.front());
    ASSERT_NE(backdrop, nullptr);
    ASSERT_EQ(backdrop->stops.size(), std::size_t(2));
    EXPECT_EQ(backdrop->stops[0].stdDeviation, 0.f);
    EXPECT_EQ(backdrop->stops[1].stdDeviation, 16.f);
    EXPECT_EQ(backdrop->stops[0].position, .25f);
    EXPECT_EQ(backdrop->stops[1].position, .75f);
    EXPECT_EQ(backdrop->angleDegrees, 180.f);
    ASSERT_EQ(filters.filter.size(), std::size_t(2));
    const LinearBlurFilter* progressive = std::get_if<LinearBlurFilter>(&filters.filter[0]);
    ASSERT_NE(progressive, nullptr);
    ASSERT_EQ(progressive->stops.size(), std::size_t(2));
    EXPECT_EQ(progressive->stops[0].stdDeviation, 2.f);
    EXPECT_EQ(progressive->stops[1].stdDeviation, 6.f);
    EXPECT_EQ(progressive->stops[0].position, .25f);
    EXPECT_EQ(progressive->stops[1].position, .75f);
    EXPECT_EQ(progressive->angleDegrees, 90.f);
    const BlurFilter* uniform = std::get_if<BlurFilter>(&filters.filter[1]);
    ASSERT_NE(uniform, nullptr);
    EXPECT_EQ(uniform->stdDeviation, 4.f);
    const ComputedStyle button = stylesheet.resolve("button", "", {}, 0);
    const LinearBlurFilter* uniformLinear = std::get_if<LinearBlurFilter>(&button.filter.front());
    ASSERT_NE(uniformLinear, nullptr);
    ASSERT_EQ(uniformLinear->stops.size(), std::size_t(2));
    EXPECT_EQ(uniformLinear->stops[0].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[1].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[0].position, 0.f);
    EXPECT_EQ(uniformLinear->stops[1].position, 1.f);
    const ComputedStyle label = stylesheet.resolve("label", "", {}, 0);
    ASSERT_EQ(label.filter.size(), std::size_t(1));
    const BlurFilter* nested = std::get_if<BlurFilter>(&label.filter.front());
    ASSERT_NE(nested, nullptr);
    EXPECT_EQ(nested->stdDeviation, 4.f);
    EXPECT_TRUE(label.backdropFilter.empty());

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

TEST(StyleSheetTest, ParsesBackgroundMaskAndCursorLayers) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("i { background-image: url(icons/search.svg), linear-gradient(#fff, #000); "
                               "background-position: 10% 20%, right bottom; background-size: cover, 12px 14px; "
                               "background-repeat: no-repeat, repeat-x; background-origin: border-box, content-box; "
                               "background-clip: padding-box, border-box; background-attachment: fixed, local; "
                               "mask-image: url(icons/mask.svg); mask-mode: alpha; mask-position: right bottom; mask-size: contain; "
                               "mask-repeat: no-repeat; mask-origin: border-box; mask-clip: content-box; mask-composite: exclude; "
                               "mask-type: alpha; cursor: url(cursors/pointer.cur) 25 50, pointer; }")
                    .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.backgroundLayers.size(), 2U);
    EXPECT_EQ(style.backgroundLayers[0].resource, "icons/search.svg");
    ASSERT_TRUE(style.backgroundLayers[1].gradient.has_value());
    EXPECT_NEAR(style.backgroundLayers[0].position.y.percent, .8f, 1.0e-4f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeMode::Cover);
    EXPECT_EQ(style.backgroundLayers[1].size.mode, BackgroundSizeMode::Explicit);
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::NoRepeat);
    EXPECT_EQ(style.backgroundLayers[1].repeat, BackgroundRepeat::RepeatX);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Fixed);
    EXPECT_EQ(style.backgroundLayers[1].attachment, BackgroundAttachment::Local);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    EXPECT_EQ(style.maskLayers[0].image.resource, "icons/mask.svg");
    EXPECT_EQ(style.maskLayers[0].mode, MaskMode::Alpha);
    EXPECT_EQ(style.maskLayers[0].composite, MaskComposite::Exclude);
    EXPECT_EQ(style.maskLayers[0].type, MaskType::Alpha);

    EXPECT_EQ(style.cursor, CursorStyle::Pointer);
    ASSERT_EQ(style.cursorImages.size(), 1U);
    EXPECT_EQ(style.cursorImages[0].resource, "cursors/pointer.cur");
    ASSERT_TRUE(style.cursorImages[0].hotspotX.has_value());
    ASSERT_TRUE(style.cursorImages[0].hotspotY.has_value());
    EXPECT_FLOAT_EQ(*style.cursorImages[0].hotspotX, 25.f);
    EXPECT_FLOAT_EQ(*style.cursorImages[0].hotspotY, 50.f);

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 3U);
    EXPECT_EQ(references[0].value, "icons/search.svg");
    EXPECT_FALSE(references[0].optional);
    EXPECT_EQ(references[1].value, "icons/mask.svg");
    EXPECT_TRUE(references[1].optional);
    EXPECT_EQ(references[2].value, "cursors/pointer.cur");
    EXPECT_FALSE(references[2].optional);
    EXPECT_TRUE(references[2].cursor);
}

TEST(StyleSheetTest, PublishesDeferredResourceReferences) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia(":root { --background-image: url(icons/variable.svg); --mask-image: url(icons/variable-mask.svg); "
                               "--pointer: url(cursors/variable.cur), pointer; } "
                               "i { background-image: var(--background-image); mask-image: var(--mask-image); cursor: var(--pointer); }")
                    .ok());

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 3U);
    EXPECT_EQ(references[0].value, "icons/variable.svg");
    EXPECT_FALSE(references[0].optional);
    EXPECT_EQ(references[1].value, "icons/variable-mask.svg");
    EXPECT_TRUE(references[1].optional);
    EXPECT_EQ(references[2].value, "cursors/variable.cur");
    EXPECT_FALSE(references[2].optional);
    EXPECT_TRUE(references[2].cursor);
}

TEST(StyleSheetTest, ParsesMaskShorthand) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { mask: url(icons/mask.svg) alpha; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.maskLayers.size(), 1U);
    EXPECT_EQ(style.maskLayers.front().image.resource, "icons/mask.svg");
    EXPECT_EQ(style.maskLayers.front().mode, MaskMode::Alpha);
    EXPECT_EQ(style.maskLayers.front().composite, MaskComposite::Add);
    EXPECT_EQ(style.maskLayers.front().type, MaskType::Alpha);
}

TEST(StyleSheetTest, DecodesEscapedImageURLsAndFunctionNames) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(R"(i[data="a\\"] {} i { background-image: url(icons/a\20 b.svg); })").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_EQ(style.backgroundLayers.front().resource, "icons/a b.svg");

    StyleSheet escapedFunction;
    ASSERT_TRUE(escapedFunction.loadRadia(R"(i { background-image: u\72l(icons/search.svg); })").ok());
    ASSERT_EQ(escapedFunction.resolve("i", "", {}, 0).backgroundLayers.size(), 1U);
    EXPECT_EQ(escapedFunction.resolve("i", "", {}, 0).backgroundLayers.front().resource, "icons/search.svg");
}

TEST(StyleSheetTest, DecodesEscapedValueFunctionsAndUnits) {
    constexpr char kStyles[] = R"(:root { --accent: #204060; } panel {
        background-color: v\61 r(--accent);
        border-color: linear-g\72 adient(red, blue);
        filter: bl\75 r(4px);
        width: 17p\78;
    })";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    EXPECT_NEAR(style.backgroundColor.r, 32.f / 255.f, 1.0e-4f);
    ASSERT_TRUE(style.borderGradient.has_value());
    EXPECT_EQ(style.borderGradient->kind, GradientKind::Linear);
    ASSERT_EQ(style.filter.size(), std::size_t(1));
    const BlurFilter* filter = std::get_if<BlurFilter>(&style.filter.front());
    ASSERT_NE(filter, nullptr);
    EXPECT_EQ(filter->stdDeviation, 4.f);
    EXPECT_EQ(style.width.pixels(), 17.f);
}

TEST(StyleSheetTest, SkipsEmptyImageURL) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(R"(i { background-image: url(""); })", "empty-url.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_TRUE(stylesheet.resourceReferences().empty());
}

TEST(StyleSheetTest, ParsesCSSCursorHotspotNumbers) {
    StyleSheet percentage;
    const auto percentageResult = percentage.loadRadia("i { cursor: url(cursors/default.png) 25% 50%, pointer; }");
    ASSERT_TRUE(percentageResult.ok());
    ASSERT_FALSE(percentageResult.warnings.empty());

    StyleSheet pixels;
    const auto pixelsResult = pixels.loadRadia("i { cursor: url(cursors/default.png) 25px 50px, pointer; }");
    ASSERT_TRUE(pixelsResult.ok());
    ASSERT_FALSE(pixelsResult.warnings.empty());

    StyleSheet outOfBounds;
    ASSERT_TRUE(outOfBounds.loadRadia("i { cursor: url(cursors/default.png) -1 1e30, pointer; }").ok());
    const ComputedStyle style = outOfBounds.resolve("i", "", {}, 0);
    ASSERT_EQ(style.cursorImages.size(), 1U);
    ASSERT_TRUE(style.cursorImages.front().hotspotX.has_value());
    ASSERT_TRUE(style.cursorImages.front().hotspotY.has_value());
    EXPECT_FLOAT_EQ(*style.cursorImages.front().hotspotX, -1.f);
    EXPECT_FLOAT_EQ(*style.cursorImages.front().hotspotY, 1e30f);

    StyleSheet incomplete;
    const auto incompleteResult = incomplete.loadRadia("i { cursor: url(cursors/default.png) 25, pointer; }");
    ASSERT_TRUE(incompleteResult.ok());
    ASSERT_FALSE(incompleteResult.warnings.empty());
}

TEST(StyleSheetTest, ParsesRasterBackgroundImages) {
    StyleSheet longhand;
    ASSERT_TRUE(longhand.loadRadia("i { background-image: url(images/pattern.png); }").ok());
    const ComputedStyle longhandStyle = longhand.resolve("i", "", {}, 0);
    ASSERT_EQ(longhandStyle.backgroundLayers.size(), 1U);
    EXPECT_EQ(longhandStyle.backgroundLayers.front().resource, "images/pattern.png");

    StyleSheet shorthand;
    ASSERT_TRUE(shorthand.loadRadia("i { background: url(images/pattern.png); }").ok());
    const ComputedStyle shorthandStyle = shorthand.resolve("i", "", {}, 0);
    ASSERT_EQ(shorthandStyle.backgroundLayers.size(), 1U);
    EXPECT_EQ(shorthandStyle.backgroundLayers.front().resource, "images/pattern.png");
}

TEST(StyleSheetTest, ParsesBackgroundShorthandAfterSlash) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icons/search.svg) center / contain no-repeat; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_EQ(style.backgroundLayers.front().resource, "icons/search.svg");
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, .5f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, .5f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeMode::Contain);
    EXPECT_EQ(style.backgroundLayers.front().repeat, BackgroundRepeat::NoRepeat);
}

TEST(StyleSheetTest, UsesCSSImageInitialValues) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icon.svg); mask-image: url(mask.svg); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.backgroundColor.r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.a, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeMode::Auto);
    EXPECT_FALSE(style.backgroundLayers[0].size.width.has_value());
    EXPECT_FALSE(style.backgroundLayers[0].size.height.has_value());
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Scroll);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.y.percent, 1.f);
    EXPECT_EQ(style.maskLayers[0].image.size.mode, BackgroundSizeMode::Auto);
    EXPECT_FALSE(style.maskLayers[0].image.size.width.has_value());
    EXPECT_FALSE(style.maskLayers[0].image.size.height.has_value());
    EXPECT_EQ(style.maskLayers[0].image.repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.maskLayers[0].image.origin, BackgroundBox::BorderBox);
    EXPECT_EQ(style.maskLayers[0].image.clip, BackgroundBox::BorderBox);
    EXPECT_EQ(style.maskLayers[0].type, MaskType::Alpha);
}

TEST(StyleSheetTest, BackgroundShorthandResetsAllComponents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("i { background-color: #ff0000; background-position: right bottom; background-size: cover; "
                               "background-repeat: no-repeat; background-origin: content-box; background-clip: padding-box; "
                               "background-attachment: fixed; background: transparent; }")
                    .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    EXPECT_FLOAT_EQ(style.backgroundColor.r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor.a, 0.f);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_TRUE(style.backgroundLayers.front().resource.empty());
    EXPECT_FALSE(style.backgroundLayers.front().gradient.has_value());
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeMode::Auto);
    EXPECT_FALSE(style.backgroundLayers.front().size.width.has_value());
    EXPECT_FALSE(style.backgroundLayers.front().size.height.has_value());
    EXPECT_EQ(style.backgroundLayers.front().repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.backgroundLayers.front().attachment, BackgroundAttachment::Scroll);
    EXPECT_EQ(style.backgroundLayers.front().origin, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers.front().clip, BackgroundBox::BorderBox);
}

TEST(StyleSheetTest, SkipsDuplicateImageLayerComponents) {
    const char* invalid[] = {
        "i { background: url(a.svg) repeat repeat; }",
        "i { background: url(a.svg) fixed scroll; }",
        "i { mask: url(a.svg) alpha alpha; }",
        "i { mask: url(a.svg) add add; }",
    };
    for (const char* styles : invalid) {
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(styles);
        EXPECT_TRUE(result.ok()) << styles;
        EXPECT_FALSE(result.warnings.empty()) << styles;
    }

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(a.svg) repeat no-repeat; }").ok());
    EXPECT_EQ(stylesheet.resolve("i", "", {}, 0).backgroundLayers.front().repeat, BackgroundRepeat::RepeatX);
}

TEST(StyleSheetTest, NormalizesImageLonghandsRegardlessOfDeclarationOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("i { background-position: right, left; background-attachment: fixed, local; "
                               "background-image: url(one.svg), url(two.svg); mask-position: right, left; "
                               "mask-image: url(one.svg), url(two.svg); }")
                    .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {}, 0);
    ASSERT_EQ(style.backgroundLayers.size(), 2U);
    EXPECT_EQ(style.backgroundLayers[0].position.x.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers[1].position.x.percent, 0.f);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Fixed);
    EXPECT_EQ(style.backgroundLayers[1].attachment, BackgroundAttachment::Local);
    ASSERT_EQ(style.maskLayers.size(), 2U);
    EXPECT_EQ(style.maskLayers[0].image.position.x.percent, 1.f);
    EXPECT_EQ(style.maskLayers[1].image.position.x.percent, 0.f);
}

TEST(StyleSheetTest, DoesNotApplyInheritedOpacityToMaskCoverage) {
    ComputedStyle style;
    style.maskLayers = {MaskLayer{}};
    style.maskLayers[0].image.gradient = Gradient{};
    style.maskLayers[0].image.gradient->stops = {{Color(1.f, 1.f, 1.f, .8f), 0.f}, {Color(1.f, 1.f, 1.f, .4f), 1.f}};

    radia::ui::applyOpacity(style, .5f);

    EXPECT_FLOAT_EQ(style.opacity, .5f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.gradient->stops[0].color.a, .8f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.gradient->stops[1].color.a, .4f);
}

TEST(StyleSheetTest, SkipsInvalidFilters) {
    const char* invalidSources[] = {
        "panel { background-color: linear-gradient(#fff); }",
        "panel { background-color: radial-gradient(square, #fff, #000); }",
        "panel { border-color: conic-gradient(from nowhere, #fff, #000); }",
        "panel { border: 1px repeating-linear-gradient(#fff 20%, #000 20%); }",
        "panel { box-shadow: 0 0 -1px #000; }",
        "panel { background-color: ButtonFace; }",
        "panel { border-color: ButtonBorder; }",
        "panel { border: 1px ButtonBorder; }",
        "panel { color: ButtonText; }",
        "panel { outline: 10% #000; }",
        "panel { outline: 0% #000; }",
        "panel { outline: 1px 2px #000; }",
        "panel { outline-offset: 0%; }",
        "panel { outline: 1px dashed solid #000; }",
        "panel { outline: #000 auto 1px; }",
        "panel { outline: -focus-ring-color auto 1px; }",
        "panel { filter: blur(4); }",
        "panel { filter: blur(-1px); }",
        "panel { filter: blur(1%); }",
        "panel { filter: blur(0%); }",
        "panel { filter: blur(); }",
        "panel { filter: blur(1px, 2px); }",
        "panel { box-shadow: 0% 0% #000; }",
        "panel { filter: brightness(1); }",
        "panel { filter: blur(1px) none; }",
        "panel { filter: linear-blur(to nowhere, 0px 0%, 4px 100%); }",
        "panel { filter: linear-blur(0px 75%, 4px 25%); }",
        "panel { filter: linear-blur(0px 0%, 4px 50%, 8px 100%); }",
    };
    constexpr char kLargeBlurStyles[] = "panel { filter: blur(64px); }";

    for (const char* source : invalidSources) {
        SCOPED_TRACE(Message() << "invalid filter: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "effects.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
    }

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLargeBlurStyles, "large-filter.css").ok());
}

TEST(StyleSheetTest, SkipsInvalidMinSize) {
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
        StyleSheet invalidStylesheet;
        const auto result = invalidStylesheet.loadRadia(test.styles, "min-size.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }

    const ComputedStyle preservedStyle = stylesheet.resolve("panel", "", {"one"}, 0);
    ASSERT_TRUE(preservedStyle.minWidth.has_value());
    EXPECT_EQ(preservedStyle.minWidth->pixels, 24.f);
}

TEST(StyleSheetTest, CopiesStylesheetState) {
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
    ASSERT_TRUE(malformed.ok());
    ASSERT_FALSE(malformed.warnings.empty());
    EXPECT_EQ(malformed.warnings.front().source, "derived/skin.css");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 10.f);
}

TEST(StyleSheetTest, OverridesDefaultRule) {
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Skin, {"skin.css", "panel { width: 20px; }"}},
        {StyleOrigin::Default, {"defaults.css", "panel.primary { width: 10px; }"}},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"primary"}, 0).width.pixels(), 20.f);
}

TEST(StyleSheetTest, RestrictsInternalAlignment) {
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

TEST(StyleSheetTest, ResolvesRecursiveImports) {
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

TEST(StyleSheetTest, AcceptsStringAndURLImportTargets) {
    constexpr char kEntrypointStyles[] = R"(@impor\74 "components/a\2e css";
@import url(components/b.css);
@import url("components/c.css");
panel { width: 30px; })";

    StyleSheet stylesheet;
    ResourceLayer layer{"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"components/a.css", "panel { height: 10px; }"},
        {"components/b.css", "panel { min-width: 11px; }"},
        {"components/c.css", "panel { max-width: 12px; }"},
    };

    const auto result = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}});
    ASSERT_TRUE(result.ok());
    const auto& dependencies = stylesheet.dependencies();
    ASSERT_TRUE(dependencies.contains("theme/main.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/a.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/b.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/c.css"));
}

TEST(StyleSheetTest, SkipsUnsupportedImportConditionsWithoutLoading) {
    constexpr char kStyles[] = "@import url(does-not-exist.css) screen; panel { width: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, {"theme/main.css", kStyles}}});
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsLaterImportsAfterUnsupportedConditions) {
    constexpr char kStyles[] = "@import \"skipped.css\" screen; @import \"valid.css\"; panel { width: 13px; }";

    ResourceLayer layer{"theme/main.css", kStyles};
    layer.entrypoint = "main.css";
    layer.modules = {{"valid.css", "panel { height: 17px; }"}};

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}});

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_TRUE(stylesheet.dependencies().at("theme/main.css").contains("theme/valid.css"));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).height.pixels(), 17.f);
}

TEST(StyleSheetTest, DoesNotTreatCDOAsImportWhitespace) {
    constexpr char kStyles[] = "@import \"missing.css\" <!--; panel { width: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "import.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 13.f);
}

TEST(StyleSheetTest, RecoversMalformedImportAtRuleAtItsBoundary) {
    constexpr char kStyles[] = "@import \"missing.css\" panel { width: 99px; } label { height: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "theme/main.css");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.syntax");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}, 0).width.isAuto());
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsLaterImportsAfterMalformedImportAtRules) {
    constexpr char kStyles[] = "@import \"broken.css\" panel { width: 99px; } @import ???; @import \"\"; "
                               "@import \"valid.css\"; panel { width: 13px; }";

    ResourceLayer layer{"theme/main.css", kStyles};
    layer.entrypoint = "main.css";
    layer.modules = {{"valid.css", "panel { height: 17px; }"}};

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}});

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(3));
    for (const auto& warning : result.warnings) EXPECT_EQ(warning.code, "stylesheet.import.syntax");
    EXPECT_TRUE(stylesheet.dependencies().at("theme/main.css").contains("theme/valid.css"));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 13.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).height.pixels(), 17.f);
}

TEST(StyleSheetTest, PreservesStylesheetOnImportFailure) {
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 44.f);

    auto cycle = layer(kCycleImport);
    cycle.modules["cycle.css"] = kCycleModule;
    const auto cycleResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, cycle}});
    ASSERT_FALSE(cycleResult.ok());
    ASSERT_FALSE(cycleResult.errors.empty());
    EXPECT_EQ(cycleResult.errors.front().code, "stylesheet.import.cycle");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 44.f);

    const auto traversalResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer(kTraversalImport)}});
    ASSERT_FALSE(traversalResult.ok());
    ASSERT_FALSE(traversalResult.errors.empty());
    EXPECT_EQ(traversalResult.errors.front().code, "stylesheet.import.path_invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 44.f);

    auto malformed = layer(kMalformedImport);
    malformed.modules["broken.css"] = kMalformedModule;
    const auto malformedResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, malformed}});
    ASSERT_TRUE(malformedResult.ok());
    ASSERT_FALSE(malformedResult.warnings.empty());
    EXPECT_EQ(malformedResult.warnings.front().source, "theme/broken.css");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}, 0).width.isAuto());

    auto late = layer(kLateImport);
    late.modules["late.css"] = kLateModule;
    const auto lateResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, late}});
    ASSERT_TRUE(lateResult.ok());
    ASSERT_FALSE(lateResult.warnings.empty());
    EXPECT_EQ(lateResult.warnings.front().code, "stylesheet.import.order");

    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 1.f);
}

TEST(StyleSheetTest, NormalizesSelectorNames) {
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
    ASSERT_TRUE(invalidId.ok());
    ASSERT_FALSE(invalidId.warnings.empty());
    EXPECT_EQ(invalidId.warnings.front().code, "stylesheet.selector.id_invalid");

    const auto invalidPseudoElement = stylesheet.loadRadia(kInvalidPseudoElementSelector);
    ASSERT_TRUE(invalidPseudoElement.ok());
    ASSERT_FALSE(invalidPseudoElement.warnings.empty());
    EXPECT_EQ(invalidPseudoElement.warnings.front().code, "stylesheet.selector.pseudo_element_invalid");
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
    EXPECT_EQ(chord.rowGap.fixedPixels(), 5.f);
    EXPECT_EQ(key.padding.left, 2.f);
    EXPECT_EQ(key.borderRadius.topLeft.horizontal.pixels, 3.f);
    EXPECT_EQ(key.borderRadius.topLeft.vertical.pixels, 3.f);
    EXPECT_NE(key.rowGap.fixedPixels(), 5.f);

    const auto rejectedPseudoElement = stylesheet.loadRadia(kRejectedKbdPseudoElement);
    ASSERT_TRUE(rejectedPseudoElement.ok());
    ASSERT_FALSE(rejectedPseudoElement.warnings.empty());
    EXPECT_EQ(rejectedPseudoElement.warnings.front().code, "stylesheet.selector.pseudo_element_unknown");
}

TEST(StyleSheetTest, PreservesNestedRuleOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button { color: #ff0000ff; & { color: #00ff00ff; } color: #0000ffff; }").ok());

    const ComputedStyle style = stylesheet.resolve("button", "", {}, 0);
    EXPECT_FLOAT_EQ(style.color.r, 0.f);
    EXPECT_FLOAT_EQ(style.color.g, 0.f);
    EXPECT_FLOAT_EQ(style.color.b, 1.f);
}

TEST(StyleSheetTest, PreservesRulesAcrossCSSTokenBoundaries) {
    const std::string source =
        R"(panel { width: 11px; content: "}"; background-image: url(icon\)name.svg); background-color: rgb(1, 2, 3); } /* } ; */ label { height: 13px; })";
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(source, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.errors.empty());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 11.f);
    ASSERT_TRUE(stylesheet.resolve("panel", "", {}, 0).content.has_value());
    EXPECT_EQ(*stylesheet.resolve("panel", "", {}, 0).content, "}");
    ASSERT_EQ(stylesheet.resolve("panel", "", {}, 0).backgroundLayers.size(), std::size_t(1));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).backgroundLayers.front().resource, "icon)name.svg");
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, DiscardsClosedCommentsWithoutChangingSelectors) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button/**/.primary { width: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {"primary"}, 0).width.pixels(), 13.f);
}

TEST(StyleSheetTest, ReportsUnclosedComments) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { width: 13px; } /* unclosed", "comment.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_comment");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsValidDeclarationsAroundInvalidDeclaration) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel {\n  width: 11px;\n  malformed;\n  height: 13px;\n}", "declarations.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(result.warnings.front().line, std::size_t(3));
    EXPECT_EQ(result.warnings.front().column, std::size_t(3));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsQualifiedRulePreludeTogether) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("bogus; label { height: 13px; }", "qualified.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.element_unknown");
    EXPECT_TRUE(stylesheet.resolve("label", "", {}, 0).height.isAuto());
}

TEST(StyleSheetTest, RejectsDanglingChildCombinators) {
    struct InvalidSelector {
        const char* source;
        const char* target;
    };
    const InvalidSelector cases[] = {
        {"> label { height: 13px; }", "label"},
        {"label > { height: 13px; }", "label"},
        {"label > > input { height: 13px; }", "input"},
        {"button { & > { height: 99px; } }", "button"},
    };

    for (const InvalidSelector& test : cases) {
        SCOPED_TRACE(test.source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.source, "combinator.css");

        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result.warnings.size(), std::size_t(1));
        EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.empty");
        EXPECT_TRUE(stylesheet.resolve(test.target, "", {}, 0).height.isAuto());
    }
}

TEST(StyleSheetTest, ReportsMalformedRootDeclarationOnce) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":root { malformed; color: #ffffff; }", "root.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
}

TEST(StyleSheetTest, KeepsNeighboringRulesAroundInvalidCSS) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { width: 11px; } @media screen { panel { width: 99px; } } "
                                             "button::unknown { width: 99px; } label { height: 13px; }",
                                             "rules.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings[0].code, "stylesheet.at_rule.unsupported");
    EXPECT_EQ(result.warnings[1].code, "stylesheet.selector.pseudo_element_unknown");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).width.pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsEarlierRangeValuesBeforeAnUnclosedRule) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { min-size: 11px 13px; }\nlabel { height: 13px;", "unclosed.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_block");
    const ComputedStyle style = stylesheet.resolve("panel", "", {}, 0);
    ASSERT_TRUE(style.minHeight.has_value());
    ASSERT_TRUE(style.minWidth.has_value());
    EXPECT_EQ(style.minHeight->pixels, 11.f);
    EXPECT_EQ(style.minWidth->pixels, 13.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, SkipsAnInvalidSelectorListAsAWhole) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel, button::unknown { width: 99px; } panel { height: 13px; }");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_unknown");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}, 0).width.isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}, 0).height.pixels(), 13.f);
}

TEST(StyleSheetTest, ReportsEachSharedImportWarning) {
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
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings[0].source, "theme/shared.css");
    EXPECT_EQ(result.warnings[1].source, "theme/shared.css");
    EXPECT_NE(result.warnings[0].message.find("Import chain:"), std::string::npos);
    EXPECT_NE(result.warnings[1].message.find("Import chain:"), std::string::npos);
}

TEST(StyleSheetTest, MarksBorderStateLayoutAffecting) {
    constexpr char kStateBorderStyles[] = "fieldset { border: 1px solid #ffffff; } "
                                          "fieldset:hover { border: 4px solid #ffffff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStateBorderStyles).ok());
    EXPECT_TRUE(stylesheet.stateAffectsLayout(ElementState::Hovered));
}

TEST(StyleSheetTest, MarksAppearanceStateLayout) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:hover { appearance: none; }").ok());
    EXPECT_TRUE(stylesheet.stateAffectsLayout(ElementState::Hovered));
}

TEST(StyleSheetTest, MatchesEffectiveFieldsetDisabledState) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("fieldset:disabled { opacity: .25; } button:disabled { opacity: .5; }").ok());

    auto fieldset = makeElement<Element>("fieldset");
    auto legend = makeElement<Element>("legend");
    auto legendButton = makeElement<HTMLButtonElement>();
    auto normalButton = makeElement<HTMLButtonElement>();
    HTMLButtonElement* legendButtonPtr = legendButton.get();
    HTMLButtonElement* normalButtonPtr = normalButton.get();
    legend->append(std::move(legendButton));
    fieldset->append(std::move(legend));
    fieldset->append(std::move(normalButton));

    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*fieldset).opacity, 1.f);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity, 1.f);

    fieldset->disabled(true);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*fieldset).opacity, .25f);
    EXPECT_TRUE(normalButtonPtr->disabled());
    EXPECT_FALSE(legendButtonPtr->disabled());
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity, .5f);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*legendButtonPtr).opacity, 1.f);

    fieldset->disabled(false);
    EXPECT_FALSE(normalButtonPtr->disabled());
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity, 1.f);
}
