/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "elements/element.h"
#include "layout/schema.h"
#include "style/model.h"
#include "style/stylesheet.h"

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
    if (selector.attributeName.empty()) return true;
    if (!element) return false;
    const std::string* value = detail::styleAttribute(*element, selector.attributeName);
    if (selector.attributePresence) return value != nullptr;
    if (selector.attributeName == "name") return value && *value == selector.attributeValue;
    return value && schemaNameKey(*value) == schemaNameKey(selector.attributeValue);
}

bool matchesRoot(const StyleSelector& selector, const Element* element) {
    return !selector.root || (element && element->parentElement() == nullptr);
}

bool selectorCanBeOwnedBy(const StyleSelector& selector, const Element& element) {
    return (selector.element.empty() || selector.element == element.styleElement())
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
        + (!selector.attributeName.empty() ? 10 : 0)
        + (!selector.className.empty() ? 10 : 0)
        + (!selector.state.empty() ? 10 : 0)
        + (selector.direction ? 10 : 0)
        + (!selector.partState.empty() ? 10 : 0)
        + (!selector.element.empty() ? 1 : 0)
        + static_cast<int>(selector.parts.size());
}

int specificity(const StyleRule& rule) {
    int result = 0;
    for (const StyleSelector& selector : rule.selectors) result += specificity(selector);
    return result;
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
                     uint16_t ownerStates, const std::vector<std::string>& parts, uint16_t partStates, const Element* target,
                     LayoutDirection direction) {
    return (selector.element.empty() || selector.element == element)
        && matchesRoot(selector, target)
        && matchesAttribute(selector, target)
        && (selector.id.empty() || selector.id == id)
        && (selector.className.empty() || classes.find(selector.className) != classes.end())
        && matchesDirection(selector.direction, direction)
        && matchesState(selector.state, ownerStates)
        && matchesState(selector.partState, partStates)
        && selector.parts == parts;
}

const Element* structuralParent(const Element* element) {
    if (!element) return nullptr;
    const Element* parent = element->parentElement();
    while (parent && !parent->part().empty()) parent = parent->parentElement();
    return parent;
}

