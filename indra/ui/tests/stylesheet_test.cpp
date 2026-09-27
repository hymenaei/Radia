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
#include "ComputedStyleProperties.h"
#include "css/rules.h"
#include "css/stylesheet.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
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
using radia::ui::BackgroundSizeType;
using radia::ui::BlurFilter;
using radia::ui::Color;
using radia::ui::ColorSchemeMode;
using radia::ui::ComputedStyle;
using radia::ui::CSSPseudoClass;
using radia::ui::CursorStyle;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::FixedTextMetrics;
using radia::ui::FontFamilies;
using radia::ui::FontFamily;
using radia::ui::GenericFontFamily;
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
using radia::ui::StyleImage;
using radia::ui::StyleLayer;
using radia::ui::StyleOrigin;
using radia::ui::StylePass;
using radia::ui::StyleRule;
using radia::ui::StyleSheet;
using radia::ui::TextAlign;
using radia::ui::TextDecoration;
using radia::ui::TextWrapMode;
using radia::ui::TextWrapStyle;
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
    const ComputedStyle button = stylesheet.resolve("button", "", {});
    EXPECT_NEAR(button.backgroundColor().resolvedColor().g, 1.f, 1.0e-4f);
    auto control = makeElementValue<HTMLButtonElement>();
    auto labelElement = makeElement<Element>("span");
    Element* label = labelElement.get();
    labelElement->textContent("Inherited");
    control.append(std::move(labelElement));
    EXPECT_NEAR(computedStyle(stylesheet, *label).color().resolvedColor().a, .5f, 1.0e-4f);
    EXPECT_EQ(computedStyle(stylesheet, *label).fontSize(), 17.f);
    auto standalone = makeElementValue<HTMLLabelElement>("Standalone");
    EXPECT_EQ(computedStyle(stylesheet, standalone).fontSize(), 29.f);
}

TEST(StyleSheetTest, FontSizes) {
    StyleSheet stylesheet;
    const auto loaded = stylesheet.loadRadia("panel { font-size: 2rem; } label { font-size: 1rem; } span { font-size: 2em; }");
    ASSERT_TRUE(loaded.ok());
    ASSERT_TRUE(loaded.warnings.empty()) << loaded.warnings.front().formatted();

    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    auto text = makeElement<Element>("span");
    Element* textPointer = text.get();
    label->append(std::move(text));
    panel.append(std::move(label));

    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_EQ(styles.style(panel).fontSize(), 26.f);
    EXPECT_EQ(styles.style(*labelPointer).fontSize(), 26.f);
    EXPECT_EQ(styles.style(*textPointer).fontSize(), 52.f);
}

TEST(StyleSheetTest, ResolvesMatchParentTextAlignment) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { text-align: right; } panel label { text-align: match-parent; }").ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    panel.append(std::move(label));

    EXPECT_EQ(computedStyle(stylesheet, panel).textAlign(), TextAlign::Right);
    EXPECT_EQ(computedStyle(stylesheet, *labelPointer).textAlign(), TextAlign::Right);
}

TEST(StyleSheetTest, RootSelector) {
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
    EXPECT_EQ(rootStyle.usedColorScheme, ColorSchemeMode::Light);
    EXPECT_EQ(rootStyle.width().pixels(), 30.f);
    ASSERT_TRUE(rootStyle.minWidth().has_value());
    EXPECT_EQ(rootStyle.minWidth()->pixels(), 18.f);

    const ComputedStyle labelStyle = computedStyle(stylesheet, *label);
    EXPECT_EQ(labelStyle.width().pixels(), 17.f);
    EXPECT_EQ(labelStyle.usedColorScheme, ColorSchemeMode::Light);
    EXPECT_NEAR(labelStyle.color().resolvedColor().r, 32.f / 255.f, 1.0e-4f);
    EXPECT_FALSE(labelStyle.minWidth().has_value());

    auto standaloneRoot = makeElementValue<HTMLPanelElement>();
    EXPECT_EQ(computedStyle(stylesheet, standaloneRoot).usedColorScheme, ColorSchemeMode::Light);
}

TEST(StyleSheetTest, RootOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(".foo { --size: 11px; } :root { --size: 22px; } .foo { width: var(--size); }").ok());

    auto rootOwner = makeElement<HTMLPanelElement>();
    rootOwner->classList().add("foo");
    Document document(std::move(rootOwner));

    const ComputedStyle style = computedStyle(stylesheet, *document.documentElement());
    EXPECT_EQ(style.customProperties.at("--size").source, "22px");
    EXPECT_EQ(style.width().pixels(), 22.f);
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
    EXPECT_EQ(resourceStyle.width().pixels(), 24.f);
    EXPECT_EQ(resourceStyle.opacity().value, 1.f);

    const ComputedStyle labelStyle = computedStyle(stylesheet, *labelPointer);
    EXPECT_EQ(labelStyle.width().pixels(), 13.f);
}

TEST(StyleSheetTest, StartsWithoutImplicitCoreRules) {
    StyleSheet stylesheet;
    const ComputedStyle paragraph = stylesheet.resolve("p", "", {});

    EXPECT_FALSE(paragraph.displaySet);
    EXPECT_EQ(paragraph.fontWeight().value, 400.f);
}

TEST(StyleSheetTest, KeepsValidDeclarationsAroundInvalidColor) {
    constexpr char kInvalidColorStyles[] = "input { background-color: ##invalid; width: 8px; }";

    StyleSheet stylesheet;
    const auto invalid = stylesheet.loadRadia(kInvalidColorStyles, "invalid.css");

    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(invalid.warnings.front().source, "invalid.css");
    EXPECT_EQ(stylesheet.resolve("input", "", {}).width().pixels(), 8.f);
}

TEST(StyleSheetTest, ParsesCommentSeparatedColorComponents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { color: rgb(1/**/2/**/3); }").ok());

    const Color color = stylesheet.resolve("panel", "", {}).color().resolvedColor();
    EXPECT_NEAR(color.r, 1.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.g, 2.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.b, 3.f / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, ParsesBoxShorthands) {
    constexpr char kBoxSpacingStyles[] = "panel { margin: 1px auto 3px -4px; padding: 5px 6px; gap: 7px 11px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxSpacingStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(style.margin().top.fixedPixels(), 1.f);
    EXPECT_TRUE(style.margin().right.isAuto());
    EXPECT_EQ(style.margin().bottom.fixedPixels(), 3.f);
    EXPECT_EQ(style.margin().left.fixedPixels(), -4.f);
    EXPECT_FALSE(style.margin().left.isAuto());
    EXPECT_EQ(style.padding().left.pixels, 6.f);
    EXPECT_EQ(style.rowGap().fixedPixels(), 7.f);
    EXPECT_EQ(style.columnGap().fixedPixels(), 11.f);
}

TEST(StyleSheetTest, ParsesGapLonghands) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { row-gap: 13px; column-gap: 17px; } panel.normal { gap: normal; } panel.wide { gap: thick; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(style.rowGap().fixedPixels(), 13.f);
    EXPECT_EQ(style.columnGap().fixedPixels(), 17.f);
    const ComputedStyle normal = stylesheet.resolve("panel", "", {"normal"});
    EXPECT_EQ(normal.rowGap().fixedPixels(), 0.f);
    EXPECT_EQ(normal.columnGap().fixedPixels(), 0.f);
    const ComputedStyle wide = stylesheet.resolve("panel", "", {"wide"});
    EXPECT_EQ(wide.rowGap().fixedPixels(), 5.f);
    EXPECT_EQ(wide.columnGap().fixedPixels(), 5.f);
}

TEST(StyleSheetTest, AcceptsUnitlessShadowZero) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { box-shadow: 0 0 0 #123456; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(style.boxShadow().size(), std::size_t(1));
    EXPECT_EQ(style.boxShadow().front().horizontal, 0.f);
    EXPECT_EQ(style.boxShadow().front().vertical, 0.f);
    EXPECT_EQ(style.boxShadow().front().blur, 0.f);
}

TEST(StyleSheetTest, Focus) {
    constexpr char kFocusStyles[] = "button { border-width: 1px; &:focus { opacity: .8; } "
                                    "&:focus-visible { border-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFocusStyles).ok());
    const auto focused = {CSSPseudoClass::Focus};
    const auto focusVisible = {CSSPseudoClass::Focus, CSSPseudoClass::FocusVisible};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).opacity().value, .8f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).borderWidth().top.pixels, 1.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth().top.pixels, 3.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {CSSPseudoClass::FocusVisible}).borderWidth().top.pixels, 1.f);
}

TEST(StyleSheetTest, PseudoOnly) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(":focus-visible { border-width: 2px; }").ok());

    const auto focusVisible = {CSSPseudoClass::Focus, CSSPseudoClass::FocusVisible};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth().top.pixels, 2.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, {CSSPseudoClass::Focus}).borderWidth().top.pixels, 3.f);
}

TEST(StyleSheetTest, UnknownDir) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("input:dir(sideways), input:dir(ltr):dir(rtl) { width: 12px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("dir", "rtl");
    EXPECT_TRUE(stylesheet.resolveElement(input).width().isAuto());
}

TEST(StyleSheetTest, DirPseudo) {
    constexpr char kDirectionStyles[] = "input { &:dir(rtl):checked::slider-thumb { translate: -22px 0; } "
                                        "&:dir(ltr):checked::slider-thumb { translate: 22px 0; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kDirectionStyles).ok());
    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true).checked(true);
    input.setAttribute("dir", "rtl");

    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").translate().x.pixels, -22.f);
    input.setAttribute("dir", "ltr");
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").translate().x.pixels, 22.f);
    input.checked(false);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").translate().x.pixels, 0.f);
}

TEST(StyleSheetTest, DirAuto) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:dir(ltr) { width: 10px; } input:dir(rtl) { width: 20px; }").ok());

    auto root = makeElement<HTMLPanelElement>();
    root->setAttribute("dir", "auto");
    auto excluded = makeElement<HTMLPanelElement>();
    excluded->setAttribute("dir", "rtl");
    excluded->append(std::make_unique<radia::ui::Text>("\xD7\x90"));
    root->append(std::move(excluded));
    auto* text = root->append(std::make_unique<radia::ui::Text>("A"))->asText();
    auto input = makeElement<HTMLInputElement>();
    HTMLInputElement* inputPointer = input.get();
    root->append(std::move(input));

    StylePass styles(stylesheet, FixedTextMetrics{}, LayoutDirection::RightToLeft);
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 10.f);
    EXPECT_EQ(inputPointer->directionality(), LayoutDirection::LeftToRight);

    text->setData("\xD7\x90");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
    EXPECT_EQ(inputPointer->directionality(), LayoutDirection::RightToLeft);

    inputPointer->setAttribute("dir", "auto");
    inputPointer->setAttribute("value", "A");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 10.f);
    inputPointer->setAttribute("value", "\xD7\x90");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
    inputPointer->setAttribute("dir", "sideways");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
}

TEST(StyleSheetTest, KeepsValidDeclarationsAroundUnknown) {
    constexpr char kInvalidStyles[] = "button { width: 99px; unknown-property: 1; height: 7px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kInvalidStyles, "candidate.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}).width().pixels(), 99.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}).height().pixels(), 7.f);
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.unknown");
    EXPECT_EQ(result.warnings.front().source, "candidate.css");
}

