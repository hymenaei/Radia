/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "css/rules.h"
#include "css/stylesheet.h"
#include "dom/element.h"
#include "html/elementnames.h"
#include "style/property.h"

namespace radia::ui {
namespace {
bool matchesState(const std::string& state, uint16_t states) {
    if (state.empty()) return true;
    if (state == "hover") return hasState(states, ElementState::Hovered);
    if (state == "active") return hasState(states, ElementState::Active);
    if (state == "focus") return hasState(states, ElementState::Focused);
    if (state == "focus-visible") return hasState(states, ElementState::Focused) && hasState(states, ElementState::FocusVisible);
    if (state == "disabled") return hasState(states, ElementState::Disabled);
    if (state == "checked") return hasState(states, ElementState::Checked);
    if (state == "minimized") return hasState(states, ElementState::Minimized);
    if (state == "invalid") return hasState(states, ElementState::Invalid);
    if (state == "indeterminate") return hasState(states, ElementState::Indeterminate);
    return false;
}

bool matchesDirection(const std::optional<LayoutDirection>& selectorDirection, LayoutDirection direction) {
    return !selectorDirection || *selectorDirection == direction;
}

std::optional<ElementState> selectorState(const std::string& state) {
    if (state == "hover") return ElementState::Hovered;
    if (state == "active") return ElementState::Active;
    if (state == "focus") return ElementState::Focused;
    if (state == "focus-visible") return ElementState::FocusVisible;
    if (state == "disabled") return ElementState::Disabled;
    if (state == "checked") return ElementState::Checked;
    if (state == "minimized") return ElementState::Minimized;
    if (state == "invalid") return ElementState::Invalid;
    if (state == "indeterminate") return ElementState::Indeterminate;
    return std::nullopt;
}

bool matchesAttribute(const StyleSelector& selector, const Element* element) {
    if (selector.attributes.empty()) return true;
    if (!element) return false;
    return std::all_of(selector.attributes.begin(), selector.attributes.end(), [element](const StyleAttributeSelector& attribute) {
        const std::string* value = detail::styleAttribute(*element, attribute.name);
        if (attribute.presence) return value != nullptr;
        if (attribute.name == "name") return value && *value == attribute.value;
        return value && canonicalizeHTMLName(*value) == canonicalizeHTMLName(attribute.value);
    });
}

bool matchesRoot(const StyleSelector& selector, const Element* element) {
    return !selector.root || (element && element->parentElement() == nullptr);
}

bool selectorCanBeOwnedBy(const StyleSelector& selector, const Element& element) {
    return (selector.element.empty() || selector.element == element.elementName())
        && matchesRoot(selector, &element)
        && matchesAttribute(selector, &element)
        && (selector.id.empty() || selector.id == element.id())
        && (selector.className.empty() || element.classes().find(selector.className) != element.classes().end());
}

std::optional<std::size_t> stateIndex(ElementState state) {
    const std::uint16_t bit = static_cast<std::uint16_t>(state);
    if (bit == 0 || (bit & static_cast<std::uint16_t>(bit - 1)) != 0) return std::nullopt;
    std::size_t index = 0;
    for (std::uint16_t value = bit; value > 1; value >>= 1) ++index;
    return index;
}

int specificity(const StyleSelector& selector) {
    return (!selector.id.empty() ? 100 : 0)
        + (selector.root ? 10 : 0)
        + static_cast<int>(selector.attributes.size()) * 10
        + (!selector.className.empty() ? 10 : 0)
        + (!selector.state.empty() ? 10 : 0)
        + (selector.direction ? 10 : 0)
        + (!selector.element.empty() ? 1 : 0)
        + (!selector.pseudoElement.empty() ? 1 : 0);
}

int specificity(const StyleRule& rule) {
    int result = 0;
    for (const StyleSelector& selector : rule.selectors) result += specificity(selector);
    return result;
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
                     uint16_t ownerStates, std::string_view pseudoElement, const Element* target, LayoutDirection direction) {
    return (selector.element.empty() || selector.element == element)
        && matchesRoot(selector, target)
        && matchesAttribute(selector, target)
        && (selector.id.empty() || selector.id == id)
        && (selector.className.empty() || classes.find(selector.className) != classes.end())
        && matchesDirection(selector.direction, direction)
        && matchesState(selector.state, ownerStates)
        && selector.pseudoElement == pseudoElement;
}

const Element* structuralParent(const Element* element) {
    if (!element) return nullptr;
    return element->parentElement();
}

bool matchesStructuralSelector(const StyleSelector& selector, const Element& element, LayoutDirection direction) {
    return matchesSelector(selector, element.elementName(), element.id(), element.classes(), element.states(), {}, &element, direction);
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                 std::string_view pseudoElement, const Element* target, const std::vector<std::string>* inlineAncestors, LayoutDirection direction) {
    if (rule.selectors.empty() || !matchesSelector(rule.selectors.back(), element, id, classes, ownerStates, pseudoElement, target, direction))
        return false;
    if (rule.selectors.size() == 1) return true;
    if (!target || rule.combinators.size() + 1 != rule.selectors.size()) return false;

    std::size_t inlineIndex = inlineAncestors ? inlineAncestors->size() : 0;
    const Element* ancestor = inlineAncestors ? target : structuralParent(target);
    const auto nextAncestorMatches = [&](const StyleSelector& selector) -> std::optional<bool> {
        static const std::set<std::string> sNoClasses;
        if (inlineAncestors && inlineIndex) {
            const std::string& inlineElement = (*inlineAncestors)[--inlineIndex];
            return matchesSelector(selector, inlineElement, {}, sNoClasses, 0, {}, nullptr, direction);
        }
        if (!ancestor) return std::nullopt;
        const Element* candidate = ancestor;
        ancestor = structuralParent(ancestor);
        return matchesStructuralSelector(selector, *candidate, direction);
    };
    for (std::size_t index = rule.selectors.size() - 1; index-- > 0;) {
        const StyleSelector& selector = rule.selectors[index];
        if (rule.combinators[index] == SelectorCombinator::Child) {
            const std::optional<bool> matches = nextAncestorMatches(selector);
            if (!matches || !*matches) return false;
            continue;
        }
        std::optional<bool> matches;
        do matches = nextAncestorMatches(selector);
        while (matches && !*matches);
        if (!matches) return false;
    }
    return true;
}
} // namespace

void StyleModel::addRule(const StyleRule& rule) {
    StyleRule copy = rule;
    copy.sourceOrder = static_cast<int>(rules.size());
    rules.push_back(std::move(copy));
}

StyleRuleSet::StyleRuleSet(StyleModel&& model)
    : mColorTokens(std::move(model.colorTokens)), mNumberTokens(std::move(model.numberTokens)), mDependencies(std::move(model.dependencies)),
      mRules(std::move(model.rules)) {
    std::stable_sort(mRules.begin(), mRules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        if (lhs.origin != rhs.origin) return static_cast<std::uint8_t>(lhs.origin) < static_cast<std::uint8_t>(rhs.origin);
        const int left = specificity(lhs);
        const int right = specificity(rhs);
        return left == right ? lhs.sourceOrder < rhs.sourceOrder : left < right;
    });
    buildIndexes();
}

