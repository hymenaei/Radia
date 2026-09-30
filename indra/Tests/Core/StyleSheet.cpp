/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/CSSRules.h>
#include <Core/Color.h>
#include <Core/Document.h>
#include <Core/ElementInternal.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/HTMLInputElement.h>
#include <Core/HTMLLabelElement.h>
#include <Core/HTMLPanelElement.h>
#include <Core/StylePass.h>
#include <Core/StyleSheet.h>
#include <Core/Text.h>
#include <Core/TextMeasurer.h>
#include <cstddef>
#include <gtest/gtest.h>
#include <string>
#include <utility>
#include <variant>

namespace {
using Core::Color;
using Core::Element;
using Core::FixedTextMeasurer;
using Core::HTMLButtonElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::CSS::StyleSheet;
using Core::detail::makeElement;
using Core::detail::makeElementValue;
using Core::Style::AccentColor;
using Core::Style::ColorSchemeMode;
using Core::Style::ComputedStyle;
using Core::Style::CursorStyle;
using Core::Style::FontFamilies;
using Core::Style::GenericFontFamily;
using Core::Style::LineHeight;
using Core::Style::Overflow;
using Core::Style::Pass;
using Core::Style::PointerEvents;
using Core::Style::ScrollbarGutter;
using Core::Style::ScrollbarWidth;
using Core::Style::TextAlign;
using Core::Style::TextDecoration;
using Core::Style::TextWrapMode;
using Core::Style::TextWrapStyle;
using Core::Style::VerticalAlign;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    Pass styles(stylesheet, FixedTextMeasurer {});
    return styles.style(element);
}

constexpr char kColorTokenStyles[] = ":root { --accent: hsl(120 100% 50%); --ink: rgb(255, 0, 0, 50%); } "
                                     "button { background-color: var(--accent); color: var(--ink); "
                                     "font-size: 17px; } "
                                     "label { color: #00ff00ff; font-size: 29px; }";
} // namespace

TEST(StyleSheet, ResolvesInheritedColors) {
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

TEST(StyleSheet, FontSizes) {
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

    Pass styles(stylesheet, FixedTextMeasurer {});
    EXPECT_EQ(styles.style(panel).fontSize(), 26.f);
    EXPECT_EQ(styles.style(*labelPointer).fontSize(), 26.f);
    EXPECT_EQ(styles.style(*textPointer).fontSize(), 52.f);
}

TEST(StyleSheet, ResolvesMatchParentTextAlignment) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { text-align: right; } panel label { text-align: match-parent; }").ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    panel.append(std::move(label));

    EXPECT_EQ(computedStyle(stylesheet, panel).textAlign(), TextAlign::Right);
    EXPECT_EQ(computedStyle(stylesheet, *labelPointer).textAlign(), TextAlign::Right);
}

TEST(StyleSheet, StartsWithoutImplicitCoreRules) {
    StyleSheet stylesheet;
    const ComputedStyle paragraph = stylesheet.resolve("p", "", {});

    EXPECT_FALSE(paragraph.displaySet);
    EXPECT_EQ(paragraph.fontWeight().value, 400.f);
}

TEST(StyleSheet, KeepsValidDeclarationsAroundInvalidColor) {
    constexpr char kInvalidColorStyles[] = "input { background-color: ##invalid; width: 8px; }";

    StyleSheet stylesheet;
    const auto invalid = stylesheet.loadRadia(kInvalidColorStyles, "invalid.css");

    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(invalid.warnings.front().source, "invalid.css");
    EXPECT_EQ(stylesheet.resolve("input", "", {}).width().pixels(), 8.f);
}

