/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <map>
#include <memory>
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
#include "CSSPseudoSelectors.h"
#include "CSSValue.h"
#include "style/computedstyle.h"
#include "types.h"

namespace radia::ui {
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

struct DeferredStyleValue {
    std::string source;
};

enum class StyleWideKeyword : std::uint8_t { Inherit, Unset };

struct StyleModel;
struct StyleRuleSet;
struct StyleRule;
struct StyleSelectorFunction;

using StyleValue = std::variant<InitialStyleValue, DeferredStyleValue, StyleWideKeyword, CSSValue, StyleColor, StyleImage, ColorScheme,
                                StyleImageLayers, StyleMaskLayers, Dimension, Length, InsetEdge, std::optional<Dimension>, RectEdges<float>,
                                RectEdges<LineWidth>, RectEdges<MarginEdge>, CornerRadius, BorderRadius, GapGutter, GridArea, Translate, CursorValue,
                                ScrollbarColor, BoxShadows, Filter, Outline, std::optional<std::string>, float, Order, bool, BoxSizing, BorderStyle,
                                Display, Position, Visibility, FlexDirection, ItemPosition, ContentPosition, ContentDistribution, Overflow,
                                ScrollbarWidth, ScrollbarGutter, FontFamilies, TextAlign, TextOverflow, TextWrapMode, TextWrapStyle, VerticalAlign,
                                TextDecoration, PointerEvents, CursorStyle, StrokeCap, SelfAlignmentData, ContentAlignmentData, VerticalAlignValue>;

struct StyleDeclaration {
    std::string property;
    StyleValue value;

    StyleDeclaration(std::string_view propertyName, StyleValue declarationValue) : property(propertyName), value(std::move(declarationValue)) {}
};

namespace detail {
StyleRule parseSelector(const std::string& selector);
StyleRule parseSelector(const CSSTokenStream& stream, CSSTokenRange range);
} // namespace detail

enum class SelectorCombinator { Descendant, Child, NextSibling, SubsequentSibling, Column };

struct StyleAttributeSelector {
    enum class Match { Exact, IncludesWord, IncludesHyphen, Prefix, Suffix, Substring };

    std::string name;
    std::string value;
    bool presence = false;
    Match match = Match::Exact;
    bool caseInsensitive = false;
    bool caseSensitivitySpecified = false;
};

struct CustomPropertyDeclaration {
    std::string name;
    CustomPropertyValue value;
};

struct StyleSpecificity {
    std::uint32_t ids = 0;
    std::uint32_t classesAttributesAndPseudoClasses = 0;
    std::uint32_t elements = 0;

    friend bool operator==(const StyleSpecificity&, const StyleSpecificity&) = default;
    friend bool operator<(const StyleSpecificity& left, const StyleSpecificity& right) {
        if (left.ids != right.ids) return left.ids < right.ids;
        if (left.classesAttributesAndPseudoClasses != right.classesAttributesAndPseudoClasses)
            return left.classesAttributesAndPseudoClasses < right.classesAttributesAndPseudoClasses;
        return left.elements < right.elements;
    }
};

struct StyleSelector {
    bool universal = false;
    bool attributeSyntaxInvalid = false;
    bool idSyntaxInvalid = false;
    bool classSyntaxInvalid = false;
    bool pseudoElementSyntaxInvalid = false;
    bool pseudoClassSyntaxInvalid = false;
    bool pseudoClassArgumentSyntaxInvalid = false;
    bool functionSyntaxUnsupported = false;
    std::string invalidPseudoClass;
    std::string element;
    std::vector<StyleAttributeSelector> attributes;
    std::vector<std::string> ids;
    std::vector<std::string> classNames;
    std::vector<CSSPseudoClass> pseudoClasses;
    std::string pseudoElement;
    std::vector<std::shared_ptr<StyleSelectorFunction>> selectorFunctions;
};

struct StyleRule {
    StyleOrigin origin = StyleOrigin::UserAgent;
    std::vector<StyleSelector> selectors;
    std::vector<SelectorCombinator> combinators;
    std::vector<CustomPropertyDeclaration> customProperties;
    std::vector<StyleDeclaration> declarations;
    std::size_t sourceOrder = 0;
};

struct StyleSelectorFunction {
    CSSPseudoClass pseudoClass = CSSPseudoClass::Is;
    std::optional<std::string> identifier;
    std::vector<StyleRule> arguments;
};

struct StyleModel {
    void addRule(const StyleRule& rule);
    StyleRuleSet build() &&;

    static std::optional<CSS::Color> consumeCSSColor(detail::CSSValueRange value);
    static float parseNumberValue(detail::CSSValueRange value, float fallback);
    static std::optional<Length> parseLengthValue(detail::CSSValueRange value);
    static std::optional<Gradient> parseGradient(detail::CSSValueRange value, ColorSchemeMode scheme);
    static std::optional<Filter> parseFilter(detail::CSSValueRange value);
    static std::optional<Outline> parseOutline(detail::CSSValueRange value, ColorSchemeMode scheme);
    static std::optional<std::vector<StyleDeclaration>> compileDeclaration(std::string_view property, const detail::CSSTokenStream& stream,
                                                                           detail::CSSTokenRange valueRange, const std::string& selector,
                                                                           StyleSheetLoadResult& result, const std::string& sourceName,
                                                                           ColorSchemeMode scheme = ColorSchemeMode::Dark,
                                                                           bool resolveColorFunctions = false);
    void parseBlock(const detail::CSSTokenStream& stream, detail::CSSTokenRange selectorRange, detail::CSSTokenRange bodyRange,
                    const StyleRule& parent, StyleOrigin origin, StyleSheetLoadResult& result, const std::string& sourceName);

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

    bool pseudoClassAffectsLayout(CSSPseudoClass pseudoClass) const;
    bool pseudoClassAffectsLayout(const Element& element, CSSPseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(CSSPseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(const Element& element, CSSPseudoClass pseudoClass) const;
    bool pseudoClassAffectsDescendants(const Element& element, CSSPseudoClass pseudoClass) const;
    bool pseudoClassAffectsFollowingSiblings(const Element& element, CSSPseudoClass pseudoClass) const;

    ComputedStyle resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes,
                                  std::initializer_list<CSSPseudoClass> matchingPseudoClasses, std::string_view pseudoElement,
                                  const Element* target = nullptr, const std::vector<std::string>* inlineAncestors = nullptr,
                                  const CustomPropertyMap* inheritedCustomProperties = nullptr, ColorSchemeContext colorSchemeContext = {},
                                  const ColorScheme* inheritedColorScheme = nullptr, const ComputedStyle* parentStyle = nullptr,
                                  const ComputedStyle& rootStyle = ComputedStyle::initialStyle()) const;

private:
    void buildIndexes();

    StyleSheet::DependencyMap mDependencies;
    std::vector<StyleRule> mRules;
    std::vector<StyleResourceReference> mResourceReferences;
    std::map<CSSPseudoClass, std::vector<std::size_t>> mLayoutPseudoClassRules;
    std::map<CSSPseudoClass, std::vector<std::size_t>> mHitTestPseudoClassRules;
    std::map<CSSPseudoClass, std::vector<std::size_t>> mDescendantPseudoClassRules;
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