void StyleRuleSet::buildIndexes() {
    const auto addIndex = [](auto& index, const std::string& key, std::size_t ruleIndex) {
        if (!key.empty()) index[key].push_back(ruleIndex);
    };
    const auto addUnique = [](auto& values, std::size_t value) {
        if (std::find(values.begin(), values.end(), value) == values.end()) values.push_back(value);
    };
    const auto addStateRule = [&](const std::string& state, auto& mask, auto& buckets, std::size_t ruleIndex) {
        const std::optional<ElementState> candidate = selectorState(state);
        if (!candidate) return;
        const std::optional<std::size_t> index = stateIndex(*candidate);
        if (!index) return;
        mask |= static_cast<std::uint16_t>(*candidate);
        addUnique(buckets[*index], ruleIndex);
    };
    const auto addStateBucket = [&](const std::string& state, auto& buckets, std::size_t ruleIndex) {
        const std::optional<ElementState> candidate = selectorState(state);
        if (!candidate) return;
        const std::optional<std::size_t> index = stateIndex(*candidate);
        if (!index) return;
        addUnique(buckets[*index], ruleIndex);
    };

    for (std::size_t ruleIndex = 0; ruleIndex < mRules.size(); ++ruleIndex) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!rule.selectors.empty()) {
            const StyleSelector& selector = rule.selectors.back();
            if (selector.element.empty() && selector.id.empty() && selector.className.empty()) mUniversalRuleIndices.push_back(ruleIndex);
            addIndex(mElementRuleIndices, selector.element, ruleIndex);
            addIndex(mIdRuleIndices, selector.id, ruleIndex);
            addIndex(mClassRuleIndices, selector.className, ruleIndex);
        }

        const bool layoutDeclaration = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                                   [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
        const bool hitTestDeclaration = std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
            return declaration.property.get().affectsHitTesting();
        });
        const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                          [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if (layoutDeclaration) addStateRule(selector.state, mLayoutStateMask, mLayoutStateRules, ruleIndex);
            if (hitTestDeclaration) addStateRule(selector.state, mHitTestStateMask, mHitTestStateRules, ruleIndex);
            if (selectorIndex + 1 < rule.selectors.size() || inherits) addStateBucket(selector.state, mDescendantStateRules, ruleIndex);
        }
    }

    const auto sortUnique = [](auto& index) {
        for (auto& [key, values] : index) {
            std::sort(values.begin(), values.end());
            values.erase(std::unique(values.begin(), values.end()), values.end());
        }
    };
    std::sort(mUniversalRuleIndices.begin(), mUniversalRuleIndices.end());
    sortUnique(mElementRuleIndices);
    sortUnique(mIdRuleIndices);
    sortUnique(mClassRuleIndices);
    for (auto& candidates : mDescendantStateRules) {
        std::sort(candidates.begin(), candidates.end());
        candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());
    }
}