TEST(StyleSheet, ParsesCommentSeparatedColorComponents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { color: rgb(1/**/2/**/3); }").ok());

    const Color color = stylesheet.resolve("panel", "", {}).color().resolvedColor();
    EXPECT_NEAR(color.r, 1.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.g, 2.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(color.b, 3.f / 255.f, 1.0e-6f);
}

TEST(StyleSheet, ParsesBoxShorthands) {
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

TEST(StyleSheet, ParsesGapLonghands) {
    StyleSheet stylesheet;
    ASSERT_TRUE(
        stylesheet.loadRadia("panel { row-gap: 13px; column-gap: 17px; } panel.normal { gap: normal; } panel.wide { gap: thick; }").ok());

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

TEST(StyleSheet, AcceptsUnitlessShadowZero) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { box-shadow: 0 0 0 #123456; }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(style.boxShadow().size(), std::size_t(1));
    EXPECT_EQ(style.boxShadow().front().horizontal, 0.f);
    EXPECT_EQ(style.boxShadow().front().vertical, 0.f);
    EXPECT_EQ(style.boxShadow().front().blur, 0.f);
}

TEST(StyleSheet, KeepsValidDeclarationsAroundUnknown) {
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

TEST(StyleSheet, RejectsInvalidDeclarationPropertyNames) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":root { --bad name: #ff0000; } panel { width: 13px; }", "property-name.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheet, AcceptsArbitraryCustomPropertyValues) {
    constexpr char kCustomPropertyStyles[] = ":root { --bad: nonsense; --empty:; --number: .1; --case-token: MiXeD; } "
                                             ":root { --block: { key: value; }; } "
                                             "panel.empty { width: var(--empty, 12px); } panel.fallback { width: var(--missing,); } "
                                             "panel.boundary { opacity: var(--number)0; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kCustomPropertyStyles, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"empty"}).width().isAuto());
    EXPECT_TRUE(stylesheet.resolve("panel", "", {"fallback"}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"boundary"}).opacity().value, 1.f);
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(style.customProperties.at("--case-token").source, "MiXeD");
    ASSERT_TRUE(style.customProperties.contains("--block"));
    EXPECT_EQ(style.customProperties.at("--block").source, "{ key: value; }");
}

TEST(StyleSheet, RejectsReferencesToMissingTokens) {
    constexpr char kMissingTokenStyles[] = "button { width: var(--missing); }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kMissingTokenStyles, "missing-token.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_TRUE(stylesheet.resolve("button", "", {}).width().isAuto());
}

TEST(StyleSheet, InvalidCustomPropertySubstitutionUsesUnsetValue) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { width: 24px; opacity: .5; } label.invalid { width: var(--missing); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    label.classList().add("invalid");

    const ComputedStyle style = computedStyle(stylesheet, label);
    EXPECT_TRUE(style.width().isAuto());
    EXPECT_EQ(style.opacity().value, .5f);
}

TEST(StyleSheet, ResolvesCustomPropertyFallbacksAndCycles) {
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

TEST(StyleSheet, DoesNotUseFallbackInsideCustomPropertyCycle) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --a: var(--b, 2px); --b: var(--a, 3px); width: var(--a, 7px); height: var(--a); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width().pixels(), 7.f);
    EXPECT_TRUE(style.height().isAuto());
}

TEST(StyleSheet, TreatsInitialCustomPropertyAsGuaranteedInvalid) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { --token: initial; width: var(--token, 17px); height: var(--token); }").ok());

    auto label = makeElementValue<HTMLLabelElement>();
    const ComputedStyle style = computedStyle(stylesheet, label);

    EXPECT_EQ(style.width().pixels(), 17.f);
    EXPECT_TRUE(style.height().isAuto());
}

TEST(StyleSheet, InheritsTextWrapLonghandsIndependently) {
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
    EXPECT_EQ(modeStyle.textWrapStyle(), Core::Style::TextWrapStyle::Pretty);
    EXPECT_EQ(styleStyle.textWrapMode(), TextWrapMode::NoWrap);
    EXPECT_EQ(styleStyle.textWrapStyle(), Core::Style::TextWrapStyle::Stable);
    EXPECT_EQ(inheritStyle.textWrapMode(), TextWrapMode::NoWrap);
    EXPECT_EQ(inheritStyle.textWrapStyle(), Core::Style::TextWrapStyle::Pretty);
}

TEST(StyleSheet, RejectsInvalidTextWrapWithoutPartialApplication) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("label { text-wrap: nowrap unknown; }");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");

    const ComputedStyle style = stylesheet.resolve("label", "", {});
    EXPECT_EQ(style.textWrapMode(), TextWrapMode::Wrap);
    EXPECT_EQ(style.textWrapStyle(), Core::Style::TextWrapStyle::Auto);
}

