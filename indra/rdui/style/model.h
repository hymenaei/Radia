/**
 * @file model.h
 * @brief Private stylesheet model shared by the style compiler, parser, and resolver.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef RD_STYLE_MODEL_H
#define RD_STYLE_MODEL_H

#include <array>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <map>
#include <optional>
#include <set>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>
#include "style/stylesheet.h"

namespace rdui {
struct StylePaint {
    Color color;
    std::optional<Gradient> gradient;
};

struct StyleBorder {
    float width = 0.f;
    StylePaint paint;
};

struct StyleSize {
    Dimension height;
    Dimension width;
};

struct StyleIconStroke {
    float width = 0.f;
    Color color;
};

struct StyleModel;
struct StyleRule;

namespace detail { struct StylePropertyDefinition; }

using StyleValue =
    std::variant<Color, StylePaint, StyleBorder, StyleSize, StyleIconStroke, EdgeInsets, MarginInsets, Dimension, Length, std::optional<Length>,
                 GapValue, std::vector<BoxShadow>, std::vector<Effect>, Outline, float, int, bool, FontFamily, TextAlign, TextOverflow, TextWrap,
                 VerticalAlign, Flow, JustifyContent, AlignItems, AlignSelf, Overflow, PointerEvents, CursorStyle, StrokeCap>;

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
    StyleSpecifyFunction specify = nullptr;
    StyleInheritFunction inherit = nullptr;
    StylePropertyImpact impact = StylePropertyImpact::Layout;

    bool isPaintOnly() const { return hasImpact(impact, StylePropertyImpact::Paint) && !hasImpact(impact, StylePropertyImpact::Layout); }
    bool isInherited() const { return hasImpact(impact, StylePropertyImpact::Inherited); }
    bool affectsHitTesting() const { return hasImpact(impact, StylePropertyImpact::HitTest); }
};

const StylePropertyDefinition* findStyleProperty(std::string_view name);
const StylePropertyDefinition* stylePropertyBegin();
const StylePropertyDefinition* stylePropertyEnd();
void applyStyleDeclaration(Style& style, const StyleDeclaration& declaration);

std::vector<std::string> splitPartPath(const std::string& part);
StyleRule parseSelector(const std::string& selector);
} // namespace detail

enum class SelectorCombinator { Descendant, Child };

struct StyleSelector {
    bool universal = false;
    std::string element;
    std::string id;
    std::string className;
    std::string state;
    std::string partState;
    std::vector<std::string> parts;
};

struct StyleRule {
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
    float parseNumberValue(const std::string& value, float fallback) const;
    std::optional<Length> parseLengthValue(const std::string& value) const;
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
    Style resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t ownerStates,
                          const std::vector<std::string>& partPath, uint8_t partStates, const Widget* widget = nullptr,
                          const std::vector<std::string>* inlineAncestors = nullptr) const;
    void parseBlock(const std::string& selector, const std::string& body, const StyleRule& parent, StyleSheetLoadResult& result,
                    const std::string& sourceName);

    std::map<std::string, Color> colorTokens;
    std::map<std::string, float> numberTokens;
    StyleSheet::DependencyMap dependencies;
    std::vector<StyleRule> rules;
    std::uint64_t generation = 0;
    mutable std::uint8_t layoutStateMask = 0;
    mutable bool layoutStateMaskValid = false;
    mutable std::array<std::vector<std::size_t>, 8> layoutStateRules;
    mutable std::uint8_t hitTestStateMask = 0;
    mutable bool hitTestStateMaskValid = false;
    mutable std::array<std::vector<std::size_t>, 8> hitTestStateRules;
    mutable bool descendantStateRulesValid = false;
    mutable std::array<std::vector<std::size_t>, 8> descendantStateRules;
    mutable bool ruleIndexValid = false;
    mutable std::vector<std::size_t> universalRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> elementRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> idRuleIndices;
    mutable std::unordered_map<std::string, std::vector<std::size_t>> classRuleIndices;

    bool stateAffectsLayout(WidgetState state) const;
    bool stateAffectsLayout(const Widget& widget, WidgetState state) const;
    bool stateAffectsHitTesting(WidgetState state) const;
    bool stateAffectsHitTesting(const Widget& widget, WidgetState state) const;
    bool stateAffectsDescendants(const Widget& widget, WidgetState state) const;
};

struct StyleSheet::Impl : StyleModel {};
} // namespace rdui
#endif // RD_STYLE_MODEL_H