TEST(StyleSheetTest, RejectsInvalidDeclarationPropertyNames) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":root { --bad name: #ff0000; } panel { width: 13px; }", "property-name.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheetTest, AcceptsArbitraryCustomPropertyValues) {
    constexpr char kCustomPropertyStyles[] = ":root { --bad: nonsense; --empty:; --number: .1; --case-token: MiXeD; } "
                                             "panel.empty { width: var(--empty, 12px); } panel.fallback { width: var(--missing,); } "
                                             "panel.boundary { opacity: var(--number)0; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kCustomPropertyStyles, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"empty"}).width().isAuto());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"fallback"}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"boundary"}).opacity().value, 1.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).customProperties.at("--case-token").source, "MiXeD");
}

TEST(StyleSheetTest, RejectsReferencesToMissingTokens) {
    constexpr char kMissingTokenStyles[] = "button { width: var(--missing); }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kMissingTokenStyles, "missing-token.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("button", "", {}).width().isAuto());
}

TEST(StyleSheetTest, InvalidCustomPropertySubstitutionUsesUnsetValue) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { width: 24px; opacity: .5; } label.invalid { width: var(--missing); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    label.classList().add("invalid");

    const ComputedStyle style = computedStyle(stylesheet, label);
    EXPECT_TRUE(style.width().isAuto());
    EXPECT_EQ(style.opacity().value, .5f);
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
    EXPECT_EQ(fallbackStyle.width().pixels(), 24.f);
    EXPECT_EQ(fallbackStyle.height().pixels(), 12.f);
    ASSERT_TRUE(fallbackStyle.minWidth().has_value());
    EXPECT_EQ(fallbackStyle.minWidth()->pixels(), 8.f);
    EXPECT_NEAR(fallbackStyle.color().resolvedColor().r, 0x12 / 255.f, 1.0e-4f);
    EXPECT_EQ(fallbackStyle.opacity().value, .5f);

    const ComputedStyle invalidStyle = computedStyle(stylesheet, *invalidPointer);
    EXPECT_TRUE(invalidStyle.height().isAuto());
    EXPECT_EQ(invalidStyle.width().pixels(), 24.f);
    EXPECT_EQ(invalidStyle.opacity().value, .5f);
}

TEST(StyleSheetTest, DoesNotUseFallbackInsideCustomPropertyCycle) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --a: var(--b, 2px); --b: var(--a, 3px); width: var(--a, 7px); height: var(--a); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width().pixels(), 7.f);
    EXPECT_TRUE(style.height().isAuto());
}

TEST(StyleSheetTest, TreatsInitialCustomPropertyAsGuaranteedInvalid) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --token: initial; width: var(--token, 17px); height: var(--token); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width().pixels(), 17.f);
    EXPECT_TRUE(style.height().isAuto());
}

TEST(StyleSheetTest, MatchesChildSelectors) {
    constexpr char kChildOwnerStyles[] = "button.primary > i { width: 10px; } "
                                         "button.primary:hover > i { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kChildOwnerStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("primary");
    Element& icon = appendIcon(button, "search");

    EXPECT_EQ(computedStyle(stylesheet, icon).width().pixels(), 10.f);
    ElementInternalAccess::setHovered(button, true);
    EXPECT_EQ(computedStyle(stylesheet, icon).width().pixels(), 18.f);
}

TEST(StyleSheetTest, IgnoresCommentsAroundChildCombinators) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button /* comment */ > i { width: 19px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    Element& icon = appendIcon(button, "search");
    EXPECT_EQ(computedStyle(stylesheet, icon).width().pixels(), 19.f);
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

    EXPECT_EQ(computedStyle(stylesheet, *buttonPointer).width().pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, *inputPointer).height().pixels(), 12.f);
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
    ASSERT_FALSE(style.width().isAuto());
    EXPECT_EQ(style.width().pixels(), 33.f);
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
    EXPECT_TRUE(styles.style(*buttonPointer).width().isAuto());
    EXPECT_TRUE(styles.style(*buttonPointer).height().isAuto());
    EXPECT_TRUE(styles.style(*inputPointer).height().isAuto());

    labelPointer->classList().add("lead");
    EXPECT_EQ(styles.style(*buttonPointer).width().pixels(), 10.f);
    labelPointer->classList().remove("lead");
    EXPECT_TRUE(styles.style(*buttonPointer).width().isAuto());

    labelPointer->setId("lead");
    EXPECT_EQ(styles.style(*buttonPointer).height().pixels(), 14.f);
    labelPointer->setId("");
    EXPECT_TRUE(styles.style(*buttonPointer).height().isAuto());

    labelPointer->setAttribute("data-lead", "true");
    EXPECT_EQ(styles.style(*inputPointer).height().pixels(), 12.f);
    labelPointer->removeAttribute("data-lead");
    EXPECT_TRUE(styles.style(*inputPointer).height().isAuto());

    auto inserted = makeElement<HTMLLabelElement>();
    inserted->classList().add("lead");
    auto* insertedPointer = buttonPointer->before(std::move(inserted));
    ASSERT_NE(insertedPointer, nullptr);
    EXPECT_EQ(styles.style(*buttonPointer).width().pixels(), 10.f);
    auto removed = insertedPointer->remove();
    ASSERT_NE(removed, nullptr);
    EXPECT_TRUE(styles.style(*buttonPointer).width().isAuto());
}

TEST(StyleSheetTest, ComparesSpecificityAsCSS) {
    constexpr char kStyles[] = "button.a.b.c.d.e.f.g.h.i.j { width: 10px; } button#save { width: 20px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.setId("save");
    for (const char* className : {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"}) button.classList().add(className);

    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 20.f);
}

TEST(StyleSheetTest, MatchesIsAndWhereSelectorFunctions) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel > :is(label.Primary, button) { width: 11px; } "
                                             ":where(panel > label.Primary, button) { height: 13px; }");
    ASSERT_TRUE(result.ok()) << (result.errors.empty() ? "" : result.errors.front().message);
    EXPECT_TRUE(result.warnings.empty());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    label->classList().add("Primary");
    HTMLLabelElement* labelPointer = label.get();
    panel.append(std::move(label));

    const ComputedStyle style = computedStyle(stylesheet, *labelPointer);
    EXPECT_EQ(style.width().pixels(), 11.f);
    EXPECT_EQ(style.height().pixels(), 13.f);
}

TEST(StyleSheetTest, AcceptsHostPseudoClassOutsideShadowTreesWithoutMatching) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":host { opacity: .5; } :host(.thing) { width: 14px; }");
    ASSERT_TRUE(result.ok()) << (result.errors.empty() ? "" : result.errors.front().message);
    EXPECT_TRUE(result.warnings.empty());

    const ComputedStyle style = stylesheet.resolve("panel", "", {"thing"});
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);
    EXPECT_TRUE(style.width().isAuto());
}

TEST(StyleSheetTest, UsesSelectorFunctionSpecificityFromIsAndZeroFromWhere) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("button:is(.primary, #missing) { width: 10px; } button.primary { width: 20px; } "
                               "button:where(#save) { height: 10px; } button { height: 20px; }")
                    .ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setId("save");
    button.classList().add("primary");

    const ComputedStyle style = computedStyle(stylesheet, button);
    EXPECT_EQ(style.width().pixels(), 10.f);
    EXPECT_EQ(style.height().pixels(), 20.f);
}

TEST(StyleSheetTest, ForgivesInvalidIsArguments) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button:is(mystery, .Primary) { width: 17px; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());

    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("Primary");
    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 17.f);
}

TEST(StyleSheetTest, InvalidatesStateInsideIsSelectorFunctions) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button:is(:hover, .active) { width: 18px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_TRUE(styles.style(button).width().isAuto());

    ElementInternalAccess::setHovered(button, true);
    EXPECT_EQ(styles.style(button).width().pixels(), 18.f);
    ElementInternalAccess::setHovered(button, false);
    button.classList().add("active");
    EXPECT_EQ(styles.style(button).width().pixels(), 18.f);
}

TEST(StyleSheetTest, CountsRepeatedPseudoSpecificity) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button:hover:hover { width: 10px; } button:hover { width: 20px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    ElementInternalAccess::setHovered(button, true);
    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 10.f);
}

TEST(StyleSheetTest, MatchesCompoundClassesAndPseudoClasses) {
    constexpr char kStyles[] = "button.primary.priority:hover:focus { width: 23px; } button.primary { width: 7px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("primary").add("priority");
    ElementInternalAccess::setHovered(button, true);
    ElementInternalAccess::setFocused(button, true);

    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 23.f);
    ElementInternalAccess::setFocused(button, false);
    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 7.f);
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

TEST(StyleSheetTest, MatchesCaseInsensitivePseudoClassesAndRootTokens) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":/**/ROOT { --accent: #123456; } button:HOVER { width: 18px; } panel { color: var(--accent); }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {CSSPseudoClass::Hover}).width().pixels(), 18.f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {}).color().resolvedColor().r, 0x12 / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, DecodesEscapedSelectorKeywords) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button:h\\6f ver { width: 18px; } input:dir(r\\74 l) { height: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {CSSPseudoClass::Hover}).width().pixels(), 18.f);
    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("dir", "rtl");
    EXPECT_EQ(stylesheet.resolveElement(input).height().pixels(), 13.f);
}

TEST(StyleSheetTest, SeparatesPartState) {
    constexpr char kInteractivePartStyles[] = "floater > head > close { width: 10px; } floater > head > close:hover { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInteractivePartStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, true);
    auto* closeButton = floater.closeButton();
    ASSERT_NE(closeButton, nullptr);

    ElementInternalAccess::setHovered(floater, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width().pixels(), 10.f);
    ElementInternalAccess::setHovered(*closeButton, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width().pixels(), 18.f);
}

TEST(StyleSheetTest, InheritsTextWrapLonghandsIndependently) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("panel { text-wrap: nowrap pretty; } "
                               "#mode { text-wrap-mode: wrap; } "
                               "#style { text-wrap-style: stable; } "
                               "#inherit { text-wrap: nowrap stable; text-wrap-style: inherit; }")
                    .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto mode = makeElement<HTMLLabelElement>();
    mode->setId("mode");
    auto style = makeElement<HTMLLabelElement>();
    style->setId("style");
    auto inherit = makeElement<HTMLLabelElement>();
    inherit->setId("inherit");
    Element* modePtr = mode.get();
    Element* stylePtr = style.get();
    Element* inheritPtr = inherit.get();
    panel.append(std::move(mode));
    panel.append(std::move(style));
    panel.append(std::move(inherit));

    const ComputedStyle modeStyle = computedStyle(stylesheet, *modePtr);
    const ComputedStyle styleStyle = computedStyle(stylesheet, *stylePtr);
    const ComputedStyle inheritStyle = computedStyle(stylesheet, *inheritPtr);

    EXPECT_EQ(modeStyle.textWrapMode(), TextWrapMode::Wrap);
    EXPECT_EQ(modeStyle.textWrapStyle(), TextWrapStyle::Pretty);
    EXPECT_EQ(styleStyle.textWrapMode(), TextWrapMode::NoWrap);
    EXPECT_EQ(styleStyle.textWrapStyle(), TextWrapStyle::Stable);
    EXPECT_EQ(inheritStyle.textWrapMode(), TextWrapMode::NoWrap);
    EXPECT_EQ(inheritStyle.textWrapStyle(), TextWrapStyle::Pretty);
}

TEST(StyleSheetTest, RejectsInvalidTextWrapWithoutPartialApplication) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("label { text-wrap: nowrap unknown; }");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");

    const ComputedStyle style = stylesheet.resolve("label", "", {});
    EXPECT_EQ(style.textWrapMode(), TextWrapMode::Wrap);
    EXPECT_EQ(style.textWrapStyle(), TextWrapStyle::Auto);
}

