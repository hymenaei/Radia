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
#include "css/stylesheet.h"
#include "css/syntax.h"

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

enum class StyleImageComponent : std::uint8_t { All, Image, Position, Size, Repeat, Origin, Clip, Attachment, Mode, Composite, Type };

struct StyleImageLayers {
    std::vector<BackgroundLayer> layers;
    StyleImageComponent component = StyleImageComponent::All;
};

struct StyleMaskLayers {
    std::vector<MaskLayer> layers;
    StyleImageComponent component = StyleImageComponent::All;
};

struct InitialStyleValue {};

enum class StyleWideKeyword : std::uint8_t { Inherit, Unset };

struct StyleModel;
struct StyleRuleSet;
struct StyleRule;

namespace detail { struct StylePropertyDefinition; }

using StyleValue =
    std::variant<InitialStyleValue, StyleWideKeyword, Color, LightDarkColor, AccentColor, ColorScheme, StylePaint, StyleBorder, StyleSize,
                 StyleImageLayers, StyleMaskLayers, Dimension, Length, std::optional<Length>, EdgeInsets, MarginInsets, BorderRadii, GapValue,
                 GridArea, Translate, CursorValue, ScrollbarColors, std::vector<BoxShadow>, std::vector<Effect>, Outline, std::optional<std::string>,
                 float, int, bool, AppearanceMode, BoxSizing, BorderStyle, DisplayMode, PositionMode, Visibility, FlexDirection, JustifyContent,
                 JustifySelf, AlignItems, AlignSelf, Overflow, ScrollbarMode, ScrollbarWidth, ScrollbarGutter, FontFamily, TextAlign, TextOverflow,
                 TextWrap, VerticalAlign, TextDecoration, PointerEvents, CursorStyle, StrokeCap>;

using StyleColorValue = std::variant<Color, LightDarkColor>;

struct StyleDeclaration {
    std::reference_wrapper<const detail::StylePropertyDefinition> property;
    StyleValue value;

    StyleDeclaration(const detail::StylePropertyDefinition& definition, StyleValue declarationValue)
        : property(definition), value(std::move(declarationValue)) {}
};

namespace detail {
StyleRule parseSelector(const std::string& selector);
StyleRule parseSelector(const CSSTokenStream& stream, CSSTokenRange range);
} // namespace detail

enum class SelectorCombinator { Descendant, Child };
enum class StyleParsePass : std::uint8_t { Tokens, Rules };

struct StyleAttributeSelector {
    enum class Match { Exact, IncludesWord, IncludesHyphen, Prefix, Suffix, Substring };

    std::string name;
    std::string value;
    bool presence = false;
    Match match = Match::Exact;
    bool caseInsensitive = false;
    bool caseSensitivitySpecified = false;
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
    StyleRuleSet build() &&;

    Color colorToken(const std::string& name, const Color& fallback) const;
    float numberToken(const std::string& name, float fallback) const;
    Color parseColorValue(detail::CSSValueRange value, const Color& fallback) const;
    std::optional<StyleColorValue> parseColorChoiceValue(detail::CSSValueRange value) const;
    float parseNumberValue(detail::CSSValueRange value, float fallback) const;
    std::optional<Length> parseLengthValue(detail::CSSValueRange value) const;
    std::optional<BorderRadii> parseBorderRadius(detail::CSSValueRange value) const;
    std::optional<Gradient> parseGradient(detail::CSSValueRange value) const;
    std::optional<std::vector<BoxShadow>> parseShadows(detail::CSSValueRange value) const;
    std::optional<std::vector<Effect>> parseEffects(detail::CSSValueRange value) const;
    std::optional<Outline> parseOutline(detail::CSSValueRange value) const;
    std::optional<bool> parseFontStyleValue(detail::CSSValueRange value) const;
    std::optional<float> parseFontWeightValue(detail::CSSValueRange value) const;
    std::optional<Length> parseLineHeightValue(detail::CSSValueRange value) const;
    std::optional<std::vector<StyleDeclaration>> parseFontShorthand(detail::CSSValueRange value) const;
    EdgeInsets parseEdgeInsets(detail::CSSValueRange value, const EdgeInsets& fallback) const;
    std::optional<MarginInsets> parseMargin(detail::CSSValueRange value) const;
    std::optional<std::vector<StyleDeclaration>> compileDeclaration(const detail::StylePropertyDefinition& property,
                                                                    const detail::CSSTokenStream& stream, detail::CSSTokenRange valueRange,
                                                                    const std::string& selector, StyleSheetLoadResult& result,
                                                                    const std::string& sourceName) const;
    void parseBlock(const detail::CSSTokenStream& stream, detail::CSSTokenRange selectorRange, detail::CSSTokenRange bodyRange,
                    const StyleRule& parent, StyleOrigin origin, StyleParsePass pass, StyleSheetLoadResult& result, const std::string& sourceName);

    std::map<std::string, Color> colorTokens;
    std::map<std::string, float> numberTokens;
    StyleSheet::DependencyMap dependencies;
    std::vector<StyleRule> rules;
};

struct StyleRuleSet {
    StyleRuleSet() = default;
    explicit StyleRuleSet(StyleModel&& model);
    StyleRuleSet(const StyleRuleSet&) = default;
    StyleRuleSet(StyleRuleSet&&) noexcept = default;
    StyleRuleSet& operator=(const StyleRuleSet&) = delete;
    StyleRuleSet& operator=(StyleRuleSet&&) = delete;

    const StyleSheet::DependencyMap& dependencies() const { return mDependencies; }
    const std::vector<StyleResourceReference>& resourceReferences() const { return mResourceReferences; }

    bool stateAffectsLayout(ElementState state) const;
    bool stateAffectsLayout(const Element& element, ElementState state) const;
    bool stateAffectsHitTesting(ElementState state) const;
    bool stateAffectsHitTesting(const Element& element, ElementState state) const;
    bool stateAffectsDescendants(const Element& element, ElementState state) const;

    ComputedStyle resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                                  std::string_view pseudoElement, const Element* target = nullptr,
                                  const std::vector<std::string>* inlineAncestors = nullptr,
                                  LayoutDirection direction = LayoutDirection::LeftToRight) const;

private:
    void buildIndexes();

    std::map<std::string, Color> mColorTokens;
    std::map<std::string, float> mNumberTokens;
    StyleSheet::DependencyMap mDependencies;
    std::vector<StyleRule> mRules;
    std::vector<StyleResourceReference> mResourceReferences;
    std::uint16_t mLayoutStateMask = 0;
    std::array<std::vector<std::size_t>, 9> mLayoutStateRules;
    std::uint16_t mHitTestStateMask = 0;
    std::array<std::vector<std::size_t>, 9> mHitTestStateRules;
    std::array<std::vector<std::size_t>, 9> mDescendantStateRules;
    std::vector<std::size_t> mUniversalRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mElementRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mIdRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mClassRuleIndices;
};

struct StyleSheet::Impl {
    Impl() = default;
    explicit Impl(StyleRuleSet rules) : ruleSet(std::move(rules)) {}

    StyleRuleSet ruleSet;
    std::uint64_t generation = 0;
};
} // namespace radia::ui
