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
#include <Core/LayoutEngine.h>
#include <Core/StylePass.h>
#include <Core/StyleSheet.h>
#include <Core/Text.h>
#include <Core/TextMeasurer.h>
#include <cstddef>
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <utility>
#include "FloaterTestHelpers.h"

namespace {
using Core::Document;
using Core::Element;
using Core::FixedTextMeasurer;
using Core::HTMLButtonElement;
using Core::HTMLFloaterElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::CSS::PseudoClass;
using Core::CSS::SelectorCombinator;
using Core::CSS::StyleRule;
using Core::CSS::StyleSheet;
using Core::detail::ElementInternalAccess;
using Core::detail::makeElement;
using Core::detail::makeElementValue;
using Core::Layout::Direction;
using Core::Style::ColorSchemeMode;
using Core::Style::ComputedStyle;
using Core::Style::Pass;
using Core::Style::Visibility;

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

TEST(StyleSheet, RootSelector) {
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

TEST(StyleSheet, RootOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(".foo { --size: 11px; } :root { --size: 22px; } .foo { width: var(--size); }").ok());

    auto rootOwner = makeElement<HTMLPanelElement>();
    rootOwner->classList().add("foo");
    Document document(std::move(rootOwner));

    const ComputedStyle style = computedStyle(stylesheet, *document.documentElement());
    EXPECT_EQ(style.customProperties.at("--size").source, "22px");
    EXPECT_EQ(style.width().pixels(), 22.f);
}

TEST(StyleSheet, ScopesStructuralSelectors) {
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

TEST(StyleSheet, Focus) {
    constexpr char kFocusStyles[] = "button { border-width: 1px; &:focus { opacity: .8; } "
                                    "&:focus-visible { border-width: 3px; } }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kFocusStyles).ok());
    const auto focused = {PseudoClass::Focus};
    const auto focusVisible = {PseudoClass::Focus, PseudoClass::FocusVisible};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).opacity().value, .8f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focused).borderWidth().top.pixels, 1.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth().top.pixels, 3.f);
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {PseudoClass::FocusVisible}).borderWidth().top.pixels, 1.f);
}

TEST(StyleSheet, PseudoOnly) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(":focus-visible { border-width: 2px; }").ok());

    const auto focusVisible = {PseudoClass::Focus, PseudoClass::FocusVisible};
    EXPECT_EQ(stylesheet.resolve("button", "", {}, focusVisible).borderWidth().top.pixels, 2.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}, {PseudoClass::Focus}).borderWidth().top.pixels, 3.f);
}

TEST(StyleSheet, UnknownDir) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("input:dir(sideways), input:dir(ltr):dir(rtl) { width: 12px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("dir", "rtl");
    EXPECT_TRUE(stylesheet.resolveElement(input).width().isAuto());
}

TEST(StyleSheet, DirPseudo) {
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

TEST(StyleSheet, DirAuto) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:dir(ltr) { width: 10px; } input:dir(rtl) { width: 20px; }").ok());

    auto root = makeElement<HTMLPanelElement>();
    root->setAttribute("dir", "auto");
    auto excluded = makeElement<HTMLPanelElement>();
    excluded->setAttribute("dir", "rtl");
    excluded->append(std::make_unique<Core::Text>("\xD7\x90"));
    root->append(std::move(excluded));
    auto* text = root->append(std::make_unique<Core::Text>("A"))->asText();
    auto input = makeElement<HTMLInputElement>();
    HTMLInputElement* inputPointer = input.get();
    root->append(std::move(input));

    Pass styles(stylesheet, FixedTextMeasurer {}, Direction::RightToLeft);
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 10.f);
    EXPECT_EQ(inputPointer->directionality(), Direction::LeftToRight);

    text->setData("\xD7\x90");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
    EXPECT_EQ(inputPointer->directionality(), Direction::RightToLeft);

    inputPointer->setAttribute("dir", "auto");
    inputPointer->setAttribute("value", "A");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 10.f);
    inputPointer->setAttribute("value", "\xD7\x90");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
    inputPointer->setAttribute("dir", "sideways");
    EXPECT_EQ(styles.style(*inputPointer).width().pixels(), 20.f);
}

