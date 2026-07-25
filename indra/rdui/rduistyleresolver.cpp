#include "linden_common.h"
#include "rduistylecompiler.h"
#include "rduistylesheet.h"
#include "rduiwidget.h"
#include <algorithm>
#include <optional>

namespace rdui
{
    namespace
    {
        bool matchesState(const std::string& state, uint8_t states)
        {
            if (state.empty()) return true;
            if (state == "hover") return has_state(states, WidgetState::Hovered);
            if (state == "active") return has_state(states, WidgetState::Active);
            if (state == "focus") return has_state(states, WidgetState::Focused);
            if (state == "focus-visible")
                return has_state(states, WidgetState::Focused) && has_state(states, WidgetState::FocusVisible);
            if (state == "disabled") return has_state(states, WidgetState::Disabled);
            if (state == "checked") return has_state(states, WidgetState::Checked);
            if (state == "minimized") return has_state(states, WidgetState::Minimized);
            if (state == "invalid") return has_state(states, WidgetState::Invalid);
            return false;
        }

        int specificity(const StyleSelector& selector)
        {
            return (!selector.id.empty() ? 100 : 0)
                 + (!selector.class_name.empty() ? 10 : 0)
                 + (!selector.state.empty() ? 10 : 0)
                 + (!selector.part_state.empty() ? 10 : 0)
                 + (!selector.element.empty() ? 1 : 0)
                 + static_cast<int>(selector.parts.size());
        }

        int specificity(const StyleRule& rule)
        {
            int result = 0;
            for (const StyleSelector& selector : rule.selectors) result += specificity(selector);
            return result;
        }

        bool matchesSelector(const StyleSelector& selector,
                             const std::string& element,
                             const std::string& id,
                             const std::set<std::string>& classes,
                             uint8_t owner_states,
                             const std::vector<std::string>& parts,
                             uint8_t part_states)
        {
            return (selector.element.empty() || selector.element == element)
                && (selector.id.empty() || selector.id == id)
                && (selector.class_name.empty() || classes.find(selector.class_name) != classes.end())
                && matchesState(selector.state, owner_states)
                && matchesState(selector.part_state, part_states)
                && selector.parts == parts;
        }

        const Widget* structuralParent(const Widget* widget)
        {
            if (!widget) return nullptr;
            const Widget* parent = widget->parent();
            while (parent && !parent->part().empty()) parent = parent->parent();
            return parent;
        }

        bool matchesStructuralSelector(const StyleSelector& selector, const Widget& widget)
        {
            static const std::vector<std::string> no_parts;
            return matchesSelector(selector, widget.styleElement(), widget.id(), widget.classes(),
                                   widget.states(), no_parts, 0);
        }

        bool matchesRule(const StyleRule& rule,
                         const std::string& element,
                         const std::string& id,
                         const std::set<std::string>& classes,
                         uint8_t owner_states,
                         const std::vector<std::string>& parts,
                         uint8_t part_states,
                         const Widget* widget,
                         const std::vector<std::string>* inline_ancestors)
        {
            if (rule.selectors.empty()
                || !matchesSelector(rule.selectors.back(), element, id, classes, owner_states, parts, part_states))
                return false;
            if (rule.selectors.size() == 1) return true;
            if (!widget || rule.combinators.size() + 1 != rule.selectors.size()) return false;

            std::size_t inline_index = inline_ancestors ? inline_ancestors->size() : 0;
            const Widget* ancestor = inline_ancestors ? widget : structuralParent(widget);
            const auto nextAncestorMatches = [&](const StyleSelector& selector) -> std::optional<bool>
            {
                static const std::set<std::string> no_classes;
                static const std::vector<std::string> no_parts;
                if (inline_ancestors && inline_index)
                {
                    const std::string& inline_element = (*inline_ancestors)[--inline_index];
                    return matchesSelector(selector, inline_element, {}, no_classes, 0, no_parts, 0);
                }
                if (!ancestor) return std::nullopt;
                const Widget* candidate = ancestor;
                ancestor = structuralParent(ancestor);
                return matchesStructuralSelector(selector, *candidate);
            };
            for (std::size_t index = rule.selectors.size() - 1; index-- > 0;)
            {
                const StyleSelector& selector = rule.selectors[index];
                if (rule.combinators[index] == SelectorCombinator::Child)
                {
                    const std::optional<bool> matches = nextAncestorMatches(selector);
                    if (!matches || !*matches) return false;
                    continue;
                }
                std::optional<bool> matches;
                do matches = nextAncestorMatches(selector); while (matches && !*matches);
                if (!matches) return false;
            }
            return true;
        }
    }

    void StyleSheet::Impl::addRule(const StyleRule& rule)
    {
        StyleRule copy = rule;
        copy.source_order = static_cast<int>(rules.size());
        rules.push_back(std::move(copy));
        std::stable_sort(rules.begin(), rules.end(), [](const StyleRule& lhs, const StyleRule& rhs)
        {
            const int left = specificity(lhs);
            const int right = specificity(rhs);
            return left == right ? lhs.source_order < rhs.source_order : left < right;
        });
    }

    Style StyleSheet::resolve(const std::string& element, const std::string& id,
                              const std::set<std::string>& classes, uint8_t states) const
    {
        return mImpl->resolveInternal(element, id, classes, states, {}, 0);
    }

    Style StyleSheet::resolvePart(const std::string& element, const std::string& id,
                                  const std::set<std::string>& classes, uint8_t owner_states,
                                  const std::string& part, uint8_t part_states) const
    {
        return mImpl->resolveInternal(element, id, classes, owner_states,
                                      detail::splitPartPath(part), part_states);
    }

    Style StyleSheet::resolveWidget(const Widget& widget) const
    {
        return mImpl->resolveInternal(widget.styleElement(), widget.id(), widget.classes(),
                                      widget.states(), {}, 0, &widget);
    }

    Style StyleSheet::resolveWidgetPart(const Widget& owner, const Widget& part) const
    {
        return mImpl->resolveInternal(owner.styleElement(), owner.id(), owner.classes(), owner.states(),
                                      detail::splitPartPath(part.part()), part.states(), &owner);
    }

    Style StyleSheet::resolveInline(const Widget& owner, const std::string& element,
                                    const std::vector<std::string>& inline_ancestors) const
    {
        static const std::set<std::string> no_classes;
        static const std::vector<std::string> no_parts;
        return mImpl->resolveInternal(element, {}, no_classes, 0, no_parts, 0,
                                      &owner, &inline_ancestors);
    }

    Style StyleSheet::Impl::resolveInternal(const std::string& element,
                                      const std::string& id,
                                      const std::set<std::string>& classes,
                                      uint8_t owner_states,
                                      const std::vector<std::string>& parts,
                                      uint8_t part_states,
                                      const Widget* widget,
                                      const std::vector<std::string>* inline_ancestors) const
    {
        Style style;
        for (const StyleRule& rule : rules)
        {
            if (!matchesRule(rule, element, id, classes, owner_states, parts, part_states,
                             widget, inline_ancestors)) continue;
            for (const StyleDeclaration& declaration : rule.declarations)
                detail::applyStyleDeclaration(style, declaration);
        }
        return style;
    }
}