bool matchesStructuralSelector(const StyleSelector& selector, const Element& element, LayoutDirection direction) {
    static const std::vector<std::string> sNoParts;
    return matchesSelector(selector, element.styleElement(), element.id(), element.classes(), element.states(), sNoParts, 0, &element, direction);
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                 const std::vector<std::string>& parts, uint16_t partStates, const Element* target, const std::vector<std::string>* inlineAncestors,
                 LayoutDirection direction) {
    if (rule.selectors.empty() || !matchesSelector(rule.selectors.back(), element, id, classes, ownerStates, parts, partStates, target, direction))
        return false;
    if (rule.selectors.size() == 1) return true;
    if (!target || rule.combinators.size() + 1 != rule.selectors.size()) return false;

    std::size_t inlineIndex = inlineAncestors ? inlineAncestors->size() : 0;
    const Element* ancestor = inlineAncestors ? target : structuralParent(target);
    const auto nextAncestorMatches = [&](const StyleSelector& selector) -> std::optional<bool> {
        static const std::set<std::string> sNoClasses;
        static const std::vector<std::string> sNoParts;
        if (inlineAncestors && inlineIndex) {
            const std::string& inlineElement = (*inlineAncestors)[--inlineIndex];
            return matchesSelector(selector, inlineElement, {}, sNoClasses, 0, sNoParts, 0, nullptr, direction);
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
    layoutStateMaskValid = false;
    hitTestStateMaskValid = false;
    descendantStateRulesValid = false;
    ruleIndexValid = false;
}

bool StyleModel::stateAffectsLayout(ElementState state) const {
    if (!layoutStateMaskValid) {
        layoutStateMask = 0;
        for (auto& candidates : layoutStateRules) candidates.clear();
        for (std::size_t ruleIndex = 0; ruleIndex < rules.size(); ++ruleIndex) {
            const StyleRule& rule = rules[ruleIndex];
            bool layoutDeclaration = false;
            for (const StyleDeclaration& declaration : rule.declarations)
                if (!declaration.property.get().isPaintOnly()) {
                    layoutDeclaration = true;
                    break;
                }
            if (!layoutDeclaration) continue;
            for (const StyleSelector& selector : rule.selectors) {
                const auto addState = [&](const std::optional<ElementState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    layoutStateMask |= static_cast<std::uint16_t>(*candidate);
                    auto& candidates = layoutStateRules[*index];
                    if (std::find(candidates.begin(), candidates.end(), ruleIndex) == candidates.end()) candidates.push_back(ruleIndex);
                };
                addState(selectorState(selector.state));
                addState(selectorState(selector.partState));
            }
        }
        layoutStateMaskValid = true;
    }
    return (layoutStateMask & static_cast<std::uint16_t>(state)) != 0;
}

bool StyleModel::stateAffectsLayout(const Element& element, ElementState state) const {
    if (!stateAffectsLayout(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    const auto hasLayoutDeclaration = [](const StyleRule& rule) {
        return std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
    };
    for (const std::size_t ruleIndex : layoutStateRules[*index]) {
        const StyleRule& rule = rules[ruleIndex];
        if (!hasLayoutDeclaration(rule)) continue;
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, element)) return true;
            const std::optional<ElementState> partState = selectorState(selector.partState);
            if (partState && *partState == state && !element.part().empty() && selectorCanBeOwnedBy(selector, element)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsHitTesting(ElementState state) const {
    if (!hitTestStateMaskValid) {
        hitTestStateMask = 0;
        for (auto& candidates : hitTestStateRules) candidates.clear();
        for (std::size_t ruleIndex = 0; ruleIndex < rules.size(); ++ruleIndex) {
            const StyleRule& rule = rules[ruleIndex];
            const bool hitTestDeclaration = std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                return declaration.property.get().affectsHitTesting();
            });
            if (!hitTestDeclaration) continue;
            for (const StyleSelector& selector : rule.selectors) {
                const auto addState = [&](const std::optional<ElementState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    hitTestStateMask |= static_cast<std::uint16_t>(*candidate);
                    auto& candidates = hitTestStateRules[*index];
                    if (std::find(candidates.begin(), candidates.end(), ruleIndex) == candidates.end()) candidates.push_back(ruleIndex);
                };
                addState(selectorState(selector.state));
                addState(selectorState(selector.partState));
            }
        }
        hitTestStateMaskValid = true;
    }
    return (hitTestStateMask & static_cast<std::uint16_t>(state)) != 0;
}

bool StyleModel::stateAffectsHitTesting(const Element& element, ElementState state) const {
    if (!stateAffectsHitTesting(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : hitTestStateRules[*index]) {
        const StyleRule& rule = rules[ruleIndex];
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, element)) return true;
            const std::optional<ElementState> partState = selectorState(selector.partState);
            if (partState && *partState == state && !element.part().empty() && selectorCanBeOwnedBy(selector, element)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsDescendants(const Element& element, ElementState state) const {
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    if (!descendantStateRulesValid) {
        for (auto& candidates : descendantStateRules) candidates.clear();
        for (std::size_t ruleIndex = 0; ruleIndex < rules.size(); ++ruleIndex) {
            const StyleRule& rule = rules[ruleIndex];
            const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                              [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
            for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
                const StyleSelector& selector = rule.selectors[selectorIndex];
                const std::optional<ElementState> selectorStateValue = selectorState(selector.state);
                const std::optional<ElementState> partState = selectorState(selector.partState);
                const auto addCandidate = [&](const std::optional<ElementState>& candidate, bool partOwned) {
                    if (!candidate) return;
                    const std::optional<std::size_t> candidateStateIndex = stateIndex(*candidate);
                    if (!candidateStateIndex) return;
                    if (selectorIndex + 1 < rule.selectors.size() || (!partOwned && !selector.parts.empty()) || inherits)
                        descendantStateRules[*candidateStateIndex].push_back(ruleIndex);
                };
                addCandidate(selectorStateValue, false);
                addCandidate(partState, true);
            }
        }
        for (auto& candidates : descendantStateRules) {
            std::sort(candidates.begin(), candidates.end());
            candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());
        }
        descendantStateRulesValid = true;
    }
    for (const std::size_t ruleIndex : descendantStateRules[*index]) {
        const StyleRule& rule = rules[ruleIndex];
        const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                          [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            const std::optional<ElementState> ownerState = selectorState(selector.state);
            if (ownerState
                && *ownerState == state
                && selectorCanBeOwnedBy(selector, element)
                && (selectorIndex + 1 < rule.selectors.size() || !selector.parts.empty() || inherits))
                return true;
            const std::optional<ElementState> partState = selectorState(selector.partState);
            if (partState
                && *partState == state
                && !element.part().empty()
                && selectorCanBeOwnedBy(selector, element)
                && (selectorIndex + 1 < rule.selectors.size() || inherits))
                return true;
        }
    }
    return false;
}

void StyleModel::sortRules() {
    std::stable_sort(rules.begin(), rules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        if (lhs.origin != rhs.origin) return static_cast<std::uint8_t>(lhs.origin) < static_cast<std::uint8_t>(rhs.origin);
        const int left = specificity(lhs);
        const int right = specificity(rhs);
        return left == right ? lhs.sourceOrder < rhs.sourceOrder : left < right;
    });
    layoutStateMaskValid = false;
    hitTestStateMaskValid = false;
    descendantStateRulesValid = false;
    ruleIndexValid = false;
}

Style StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t states,
                          LayoutDirection direction) const {
    return mImpl->resolveInternal(element, id, classes, states, {}, 0, nullptr, nullptr, direction);
}

Style StyleSheet::resolvePart(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                              const std::string& part, uint16_t partStates, LayoutDirection direction) const {
    return mImpl->resolveInternal(element, id, classes, ownerStates, detail::splitPartPath(part), partStates, nullptr, nullptr, direction);
}

Style StyleSheet::resolveElement(const Element& element, LayoutDirection direction) const {
    return mImpl->resolveInternal(element.styleElement(), element.id(), element.classes(), element.states(), {}, 0, &element, nullptr, direction);
}

Style StyleSheet::resolveElementPart(const Element& owner, const Element& part, LayoutDirection direction) const {
    return mImpl->resolveInternal(owner.styleElement(), owner.id(), owner.classes(), owner.states(), detail::splitPartPath(part.part()),
                                  part.states(), &owner, nullptr, direction);
}

Style StyleSheet::resolveInline(const Element& owner, const std::string& element, const std::vector<std::string>& inlineAncestors,
                                LayoutDirection direction) const {
    static const std::set<std::string> sNoClasses;
    static const std::vector<std::string> sNoParts;
    return mImpl->resolveInternal(element, {}, sNoClasses, 0, sNoParts, 0, &owner, &inlineAncestors, direction);
}

Style StyleModel::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                                  const std::vector<std::string>& parts, uint16_t partStates, const Element* target,
                                  const std::vector<std::string>* inlineAncestors, LayoutDirection direction) const {
    if (!ruleIndexValid) {
        universalRuleIndices.clear();
        elementRuleIndices.clear();
        idRuleIndices.clear();
        classRuleIndices.clear();
        const auto addIndex = [](auto& index, const std::string& key, std::size_t ruleIndex) {
            if (!key.empty()) index[key].push_back(ruleIndex);
        };
        for (std::size_t ruleIndex = 0; ruleIndex < rules.size(); ++ruleIndex) {
            if (rules[ruleIndex].selectors.empty()) continue;
            const StyleSelector& selector = rules[ruleIndex].selectors.back();
            if (selector.element.empty() && selector.id.empty() && selector.className.empty()) universalRuleIndices.push_back(ruleIndex);
            addIndex(elementRuleIndices, selector.element, ruleIndex);
            addIndex(idRuleIndices, selector.id, ruleIndex);
            addIndex(classRuleIndices, selector.className, ruleIndex);
        }
        const auto sortUnique = [](auto& index) {
            for (auto& [key, values] : index) {
                std::sort(values.begin(), values.end());
                values.erase(std::unique(values.begin(), values.end()), values.end());
            }
        };
        std::sort(universalRuleIndices.begin(), universalRuleIndices.end());
        sortUnique(elementRuleIndices);
        sortUnique(idRuleIndices);
        sortUnique(classRuleIndices);
        ruleIndexValid = true;
    }

    std::vector<std::size_t> candidates = universalRuleIndices;
    if (const auto found = elementRuleIndices.find(element); found != elementRuleIndices.end())
        candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    if (!id.empty())
        if (const auto found = idRuleIndices.find(id); found != idRuleIndices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    for (const std::string& className : classes)
        if (const auto found = classRuleIndices.find(className); found != classRuleIndices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    std::sort(candidates.begin(), candidates.end());
    candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());

    Style style;
    for (const std::size_t ruleIndex : candidates) {
        const StyleRule& rule = rules[ruleIndex];
        if (!matchesRule(rule, element, id, classes, ownerStates, parts, partStates, target, inlineAncestors, direction)) continue;
        for (const StyleDeclaration& declaration : rule.declarations) detail::applyStyleDeclaration(style, declaration);
    }
    resolveLightDarkColors(style);
    return style;
}
} // namespace radia::ui
