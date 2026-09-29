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
#include "CSSPseudoSelectors.h"
#include "CSSTokenStream.h"
#include "CSSValue.h"
#include "Color.h"
#include "ComputedStyle.h"
#include "LayoutGeometry.h"
#include "StyleSheet.h"

namespace Core {
class Element;
} // namespace Core

namespace Core::CSS {
enum class StyleImageComponent : std::uint8_t {
    All,
    Image,
    Position,
    Size,
    Repeat,
    Origin,
    Clip,
    Attachment,
    Mode,
    Composite,
    Type
};

struct StyleImageLayers {
    std::vector<Style::BackgroundLayer> layers;
    StyleImageComponent component = StyleImageComponent::All;
};

struct StyleMaskLayers {
    std::vector<Style::MaskLayer> layers;
    StyleImageComponent component = StyleImageComponent::All;
};

struct InitialStyleValue {};

struct DeferredStyleValue {
    std::string source;
};

enum class StyleWideKeyword : std::uint8_t {
    Inherit,
    Unset
};

struct StyleModel;
struct StyleRuleSet;
struct StyleRule;
struct StyleSelectorFunction;

using StyleValue = std::variant<InitialStyleValue, DeferredStyleValue, StyleWideKeyword, Value, Style::Color, Style::Image,
    Style::ColorScheme, StyleImageLayers, StyleMaskLayers, Style::Dimension, Style::Length, Style::InsetEdge,
    std::optional<Style::Dimension>, Layout::RectEdges<float>, Layout::RectEdges<Style::LineWidth>, Layout::RectEdges<Style::MarginEdge>,
    Style::CornerRadius, Style::BorderRadius, Style::GapGutter, Style::GridArea, Style::Translate, Style::CursorValue,
    Style::ScrollbarColor, Style::BoxShadows, Style::Filter, Style::Outline, std::optional<std::string>, float, Style::Order, bool,
    Style::BoxSizing, Style::BorderStyle, Style::Display, Style::Position, Style::Visibility, Style::FlexDirection, Style::ItemPosition,
    Style::ContentPosition, Style::ContentDistribution, Style::Overflow, Style::ScrollbarWidth, Style::ScrollbarGutter, Style::FontFamilies,
    Style::TextAlign, Style::TextOverflow, Style::TextWrapMode, Style::TextWrapStyle, Style::VerticalAlign, Style::TextDecoration,
    Style::PointerEvents, Style::CursorStyle, Style::StrokeCap, Style::SelfAlignmentData, Style::ContentAlignmentData,
    Style::VerticalAlignValue>;

struct StyleDeclaration {
    std::string property;
    StyleValue value;

    StyleDeclaration(std::string_view propertyName, StyleValue declarationValue)
        : property(propertyName)
        , value(std::move(declarationValue)) {}
};

namespace detail {
StyleRule parseSelector(const std::string& selector);
StyleRule parseSelector(const TokenStream& stream, TokenRange range);
} // namespace detail

enum class SelectorCombinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
    Column
};

struct StyleAttributeSelector {
    enum class Match {
        Exact,
        IncludesWord,
        IncludesHyphen,
        Prefix,
        Suffix,
        Substring
    };

    std::string name;
    std::string value;
    bool presence = false;
    Match match = Match::Exact;
    bool caseInsensitive = false;
    bool caseSensitivitySpecified = false;
};

struct CustomPropertyDeclaration {
    std::string name;
    Style::CustomPropertyValue value;
};

struct StyleSpecificity {
    std::uint32_t ids = 0;
    std::uint32_t classesAttributesAndPseudoClasses = 0;
    std::uint32_t elements = 0;

    friend bool operator==(const StyleSpecificity&, const StyleSpecificity&) = default;
    friend bool operator<(const StyleSpecificity& left, const StyleSpecificity& right) {
        if (left.ids != right.ids)
            return left.ids < right.ids;
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
    std::vector<PseudoClass> pseudoClasses;
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
    PseudoClass pseudoClass = PseudoClass::Is;
    std::optional<std::string> identifier;
    std::vector<StyleRule> arguments;
};

struct StyleModel {
    void addRule(const StyleRule& rule);
    StyleRuleSet build() &&;

    static std::optional<Color> consumeColor(detail::ValueRange value);
    static float parseNumberValue(detail::ValueRange value, float fallback);
    static std::optional<Style::Length> parseLengthValue(detail::ValueRange value);
    static std::optional<Style::Gradient> parseGradient(detail::ValueRange value, Style::ColorSchemeMode scheme);
    static std::optional<Style::Filter> parseFilter(detail::ValueRange value);
    static std::optional<Style::Outline> parseOutline(detail::ValueRange value, Style::ColorSchemeMode scheme);
    static std::optional<std::vector<StyleDeclaration>> compileDeclaration(std::string_view property, const detail::TokenStream& stream,
        detail::TokenRange valueRange, const std::string& selector, StyleSheetLoadResult& result, const std::string& sourceName,
        Style::ColorSchemeMode scheme = Style::ColorSchemeMode::Dark, bool resolveColorFunctions = false);
    void parseBlock(const detail::TokenStream& stream, detail::TokenRange selectorRange, detail::TokenRange bodyRange,
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

    bool pseudoClassAffectsLayout(PseudoClass pseudoClass) const;
    bool pseudoClassAffectsLayout(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(PseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsDescendants(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsFollowingSiblings(const Element& element, PseudoClass pseudoClass) const;

    Style::ComputedStyle resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes,
        std::initializer_list<PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target = nullptr,
        const std::vector<std::string>* inlineAncestors = nullptr, const Style::CustomPropertyMap* inheritedCustomProperties = nullptr,
        Style::ColorSchemeContext colorSchemeContext = {}, const Style::ColorScheme* inheritedColorScheme = nullptr,
        const Style::ComputedStyle* parentStyle = nullptr,
        const Style::ComputedStyle& rootStyle = Style::ComputedStyle::initialStyle()) const;

private:
    void buildIndexes();

    StyleSheet::DependencyMap mDependencies;
    std::vector<StyleRule> mRules;
    std::vector<StyleResourceReference> mResourceReferences;
    std::map<PseudoClass, std::vector<std::size_t>> mLayoutPseudoClassRules;
    std::map<PseudoClass, std::vector<std::size_t>> mHitTestPseudoClassRules;
    std::map<PseudoClass, std::vector<std::size_t>> mDescendantPseudoClassRules;
    std::vector<std::size_t> mUniversalRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mElementRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mIdRuleIndices;
    std::unordered_map<std::string, std::vector<std::size_t>> mClassRuleIndices;
};

struct StyleSheet::Impl {
    Impl() = default;
    explicit Impl(StyleRuleSet rules)
        : ruleSet(std::move(rules)) {}

    StyleRuleSet ruleSet;
    std::uint64_t generation = 0;
};
} // namespace Core::CSS