TEST(StyleSheetTest, MatchesPseudoElementPseudoClasses) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input::slider-thumb { width: 10px; } input::slider-thumb:hover { width: 18px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true);

    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").width().pixels(), 10.f);
    ElementInternalAccess::setHovered(input, true);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").width().pixels(), 18.f);
}

TEST(StyleSheetTest, PreservesCursorOnFailure) {
    constexpr char kCursorStyles[] = "button { cursor: pointer; } #horizontal { cursor: e-resize; } "
                                     "#diagonal { cursor: sw-resize; } #grab { cursor: grab; } "
                                     "#grabbing { cursor: grabbing; }";
    constexpr char kInvalidCursorStyles[] = "button { cursor: teleport; cursor: pointer; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCursorStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}).cursor, CursorStyle::Pointer);
    EXPECT_EQ(stylesheet.resolve("panel", "horizontal", {}).cursor, CursorStyle::EastWestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "diagonal", {}).cursor, CursorStyle::NortheastSouthwestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "grab", {}).cursor, CursorStyle::Grab);
    EXPECT_EQ(stylesheet.resolve("panel", "grabbing", {}).cursor, CursorStyle::Grabbing);
    EXPECT_NE(stylesheet.resolve("panel", "grab", {}).cursor, stylesheet.resolve("panel", "grabbing", {}).cursor);

    const auto invalid = stylesheet.loadRadia(kInvalidCursorStyles, "cursor.css");
    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(stylesheet.resolve("button", "", {}).cursor, CursorStyle::Pointer);
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
    EXPECT_EQ(computedStyle(stylesheet, genericInput).width().pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, switchInput).width().pixels(), 40.f);
}

TEST(StyleSheetTest, SelectsRadioInputsByNameAttribute) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 40.f);
}

TEST(StyleSheetTest, ClearsRemovedRadioName) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name] { width: 20px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 40.f);

    input.name("");
    EXPECT_FALSE(input.hasAttribute("name"));
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 10.f);
}

TEST(StyleSheetTest, RefreshesSerializedDisabledAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input[disabled=one] { width: 10px; } input[disabled=two] { width: 20px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("disabled", "one");
    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_EQ(styles.style(input).width().pixels(), 10.f);

    input.setAttribute("disabled", "two");
    EXPECT_EQ(styles.style(input).width().pixels(), 20.f);
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
    EXPECT_EQ(computedStyle(stylesheet, panel).width().pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).height().pixels(), 11.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).left()->pixels, 12.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).right()->pixels, 13.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).top()->pixels, 14.f);
    EXPECT_EQ(computedStyle(stylesheet, panel).bottom()->pixels, 15.f);
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, panel).opacity().value, 1.f);
    panel.setAttribute("type", "TeXt");
    ComputedStyle mixedType = computedStyle(stylesheet, panel);
    EXPECT_EQ(mixedType.flexGrow().value, 1.f);
    EXPECT_EQ(mixedType.flexShrink().value, 2.f);
    EXPECT_EQ(mixedType.order().value, 0);

    panel.setAttribute("type", "text");
    mixedType = computedStyle(stylesheet, panel);
    EXPECT_EQ(mixedType.flexGrow().value, 1.f);
    EXPECT_EQ(mixedType.flexShrink().value, 2.f);
    EXPECT_EQ(mixedType.order().value, 3);

    auto icon = makeElement<Element>("i");
    icon->classList().add("i-search");
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, *icon).opacity().value, .5f);
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
    EXPECT_EQ(computedStyle(universal, panel).width().pixels(), 11.f);

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
    EXPECT_EQ(computedStyle(stylesheet, *icon).width().pixels(), 12.f);
}

TEST(StyleSheetTest, InvalidatesDynamicAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel[hidden] { width: 11px; }").ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    StylePass styles(stylesheet, FixedTextMetrics{});
    EXPECT_TRUE(styles.style(panel).width().isAuto());
    EXPECT_TRUE(styles.style(panel).height().isAuto());

    panel.setAttribute("hidden");
    ASSERT_FALSE(styles.style(panel).width().isAuto());
    EXPECT_EQ(styles.style(panel).width().pixels(), 11.f);
    panel.removeAttribute("hidden");
    EXPECT_TRUE(styles.style(panel).width().isAuto());

    panel.setVisibility(Visibility::Hidden);
    EXPECT_FALSE(panel.hasAttribute("visibility"));
    panel.setVisibility(Visibility::Visible);
}

TEST(StyleSheetTest, SelectsIndeterminateInputs) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:indeterminate { opacity: .5; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox");
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity().value, 1.f);
    input.indeterminate(true);
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity().value, .5f);
}

TEST(StyleSheetTest, SkipsInvalidRules) {
    struct InvalidRuleCase {
        const char* source;
        const char* diagnostic;
    };

    const InvalidRuleCase cases[] = {
        {"button::label { stroke-width: 2px; }", "stylesheet.selector.pseudo_element_unknown"},
        {"input::missing { width: 10px; }", "stylesheet.selector.pseudo_element_unknown"},
        {"button::slider-thumb { width: 10px; }", "stylesheet.selector.pseudo_element_unknown"},
        {"panel { align-items: sideways; }", "stylesheet.property.value_invalid"},
        {"panel { align-items: anchor-center; }", "stylesheet.property.value_invalid"},
        {"panel { align-content: left; }", "stylesheet.property.value_invalid"},
        {"panel { align-content: right; }", "stylesheet.property.value_invalid"},
        {"button { align-self: sideways; }", "stylesheet.property.value_invalid"},
        {"button { align-self: safe anchor-center; }", "stylesheet.property.value_invalid"},
        {"input { justify-self: sideways; }", "stylesheet.property.value_invalid"},
        {"label { text-align: middle; }", "stylesheet.property.value_invalid"},
        {"panel { vertical-align: center; }", "stylesheet.property.value_invalid"},
        {"label { translate: 10px 4px 1px 2px; }", "stylesheet.property.value_invalid"},
        {"mystery { width: 10px; }", "stylesheet.selector.element_unknown"},
        {"label:cheked { opacity: .5; }", "stylesheet.selector.pseudo_class_unknown"},
        {"button { &:cheked { opacity: .5; } }", "stylesheet.selector.pseudo_class_unknown"},
        {"label { order: 1.5; }", "stylesheet.property.value_invalid"},
        {"label { order: 1px; }", "stylesheet.property.value_invalid"},
        {"label:has(button) { order: 1; }", "stylesheet.selector.function_unsupported"},
        {":host() { opacity: .5; }", "stylesheet.selector.pseudo_class_invalid"},
        {":host(.thing .child) { opacity: .5; }", "stylesheet.selector.pseudo_class_invalid"},
        {"panel { scrollbar-width: wide; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-color: #123456; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-color: #123456 #ffffff #000000; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-gutter: stable auto; }", "stylesheet.property.value_invalid"},
        {"panel { scrollbar-gutter: both-edges; }", "stylesheet.property.value_invalid"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid CSS: " << test.source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.source, "contract.css");

        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result.warnings.size(), std::size_t{1});
        EXPECT_EQ(result.warnings.front().code, test.diagnostic);
    }
}