TEST(StyleSheet, PreservesCursorOnFailure) {
    constexpr char kCursorStyles[] = "button { cursor: pointer; } #horizontal { cursor: e-resize; } "
                                     "#diagonal { cursor: sw-resize; } #grab { cursor: grab; } "
                                     "#grabbing { cursor: grabbing; }";
    constexpr char kInvalidCursorStyles[] = "button { cursor: teleport; cursor: pointer; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kCursorStyles).ok());
    EXPECT_EQ(stylesheet.resolve("button", "", {}).cursor, Core::Style::CursorStyle::Pointer);
    EXPECT_EQ(stylesheet.resolve("panel", "horizontal", {}).cursor, Core::Style::CursorStyle::EastWestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "diagonal", {}).cursor, Core::Style::CursorStyle::NortheastSouthwestResize);
    EXPECT_EQ(stylesheet.resolve("panel", "grab", {}).cursor, Core::Style::CursorStyle::Grab);
    EXPECT_EQ(stylesheet.resolve("panel", "grabbing", {}).cursor, Core::Style::CursorStyle::Grabbing);
    EXPECT_NE(stylesheet.resolve("panel", "grab", {}).cursor, stylesheet.resolve("panel", "grabbing", {}).cursor);

    const auto invalid = stylesheet.loadRadia(kInvalidCursorStyles, "cursor.css");
    ASSERT_TRUE(invalid.ok());
    ASSERT_FALSE(invalid.warnings.empty());
    EXPECT_EQ(invalid.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_EQ(stylesheet.resolve("button", "", {}).cursor, Core::Style::CursorStyle::Pointer);
}

TEST(StyleSheet, InheritsAllowedProperties) {
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
    EXPECT_EQ(inheritedStyle.fontFamily(), FontFamilies {GenericFontFamily::SansSerif});
    EXPECT_EQ(inheritedStyle.fontSize(), 19.f);
    EXPECT_EQ(inheritedStyle.fontWeight().value, 700.f);
    EXPECT_EQ(inheritedStyle.fontStyle(), Core::Style::FontStyle::Italic);
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
    EXPECT_EQ(inheritedStyle.cursor, Core::Style::CursorStyle::Grab);
    EXPECT_EQ(inheritedStyle.opacity().value, 1.f);
    EXPECT_EQ(inheritedStyle.pointerEvents(), PointerEvents::NoneValue);
    EXPECT_EQ(inheritedStyle.backgroundColor().resolvedColor().a, 0.f);

    const ComputedStyle overriddenStyle = computedStyle(stylesheet, *overriddenLabel);
    EXPECT_EQ(overriddenStyle.fontSize(), 11.f);
    EXPECT_EQ(overriddenStyle.cursor, Core::Style::CursorStyle::Default);
    EXPECT_EQ(overriddenStyle.verticalAlign().value, VerticalAlign::Bottom);
}