bool StyleRuleSet::stateAffectsLayout(ElementState state) const {
    return (mLayoutStateMask & static_cast<std::uint16_t>(state)) != 0;
}

bool StyleRuleSet::stateAffectsLayout(const Element& element, ElementState state) const {
    if (!stateAffectsLayout(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    const auto hasLayoutDeclaration = [](const StyleRule& rule) {
        return std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
    };
    for (const std::size_t ruleIndex : mLayoutStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!hasLayoutDeclaration(rule)) continue;
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, element)) return true;
        }
    }
    return false;
}

bool StyleRuleSet::stateAffectsHitTesting(ElementState state) const {
    return (mHitTestStateMask & static_cast<std::uint16_t>(state)) != 0;
}

bool StyleRuleSet::stateAffectsHitTesting(const Element& element, ElementState state) const {
    if (!stateAffectsHitTesting(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : mHitTestStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, element)) return true;
        }
    }
    return false;
}

bool StyleRuleSet::stateAffectsDescendants(const Element& element, ElementState state) const {
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : mDescendantStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                          [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState
                && *ownerState == state
                && selectorCanBeOwnedBy(selector, element)
                && (selectorIndex + 1 < rule.selectors.size() || inherits))
                return true;
        }
    }
    return false;
}

ComputedStyle StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t states,
                                  LayoutDirection direction) const {
    return mImpl->ruleSet.resolveInternal(element, id, classes, states, {}, nullptr, nullptr, direction);
}

ComputedStyle StyleSheet::resolveElement(const Element& element, LayoutDirection direction) const {
    return mImpl->ruleSet.resolveInternal(element.elementName(), element.id(), element.classes(), element.states(), {}, &element, nullptr, direction);
}

ComputedStyle StyleSheet::resolvePseudoElement(const Element& owner, std::string_view pseudoElementName, LayoutDirection direction) const {
    return mImpl->ruleSet.resolveInternal(owner.elementName(), owner.id(), owner.classes(), owner.states(), pseudoElementName, &owner, nullptr,
                                          direction);
}

ComputedStyle StyleSheet::resolveInline(const Element& owner, const std::string& element, const std::vector<std::string>& inlineAncestors,
                                        LayoutDirection direction) const {
    static const std::set<std::string> sNoClasses;
    return mImpl->ruleSet.resolveInternal(element, {}, sNoClasses, 0, {}, &owner, &inlineAncestors, direction);
}

ComputedStyle StyleRuleSet::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes,
                                            uint16_t ownerStates, std::string_view pseudoElement, const Element* target,
                                            const std::vector<std::string>* inlineAncestors, LayoutDirection direction) const {
    std::vector<std::size_t> candidates = mUniversalRuleIndices;
    if (const auto found = mElementRuleIndices.find(element); found != mElementRuleIndices.end())
        candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    if (!id.empty())
        if (const auto found = mIdRuleIndices.find(id); found != mIdRuleIndices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    for (const std::string& className : classes)
        if (const auto found = mClassRuleIndices.find(className); found != mClassRuleIndices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    std::sort(candidates.begin(), candidates.end());
    candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());

    ComputedStyle style;
    for (const std::size_t ruleIndex : candidates) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!matchesRule(rule, element, id, classes, ownerStates, pseudoElement, target, inlineAncestors, direction)) continue;
        for (const StyleDeclaration& declaration : rule.declarations) detail::applyStyleDeclaration(style, declaration);
    }
    normalizeOverflow(style);
    resolveLightDarkColors(style);
    return style;
}
} // namespace radia::ui