TEST(StyleSheetTest, SkipsMultiplePseudoElementPseudoClasses) {
    constexpr char kPseudoElementStyles[] = "input::slider-thumb:hover:checked { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kPseudoElementStyles, "contract.css");

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
                                        "accent-color: #102030ff; scrollbar-color: #314159 #271828; "
                                        "text-align: center; vertical-align: bottom; cursor: grab; opacity: .5; "
                                        "pointer-events: none; background-color: #ffffffff; } "
                                        "label#override { font-size: 11px; cursor: default; vertical-align: inherit; }";

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
    EXPECT_EQ(inheritedStyle.fontFamily(), FontFamilies{GenericFontFamily::SansSerif});
    EXPECT_EQ(inheritedStyle.fontSize(), 19.f);
    EXPECT_EQ(inheritedStyle.fontWeight().value, 700.f);
    EXPECT_EQ(inheritedStyle.fontStyle(), radia::ui::FontStyle::Italic);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(inheritedStyle.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(inheritedStyle.lineHeight().mValue).pixels, 23.f);
    EXPECT_NEAR(inheritedStyle.color().resolvedColor().b, 96.f / 255.f, 1.0e-4f);
    ASSERT_FALSE(inheritedStyle.accentColor().isKeyword());
    EXPECT_FALSE(inheritedStyle.accentColor().value().isCurrentColor());
    EXPECT_NEAR(inheritedStyle.accentColor().value().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    EXPECT_FALSE(inheritedStyle.scrollbarColor().automatic);
    EXPECT_NEAR(inheritedStyle.scrollbarColor().thumb.resolvedColor().r, 0x31 / 255.f, 1.0e-4f);
    EXPECT_NEAR(inheritedStyle.scrollbarColor().track.resolvedColor().g, 0x18 / 255.f, 1.0e-4f);
    EXPECT_EQ(inheritedStyle.textAlign(), TextAlign::Center);
    EXPECT_EQ(inheritedStyle.verticalAlign().value, VerticalAlign::Baseline);
    EXPECT_EQ(inheritedStyle.cursor, CursorStyle::Grab);
    EXPECT_EQ(inheritedStyle.opacity().value, 1.f);
    EXPECT_EQ(inheritedStyle.pointerEvents(), PointerEvents::NoneValue);
    EXPECT_EQ(inheritedStyle.backgroundColor().resolvedColor().a, 0.f);

    const ComputedStyle overriddenStyle = computedStyle(stylesheet, *overriddenLabel);
    EXPECT_EQ(overriddenStyle.fontSize(), 11.f);
    EXPECT_EQ(overriddenStyle.cursor, CursorStyle::Default);
    EXPECT_EQ(overriddenStyle.verticalAlign().value, VerticalAlign::Bottom);
}

TEST(StyleSheetTest, OmitsBorderStyleByDefault) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("panel { border: 3px #123456; } panel.medium { border: solid #123456; } "
                               "panel.none { border-style: none; border-width: 4px; }")
                    .ok());

    const ComputedStyle omitted = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(omitted.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(omitted.borderStyle().top, radia::ui::BorderStyle::NoneValue);

    const ComputedStyle medium = stylesheet.resolve("panel", "", {"medium"});
    EXPECT_EQ(medium.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(medium.borderStyle().top, radia::ui::BorderStyle::Solid);

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"});
    EXPECT_EQ(none.borderWidth().top.pixels, 4.f);
    EXPECT_EQ(none.borderStyle().top, radia::ui::BorderStyle::NoneValue);
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
    EXPECT_EQ(parentStyle.textDecoration(), TextDecoration::Underline);
    EXPECT_EQ(parentStyle.textDecorationPropagation, TextDecoration::Underline);

    const ComputedStyle inheritedStyle = computedStyle(stylesheet, *inheritedPointer);
    EXPECT_EQ(inheritedStyle.textDecoration(), TextDecoration::NoneValue);
    EXPECT_EQ(inheritedStyle.textDecorationPropagation, TextDecoration::Underline);

    auto inheritedDecoration = makeElement<HTMLLabelElement>();
    inheritedDecoration->classList().add("inherit");
    HTMLLabelElement* inheritedDecorationPointer = inheritedDecoration.get();
    panel.append(std::move(inheritedDecoration));
    const ComputedStyle explicitInheritanceStyle = computedStyle(stylesheet, *inheritedDecorationPointer);
    EXPECT_EQ(explicitInheritanceStyle.textDecoration(), TextDecoration::Underline);

    const ComputedStyle explicitStyle = computedStyle(stylesheet, *explicitPointer);
    const auto both =
        static_cast<TextDecoration>(static_cast<unsigned>(TextDecoration::Underline) | static_cast<unsigned>(TextDecoration::LineThrough));
    EXPECT_EQ(explicitStyle.textDecoration(), TextDecoration::LineThrough);
    EXPECT_EQ(explicitStyle.textDecorationPropagation, both);
}

TEST(StyleSheetTest, RelativeWeight) {
    constexpr char kStyles[] = "panel.w99 { font-weight: 99; } panel.w100 { font-weight: 100; } panel.w349 { font-weight: 349; } "
                               "panel.w400 { font-weight: 400; } "
                               "panel.w350 { font-weight: 350; } panel.w549 { font-weight: 549; } panel.w550 { font-weight: 550; } "
                               "panel.w749 { font-weight: 749; } panel.w750 { font-weight: 750; } panel.w899 { font-weight: 899; } "
                               "panel.w900 { font-weight: 900; } panel.w1000 { font-weight: 1000; } "
                               "label.bolder { font-weight: bolder; } label.lighter { font-weight: lighter; } "
                               "label.shorthand { font: bolder 13px sans-serif; }";
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const auto childWeight = [&](const char* parentClass, const char* childClass) {
        auto parent = makeElementValue<HTMLPanelElement>();
        parent.classList().add(parentClass);
        auto child = makeElement<HTMLLabelElement>("Child");
        HTMLLabelElement* childPointer = child.get();
        childPointer->classList().add(childClass);
        parent.append(std::move(child));
        return computedStyle(stylesheet, *childPointer).fontWeight().value;
    };

    EXPECT_EQ(childWeight("w400", "bolder"), 700.f);
    EXPECT_EQ(childWeight("w400", "shorthand"), 700.f);
    EXPECT_EQ(childWeight("w400", "lighter"), 100.f);
    EXPECT_EQ(childWeight("w99", "bolder"), 400.f);
    EXPECT_EQ(childWeight("w99", "lighter"), 99.f);
    EXPECT_EQ(childWeight("w100", "lighter"), 100.f);
    EXPECT_EQ(childWeight("w349", "bolder"), 400.f);
    EXPECT_EQ(childWeight("w350", "bolder"), 700.f);
    EXPECT_EQ(childWeight("w549", "bolder"), 700.f);
    EXPECT_EQ(childWeight("w550", "bolder"), 900.f);
    EXPECT_EQ(childWeight("w550", "lighter"), 400.f);
    EXPECT_EQ(childWeight("w749", "lighter"), 400.f);
    EXPECT_EQ(childWeight("w750", "lighter"), 700.f);
    EXPECT_EQ(childWeight("w899", "lighter"), 700.f);
    EXPECT_EQ(childWeight("w900", "bolder"), 900.f);
    EXPECT_EQ(childWeight("w1000", "bolder"), 1000.f);
    EXPECT_EQ(childWeight("w1000", "lighter"), 700.f);
}

TEST(StyleSheetTest, ResolvesCSSWideInheritanceKeywords) {
    constexpr char kStyles[] = "panel { width: 42px; display: block; color: #204060ff; font-size: 19px; align-self: center; "
                               "background-color: #102030ff; backdrop-filter: blur(3px); } "
                               "label.explicit-inherit { width: inherit; display: inherit; color: inherit; align-self: inherit; "
                               "background-color: inherit; backdrop-filter: inherit; } "
                               "label.unset-inherited { color: unset; font-size: unset; } "
                               "label.unset-initial { width: unset; background-color: unset; backdrop-filter: unset; } "
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
    EXPECT_EQ(parentStyle.width().pixels(), 42.f);
    EXPECT_EQ(parentStyle.display(), radia::ui::Display::Block);
    EXPECT_NEAR(parentStyle.color().resolvedColor().r, 32.f / 255.f, 1.0e-4f);
    EXPECT_EQ(parentStyle.fontSize(), 19.f);
    EXPECT_EQ(parentStyle.alignSelf().position, radia::ui::ItemPosition::Center);
    EXPECT_NEAR(parentStyle.backgroundColor().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    ASSERT_EQ(parentStyle.backdropFilter().operations.size(), 1U);

    const ComputedStyle explicitInheritStyle = computedStyle(stylesheet, *explicitInheritPtr);
    EXPECT_EQ(explicitInheritStyle.width().pixels(), 42.f);
    EXPECT_EQ(explicitInheritStyle.display(), radia::ui::Display::Block);
    EXPECT_NEAR(explicitInheritStyle.color().resolvedColor().r, parentStyle.color().resolvedColor().r, 1.0e-4f);
    EXPECT_EQ(explicitInheritStyle.alignSelf(), parentStyle.alignSelf());
    EXPECT_NEAR(explicitInheritStyle.backgroundColor().resolvedColor().r, parentStyle.backgroundColor().resolvedColor().r, 1.0e-4f);
    EXPECT_EQ(explicitInheritStyle.backdropFilter(), parentStyle.backdropFilter());

    const ComputedStyle unsetInheritedStyle = computedStyle(stylesheet, *unsetInheritedPtr);
    EXPECT_NEAR(unsetInheritedStyle.color().resolvedColor().r, parentStyle.color().resolvedColor().r, 1.0e-4f);
    EXPECT_EQ(unsetInheritedStyle.fontSize(), parentStyle.fontSize());

    const ComputedStyle unsetInitialStyle = computedStyle(stylesheet, *unsetInitialPtr);
    EXPECT_TRUE(unsetInitialStyle.width().isAuto());
    EXPECT_FLOAT_EQ(unsetInitialStyle.backgroundColor().resolvedColor().a, 0.f);
    EXPECT_TRUE(unsetInitialStyle.backdropFilter().isNone());

    EXPECT_EQ(computedStyle(stylesheet, *lateInheritPtr).width().pixels(), 42.f);
    EXPECT_EQ(computedStyle(stylesheet, *lateValuePtr).width().pixels(), 9.f);
}

TEST(StyleSheetTest, Overflow) {
    constexpr char kOverflowStyles[] = "panel { overflow: scroll auto; } "
                                       "#single { overflow: auto; } "
                                       "#longhand { overflow-x: auto; overflow-y: scroll; } "
                                       "#vertical { overflow-x: visible; overflow-y: hidden; } "
                                       "#horizontal { overflow-x: hidden; overflow-y: visible; } "
                                       "#clip { overflow: clip; } "
                                       "#clip-hidden { overflow-x: clip; overflow-y: hidden; } "
                                       "#initial { overflow: scroll; overflow-x: initial; overflow-y: initial; }";
    constexpr char kInvalidOverflowStyles[] = "panel { overflow: nonsense; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kOverflowStyles).ok());
    const ComputedStyle split = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(split.overflowX(), Overflow::Scroll);
    EXPECT_EQ(split.overflowY(), Overflow::Auto);
    const ComputedStyle single = stylesheet.resolve("panel", "single", {});
    EXPECT_EQ(single.overflowX(), Overflow::Auto);
    EXPECT_EQ(single.overflowY(), Overflow::Auto);
    const ComputedStyle longhand = stylesheet.resolve("panel", "longhand", {});
    EXPECT_EQ(longhand.overflowX(), Overflow::Auto);
    EXPECT_EQ(longhand.overflowY(), Overflow::Scroll);
    const ComputedStyle vertical = stylesheet.resolve("panel", "vertical", {});
    EXPECT_EQ(vertical.overflowX(), Overflow::Auto);
    EXPECT_EQ(vertical.overflowY(), Overflow::Hidden);
    const ComputedStyle horizontal = stylesheet.resolve("panel", "horizontal", {});
    EXPECT_EQ(horizontal.overflowX(), Overflow::Hidden);
    EXPECT_EQ(horizontal.overflowY(), Overflow::Auto);
    const ComputedStyle clip = stylesheet.resolve("panel", "clip", {});
    EXPECT_EQ(clip.overflowX(), Overflow::Clip);
    EXPECT_EQ(clip.overflowY(), Overflow::Clip);
    const ComputedStyle clipHidden = stylesheet.resolve("panel", "clip-hidden", {});
    EXPECT_EQ(clipHidden.overflowX(), Overflow::Hidden);
    EXPECT_EQ(clipHidden.overflowY(), Overflow::Hidden);
    const ComputedStyle initial = stylesheet.resolve("panel", "initial", {});
    EXPECT_EQ(initial.overflowX(), Overflow::Visible);
    EXPECT_EQ(initial.overflowY(), Overflow::Visible);

    const auto invalid = stylesheet.loadRadia(kInvalidOverflowStyles, "overflow.css");
    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
}

TEST(StyleSheetTest, ParsesBorderRadius) {
    constexpr char kStyles[] = "panel { border-radius: 10px 100px / 120px; } "
                               "#expanded { border-radius: 10px 20px 30px / 40px 50px 60px 70px; } "
                               "#mirrored { border-radius: 10%; } "
                               "#absolute { border-radius: 1in; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle split = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(split.borderRadius().topLeft.horizontal.pixels, 10.f);
    EXPECT_EQ(split.borderRadius().topRight.horizontal.pixels, 100.f);
    EXPECT_EQ(split.borderRadius().bottomRight.horizontal.pixels, 10.f);
    EXPECT_EQ(split.borderRadius().bottomLeft.horizontal.pixels, 100.f);
    EXPECT_EQ(split.borderRadius().topLeft.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius().topRight.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius().bottomRight.vertical.pixels, 120.f);
    EXPECT_EQ(split.borderRadius().bottomLeft.vertical.pixels, 120.f);

    const ComputedStyle expanded = stylesheet.resolve("panel", "expanded", {});
    EXPECT_EQ(expanded.borderRadius().topLeft.horizontal.pixels, 10.f);
    EXPECT_EQ(expanded.borderRadius().topRight.horizontal.pixels, 20.f);
    EXPECT_EQ(expanded.borderRadius().bottomRight.horizontal.pixels, 30.f);
    EXPECT_EQ(expanded.borderRadius().bottomLeft.horizontal.pixels, 20.f);
    EXPECT_EQ(expanded.borderRadius().topLeft.vertical.pixels, 40.f);
    EXPECT_EQ(expanded.borderRadius().topRight.vertical.pixels, 50.f);
    EXPECT_EQ(expanded.borderRadius().bottomRight.vertical.pixels, 60.f);
    EXPECT_EQ(expanded.borderRadius().bottomLeft.vertical.pixels, 70.f);

    const ComputedStyle mirrored = stylesheet.resolve("panel", "mirrored", {});
    EXPECT_NEAR(mirrored.borderRadius().topLeft.horizontal.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius().topLeft.vertical.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius().bottomLeft.horizontal.percent, .1f, 1.0e-6f);
    EXPECT_NEAR(mirrored.borderRadius().bottomLeft.vertical.percent, .1f, 1.0e-6f);

    const ComputedStyle absolute = stylesheet.resolve("panel", "absolute", {});
    EXPECT_EQ(absolute.borderRadius().topLeft.horizontal.pixels, 96.f);
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
                                        "#reverse { scrollbar-gutter: both-edges stable; } "
                                        "#initial { scrollbar-width: initial; scrollbar-gutter: initial; scrollbar-color: initial; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kScrollbarStyles).ok());

    const ComputedStyle overlay = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(overlay.scrollbarWidth(), ScrollbarWidth::Thin);
    EXPECT_EQ(overlay.scrollbarGutter(), ScrollbarGutter::StableBothEdges);
    EXPECT_FALSE(overlay.scrollbarColor().automatic);
    EXPECT_NEAR(overlay.scrollbarColor().thumb.resolvedColor().r, 0x11 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor().thumb.resolvedColor().g, 0x22 / 255.f, 1.0e-6f);
    EXPECT_NEAR(overlay.scrollbarColor().track.resolvedColor().b, 0x66 / 255.f, 1.0e-6f);

    const ComputedStyle classic = stylesheet.resolve("panel", "classic", {});
    EXPECT_EQ(classic.scrollbarWidth(), ScrollbarWidth::NoneValue);
    EXPECT_EQ(classic.scrollbarGutter(), ScrollbarGutter::Stable);

    EXPECT_EQ(stylesheet.resolve("panel", "reverse", {}).scrollbarGutter(), ScrollbarGutter::StableBothEdges);

    const ComputedStyle initial = stylesheet.resolve("panel", "initial", {});
    EXPECT_EQ(initial.scrollbarWidth(), ScrollbarWidth::Auto);
    EXPECT_EQ(initial.scrollbarGutter(), ScrollbarGutter::Auto);
    EXPECT_TRUE(initial.scrollbarColor().automatic);
}

TEST(StyleSheetTest, ParsesAccentColorValues) {
    constexpr char kAccentStyles[] = "panel { accent-color: #12345678; } #current { accent-color: currentcolor; } "
                                     "#automatic { accent-color: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kAccentStyles).ok());

    const ComputedStyle color = stylesheet.resolve("panel", "", {});
    ASSERT_FALSE(color.accentColor().isKeyword());
    EXPECT_FALSE(color.accentColor().value().isCurrentColor());
    EXPECT_NEAR(color.accentColor().value().resolvedColor().r, 0x12 / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.accentColor().value().resolvedColor().a, 0x78 / 255.f, 1.0e-6f);

    const ComputedStyle currentStyle = stylesheet.resolve("panel", "current", {});
    const AccentColor& current = currentStyle.accentColor();
    ASSERT_FALSE(current.isKeyword());
    EXPECT_TRUE(current.value().isCurrentColor());
    EXPECT_TRUE(stylesheet.resolve("panel", "automatic", {}).accentColor().isKeyword());
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

    EXPECT_EQ(computedStyle(stylesheet, *inheritedInputPtr).usedColorScheme, ColorSchemeMode::Light);
    EXPECT_EQ(computedStyle(stylesheet, *overriddenInputPtr).usedColorScheme, ColorSchemeMode::Dark);
}

TEST(StyleSheetTest, SystemColors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(
        stylesheet.loadRadia("panel { color-scheme: light; color: ButtonText; background-color: ButtonFace; border: 1px solid ButtonBorder; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 0.f);
    EXPECT_NEAR(style.backgroundColor().resolvedColor().r, 240.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(style.borderTopColor().resolvedColor().r, 118.f / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, SchemeColor) {
    constexpr char kLightDarkStyles[] = ":root { color-scheme: light; } panel.dark { color-scheme: dark; } "
                                        "panel { background-color: light-dark(#101010, #f0f0f0); } "
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

    EXPECT_NEAR(computedStyle(stylesheet, lightPanel).backgroundColor().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(computedStyle(stylesheet, darkPanel).backgroundColor().resolvedColor().r, 240.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(computedStyle(stylesheet, *lightLabelPtr).color().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(computedStyle(stylesheet, *darkLabelPtr).color().resolvedColor().r, 240.f / 255.f, 1.0e-4f);
}

TEST(StyleSheetTest, ResolvesCurrentColorAgainstDescendantColor) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("panel { color: #123456; } label.inherited { color: currentcolor; "
                               "border: 1px solid currentcolor; } label.own { color: #abcdef; "
                               "border: 1px solid currentcolor; }")
                    .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto inheritedLabel = makeElement<HTMLLabelElement>();
    HTMLLabelElement* inheritedLabelPointer = inheritedLabel.get();
    inheritedLabelPointer->classList().add("inherited");
    panel.append(std::move(inheritedLabel));
    auto ownColorLabel = makeElement<HTMLLabelElement>();
    HTMLLabelElement* ownColorLabelPointer = ownColorLabel.get();
    ownColorLabelPointer->classList().add("own");
    panel.append(std::move(ownColorLabel));

    const ComputedStyle inherited = computedStyle(stylesheet, *inheritedLabelPointer);
    EXPECT_NEAR(inherited.color().resolvedColor().r, 18.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(inherited.borderTopColor().resolvedColor().r, 18.f / 255.f, 1.0e-6f);
    const ComputedStyle own = computedStyle(stylesheet, *ownColorLabelPointer);
    EXPECT_NEAR(own.color().resolvedColor().r, 171.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(own.borderTopColor().resolvedColor().r, 171.f / 255.f, 1.0e-6f);
}

TEST(StyleSheetTest, ProjectsMinimizedPseudoClass) {
    constexpr char kMinimizedFloaterStyles[] = "floater > head { border-width: 0px 0px 1px; } "
                                               "floater:minimized > head { border-width: 0px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMinimizedFloaterStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, false, true);
    Element* head = floater.head();
    ASSERT_NE(head, nullptr);

    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth().bottom.pixels, 1.f);
    floater.setMinimized(true);
    EXPECT_TRUE(floater.minimized());
    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth().bottom.pixels, 0.f);
    floater.setMinimized(false);
    EXPECT_FALSE(floater.minimized());
    EXPECT_EQ(computedStyle(stylesheet, *head).borderWidth().bottom.pixels, 1.f);
}

TEST(StyleSheetTest, ParsesTypedDimensions) {
    constexpr char kTypedLengthStyles[] = "panel { width: 40px; min-width: 20px; min-height: 10px; left: -8px; line-height: 18px; }";
    constexpr char kAutoDimensionStyles[] = "panel { width: 40px; height: 20px; width: auto; height: auto; } "
                                            "button { size: auto; } i { size: auto 16px; }";
    constexpr char kUnitlessLineHeightStyles[] = "panel { font-size: 20px; line-height: 1.5; min-width: auto; min-height: auto; }";
    constexpr char kAutoGapStyles[] = "panel { gap: auto; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kTypedLengthStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_FALSE(style.width().isAuto());
    EXPECT_EQ(style.width().pixels(), 40.f);
    ASSERT_TRUE(style.minWidth().has_value());
    EXPECT_EQ(style.minWidth()->pixels(), 20.f);
    ASSERT_TRUE(style.minHeight().has_value());
    EXPECT_EQ(style.minHeight()->pixels(), 10.f);
    ASSERT_TRUE(style.left().has_value());
    EXPECT_EQ(style.left()->pixels, -8.f);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(style.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(style.lineHeight().mValue).pixels, 18.f);

    StyleSheet unitlessStylesheet;
    ASSERT_TRUE(unitlessStylesheet.loadRadia(kUnitlessLineHeightStyles).ok());
    const ComputedStyle unitless = unitlessStylesheet.resolve("panel", "", {});
    ASSERT_TRUE(std::holds_alternative<LineHeight::Number>(unitless.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Number>(unitless.lineHeight().mValue).value, 1.5f);
    EXPECT_FALSE(unitless.minWidth().has_value());
    EXPECT_FALSE(unitless.minHeight().has_value());

    ASSERT_TRUE(stylesheet.loadRadia(kAutoDimensionStyles).ok());
    const ComputedStyle automatic = stylesheet.resolve("panel", "", {});
    EXPECT_TRUE(automatic.width().isAuto());
    EXPECT_TRUE(automatic.height().isAuto());
    const ComputedStyle automaticSize = stylesheet.resolve("button", "", {});
    EXPECT_TRUE(automaticSize.width().isAuto());
    EXPECT_TRUE(automaticSize.height().isAuto());
    const ComputedStyle mixedSize = stylesheet.resolve("i", "", {});
    EXPECT_TRUE(mixedSize.height().isAuto());
    EXPECT_EQ(mixedSize.width().pixels(), 16.f);

    const auto invalidGap = stylesheet.loadRadia(kAutoGapStyles);
    ASSERT_TRUE(invalidGap.ok());
    ASSERT_FALSE(invalidGap.warnings.empty());
    EXPECT_EQ(invalidGap.warnings.front().code, "stylesheet.property.value_invalid");

    const auto percentageGap =
        stylesheet.loadRadia("panel { gap: 10%; row-gap: 20%; column-gap: 30%; } panel.zero { gap: 0%; } panel.unitless { gap: 5; }");
    ASSERT_TRUE(percentageGap.ok());
    ASSERT_EQ(percentageGap.warnings.size(), std::size_t(4));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).rowGap().fixedPixels(), 0.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {"zero"}).columnGap().fixedPixels(), 0.f);
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
    EXPECT_EQ(directStyle.opacity().value, .8f);
    EXPECT_EQ(directStyle.width().pixels(), 10.f);
    EXPECT_EQ(directStyle.height().pixels(), 11.f);
    ASSERT_TRUE(directStyle.minWidth().has_value());
    EXPECT_TRUE(directStyle.minWidth()->isPercentage());
    EXPECT_NEAR(directStyle.minWidth()->resolve(0.f, 1.f), .2f, 1.0e-4f);
    ASSERT_TRUE(directStyle.right().has_value());
    EXPECT_NEAR(directStyle.right()->percent, .05f, 1.0e-4f);

    const ComputedStyle nestedStyle = computedStyle(stylesheet, *nestedLabel);
    EXPECT_TRUE(nestedStyle.width().isAuto());
    EXPECT_EQ(nestedStyle.height().pixels(), 11.f);
    ASSERT_TRUE(nestedStyle.minHeight().has_value());
    EXPECT_NEAR(nestedStyle.minHeight()->resolve(0.f, 1.f), .25f, 1.0e-4f);
    ASSERT_TRUE(nestedStyle.bottom().has_value());
    EXPECT_NEAR(nestedStyle.bottom()->percent, .1f, 1.0e-4f);
}

TEST(StyleSheetTest, ParsesInsets) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { inset: -8px 5% 10px auto; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_TRUE(style.top().has_value());
    EXPECT_EQ(style.top()->pixels, -8.f);
    ASSERT_TRUE(style.right().has_value());
    EXPECT_NEAR(style.right()->percent, .05f, 1.0e-4f);
    ASSERT_TRUE(style.bottom().has_value());
    EXPECT_EQ(style.bottom()->pixels, 10.f);
    EXPECT_FALSE(style.left().has_value());
}

TEST(StyleSheetTest, ParsesFilter) {
    constexpr char kBoxEffectStyles[] = "panel { background-image: linear-gradient(to/**/right, #ff0000ff, "
                                        "rgb(0, 255, 0, 50%) 75%, #0000ffff); "
                                        "box-shadow: 1px 2px #11223344, 3px 4px 5px 6px "
                                        "rgb(10, 20, 30, 40%) inset; outline-offset: 3px; "
                                        "outline: light-dark(#abcdef88, #12345688) solid 2px; } panel.light { color-scheme: light; } "
                                        "label { outline: 1px dashed #ffffffff; }";
    constexpr char kFilterStyles[] = "panel { backdrop-filter: linear-blur(to bottom, 0px 25%, 16px 75%); "
                                     "filter: linear-blur(to right, 2px 25%, 6px 75%) blur(4px); } "
                                     "button { filter: linear-blur(8px); } label { --blur-radius: 4px; "
                                     "filter: blur(var(--blur-radius)); backdrop-filter: none; }";
    constexpr char kGradientStyles[] = "panel { background-image: radial-gradient(circle at 25% 75%, "
                                       "#ffffffff, #00000000 80%); border-width: 3px; } button { border-width: 2px; border-style: solid; } "
                                       "input { background-image: repeating-radial-gradient(ellipse at center, "
                                       "#ffffffff 0%, #000000ff 25%); } floater { background-image: "
                                       "repeating-conic-gradient(from .25turn, #ffffffff 0deg 30deg, "
                                       "#000000ff 60deg); } panel.solid { background-color: #112233ff; "
                                       "border-color: #445566ff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxEffectStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    const Gradient* boxGradient = style.backgroundLayers.front().image.gradient();
    ASSERT_NE(boxGradient, nullptr);
    EXPECT_EQ(boxGradient->kind, GradientKind::Linear);
    EXPECT_EQ(boxGradient->angleDegrees, 90.f);
    EXPECT_EQ(boxGradient->stops.size(), std::size_t(3));
    EXPECT_NEAR(boxGradient->stops[1].position, .75f, 1.0e-4f);
    EXPECT_NEAR(boxGradient->stops[1].color.resolvedColor().a, .5f, 1.0e-4f);
    EXPECT_EQ(style.boxShadow().size(), std::size_t(2));
    EXPECT_EQ(style.boxShadow()[0].blur, 0.f);
    EXPECT_FALSE(style.boxShadow()[0].inset);
    EXPECT_EQ(style.boxShadow()[1].spread, 6.f);
    EXPECT_TRUE(style.boxShadow()[1].inset);
    EXPECT_EQ(style.outline().width, 2.f);
    EXPECT_EQ(style.outline().offset.pixels, 3.f);
    EXPECT_NEAR(style.outline().color.resolvedColor().r, 18.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {"light"}).outline().color.resolvedColor().r, 171.f / 255.f, 1.0e-4f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).outline().offset.pixels, 0.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).outline().style, radia::ui::OutlineStyle::Dashed);

    ASSERT_TRUE(stylesheet.loadRadia(kFilterStyles).ok());
    const ComputedStyle filters = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(filters.backdropFilter().operations.size(), std::size_t(1));
    const LinearBlurFilter* backdrop = std::get_if<LinearBlurFilter>(&filters.backdropFilter().operations.front());
    ASSERT_NE(backdrop, nullptr);
    ASSERT_EQ(backdrop->stops.size(), std::size_t(2));
    EXPECT_EQ(backdrop->stops[0].stdDeviation, 0.f);
    EXPECT_EQ(backdrop->stops[1].stdDeviation, 16.f);
    EXPECT_EQ(backdrop->stops[0].position, .25f);
    EXPECT_EQ(backdrop->stops[1].position, .75f);
    EXPECT_EQ(backdrop->angleDegrees, 180.f);
    ASSERT_EQ(filters.filter.operations.size(), std::size_t(2));
    const LinearBlurFilter* progressive = std::get_if<LinearBlurFilter>(&filters.filter.operations[0]);
    ASSERT_NE(progressive, nullptr);
    ASSERT_EQ(progressive->stops.size(), std::size_t(2));
    EXPECT_EQ(progressive->stops[0].stdDeviation, 2.f);
    EXPECT_EQ(progressive->stops[1].stdDeviation, 6.f);
    EXPECT_EQ(progressive->stops[0].position, .25f);
    EXPECT_EQ(progressive->stops[1].position, .75f);
    EXPECT_EQ(progressive->angleDegrees, 90.f);
    const BlurFilter* uniform = std::get_if<BlurFilter>(&filters.filter.operations[1]);
    ASSERT_NE(uniform, nullptr);
    EXPECT_EQ(uniform->stdDeviation, 4.f);
    const ComputedStyle button = stylesheet.resolve("button", "", {});
    const LinearBlurFilter* uniformLinear = std::get_if<LinearBlurFilter>(&button.filter.operations.front());
    ASSERT_NE(uniformLinear, nullptr);
    ASSERT_EQ(uniformLinear->stops.size(), std::size_t(2));
    EXPECT_EQ(uniformLinear->stops[0].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[1].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[0].position, 0.f);
    EXPECT_EQ(uniformLinear->stops[1].position, 1.f);
    const ComputedStyle label = stylesheet.resolve("label", "", {});
    ASSERT_EQ(label.filter.operations.size(), std::size_t(1));
    const BlurFilter* nested = std::get_if<BlurFilter>(&label.filter.operations.front());
    ASSERT_NE(nested, nullptr);
    EXPECT_EQ(nested->stdDeviation, 4.f);
    EXPECT_TRUE(label.backdropFilter().isNone());

    ASSERT_TRUE(stylesheet.loadRadia(kGradientStyles).ok());
    const ComputedStyle radial = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(radial.backgroundLayers.size(), 1U);
    const Gradient* radialGradient = radial.backgroundLayers.front().image.gradient();
    ASSERT_NE(radialGradient, nullptr);
    EXPECT_EQ(radialGradient->kind, GradientKind::Radial);
    EXPECT_EQ(radialGradient->radialShape, RadialGradientShape::Circle);
    EXPECT_NEAR(radialGradient->center.x, .25f, 1.0e-4f);
    EXPECT_NEAR(radialGradient->center.y, .25f, 1.0e-4f);
    EXPECT_EQ(radial.borderWidth().top.pixels, 3.f);
    const ComputedStyle repeatingBorder = stylesheet.resolve("button", "", {});
    EXPECT_EQ(repeatingBorder.borderWidth().left.pixels, 2.f);
    const ComputedStyle repeatingRadial = stylesheet.resolve("input", "", {});
    ASSERT_EQ(repeatingRadial.backgroundLayers.size(), 1U);
    const Gradient* repeatingRadialGradient = repeatingRadial.backgroundLayers.front().image.gradient();
    ASSERT_NE(repeatingRadialGradient, nullptr);
    EXPECT_EQ(repeatingRadialGradient->kind, GradientKind::Radial);
    EXPECT_TRUE(repeatingRadialGradient->repeating);
    const ComputedStyle repeatingConic = stylesheet.resolve("floater", "", {});
    ASSERT_EQ(repeatingConic.backgroundLayers.size(), 1U);
    const Gradient* repeatingConicGradient = repeatingConic.backgroundLayers.front().image.gradient();
    ASSERT_NE(repeatingConicGradient, nullptr);
    EXPECT_EQ(repeatingConicGradient->kind, GradientKind::Conic);
    EXPECT_TRUE(repeatingConicGradient->repeating);
    EXPECT_EQ(repeatingConicGradient->angleDegrees, 90.f);
    const ComputedStyle solidOverride = stylesheet.resolve("panel", "", {"solid"});
    ASSERT_EQ(solidOverride.backgroundLayers.size(), 1U);
    ASSERT_NE(solidOverride.backgroundLayers.front().image.gradient(), nullptr);
    EXPECT_NEAR(solidOverride.backgroundColor().resolvedColor().r, 17.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(solidOverride.borderColor().top.resolvedColor().g, 85.f / 255.f, 1.0e-4f);
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

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 2U);
    ASSERT_NE(style.backgroundLayers[0].image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers[0].image.resource(), "icons/search.svg");
    ASSERT_NE(style.backgroundLayers[1].image.gradient(), nullptr);
    EXPECT_NEAR(style.backgroundLayers[0].position.y.percent, .8f, 1.0e-4f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeType::Cover);
    EXPECT_EQ(style.backgroundLayers[1].size.mode, BackgroundSizeType::Explicit);
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::NoRepeat);
    EXPECT_EQ(style.backgroundLayers[1].repeat, BackgroundRepeat::RepeatX);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Fixed);
    EXPECT_EQ(style.backgroundLayers[1].attachment, BackgroundAttachment::Local);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    ASSERT_NE(style.maskLayers[0].image.image.resource(), nullptr);
    EXPECT_EQ(*style.maskLayers[0].image.image.resource(), "icons/mask.svg");
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

TEST(StyleSheetTest, PublishesBorderImageResource) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { border-image-source: url(images/border.png); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_NE(style.borderImageSource().resource(), nullptr);
    EXPECT_EQ(*style.borderImageSource().resource(), "images/border.png");

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 1U);
    EXPECT_EQ(references.front().value, "images/border.png");
    EXPECT_FALSE(references.front().optional);
    EXPECT_FALSE(references.front().cursor);
}

TEST(StyleSheetTest, ParsesBorderImageGradient) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { border-image-source: linear-gradient(to right, #000, #fff); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    const Gradient* gradient = style.borderImageSource().gradient();
    ASSERT_NE(gradient, nullptr);
    EXPECT_EQ(gradient->kind, GradientKind::Linear);
    EXPECT_EQ(gradient->stops.size(), 2U);
}

TEST(StyleSheetTest, NegativeShadowSpread) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { box-shadow: 0 0 2px -4px #000; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    const auto& shadows = style.boxShadow();
    ASSERT_EQ(shadows.size(), 1U);
    EXPECT_FLOAT_EQ(shadows.front().blur, 2.f);
    EXPECT_FLOAT_EQ(shadows.front().spread, -4.f);
}

TEST(StyleSheetTest, PublishesDeferredResourceReferences) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia(":root { --background-image: url(icons/variable.svg); --mask-image: url(icons/variable-mask.svg); "
                               "--pointer: url(cursors/variable.cur), pointer; --border-image: url(images/variable-border.png); } "
                               "i { background-image: var(--background-image); mask-image: var(--mask-image); cursor: var(--pointer); "
                               "border-image-source: var(--border-image); }")
                    .ok());

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 4U);
    EXPECT_EQ(references[0].value, "icons/variable.svg");
    EXPECT_FALSE(references[0].optional);
    EXPECT_EQ(references[1].value, "icons/variable-mask.svg");
    EXPECT_TRUE(references[1].optional);
    EXPECT_EQ(references[2].value, "cursors/variable.cur");
    EXPECT_FALSE(references[2].optional);
    EXPECT_TRUE(references[2].cursor);
    EXPECT_EQ(references[3].value, "images/variable-border.png");
    EXPECT_FALSE(references[3].optional);
    EXPECT_FALSE(references[3].cursor);
}