TEST(StyleSheet, OmitsBorderStyleByDefault) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("panel { border: 3px #123456; } panel.medium { border: solid #123456; } "
                       "panel.none { border-style: none; border-width: 4px; }")
            .ok());

    const ComputedStyle omitted = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(omitted.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(omitted.borderStyle().top, Core::Style::BorderStyle::NoneValue);

    const ComputedStyle medium = stylesheet.resolve("panel", "", {"medium"});
    EXPECT_EQ(medium.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(medium.borderStyle().top, Core::Style::BorderStyle::Solid);

    const ComputedStyle none = stylesheet.resolve("panel", "", {"none"});
    EXPECT_EQ(none.borderWidth().top.pixels, 4.f);
    EXPECT_EQ(none.borderStyle().top, Core::Style::BorderStyle::NoneValue);
}

TEST(StyleSheet, PropagatesTextDecorationWithoutInheritingProperty) {
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

TEST(StyleSheet, RelativeWeight) {
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

TEST(StyleSheet, ResolvesCSSWideInheritanceKeywords) {
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
    EXPECT_EQ(parentStyle.display(), Core::Style::Display::Block);
    EXPECT_NEAR(parentStyle.color().resolvedColor().r, 32.f / 255.f, 1.0e-4f);
    EXPECT_EQ(parentStyle.fontSize(), 19.f);
    EXPECT_EQ(parentStyle.alignSelf().position, Core::Style::ItemPosition::Center);
    EXPECT_NEAR(parentStyle.backgroundColor().resolvedColor().r, 16.f / 255.f, 1.0e-4f);
    ASSERT_EQ(parentStyle.backdropFilter().operations.size(), 1U);

    const ComputedStyle explicitInheritStyle = computedStyle(stylesheet, *explicitInheritPtr);
    EXPECT_EQ(explicitInheritStyle.width().pixels(), 42.f);
    EXPECT_EQ(explicitInheritStyle.display(), Core::Style::Display::Block);
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

TEST(StyleSheet, Overflow) {
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

TEST(StyleSheet, ParsesBorderRadius) {
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

TEST(StyleSheet, SkipsMalformedBorderRadius) {
    constexpr char kInvalidStyles[] = "panel { border-radius: 1px /; }";
    constexpr char kMultipleSlashStyles[] = "panel { border-radius: 1px / 2px / 3px; }";
    constexpr char kTooManyValuesStyles[] = "panel { border-radius: 1px 2px 3px 4px 5px; }";

    for (const char* source : {kInvalidStyles, kMultipleSlashStyles, kTooManyValuesStyles}) {
        SCOPED_TRACE(::testing::Message() << "malformed border-radius CSS: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "border-radius.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
        EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    }
}

TEST(StyleSheet, ParsesScrollbarCSSProperties) {
    constexpr char kScrollbarStyles[] =
        "panel { scrollbar-width: thin; scrollbar-gutter: stable both-edges; scrollbar-color: #112233 #445566; } "
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

TEST(StyleSheet, ParsesAccentColorValues) {
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

TEST(StyleSheet, OverridesColorScheme) {
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

TEST(StyleSheet, SystemColors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("panel { color-scheme: light; color: ButtonText; background-color: ButtonFace; border: 1px solid ButtonBorder; }")
            .ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 0.f);
    EXPECT_NEAR(style.backgroundColor().resolvedColor().r, 240.f / 255.f, 1.0e-6f);
    EXPECT_NEAR(style.borderTopColor().resolvedColor().r, 118.f / 255.f, 1.0e-6f);
}

TEST(StyleSheet, SchemeColor) {
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

TEST(StyleSheet, ResolvesCurrentColorAgainstDescendantColor) {
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

TEST(StyleSheet, ParsesTypedDimensions) {
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

TEST(StyleSheet, ResolvesFontRelativeLineHeightLengths) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { font-size: 20px; line-height: 1em; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();

    const ComputedStyle style = computedStyle(stylesheet, panel);

    EXPECT_EQ(style.fontSize(), 20.f);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(style.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(style.lineHeight().mValue).pixels, 20.f);
}

TEST(StyleSheet, ParsesInsets) {
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

TEST(StyleSheet, RejectsNonCssNumericValues) {
    const struct {
        const char* name;
        const char* styles;
    } invalidCases[] = {
        {"number", "panel { stroke-width: 0x1p3; }"},
        {"length", "panel { background-size: 0x1p3; }"},
        {"percentage", "panel { background-size: 0x1p3%; }"},
        {"gradient stop", "panel { background-image: linear-gradient(#fff 0x1p3%, #000); }"},
        {"gradient center", "panel { background-image: radial-gradient(at 0x1p3% 50%, #fff, #000); }"},
    };

    for (const auto& test : invalidCases) {
        SCOPED_TRACE(test.name);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.styles, "invalid-number.css");
        EXPECT_TRUE(result.ok()) << test.styles;
        EXPECT_FALSE(result.warnings.empty()) << test.styles;
        if (!result.warnings.empty())
            EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid") << test.styles;
    }
}

TEST(StyleSheet, ParsesExponentialCssNumbers) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { stroke-width: 8e0; width: 8e0px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_TRUE(style.svgStrokeWidth.has_value());
    EXPECT_EQ(style.svgStrokeWidth->pixels, 8.f);
    EXPECT_EQ(style.width().pixels(), 8.f);
}

TEST(StyleSheet, ValidatesMinSize) {
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
        SCOPED_TRACE(::testing::Message() << "invalid min-size case: " << test.name);
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
