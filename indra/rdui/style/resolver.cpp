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

namespace rdui {
namespace {
bool matchesState(const std::string& state, uint8_t states) {
    if (state.empty()) return true;
    if (state == "hover") return has_state(states, WidgetState::Hovered);
    if (state == "active") return has_state(states, WidgetState::Active);
    if (state == "focus") return has_state(states, WidgetState::Focused);
    if (state == "focus-visible") return has_state(states, WidgetState::Focused) && has_state(states, WidgetState::FocusVisible);
    if (state == "disabled") return has_state(states, WidgetState::Disabled);
    if (state == "checked") return has_state(states, WidgetState::Checked);
    if (state == "minimized") return has_state(states, WidgetState::Minimized);
    if (state == "invalid") return has_state(states, WidgetState::Invalid);
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
        && (selector.class_name.empty() || widget.classes().find(selector.class_name) != widget.classes().end());
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
        + (!selector.class_name.empty() ? 10 : 0)
        + (!selector.state.empty() ? 10 : 0)
        + (!selector.part_state.empty() ? 10 : 0)
        + (!selector.element.empty() ? 1 : 0)
        + static_cast<int>(selector.parts.size());
}

int specificity(const StyleRule& rule) {
    int result = 0;
    for (const StyleSelector& selector : rule.selectors) result += specificity(selector);
    return result;
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
                     uint8_t owner_states, const std::vector<std::string>& parts, uint8_t part_states) {
    return (selector.element.empty() || selector.element == element)
        && (selector.id.empty() || selector.id == id)
        && (selector.class_name.empty() || classes.find(selector.class_name) != classes.end())
        && matchesState(selector.state, owner_states)
        && matchesState(selector.part_state, part_states)
        && selector.parts == parts;
}

const Widget* structuralParent(const Widget* widget) {
    if (!widget) return nullptr;
    const Widget* parent = widget->parent();
    while (parent && !parent->part().empty()) parent = parent->parent();
    return parent;
}

bool matchesStructuralSelector(const StyleSelector& selector, const Widget& widget) {
    static const std::vector<std::string> no_parts;
    return matchesSelector(selector, widget.styleElement(), widget.id(), widget.classes(), widget.states(), no_parts, 0);
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t owner_states,
                 const std::vector<std::string>& parts, uint8_t part_states, const Widget* widget, const std::vector<std::string>* inline_ancestors) {
    if (rule.selectors.empty() || !matchesSelector(rule.selectors.back(), element, id, classes, owner_states, parts, part_states)) return false;
    if (rule.selectors.size() == 1) return true;
    if (!widget || rule.combinators.size() + 1 != rule.selectors.size()) return false;

    std::size_t inline_index = inline_ancestors ? inline_ancestors->size() : 0;
    const Widget* ancestor = inline_ancestors ? widget : structuralParent(widget);
    const auto nextAncestorMatches = [&](const StyleSelector& selector) -> std::optional<bool> {
        static const std::set<std::string> no_classes;
        static const std::vector<std::string> no_parts;
        if (inline_ancestors && inline_index) {
            const std::string& inline_element = (*inline_ancestors)[--inline_index];
            return matchesSelector(selector, inline_element, {}, no_classes, 0, no_parts, 0);
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
    copy.source_order = static_cast<int>(rules.size());
    rules.push_back(std::move(copy));
    layout_state_mask_valid = false;
    hit_test_state_mask_valid = false;
    descendant_state_rules_valid = false;
    rule_index_valid = false;
}

bool StyleModel::stateAffectsLayout(WidgetState state) const {
    if (!layout_state_mask_valid) {
        layout_state_mask = 0;
        for (auto& candidates : layout_state_rules) candidates.clear();
        for (std::size_t rule_index = 0; rule_index < rules.size(); ++rule_index) {
            const StyleRule& rule = rules[rule_index];
            bool layout_declaration = false;
            for (const StyleDeclaration& declaration : rule.declarations)
                if (!declaration.property.get().isPaintOnly()) {
                    layout_declaration = true;
                    break;
                }
            if (!layout_declaration) continue;
            for (const StyleSelector& selector : rule.selectors) {
                const auto add_state = [&](const std::optional<WidgetState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    layout_state_mask |= static_cast<std::uint8_t>(*candidate);
                    auto& candidates = layout_state_rules[*index];
                    if (std::find(candidates.begin(), candidates.end(), rule_index) == candidates.end()) candidates.push_back(rule_index);
                };
                add_state(selectorState(selector.state));
                add_state(selectorState(selector.part_state));
            }
        }
        layout_state_mask_valid = true;
    }
    return (layout_state_mask & static_cast<std::uint8_t>(state)) != 0;
}

bool StyleModel::stateAffectsLayout(const Widget& widget, WidgetState state) const {
    if (!stateAffectsLayout(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    const auto has_layout_declaration = [](const StyleRule& rule) {
        return std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
    };
    for (const std::size_t rule_index : layout_state_rules[*index]) {
        const StyleRule& rule = rules[rule_index];
        if (!has_layout_declaration(rule)) continue;
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<WidgetState> owner_state = selectorState(selector.state);
            if (owner_state && *owner_state == state && selectorCanBeOwnedBy(selector, widget)) return true;
            // Part state belongs to the part widget itself, not its owner. A
            // state transition on a normal widget cannot affect that selector.
            const std::optional<WidgetState> part_state = selectorState(selector.part_state);
            if (part_state && *part_state == state && !widget.part().empty() && selectorCanBeOwnedBy(selector, widget)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsHitTesting(WidgetState state) const {
    if (!hit_test_state_mask_valid) {
        hit_test_state_mask = 0;
        for (auto& candidates : hit_test_state_rules) candidates.clear();
        for (std::size_t rule_index = 0; rule_index < rules.size(); ++rule_index) {
            const StyleRule& rule = rules[rule_index];
            const bool hit_test_declaration =
                std::any_of(rule.declarations.begin(), rule.declarations.end(),
                            [](const StyleDeclaration& declaration) { return declaration.property.get().affectsHitTesting(); });
            if (!hit_test_declaration) continue;
            for (const StyleSelector& selector : rule.selectors) {
                const auto add_state = [&](const std::optional<WidgetState>& candidate) {
                    if (!candidate) return;
                    const std::optional<std::size_t> index = stateIndex(*candidate);
                    if (!index) return;
                    hit_test_state_mask |= static_cast<std::uint8_t>(*candidate);
                    auto& candidates = hit_test_state_rules[*index];
                    if (std::find(candidates.begin(), candidates.end(), rule_index) == candidates.end()) candidates.push_back(rule_index);
                };
                add_state(selectorState(selector.state));
                add_state(selectorState(selector.part_state));
            }
        }
        hit_test_state_mask_valid = true;
    }
    return (hit_test_state_mask & static_cast<std::uint8_t>(state)) != 0;
}

bool StyleModel::stateAffectsHitTesting(const Widget& widget, WidgetState state) const {
    if (!stateAffectsHitTesting(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t rule_index : hit_test_state_rules[*index]) {
        const StyleRule& rule = rules[rule_index];
        for (const StyleSelector& selector : rule.selectors) {
            const std::optional<WidgetState> owner_state = selectorState(selector.state);
            if (owner_state && *owner_state == state && selectorCanBeOwnedBy(selector, widget)) return true;
            const std::optional<WidgetState> part_state = selectorState(selector.part_state);
            if (part_state && *part_state == state && !widget.part().empty() && selectorCanBeOwnedBy(selector, widget)) return true;
        }
    }
    return false;
}

bool StyleModel::stateAffectsDescendants(const Widget& widget, WidgetState state) const {
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    if (!descendant_state_rules_valid) {
        for (auto& candidates : descendant_state_rules) candidates.clear();
        for (std::size_t rule_index = 0; rule_index < rules.size(); ++rule_index) {
            const StyleRule& rule = rules[rule_index];
            const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                              [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
            for (std::size_t selector_index = 0; selector_index < rule.selectors.size(); ++selector_index) {
                const StyleSelector& selector = rule.selectors[selector_index];
                const std::optional<WidgetState> selector_state = selectorState(selector.state);
                const std::optional<WidgetState> part_state = selectorState(selector.part_state);
                const auto add_candidate = [&](const std::optional<WidgetState>& candidate, bool part_owned) {
                    if (!candidate) return;
                    const std::optional<std::size_t> state_index = stateIndex(*candidate);
                    if (!state_index) return;
                    if (selector_index + 1 < rule.selectors.size() || (!part_owned && !selector.parts.empty()) || inherits)
                        descendant_state_rules[*state_index].push_back(rule_index);
                };
                add_candidate(selector_state, false);
                add_candidate(part_state, true);
            }
        }
        for (auto& candidates : descendant_state_rules) {
            std::sort(candidates.begin(), candidates.end());
            candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());
        }
        descendant_state_rules_valid = true;
    }
    for (const std::size_t rule_index : descendant_state_rules[*index]) {
        const StyleRule& rule = rules[rule_index];
        const bool inherits = std::any_of(rule.declarations.begin(), rule.declarations.end(),
                                          [](const StyleDeclaration& declaration) { return declaration.property.get().isInherited(); });
        for (std::size_t selector_index = 0; selector_index < rule.selectors.size(); ++selector_index) {
            const StyleSelector& selector = rule.selectors[selector_index];
            const std::optional<WidgetState> owner_state = selectorState(selector.state);
            if (owner_state
                && *owner_state == state
                && selectorCanBeOwnedBy(selector, widget)
                && (selector_index + 1 < rule.selectors.size() || !selector.parts.empty() || inherits))
                return true;
            const std::optional<WidgetState> part_state = selectorState(selector.part_state);
            if (part_state
                && *part_state == state
                && !widget.part().empty()
                && selectorCanBeOwnedBy(selector, widget)
                && (selector_index + 1 < rule.selectors.size() || inherits))
                return true;
        }
    }
    return false;
}

void StyleModel::sortRules() {
    std::stable_sort(rules.begin(), rules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        const int left = specificity(lhs);
        const int right = specificity(rhs);
        return left == right ? lhs.source_order < rhs.source_order : left < right;
    });
    layout_state_mask_valid = false;
    hit_test_state_mask_valid = false;
    descendant_state_rules_valid = false;
    rule_index_valid = false;
}

Style StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t states) const {
    return mImpl->resolveInternal(element, id, classes, states, {}, 0);
}

Style StyleSheet::resolvePart(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t owner_states,
                              const std::string& part, uint8_t part_states) const {
    return mImpl->resolveInternal(element, id, classes, owner_states, detail::splitPartPath(part), part_states);
}

Style StyleSheet::resolveWidget(const Widget& widget) const {
    return mImpl->resolveInternal(widget.styleElement(), widget.id(), widget.classes(), widget.states(), {}, 0, &widget);
}

Style StyleSheet::resolveWidgetPart(const Widget& owner, const Widget& part) const {
    return mImpl->resolveInternal(owner.styleElement(), owner.id(), owner.classes(), owner.states(), detail::splitPartPath(part.part()),
                                  part.states(), &owner);
}

Style StyleSheet::resolveInline(const Widget& owner, const std::string& element, const std::vector<std::string>& inline_ancestors) const {
    static const std::set<std::string> no_classes;
    static const std::vector<std::string> no_parts;
    return mImpl->resolveInternal(element, {}, no_classes, 0, no_parts, 0, &owner, &inline_ancestors);
}

Style StyleModel::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t owner_states,
                                  const std::vector<std::string>& parts, uint8_t part_states, const Widget* widget,
                                  const std::vector<std::string>* inline_ancestors) const {
    if (!rule_index_valid) {
        universal_rule_indices.clear();
        element_rule_indices.clear();
        id_rule_indices.clear();
        class_rule_indices.clear();
        const auto add_index = [](auto& index, const std::string& key, std::size_t rule_index) {
            if (!key.empty()) index[key].push_back(rule_index);
        };
        for (std::size_t rule_index = 0; rule_index < rules.size(); ++rule_index) {
            if (rules[rule_index].selectors.empty()) continue;
            const StyleSelector& selector = rules[rule_index].selectors.back();
            if (selector.element.empty() && selector.id.empty() && selector.class_name.empty()) universal_rule_indices.push_back(rule_index);
            add_index(element_rule_indices, selector.element, rule_index);
            add_index(id_rule_indices, selector.id, rule_index);
            add_index(class_rule_indices, selector.class_name, rule_index);
        }
        const auto sort_unique = [](auto& index) {
            for (auto& [key, values] : index) {
                std::sort(values.begin(), values.end());
                values.erase(std::unique(values.begin(), values.end()), values.end());
            }
        };
        std::sort(universal_rule_indices.begin(), universal_rule_indices.end());
        sort_unique(element_rule_indices);
        sort_unique(id_rule_indices);
        sort_unique(class_rule_indices);
        rule_index_valid = true;
    }

    std::vector<std::size_t> candidates = universal_rule_indices;
    if (const auto found = element_rule_indices.find(element); found != element_rule_indices.end())
        candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    if (!id.empty())
        if (const auto found = id_rule_indices.find(id); found != id_rule_indices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    for (const std::string& class_name : classes)
        if (const auto found = class_rule_indices.find(class_name); found != class_rule_indices.end())
            candidates.insert(candidates.end(), found->second.begin(), found->second.end());
    std::sort(candidates.begin(), candidates.end());
    candidates.erase(std::unique(candidates.begin(), candidates.end()), candidates.end());

    Style style;
    for (const std::size_t rule_index : candidates) {
        const StyleRule& rule = rules[rule_index];
        if (!matchesRule(rule, element, id, classes, owner_states, parts, part_states, widget, inline_ancestors)) continue;
        for (const StyleDeclaration& declaration : rule.declarations) detail::applyStyleDeclaration(style, declaration);
    }
    return style;
}
} // namespace rdui
