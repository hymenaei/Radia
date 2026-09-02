/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>
#include "style/stylesheet.h"

namespace radia::ui {
struct StylePaint {
    Color color;
    std::optional<Gradient> gradient;
    std::optional<LightDarkColor> lightDarkColor;
    bool currentColor = false;
};

struct StyleBorder {
    float width = 0.f;
    StylePaint paint;
    BorderStyle style = BorderStyle::Solid;
};

struct StyleSize {
    Dimension height;
    Dimension width;
};

struct StyleIconStroke {
    float width = 0.f;
    Color color;
    std::optional<LightDarkColor> lightDarkColor;
};

struct InitialStyleValue {};

enum class StyleWideKeyword : std::uint8_t { Inherit, Unset };

struct StyleModel;
struct StyleRule;

namespace detail { struct StylePropertyDefinition; }

using StyleValue = std::variant<InitialStyleValue, StyleWideKeyword, Color, LightDarkColor, StylePaint, StyleBorder, StyleSize, StyleIconStroke,
                                EdgeInsets, MarginInsets, Dimension, Length, BorderRadii, std::optional<Length>, GapValue, std::vector<BoxShadow>,
                                std::vector<Effect>, Outline, GridArea, Translate, std::optional<std::string>, float, int, bool, AppearanceMode,
                                ColorScheme, BoxSizing, BorderStyle, FontFamily, TextAlign, TextOverflow, TextWrap, VerticalAlign, TextDecoration,
                                DisplayMode, FlexDirection, PositionMode, JustifyContent, JustifySelf, AlignItems, AlignSelf, Overflow, ScrollbarMode,
                                ScrollbarWidth, ScrollbarGutter, PointerEvents, CursorStyle, StrokeCap, Visibility, ScrollbarColors, AccentColor>;

using StyleColorValue = std::variant<Color, LightDarkColor>;

struct StyleDeclaration {
    std::reference_wrapper<const detail::StylePropertyDefinition> property;
    StyleValue value;

    StyleDeclaration(const detail::StylePropertyDefinition& definition, StyleValue declarationValue)
        : property(definition), value(std::move(declarationValue)) {}
};

namespace detail {
struct StyleCompileContext;
using StyleCompileResult = std::optional<std::vector<StyleDeclaration>>;
using StyleCompileFunction = StyleCompileResult (*)(StyleCompileContext&);
using StyleApplyFunction = void (*)(Style&, const StyleValue&);
using StyleResetFunction = void (*)(Style&);
using StyleSpecifyFunction = void (*)(Style&);
using StyleInheritFunction = void (*)(Style&, const Style&);

enum class StylePropertyImpact : uint8_t { Layout = 1 << 0, Paint = 1 << 1, Inherited = 1 << 2, HitTest = 1 << 3 };

inline constexpr StylePropertyImpact operator|(StylePropertyImpact left, StylePropertyImpact right) {
    return static_cast<StylePropertyImpact>(static_cast<uint8_t>(left) | static_cast<uint8_t>(right));
}

inline constexpr bool hasImpact(StylePropertyImpact value, StylePropertyImpact flag) {
    return (static_cast<uint8_t>(value) & static_cast<uint8_t>(flag)) != 0;
}

struct StylePropertyDefinition {
    std::string_view name;
    StyleCompileFunction compile = nullptr;
    StyleApplyFunction apply = nullptr;
    StyleResetFunction reset = nullptr;
    StyleSpecifyFunction specify = nullptr;
    StyleInheritFunction inherit = nullptr;
    StylePropertyImpact impact = StylePropertyImpact::Layout;
    bool defaultOnly = false;

    bool isPaintOnly() const { return hasImpact(impact, StylePropertyImpact::Paint) && !hasImpact(impact, StylePropertyImpact::Layout); }
    bool isInherited() const { return hasImpact(impact, StylePropertyImpact::Inherited); }
    bool affectsHitTesting() const { return hasImpact(impact, StylePropertyImpact::HitTest); }
    InheritedStyleProperties inheritedBit() const { return static_cast<InheritedStyleProperties>(inheritedProperty); }