TEST(StyleSheet, MatchesChildSelectors) {
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

TEST(StyleSheet, IgnoresCommentsAroundChildCombinators) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button /* comment */ > i { width: 19px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    Element& icon = appendIcon(button, "search");
    EXPECT_EQ(computedStyle(stylesheet, icon).width().pixels(), 19.f);
}

TEST(StyleSheet, MatchesSiblingSelectors) {
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

TEST(StyleSheet, BacktracksMixedSelectorAncestors) {
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

TEST(StyleSheet, InvalidatesFollowingSiblingSelectorsOnMutation) {
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

    Pass styles(stylesheet, FixedTextMeasurer {});
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

TEST(StyleSheet, ComparesSpecificityAsCSS) {
    constexpr char kStyles[] = "button.a.b.c.d.e.f.g.h.i.j { width: 10px; } button#save { width: 20px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.setId("save");
    for (const char* className : {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"})
        button.classList().add(className);

    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 20.f);
}

TEST(StyleSheet, MatchesIsAndWhereSelectorFunctions) {
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

TEST(StyleSheet, AcceptsHostPseudoClassOutsideShadowTreesWithoutMatching) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":host { opacity: .5; } :host(.thing) { width: 14px; }");
    ASSERT_TRUE(result.ok()) << (result.errors.empty() ? "" : result.errors.front().message);
    EXPECT_TRUE(result.warnings.empty());

    const ComputedStyle style = stylesheet.resolve("panel", "", {"thing"});
    EXPECT_FLOAT_EQ(style.opacity().value, 1.f);
    EXPECT_TRUE(style.width().isAuto());
}

TEST(StyleSheet, UsesSelectorFunctionSpecificityFromIsAndZeroFromWhere) {
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

TEST(StyleSheet, ForgivesInvalidIsArguments) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button:is(mystery, .Primary) { width: 17px; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());

    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("Primary");
    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 17.f);
}

TEST(StyleSheet, InvalidatesStateInsideIsSelectorFunctions) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button:is(:hover, .active) { width: 18px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    Pass styles(stylesheet, FixedTextMeasurer {});
    EXPECT_TRUE(styles.style(button).width().isAuto());

    ElementInternalAccess::setHovered(button, true);
    EXPECT_EQ(styles.style(button).width().pixels(), 18.f);
    ElementInternalAccess::setHovered(button, false);
    button.classList().add("active");
    EXPECT_EQ(styles.style(button).width().pixels(), 18.f);
}

TEST(StyleSheet, CountsRepeatedPseudoSpecificity) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button:hover:hover { width: 10px; } button:hover { width: 20px; }").ok());

    auto button = makeElementValue<HTMLButtonElement>();
    ElementInternalAccess::setHovered(button, true);
    EXPECT_EQ(computedStyle(stylesheet, button).width().pixels(), 10.f);
}

TEST(StyleSheet, MatchesCompoundClassesAndPseudoClasses) {
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

TEST(StyleSheet, ParsesColumnCombinator) {
    const StyleRule rule = Core::CSS::detail::parseSelector("label || button");

    ASSERT_EQ(rule.selectors.size(), std::size_t(2));
    ASSERT_EQ(rule.combinators.size(), std::size_t(1));
    EXPECT_EQ(rule.combinators.front(), SelectorCombinator::Column);

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("label || button { width: 10px; }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
}

TEST(StyleSheet, MatchesCaseInsensitivePseudoClassesAndRootTokens) {
    StyleSheet stylesheet;
    const auto result =
        stylesheet.loadRadia(":/**/ROOT { --accent: #123456; } button:HOVER { width: 18px; } panel { color: var(--accent); }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {PseudoClass::Hover}).width().pixels(), 18.f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {}).color().resolvedColor().r, 0x12 / 255.f, 1.0e-6f);
}

TEST(StyleSheet, DecodesEscapedSelectorKeywords) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button:h\\6f ver { width: 18px; } input:dir(r\\74 l) { height: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {}, {PseudoClass::Hover}).width().pixels(), 18.f);
    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("dir", "rtl");
    EXPECT_EQ(stylesheet.resolveElement(input).height().pixels(), 13.f);
}

TEST(StyleSheet, SeparatesPartState) {
    constexpr char kInteractivePartStyles[] = "floater > head > close { width: 10px; } floater > head > close:hover { width: 18px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kInteractivePartStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater, true);
    auto* closeButton = floater.closeButton();
    ASSERT_NE(closeButton, nullptr);

    ElementInternalAccess::setHovered(floater, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width().pixels(), 10.f);
    ElementInternalAccess::setHovered(*closeButton, true);
    EXPECT_EQ(computedStyle(stylesheet, *closeButton).width().pixels(), 18.f);
}

TEST(StyleSheet, MatchesPseudoElementPseudoClasses) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input::slider-thumb { width: 10px; } input::slider-thumb:hover { width: 18px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox").switchMode(true);

    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").width().pixels(), 10.f);
    ElementInternalAccess::setHovered(input, true);
    EXPECT_EQ(stylesheet.resolvePseudoElement(input, "slider-thumb").width().pixels(), 18.f);
}

TEST(StyleSheet, CompilesTargetRules) {
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

TEST(StyleSheet, SelectsSwitchInputsByAttribute) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[switch] { width: 40px; }").ok());

    auto genericInput = makeElementValue<HTMLInputElement>();
    auto switchInput = makeElementValue<HTMLInputElement>();
    switchInput.type("checkbox").switchMode(true);
    EXPECT_EQ(computedStyle(stylesheet, genericInput).width().pixels(), 10.f);
    EXPECT_EQ(computedStyle(stylesheet, switchInput).width().pixels(), 40.f);
}

TEST(StyleSheet, SelectsRadioInputsByNameAttribute) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 40.f);
}

TEST(StyleSheet, ClearsRemovedRadioName) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input { width: 10px; } input[name] { width: 20px; } input[name=choice] { width: 40px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("radio").name("choice");
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 40.f);

    input.name("");
    EXPECT_FALSE(input.hasAttribute("name"));
    EXPECT_EQ(computedStyle(stylesheet, input).width().pixels(), 10.f);
}