TEST(StyleSheetTest, ParsesMaskShorthand) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { mask: url(icons/mask.svg) alpha; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.maskLayers.size(), 1U);
    ASSERT_NE(style.maskLayers.front().image.image.resource(), nullptr);
    EXPECT_EQ(*style.maskLayers.front().image.image.resource(), "icons/mask.svg");
    EXPECT_EQ(style.maskLayers.front().mode, MaskMode::Alpha);
    EXPECT_EQ(style.maskLayers.front().composite, MaskComposite::Add);
    EXPECT_EQ(style.maskLayers.front().type, MaskType::Alpha);
}

TEST(StyleSheetTest, DecodesEscapedImageURLsAndFunctionNames) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(R"(i[data="a\\"] {} i { background-image: url(icons/a\20 b.svg); })").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    ASSERT_NE(style.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers.front().image.resource(), "icons/a b.svg");

    StyleSheet escapedFunction;
    ASSERT_TRUE(escapedFunction.loadRadia(R"(i { background-image: u\72l(icons/search.svg); })").ok());
    ASSERT_EQ(escapedFunction.resolve("i", "", {}).backgroundLayers.size(), 1U);
    ASSERT_NE(escapedFunction.resolve("i", "", {}).backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*escapedFunction.resolve("i", "", {}).backgroundLayers.front().image.resource(), "icons/search.svg");
}