    InheritedStyleProperty inheritedProperty = InheritedStyleProperty::NotInherited;
};

const StylePropertyDefinition* findStyleProperty(std::string_view name);
const StylePropertyDefinition* stylePropertyBegin();
const StylePropertyDefinition* stylePropertyEnd();
void applyStyleDeclaration(Style& style, const StyleDeclaration& declaration);

StyleRule parseSelector(const std::string& selector);
} // namespace detail

enum class SelectorCombinator { Descendant, Child };
enum class StyleParsePass : std::uint8_t { Tokens, Rules };

struct StyleAttributeSelector {
    std::string name;
    std::string value;
    bool presence = false;
};

struct StyleSelector {
    bool universal = false;
    bool root = false;
    bool attributeSyntaxInvalid = false;
    bool idSyntaxInvalid = false;
    bool classSyntaxInvalid = false;
    bool pseudoElementSyntaxInvalid = false;
    bool directionSyntaxInvalid = false;
    std::string element;
    std::vector<StyleAttributeSelector> attributes;
    std::string id;
    std::string className;
    std::string state;
    std::optional<LayoutDirection> direction;
    std::string pseudoElement;
};

struct StyleRule {
    StyleOrigin origin = StyleOrigin::Default;
    std::vector<StyleSelector> selectors;
    std::vector<SelectorCombinator> combinators;
    std::vector<StyleDeclaration> declarations;
    int sourceOrder = 0;
};

struct StyleModel {
    void setColorToken(const std::string& name, const Color& color);
    void setNumberToken(const std::string& name, float value);
    void addRule(const StyleRule& rule);
    void sortRules();

    Color colorToken(const std::string& name, const Color& fallback) const;
    float numberToken(const std::string& name, float fallback) const;
    Color parseColorValue(const std::string& value, const Color& fallback) const;
    std::optional<StyleColorValue> parseColorChoiceValue(const std::string& value) const;
    float parseNumberValue(const std::string& value, float fallback) const;
    std::optional<Length> parseLengthValue(const std::string& value) const;
    std::optional<BorderRadii> parseBorderRadius(const std::string& value) const;
    std::optional<Gradient> parseGradient(const std::string& value) const;
    std::optional<std::vector<BoxShadow>> parseShadows(const std::string& value) const;
    std::optional<std::vector<Effect>> parseEffects(const std::string& value) const;
    std::optional<Outline> parseOutline(const std::string& value) const;
    std::optional<bool> parseFontStyleValue(const std::string& value) const;
    std::optional<float> parseFontWeightValue(const std::string& value) const;
    std::optional<Length> parseLineHeightValue(const std::string& value) const;
    std::optional<std::vector<StyleDeclaration>> parseFontShorthand(const std::string& value) const;
    EdgeInsets parseEdgeInsets(const std::string& value, const EdgeInsets& fallback) const;
    std::optional<MarginInsets> parseMargin(const std::string& value) const;
    std::optional<std::vector<StyleDeclaration>> compileDeclaration(const detail::StylePropertyDefinition& property, const std::string& value,
                                                                    const std::string& selector, StyleSheetLoadResult& result,
                                                                    const std::string& sourceName) const;
    Style resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                          std::string_view pseudoElement, const Element* target = nullptr, const std::vector<std::string>* inlineAncestors = nullptr,
                          LayoutDirection direction = LayoutDirection::LeftToRight) const;
    void parseBlock(const std::string& selector, const std::string& body, const StyleRule& parent, StyleOrigin origin, StyleParsePass pass,
                    StyleSheetLoadResult& result, const std::string& sourceName);

    std::map<std::string, Color> colorTokens;
    std::map<std::string, float> numberTokens;
    StyleSheet::DependencyMap dependencies;
    std::vector<StyleRule> rules;
    std::uint64_t generation = 0;
    mutable std::uint16_t layoutStateMask = 0;
    mutable bool layoutStateMaskValid = false;
    mutable std::array<std::vector<std::size_t>, 9> layoutStateRules;
    mutable std::uint16_t hitTestStateMask = 0;
    mutable bool hitTestStateMaskValid = false;
    mutable std::array<std::vector<std::size_t>, 9> hitTestStateRules;
    mutable bool descendantStateRulesValid = false;
    mutable std::array<std::vector<std::size_t>, 9> descendantStateRules;
    mutable bool ruleIndexValid = false;
    mutable std::vector<std::size_t> universalRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> elementRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> idRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> classRuleIndices;

    bool stateAffectsLayout(ElementState state) const;
    bool stateAffectsLayout(const Element& element, ElementState state) const;
    bool stateAffectsHitTesting(ElementState state) const;
    bool stateAffectsHitTesting(const Element& element, ElementState state) const;
    bool stateAffectsDescendants(const Element& element, ElementState state) const;
};

struct StyleSheet::Impl : StyleModel {};
} // namespace radia::ui