TEST(StyleSheet, RefreshesSerializedDisabledAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input[disabled=one] { width: 10px; } input[disabled=two] { width: 20px; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.setAttribute("disabled", "one");
    Pass styles(stylesheet, FixedTextMeasurer {});
    EXPECT_EQ(styles.style(input).width().pixels(), 10.f);

    input.setAttribute("disabled", "two");
    EXPECT_EQ(styles.style(input).width().pixels(), 20.f);
}

TEST(StyleSheet, MatchesCSSAttributeOperators) {
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

TEST(StyleSheet, RequiresExplicitUniversalForAttributeSelectors) {
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

TEST(StyleSheet, DecodesEscapedAttributeSelectorDelimiters) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(R"(i[data=foo\]] { width: 12px; })").ok());

    auto icon = makeElement<Element>("i");
    icon->setAttribute("data", "foo]");
    EXPECT_EQ(computedStyle(stylesheet, *icon).width().pixels(), 12.f);
}

TEST(StyleSheet, InvalidatesDynamicAttributeSelectors) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel[hidden] { width: 11px; }").ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    Pass styles(stylesheet, FixedTextMeasurer {});
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

TEST(StyleSheet, SelectsIndeterminateInputs) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:indeterminate { opacity: .5; }").ok());

    auto input = makeElementValue<HTMLInputElement>();
    input.type("checkbox");
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity().value, 1.f);
    input.indeterminate(true);
    EXPECT_FLOAT_EQ(computedStyle(stylesheet, input).opacity().value, .5f);
}

TEST(StyleSheet, SkipsInvalidRules) {
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
        SCOPED_TRACE(::testing::Message() << "invalid CSS: " << test.source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(test.source, "contract.css");

        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result.warnings.size(), std::size_t {1});
        EXPECT_EQ(result.warnings.front().code, test.diagnostic);
    }
}

TEST(StyleSheet, SkipsMultiplePseudoElementPseudoClasses) {
    constexpr char kPseudoElementStyles[] = "input::slider-thumb:hover:checked { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kPseudoElementStyles, "contract.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheet, SkipsNestedPseudoElements) {
    constexpr char kNestedPseudoElementStyles[] = "input::slider-track::slider-fill { width: 10px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kNestedPseudoElementStyles, "contract.css");

    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result.warnings.empty());
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_invalid");
}

TEST(StyleSheet, ProjectsMinimizedPseudoClass) {
    constexpr char kMinimizedFloaterStyles[] = "floater > head { border-width: 0px 0px 1px; } "
                                               "floater:minimized > head { border-width: 0px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kMinimizedFloaterStyles).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater, false, true);
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

TEST(StyleSheet, MatchesStructuralSelectors) {
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

TEST(StyleSheet, NormalizesSelectorNames) {
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

TEST(StyleSheet, ResolvesNestedInlineKbdSelectors) {
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

TEST(StyleSheet, NestedOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("button { color: #ff0000ff; & { color: #00ff00ff; } color: #0000ffff; }").ok());

    const ComputedStyle style = stylesheet.resolve("button", "", {});
    EXPECT_FLOAT_EQ(style.color().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.color().resolvedColor().b, 1.f);
}

TEST(StyleSheet, RejectsDanglingChildCombinators) {
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

TEST(StyleSheet, SkipsAnInvalidSelectorListAsAWhole) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel, button::unknown { width: 99px; } panel { height: 13px; }");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.pseudo_element_unknown");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, BorderPseudoLayout) {
    constexpr char kStateBorderStyles[] = "fieldset { border: 1px solid #ffffff; } "
                                          "fieldset:hover { border: 4px solid #ffffff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStateBorderStyles).ok());
    EXPECT_TRUE(stylesheet.pseudoClassAffectsLayout(PseudoClass::Hover));
}

TEST(StyleSheet, AppearancePseudoLayout) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("input:hover { appearance: none; }").ok());
    EXPECT_TRUE(stylesheet.pseudoClassAffectsLayout(PseudoClass::Hover));
}

TEST(StyleSheet, MatchesEffectiveFieldsetDisabledState) {
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