TEST(StyleSheetTest, DecodesEscapedValueFunctionsAndUnits) {
    constexpr char kStyles[] = R"(:root { --accent: #204060; } panel {
        background-color: v\61 r(--accent);
        filter: bl\75 r(4px);
        width: 17p\78;
    })";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_NEAR(style.backgroundColor().resolvedColor().r, 32.f / 255.f, 1.0e-4f);
    ASSERT_EQ(style.filter.operations.size(), std::size_t(1));
    const BlurFilter* filter = std::get_if<BlurFilter>(&style.filter.operations.front());
    ASSERT_NE(filter, nullptr);
    EXPECT_EQ(filter->stdDeviation, 4.f);
    EXPECT_EQ(style.width().pixels(), 17.f);
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
    const ComputedStyle style = outOfBounds.resolve("i", "", {});
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
    const ComputedStyle longhandStyle = longhand.resolve("i", "", {});
    ASSERT_EQ(longhandStyle.backgroundLayers.size(), 1U);
    ASSERT_NE(longhandStyle.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*longhandStyle.backgroundLayers.front().image.resource(), "images/pattern.png");

    StyleSheet shorthand;
    ASSERT_TRUE(shorthand.loadRadia("i { background: url(images/pattern.png); }").ok());
    const ComputedStyle shorthandStyle = shorthand.resolve("i", "", {});
    ASSERT_EQ(shorthandStyle.backgroundLayers.size(), 1U);
    ASSERT_NE(shorthandStyle.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*shorthandStyle.backgroundLayers.front().image.resource(), "images/pattern.png");
}

