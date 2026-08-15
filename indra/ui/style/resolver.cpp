/**
 * @file resolver.cpp
 * @brief Resolves stylesheet rules into styles for individual Widgets and parts.
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

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "style/model.h"
#include "style/stylesheet.h"
#include "widgets/widget.h"

namespace radia::ui {
namespace {
bool matchesState(const std::string& state, uint8_t states) {
    if (state.empty()) return true;
    if (state == "hover") return hasState(states, WidgetState::Hovered);
    if (state == "active") return hasState(states, WidgetState::Active);
    if (state == "focus") return hasState(states, WidgetState::Focused);
    if (state == "focus-visible") return hasState(states, WidgetState::Focused) && hasState(states, WidgetState::FocusVisible);
    if (state == "disabled") return hasState(states, WidgetState::Disabled);
    if (state == "checked") return hasState(states, WidgetState::Checked);
    if (state == "minimized") return hasState(states, WidgetState::Minimized);
    if (state == "invalid") return hasState(states, WidgetState::Invalid);
    return false;
}

std::optional<WidgetState> selectorState(const std::string& state) {
    if (state == "hover") return WidgetState::Hovered;
    if (state == "active") return WidgetState::Active;
    if (state == "focus") return WidgetState::Focused;
    if (state == "focus-visible") return WidgetState::FocusVisible;
    if (state == "disabled") return WidgetState::Disabled;
    if (state == "checked") return WidgetState::Checked;
    if (state == "minimized") return WidgetState::Minimized;
    if (state == "invalid") return WidgetState::Invalid;
    return std::nullopt;
}

bool selectorCanBeOwnedBy(const StyleSelector& selector, const Widget& widget) {
    return (selector.element.empty() || selector.element == widget.styleElement())
        && (selector.id.empty() || selector.id == widget.id())
        && (selector.className.empty() || widget.classes().find(selector.className) != widget.classes().end());
}

std::optional<std::size_t> stateIndex(WidgetState state) {
    const std::uint8_t bit = static_cast<std::uint8_t>(state);
    if (bit == 0 || (bit & static_cast<std::uint8_t>(bit - 1)) != 0) return std::nullopt;
    std::size_t index = 0;
    for (std::uint8_t value = bit; value > 1; value >>= 1) ++index;
    return index;
}

int specificity(const StyleSelector& selector) {
    return (!selector.id.empty() ? 100 : 0)
        + (!selector.className.empty() ? 10 : 0)
        + (!selector.state.empty() ? 10 : 0)
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
                     uint8_t ownerStates, const std::vector<std::string>& parts, uint8_t partStates) {
    return (selector.element.empty() || selector.element == element)
        && (selector.id.empty() || selector.id == id)
        && (selector.className.empty() || classes.find(selector.className) != classes.end())
        && matchesState(selector.state, ownerStates)
        && matchesState(selector.partState, partStates)
        && selector.parts == parts;
}

const Widget* structuralParent(const Widget* widget) {
    if (!widget) return nullptr;
    const Widget* parent = widget->parent();
    while (parent && !parent->part().empty()) parent = parent->parent();
    return parent;
}

bool matchesStructuralSelector(const StyleSelector& selector, const Widget& widget) {
    static const std::vector<std::string> sNoParts;
    return matchesSelector(selector, widget.styleElement(), widget.id(), widget.classes(), widget.states(), sNoParts, 0);
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t ownerStates,
                 const std::vector<std::string>& parts, uint8_t partStates, const Widget* widget, const std::vector<std::string>* inlineAncestors) {
    if (rule.selectors.empty() || !matchesSelector(rule.selectors.back(), element, id, classes, ownerStates, parts, partStates)) return false;
    if (rule.selectors.size() == 1) return true;
    if (!widget || rule.combinators.size() + 1 != rule.selectors.size()) return false;

    std::size_t inlineIndex = inlineAncestors ? inlineAncestors->size() : 0;
    const Widget* ancestor = inlineAncestors ? widget : structuralParent(widget);
    const auto nextAncestorMatches = [&](const StyleSelector& selector) -> std::optional<bool> {
        static const std::set<std::string> sNoClasses;
        static const std::vector<std::string> sNoParts;
        if (inlineAncestors && inlineIndex) {
            const std::string& inlineElement = (*inlineAncestors)[--inlineIndex];
            return matchesSelector(selector, inlineElement, {}, sNoClasses, 0, sNoParts, 0);
        }
        if (!ancestor) return std::nullopt;
        const Widget* candidate = ancestor;
        ancestor = structuralParent(ancestor);
        return matchesStructuralSelector(selector, *candidate);
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

bool StyleModel::stateAffectsLayout(WidgetState state) const {
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
                const auto addState = [&](const std::optional<WidgetState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    layoutStateMask |= static_cast<std::uint8_t>(*candidate);
                    auto& candidates = layoutStateRules[*index];
                    if (std::find(candidates.begin(), candidates.end(), ruleIndex) == candidates.end()) candidates.push_back(ruleIndex);
                };
                addState(selectorState(selector.state));
                addState(selectorState(selector.partState));
            }
        }
        layoutStateMaskValid = true;
    }
    return (layoutStateMask & static_cast<std::uint8_t>(state)) != 0;
}

bool StyleModel::stateAffectsLayout(const Widget& widget, WidgetState state) const {
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
            const std::optional<WidgetState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, widget)) return true;
            const std::optional<WidgetState> partState = selectorState(selector.partState);
            if (partState && *partState == state && !widget.part().empty() && selectorCanBeOwnedBy(selector, widget)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsHitTesting(WidgetState state) const {
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
                const auto addState = [&](const std::optional<WidgetState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    hitTestStateMask |= static_cast<std::uint8_t>(*candidate);
                    auto& candidates = hitTestStateRules[*index];
                    if (std::find(candidates.begin(), candidates.end(), ruleIndex) == candidates.end()) candidates.push_back(ruleIndex);
                };
                addState(selectorState(selector.state));
                addState(selectorState(selector.partState));
            }
        }
        hitTestStateMaskValid = true;
    }
    return (hitTestStateMask & static_cast<std::uint8_t>(state)) != 0;
}

bool StyleModel::stateAffectsHitTesting(const Widget& widget, WidgetState state) const {
    if (!stateAffectsHitTesting(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : hitTestStateRules[*index]) {
        const StyleRule& rule = rules[ruleIndex];
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<WidgetState> ownerState = selectorState(selector.state);
            if (ownerState && *ownerState == state && selectorCanBeOwnedBy(selector, widget)) return true;
            const std::optional<WidgetState> partState = selectorState(selector.partState);
            if (partState && *partState == state && !widget.part().empty() && selectorCanBeOwnedBy(selector, widget)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsDescendants(const Widget& widget, WidgetState state) const {
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
                const std::optional<WidgetState> selectorStateValue = selectorState(selector.state);
                const std::optional<WidgetState> partState = selectorState(selector.partState);
                const auto addCandidate = [&](const std::optional<WidgetState>& candidate, bool partOwned) {
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
            const std::optional<WidgetState> ownerState = selectorState(selector.state);
            if (ownerState
                && *ownerState == state
                && selectorCanBeOwnedBy(selector, widget)
                && (selectorIndex + 1 < rule.selectors.size() || !selector.parts.empty() || inherits))
                return true;
            const std::optional<WidgetState> partState = selectorState(selector.partState);
            if (partState
                && *partState == state
                && !widget.part().empty()
                && selectorCanBeOwnedBy(selector, widget)
                && (selectorIndex + 1 < rule.selectors.size() || inherits))
                return true;
        }
    }
    return false;
}

void StyleModel::sortRules() {
    std::stable_sort(rules.begin(), rules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        const int left = specificity(lhs);
        const int right = specificity(rhs);
        return left == right ? lhs.sourceOrder < rhs.sourceOrder : left < right;
    });
    layoutStateMaskValid = false;
    hitTestStateMaskValid = false;
    descendantStateRulesValid = false;
    ruleIndexValid = false;
}

Style StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t states) const {
    return mImpl->resolveInternal(element, id, classes, states, {}, 0);
}

Style StyleSheet::resolvePart(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t ownerStates,
                              const std::string& part, uint8_t partStates) const {
    return mImpl->resolveInternal(element, id, classes, ownerStates, detail::splitPartPath(part), partStates);
}

Style StyleSheet::resolveWidget(const Widget& widget) const {
    return mImpl->resolveInternal(widget.styleElement(), widget.id(), widget.classes(), widget.states(), {}, 0, &widget);
}

Style StyleSheet::resolveWidgetPart(const Widget& owner, const Widget& part) const {
    return mImpl->resolveInternal(owner.styleElement(), owner.id(), owner.classes(), owner.states(), detail::splitPartPath(part.part()),
                                  part.states(), &owner);
}

Style StyleSheet::resolveInline(const Widget& owner, const std::string& element, const std::vector<std::string>& inlineAncestors) const {
    static const std::set<std::string> sNoClasses;
    static const std::vector<std::string> sNoParts;
    return mImpl->resolveInternal(element, {}, sNoClasses, 0, sNoParts, 0, &owner, &inlineAncestors);
}

Style StyleModel::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t ownerStates,
                                  const std::vector<std::string>& parts, uint8_t partStates, const Widget* widget,
                                  const std::vector<std::string>* inlineAncestors) const {
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
        if (!matchesRule(rule, element, id, classes, ownerStates, parts, partStates, widget, inlineAncestors)) continue;
        for (const StyleDeclaration& declaration : rule.declarations) detail::applyStyleDeclaration(style, declaration);
    }
    return style;
}
} // namespace radia::ui