TEST(StyleSheetTest, ParsesBackgroundShorthandAfterSlash) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icons/search.svg) center / contain no-repeat; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    ASSERT_NE(style.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers.front().image.resource(), "icons/search.svg");
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, .5f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, .5f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeType::Contain);
    EXPECT_EQ(style.backgroundLayers.front().repeat, BackgroundRepeat::NoRepeat);
}

TEST(StyleSheetTest, UsesCSSImageInitialValues) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icon.svg); mask-image: url(mask.svg); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeType::Auto);
    EXPECT_FALSE(style.backgroundLayers[0].size.width.has_value());
    EXPECT_FALSE(style.backgroundLayers[0].size.height.has_value());
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Scroll);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.y.percent, 1.f);
    EXPECT_EQ(style.maskLayers[0].image.size.mode, BackgroundSizeType::Auto);
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

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_EQ(style.backgroundLayers.front().image.value.index(), 0U);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeType::Auto);
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
    EXPECT_EQ(stylesheet.resolve("i", "", {}).backgroundLayers.front().repeat, BackgroundRepeat::RepeatX);
}

TEST(StyleSheetTest, NormalizesImageLonghandsRegardlessOfDeclarationOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("i { background-position: right, left; background-attachment: fixed, local; "
                               "background-image: url(one.svg), url(two.svg); mask-position: right, left; "
                               "mask-image: url(one.svg), url(two.svg); }")
                    .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
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
    style.maskLayers[0].image.image = StyleImage{Gradient{}};
    style.maskLayers[0].image.image.gradient()->stops = {{Color(1.f, 1.f, 1.f, .8f), 0.f}, {Color(1.f, 1.f, 1.f, .4f), 1.f}};

    resolveStyleColors(style, Color{});
    radia::ui::applyOpacity(style, .5f);

    EXPECT_FLOAT_EQ(style.opacity().value, .5f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.image.gradient()->stops[0].color.resolvedColor().a, .8f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.image.gradient()->stops[1].color.resolvedColor().a, .4f);
}

TEST(StyleSheetTest, SkipsInvalidFilters) {
    const char* invalidSources[] = {
        "panel { background-color: linear-gradient(#fff); }",
        "panel { background-color: radial-gradient(square, #fff, #000); }",
        "panel { border-color: conic-gradient(from nowhere, #fff, #000); }",
        "panel { border: 1px repeating-linear-gradient(#fff 20%, #000 20%); }",
        "panel { box-shadow: 0 0 -1px #000; }",
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

TEST(StyleSheetTest, ValidatesMinSize) {
    constexpr char kMinSizeStyles[] = "panel.one { min-size: 24px; } panel.two { min-size: 30% 80px; } "
                                      "panel.longhand-after { min-size: 10px 20px; min-width: 40px; } "
                                      "panel.shorthand-after { min-height: 5px; min-size: 12px 18px; } "
                                      "panel.auto { min-size: auto; } panel.none { max-size: none; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMinSizeStyles).ok());

    const ComputedStyle one = stylesheet.resolve("panel", "", {"one"});
    ASSERT_TRUE(one.minHeight().has_value());
    ASSERT_TRUE(one.minWidth().has_value());
    EXPECT_EQ(one.minHeight()->pixels(), 24.f);
    EXPECT_EQ(one.minWidth()->pixels(), 24.f);

    const ComputedStyle two = stylesheet.resolve("panel", "", {"two"});
    ASSERT_TRUE(two.minHeight().has_value());
    ASSERT_TRUE(two.minWidth().has_value());
    EXPECT_NEAR(two.minHeight()->resolve(0.f, 1.f), .3f, 1.0e-4f);
    EXPECT_EQ(two.minWidth()->pixels(), 80.f);

    const ComputedStyle longhandAfter = stylesheet.resolve("panel", "", {"longhand-after"});
    ASSERT_TRUE(longhandAfter.minWidth().has_value());
    ASSERT_TRUE(longhandAfter.minHeight().has_value());
    EXPECT_EQ(longhandAfter.minWidth()->pixels(), 40.f);
    EXPECT_EQ(longhandAfter.minHeight()->pixels(), 10.f);

    const ComputedStyle shorthandAfter = stylesheet.resolve("panel", "", {"shorthand-after"});
    ASSERT_TRUE(shorthandAfter.minHeight().has_value());
    ASSERT_TRUE(shorthandAfter.minWidth().has_value());
    EXPECT_EQ(shorthandAfter.minHeight()->pixels(), 12.f);
    EXPECT_EQ(shorthandAfter.minWidth()->pixels(), 18.f);

    const ComputedStyle automatic = stylesheet.resolve("panel", "", {"auto"});
    EXPECT_FALSE(automatic.minHeight().has_value());
    EXPECT_FALSE(automatic.minWidth().has_value());

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"});
    EXPECT_FALSE(none.maxHeight().has_value());
    EXPECT_FALSE(none.maxWidth().has_value());

    struct InvalidMinSizeCase {
        const char* name;
        const char* styles;
    };
    const InvalidMinSizeCase invalidCases[] = {
        {"negative length", "panel { min-size: -1px; }"},
        {"too many values", "panel { min-size: 1px 2px 3px; }"},
        {"max auto", "panel { max-size: auto; }"},
    };
    for (const auto& test : invalidCases) {
        SCOPED_TRACE(Message() << "invalid min-size case: " << test.name);
        StyleSheet invalidStylesheet;
        const auto result = invalidStylesheet.loadRadia(test.styles, "min-size.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }

    const ComputedStyle preservedStyle = stylesheet.resolve("panel", "", {"one"});
    ASSERT_TRUE(preservedStyle.minWidth().has_value());
    EXPECT_EQ(preservedStyle.minWidth()->pixels(), 24.f);
}

TEST(StyleSheetTest, CopiesStylesheetState) {
    constexpr char kOriginalStyles[] = "panel { width: 10px; }";
    constexpr char kReplacementStyles[] = "panel { width: 20px; }";

    StyleSheet original;
    ASSERT_TRUE(original.loadRadia(kOriginalStyles, "original.css").ok());
    const std::uint64_t copiedGeneration = original.generation();

    StyleSheet copy = original;
    ASSERT_TRUE(original.loadRadia(kReplacementStyles, "replacement.css").ok());
    EXPECT_EQ(copy.resolve("panel", "", {}).width().pixels(), 10.f);
    EXPECT_EQ(copy.generation(), copiedGeneration);
    EXPECT_EQ(original.resolve("panel", "", {}).width().pixels(), 20.f);

    StyleSheet assigned;
    assigned = copy;
    EXPECT_EQ(assigned.resolve("panel", "", {}).width().pixels(), 10.f);

    StyleSheet moved = std::move(assigned);
    EXPECT_EQ(moved.resolve("panel", "", {}).width().pixels(), 10.f);
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
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(resolved.width().pixels(), 20.f);
    EXPECT_EQ(resolved.height().pixels(), 30.f);

    const auto malformed = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"base/skin.css", kBaseLayerStyles}},
        {StyleOrigin::Skin, {"derived/skin.css", kMalformedLayerStyles}},
    });
    ASSERT_TRUE(malformed.ok());
    ASSERT_FALSE(malformed.warnings.empty());
    EXPECT_EQ(malformed.warnings.front().source, "derived/skin.css");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 10.f);
}

TEST(StyleSheetTest, OverridesDefaultRule) {
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Skin, {"skin.css", "panel { width: 20px; }"}},
        {StyleOrigin::UserAgent, {"ua.css", "panel.primary { width: 10px; }"}},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"primary"}).width().pixels(), 20.f);
}

TEST(StyleSheetTest, RestrictsInternalAlignment) {
    StyleSheet stylesheet;
    const auto skinOnly = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(skinOnly.ok());
    ASSERT_EQ(skinOnly.warnings.size(), std::size_t(1));
    EXPECT_EQ(skinOnly.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_FALSE(stylesheet.resolve("button", "", {}).alignContentBlockCenter);

    const auto defaultAndSkin = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: normal; }"}},
        {StyleOrigin::UserAgent, {"ua.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(defaultAndSkin.ok());
    ASSERT_EQ(defaultAndSkin.warnings.size(), std::size_t(1));
    EXPECT_EQ(defaultAndSkin.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_TRUE(stylesheet.resolve("button", "", {}).alignContentBlockCenter);
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
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(resolved.width().pixels(), 30.f);
    EXPECT_EQ(resolved.height().pixels(), 18.f);

    const auto& dependencies = stylesheet.dependencies();
    ASSERT_TRUE(dependencies.contains("theme/main.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/panel.css"));
    ASSERT_TRUE(dependencies.contains("theme/components/panel.css"));
    EXPECT_TRUE(dependencies.at("theme/components/panel.css").contains("theme/foundation/sizes.css"));
}

TEST(StyleSheetTest, ImportOrder) {
    constexpr char kEntrypointStyles[] = "@import \"first.css\"; @import \"second.css\";";

    ResourceLayer layer{"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"first.css", ".foo { width: 11px; }"},
        {"second.css", ".foo { width: 22px; }"},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer}}).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"foo"}).width().pixels(), 22.f);
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 17.f);
}

TEST(StyleSheetTest, DoesNotTreatCDOAsImportWhitespace) {
    constexpr char kStyles[] = "@import \"missing.css\" <!--; panel { width: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "import.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheetTest, RecoversMalformedImportAtRuleAtItsBoundary) {
    constexpr char kStyles[] = "@import \"missing.css\" panel { width: 99px; } label { height: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "theme/main.css");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.syntax");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 17.f);
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    auto cycle = layer(kCycleImport);
    cycle.modules["cycle.css"] = kCycleModule;
    const auto cycleResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, cycle}});
    ASSERT_FALSE(cycleResult.ok());
    ASSERT_FALSE(cycleResult.errors.empty());
    EXPECT_EQ(cycleResult.errors.front().code, "stylesheet.import.cycle");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    const auto traversalResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, layer(kTraversalImport)}});
    ASSERT_FALSE(traversalResult.ok());
    ASSERT_FALSE(traversalResult.errors.empty());
    EXPECT_EQ(traversalResult.errors.front().code, "stylesheet.import.path_invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    auto malformed = layer(kMalformedImport);
    malformed.modules["broken.css"] = kMalformedModule;
    const auto malformedResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, malformed}});
    ASSERT_TRUE(malformedResult.ok());
    ASSERT_FALSE(malformedResult.warnings.empty());
    EXPECT_EQ(malformedResult.warnings.front().source, "theme/broken.css");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());

    auto late = layer(kLateImport);
    late.modules["late.css"] = kLateModule;
    const auto lateResult = stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, late}});
    ASSERT_TRUE(lateResult.ok());
    ASSERT_FALSE(lateResult.warnings.empty());
    EXPECT_EQ(lateResult.warnings.front().code, "stylesheet.import.order");

    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 1.f);
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
    EXPECT_EQ(stylesheet.resolve("button", "", {}).width().pixels(), 23.f);

    ASSERT_TRUE(stylesheet.loadRadia(kUnderscoreSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad_id", {}).width().pixels(), 29.f);

    ASSERT_TRUE(stylesheet.loadRadia(kEscapedIdSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad.id", {}).width().pixels(), 31.f);

    ASSERT_TRUE(stylesheet.loadRadia(kEscapedColonSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "bad:id", {}).width().pixels(), 37.f);

    ASSERT_TRUE(stylesheet.loadRadia(kHexEscapedIdSelector).ok());
    EXPECT_EQ(stylesheet.resolve("button", "123:bad.id", {}).width().pixels(), 41.f);

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
    EXPECT_EQ(chord.padding().left.pixels, 1.f);
    EXPECT_EQ(chord.rowGap().fixedPixels(), 5.f);
    EXPECT_EQ(key.padding().left.pixels, 2.f);
    EXPECT_EQ(key.borderRadius().topLeft.horizontal.pixels, 3.f);
    EXPECT_EQ(key.borderRadius().topLeft.vertical.pixels, 3.f);
    EXPECT_NE(key.rowGap().fixedPixels(), 5.f);

    const auto rejectedPseudoElement = stylesheet.loadRadia(kRejectedKbdPseudoElement);
    ASSERT_TRUE(rejectedPseudoElement.ok());
    ASSERT_FALSE(rejectedPseudoElement.warnings.empty());
    EXPECT_EQ(rejectedPseudoElement.warnings.front().code, "stylesheet.selector.pseudo_element_unknown");
}

TEST(StyleSheetTest, NestedOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button { color: #ff0000ff; & { color: #00ff00ff; } color: #0000ffff; }").ok());

    const ComputedStyle style = stylesheet.resolve("button", "", {});
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 1.f);
}

TEST(StyleSheetTest, PreservesRulesAcrossCSSTokenBoundaries) {
    const std::string source =
        R"(panel { width: 11px; content: "}"; background-image: url(icon\)name.svg); background-color: rgb(1, 2, 3); } /* } ; */ label { height: 13px; })";
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(source, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.errors.empty());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    ASSERT_TRUE(stylesheet.resolve("panel", "", {}).content.has_value());
    EXPECT_EQ(*stylesheet.resolve("panel", "", {}).content, "}");
    ASSERT_EQ(stylesheet.resolve("panel", "", {}).backgroundLayers.size(), std::size_t(1));
    ASSERT_NE(stylesheet.resolve("panel", "", {}).backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*stylesheet.resolve("panel", "", {}).backgroundLayers.front().image.resource(), "icon)name.svg");
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheetTest, DiscardsClosedCommentsWithoutChangingSelectors) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button/**/.primary { width: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {"primary"}).width().pixels(), 13.f);
}

TEST(StyleSheetTest, ReportsUnclosedComments) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { width: 13px; } /* unclosed", "comment.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_comment");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsValidDeclarationsAroundInvalidDeclaration) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel {\n  width: 11px;\n  malformed;\n  height: 13px;\n}", "declarations.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(result.warnings.front().line, std::size_t(3));
    EXPECT_EQ(result.warnings.front().column, std::size_t(3));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsQualifiedRulePreludeTogether) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("bogus; label { height: 13px; }", "qualified.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t{1});
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.element_unknown");
    EXPECT_TRUE(stylesheet.resolve("label", "", {}).height().isAuto());
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
        EXPECT_TRUE(stylesheet.resolve(test.target, "", {}).height().isAuto());
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
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheetTest, KeepsEarlierRangeValuesBeforeAnUnclosedRule) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { min-size: 11px 13px; }\nlabel { height: 13px;", "unclosed.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_block");
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_TRUE(style.minHeight().has_value());
    ASSERT_TRUE(style.minWidth().has_value());
    EXPECT_EQ(style.minHeight()->pixels(), 11.f);
    EXPECT_EQ(style.minWidth()->pixels(), 13.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheetTest, SkipsAnInvalidSelectorListAsAWhole) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel, button::unknown { width: 99px; } panel { height: 13px; }");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_unknown");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 13.f);
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

TEST(StyleSheetTest, BorderPseudoLayout) {
    constexpr char kStateBorderStyles[] = "fieldset { border: 1px solid #ffffff; } "
                                          "fieldset:hover { border: 4px solid #ffffff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStateBorderStyles).ok());
    EXPECT_TRUE(stylesheet.pseudoClassAffectsLayout(CSSPseudoClass::Hover));
}

TEST(StyleSheetTest, AppearancePseudoLayout) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:hover { appearance: none; }").ok());
    EXPECT_TRUE(stylesheet.pseudoClassAffectsLayout(CSSPseudoClass::Hover));
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

    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*fieldset).opacity().value, 1.f);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity().value, 1.f);

    fieldset->disabled(true);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*fieldset).opacity().value, .25f);
    EXPECT_TRUE(normalButtonPtr->disabled());
    EXPECT_FALSE(legendButtonPtr->disabled());
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity().value, .5f);
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*legendButtonPtr).opacity().value, 1.f);

    fieldset->disabled(false);
    EXPECT_FALSE(normalButtonPtr->disabled());
    EXPECT_FLOAT_EQ(stylesheet.resolveElement(*normalButtonPtr).opacity().value, 1.f);
}

TEST(StyleSheetTest, FontFaceOrder) {
    constexpr char kEntrypoint[] = R"css(@import "typography.css";
@font-face { font-family: MainSans; src: url(fonts/main.bin); }
@font-face { font-family: "Main Sans Bold"; src: url("fonts/main-bold.bin"); font-weight: bold; }
)css";
    constexpr char kTypography[] = R"css(@font-face {
  font-family: "Imported Sans";
  src: url("fonts/imported.woff2") format("woff2"), url(fonts/fallback.bin);
  font-style: italic;
  font-weight: 650;
  font-width: 87.5%;
}
)css";

    ResourceLayer layer{"theme/skin.css", kEntrypoint, "skin.css", {{"typography.css", kTypography}}};
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer{StyleOrigin::Skin, std::move(layer)}}).ok());

    const std::vector<radia::ui::FontFace>& fontFaces = stylesheet.fontFaces();
    ASSERT_EQ(fontFaces.size(), 3U);
    EXPECT_EQ(fontFaces[0].family, "Imported Sans");
    EXPECT_EQ(fontFaces[0].selection.style, radia::ui::FontStyle::Italic);
    EXPECT_FLOAT_EQ(fontFaces[0].selection.weight.value, 650.f);
    EXPECT_FLOAT_EQ(fontFaces[0].selection.width.percentage, 87.5f);
    ASSERT_EQ(fontFaces[0].sources.size(), 2U);
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(fontFaces[0].sources[0].value).url, "fonts/imported.woff2");
    EXPECT_EQ(fontFaces[0].sources[0].sourceName, "theme/typography.css");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(fontFaces[0].sources[1].value).url, "fonts/fallback.bin");
    EXPECT_EQ(fontFaces[1].family, "MainSans");
    EXPECT_EQ(fontFaces[2].family, "Main Sans Bold");
    EXPECT_FLOAT_EQ(fontFaces[2].selection.weight.value, 700.f);
}

TEST(StyleSheetTest, FontSources) {
    constexpr char kStyles[] = R"css(
@font-face {
  font-family: Supported;
  src: local("Missing"), url(unknown.bin) format("future-format"),
       url(fallback.woff2) format(woff2) tech(features-opentype, color-COLRv1),
       url(variations.woff2) tech(variations);
}
)css";
    constexpr char kUnsupported[] = "@font-face { font-family: Unsupported; src: url(unsupported.bin) format(\"future-format\"); }";
    constexpr char kMalformed[] = "@font-face { font-family: Malformed; src: url(malformed.woff2) unexpected; }";

    radia::ui::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");
    ASSERT_EQ(stylesheet.fontFaces().front().sources.size(), 2U);
    ASSERT_TRUE(std::holds_alternative<radia::ui::FontFaceLocal>(stylesheet.fontFaces().front().sources.front().value));
    EXPECT_EQ(std::get<radia::ui::FontFaceLocal>(stylesheet.fontFaces().front().sources.front().value).name, "Missing");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(stylesheet.fontFaces().front().sources.back().value).url, "fallback.woff2");

    const radia::ui::StyleSheetLoadResult rejected = stylesheet.loadRadia(kUnsupported);
    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "stylesheet.font_face.source_unsupported");
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");

    const radia::ui::StyleSheetLoadResult malformed = stylesheet.loadRadia(kMalformed);
    ASSERT_FALSE(malformed.ok());
    ASSERT_FALSE(malformed.errors.empty());
    EXPECT_EQ(malformed.errors.front().code, "stylesheet.font_face.source_unsupported");
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");
}

TEST(StyleSheetTest, LocalFontSources) {
    constexpr char kStyles[] = "@font-face { font-family: Local; src: local(\"Full Name\"), local(PostScript Name), url(local.woff2); }";

    radia::ui::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    const auto& sources = stylesheet.fontFaces().front().sources;
    ASSERT_EQ(sources.size(), 3U);
    ASSERT_TRUE(std::holds_alternative<radia::ui::FontFaceLocal>(sources[0].value));
    EXPECT_EQ(std::get<radia::ui::FontFaceLocal>(sources[0].value).name, "Full Name");
    ASSERT_TRUE(std::holds_alternative<radia::ui::FontFaceLocal>(sources[1].value));
    EXPECT_EQ(std::get<radia::ui::FontFaceLocal>(sources[1].value).name, "PostScript Name");
    ASSERT_TRUE(std::holds_alternative<radia::ui::FontFaceURL>(sources[2].value));
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(sources[2].value).url, "local.woff2");
}

TEST(StyleSheetTest, FontFaceMatchOrder) {
    constexpr char kStyles[] = R"css(
@font-face { font-family: Match; src: url(narrow-bold.woff2); font-weight: 700; font-width: 90%; }
@font-face { font-family: Match; src: url(italic.woff2); font-style: italic; font-weight: 700; }
@font-face { font-family: Match; src: url(weight-500.woff2); font-weight: 500; }
@font-face { font-family: Match; src: url(first-400.woff2); font-weight: 400; }
@font-face { font-family: Match; src: url(last-400.woff2); font-weight: 400; }
)css";

    radia::ui::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    const radia::ui::FontSelectionRequest request{radia::ui::FontWeight{700.f}, radia::ui::FontWidth{100.f}, radia::ui::FontStyle::Normal};

    const std::vector<const radia::ui::FontFace*> faces = radia::ui::fontFacesInMatchOrder(stylesheet.fontFaces(), "match", request);

    ASSERT_EQ(faces.size(), 5U);
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(faces[0]->sources[0].value).url, "weight-500.woff2");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(faces[1]->sources[0].value).url, "last-400.woff2");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(faces[2]->sources[0].value).url, "first-400.woff2");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(faces[3]->sources[0].value).url, "italic.woff2");
    EXPECT_EQ(std::get<radia::ui::FontFaceURL>(faces[4]->sources[0].value).url, "narrow-bold.woff2");
}
