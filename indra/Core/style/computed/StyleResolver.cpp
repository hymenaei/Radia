/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <vector>
#include "CSSRules.h"
#include "CSSTokenStream.h"
#include "Element.h"
#include "HTMLFloaterElement.h"
#include "HTMLInputElement.h"
#include "HTMLName.h"
#include "StyleProperty.h"
#include "StyleSheet.h"

namespace Core::Style {
namespace detail {
using CSS::detail::decodeIdentifier;
using CSS::detail::FunctionRange;
using CSS::detail::isTrivia;
using CSS::detail::isWhitespace;
using CSS::detail::kNoMatchingToken;
using CSS::detail::lower;
using CSS::detail::normalizeKeyword;
using CSS::detail::parseFunction;
using CSS::detail::parseUrl;
using CSS::detail::serializeRange;
using CSS::detail::splitOnDelimiter;
using CSS::detail::Token;
using CSS::detail::TokenKind;
using CSS::detail::TokenRange;
using CSS::detail::TokenStream;
using CSS::detail::trimRange;
} // namespace detail

using CSS::CustomPropertyDeclaration;
using CSS::DeferredStyleValue;
using CSS::PseudoClass;
using CSS::pseudoClassDescriptor;
using CSS::PseudoClassSpecificity;
using CSS::SelectorCombinator;
using CSS::StyleAttributeSelector;
using CSS::StyleModel;
using CSS::StyleResourceReference;
using CSS::StyleRule;
using CSS::StyleRuleSet;
using CSS::StyleSelector;
using CSS::StyleSelectorFunction;
using CSS::StyleSheet;
using CSS::StyleSheetLoadResult;
using CSS::StyleSpecificity;

namespace {
bool isChecked(const Element& element) {
    const auto* input = dynamic_cast<const HTMLInputElement*>(&element);
    return input && input->checked();
}

bool isIndeterminate(const Element& element) {
    const auto* input = dynamic_cast<const HTMLInputElement*>(&element);
    return input && (input->indeterminate() || input->radioGroupIsIndeterminate());
}

bool isInvalid(const Element& element) {
    const auto* input = dynamic_cast<const HTMLInputElement*>(&element);
    return input && input->invalid();
}

bool isMinimized(const Element& element) {
    const auto* floater = dynamic_cast<const HTMLFloaterElement*>(&element);
    return floater && floater->minimized();
}

bool matchesPseudoClass(CSS::PseudoClass pseudoClass, const Element* element,
    std::initializer_list<CSS::PseudoClass> matchingPseudoClasses) {
    if (pseudoClass == CSS::PseudoClass::Host)
        return false;
    const auto wasRequested = [matchingPseudoClasses](CSS::PseudoClass candidate) {
        return std::find(matchingPseudoClasses.begin(), matchingPseudoClasses.end(), candidate) != matchingPseudoClasses.end();
    };
    if (!element) {
        if (pseudoClass == CSS::PseudoClass::Root)
            return true;
        if (pseudoClass == CSS::PseudoClass::FocusVisible)
            return wasRequested(CSS::PseudoClass::Focus) && wasRequested(pseudoClass);
        return wasRequested(pseudoClass);
    }
    if (wasRequested(pseudoClass))
        return true;
    switch (pseudoClass) {
    case CSS::PseudoClass::Active:
        return element->active();
    case CSS::PseudoClass::Checked:
        return isChecked(*element);
    case CSS::PseudoClass::Disabled:
        return element->disabled();
    case CSS::PseudoClass::Dir:
        return false;
    case CSS::PseudoClass::Focus:
        return element->focused();
    case CSS::PseudoClass::FocusVisible:
        return element->focused() && element->focusVisible();
    case CSS::PseudoClass::Hover:
        return element->hovered();
    case CSS::PseudoClass::Indeterminate:
        return isIndeterminate(*element);
    case CSS::PseudoClass::Invalid:
        return isInvalid(*element);
    case CSS::PseudoClass::Minimized:
        return isMinimized(*element);
    case CSS::PseudoClass::Root:
        return element->parentElement() == nullptr || element->idScopeRoot();
    case CSS::PseudoClass::Is:
    case CSS::PseudoClass::Where:
        return false;
    case CSS::PseudoClass::Host:
        return false;
    }
    return false;
}

bool matchesAttribute(const StyleSelector& selector, const Element* element) {
    if (selector.attributes.empty())
        return true;
    if (!element)
        return false;
    return std::all_of(selector.attributes.begin(), selector.attributes.end(), [element](const StyleAttributeSelector& attribute) {
        const Element::Attribute* serialized = element->attribute(attribute.name);
        if (attribute.presence)
            return serialized != nullptr;
        if (!serialized || !serialized->value)
            return false;

        std::string actual = *serialized->value;
        std::string expected = attribute.value;
        if (attribute.caseInsensitive
            || (!attribute.caseSensitivitySpecified && (attribute.name == "type" || attribute.name == "switch"))) {
            actual = detail::lower(std::move(actual));
            expected = detail::lower(std::move(expected));
        }
        switch (attribute.match) {
        case StyleAttributeSelector::Match::Exact:
            return actual == expected;
        case StyleAttributeSelector::Match::Prefix:
            return actual.rfind(expected, 0) == 0;
        case StyleAttributeSelector::Match::Suffix:
            return actual.size() >= expected.size() && actual.compare(actual.size() - expected.size(), expected.size(), expected) == 0;
        case StyleAttributeSelector::Match::Substring:
            return actual.find(expected) != std::string::npos;
        case StyleAttributeSelector::Match::IncludesHyphen:
            return actual == expected || (actual.size() > expected.size() && actual.rfind(expected + '-', 0) == 0);
        case StyleAttributeSelector::Match::IncludesWord: {
            std::size_t start = 0;
            while (start < actual.size()) {
                while (start < actual.size() && detail::isWhitespace(actual[start]))
                    ++start;
                const std::size_t end = actual.find_first_of(" \t\r\n\f", start);
                if (actual.substr(start, end == std::string::npos ? std::string::npos : end - start) == expected)
                    return true;
                if (end == std::string::npos)
                    break;
                start = end;
            }
            return false;
        }
        }
        return false;
    });
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<CSS::PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target,
    const std::vector<std::string>* inlineAncestors);

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<CSS::PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target,
    const std::vector<std::string>* inlineAncestors);

bool selectorHasPseudoClass(const StyleSelector& selector, CSS::PseudoClass pseudoClass);

bool selectorCanBeOwnedBy(const StyleSelector& selector, const Element& element, CSS::PseudoClass pseudoClass) {
    return selectorHasPseudoClass(selector, pseudoClass)
        && matchesSelector(selector, element.elementName(), element.id(), element.classes(), {pseudoClass}, selector.pseudoElement,
            &element, nullptr);
}

StyleSpecificity specificity(const StyleRule& rule);

StyleSpecificity specificity(const StyleSelector& selector) {
    StyleSpecificity result {static_cast<std::uint32_t>(selector.ids.size()),
        static_cast<std::uint32_t>(selector.classNames.size() + selector.attributes.size() + selector.pseudoClasses.size()),
        static_cast<std::uint32_t>((selector.element.empty() || selector.universal ? 0 : 1) + (!selector.pseudoElement.empty() ? 1 : 0))};
    for (const auto& selectorFunction : selector.selectorFunctions) {
        const PseudoClassSpecificity mode = pseudoClassDescriptor(selectorFunction->pseudoClass)->specificity;
        if (mode == PseudoClassSpecificity::Zero)
            continue;
        if (mode == PseudoClassSpecificity::Class || mode == PseudoClassSpecificity::ClassPlusArgument)
            ++result.classesAttributesAndPseudoClasses;
        if (mode == PseudoClassSpecificity::Class)
            continue;
        StyleSpecificity argumentSpecificity;
        for (const StyleRule& argument : selectorFunction->arguments) {
            const StyleSpecificity candidate = specificity(argument);
            if (argumentSpecificity < candidate)
                argumentSpecificity = candidate;
        }
        result.ids += argumentSpecificity.ids;
        result.classesAttributesAndPseudoClasses += argumentSpecificity.classesAttributesAndPseudoClasses;
        result.elements += argumentSpecificity.elements;
    }
    return result;
}

StyleSpecificity specificity(const StyleRule& rule) {
    StyleSpecificity result;
    for (const StyleSelector& selector : rule.selectors) {
        const StyleSpecificity value = specificity(selector);
        result.ids += value.ids;
        result.classesAttributesAndPseudoClasses += value.classesAttributesAndPseudoClasses;
        result.elements += value.elements;
    }
    return result;
}

bool selectorHasPseudoClass(const StyleSelector& selector, CSS::PseudoClass pseudoClass) {
    if (std::find(selector.pseudoClasses.begin(), selector.pseudoClasses.end(), pseudoClass) != selector.pseudoClasses.end())
        return true;
    for (const auto& selectorFunction : selector.selectorFunctions) {
        if (selectorFunction->pseudoClass == CSS::PseudoClass::Dir && pseudoClass == CSS::PseudoClass::Dir)
            return true;
        for (const StyleRule& argument : selectorFunction->arguments)
            for (const StyleSelector& component : argument.selectors)
                if (selectorHasPseudoClass(component, pseudoClass))
                    return true;
    }
    return false;
}

std::set<CSS::PseudoClass> selectorPseudoClasses(const StyleSelector& selector) {
    std::set<CSS::PseudoClass> result(selector.pseudoClasses.begin(), selector.pseudoClasses.end());
    for (const auto& selectorFunction : selector.selectorFunctions) {
        if (selectorFunction->pseudoClass == CSS::PseudoClass::Dir)
            result.insert(CSS::PseudoClass::Dir);
        for (const StyleRule& argument : selectorFunction->arguments)
            for (const StyleSelector& component : argument.selectors) {
                const std::set<CSS::PseudoClass> nested = selectorPseudoClasses(component);
                result.insert(nested.begin(), nested.end());
            }
    }
    return result;
}

bool selectorPseudoClassAffectsDescendants(const StyleSelector& selector, CSS::PseudoClass pseudoClass) {
    for (const auto& selectorFunction : selector.selectorFunctions)
        for (const StyleRule& argument : selectorFunction->arguments) {
            for (std::size_t index = 0; index < argument.selectors.size(); ++index) {
                const StyleSelector& component = argument.selectors[index];
                if (selectorHasPseudoClass(component, pseudoClass) && index + 1 < argument.selectors.size())
                    return true;
                if (selectorPseudoClassAffectsDescendants(component, pseudoClass))
                    return true;
            }
        }
    return false;
}

bool selectorPseudoClassAffectsFollowingSiblings(const StyleSelector& selector, CSS::PseudoClass pseudoClass) {
    for (const auto& selectorFunction : selector.selectorFunctions)
        for (const StyleRule& argument : selectorFunction->arguments) {
            for (std::size_t index = 0; index + 1 < argument.selectors.size(); ++index) {
                const SelectorCombinator combinator = argument.combinators[index];
                if ((combinator == SelectorCombinator::NextSibling || combinator == SelectorCombinator::SubsequentSibling)
                    && selectorHasPseudoClass(argument.selectors[index], pseudoClass))
                    return true;
                if (selectorPseudoClassAffectsFollowingSiblings(argument.selectors[index], pseudoClass))
                    return true;
            }
        }
    return false;
}

bool selectorHasNestedDescendantPseudoClass(const StyleSelector& selector) {
    for (const auto& selectorFunction : selector.selectorFunctions)
        for (const StyleRule& argument : selectorFunction->arguments)
            for (std::size_t index = 0; index < argument.selectors.size(); ++index) {
                if (index + 1 < argument.selectors.size() && !selectorPseudoClasses(argument.selectors[index]).empty())
                    return true;
                if (selectorHasNestedDescendantPseudoClass(argument.selectors[index]))
                    return true;
            }
    return false;
}

bool selectorHasNestedFollowingSiblingPseudoClass(const StyleSelector& selector) {
    for (const auto& selectorFunction : selector.selectorFunctions)
        for (const StyleRule& argument : selectorFunction->arguments)
            for (std::size_t index = 0; index + 1 < argument.selectors.size(); ++index) {
                const SelectorCombinator combinator = argument.combinators[index];
                if ((combinator == SelectorCombinator::NextSibling || combinator == SelectorCombinator::SubsequentSibling)
                    && !selectorPseudoClasses(argument.selectors[index]).empty())
                    return true;
                if (selectorHasNestedFollowingSiblingPseudoClass(argument.selectors[index]))
                    return true;
            }
    return false;
}

bool matchesSelectorFunction(const StyleSelectorFunction& selectorFunction, const std::string& element, const std::string& id,
    const std::set<std::string>& classes, std::initializer_list<CSS::PseudoClass> matchingPseudoClasses, std::string_view pseudoElement,
    const Element* target, const std::vector<std::string>* inlineAncestors) {
    if (selectorFunction.pseudoClass == CSS::PseudoClass::Host)
        return false;
    if (selectorFunction.pseudoClass == CSS::PseudoClass::Dir) {
        if (!target || !selectorFunction.identifier)
            return false;
        if (*selectorFunction.identifier == "ltr")
            return target->directionality() == Layout::Direction::LeftToRight;
        if (*selectorFunction.identifier == "rtl")
            return target->directionality() == Layout::Direction::RightToLeft;
        return false;
    }
    return std::any_of(selectorFunction.arguments.begin(), selectorFunction.arguments.end(), [&](const StyleRule& argument) {
        return matchesRule(argument, element, id, classes, matchingPseudoClasses, pseudoElement, target, inlineAncestors);
    });
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<CSS::PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target,
    const std::vector<std::string>* inlineAncestors) {
    return (selector.element.empty() || selector.element == element) && matchesAttribute(selector, target)
        && std::all_of(selector.ids.begin(), selector.ids.end(),
            [&id](const std::string& selectorId) {
                return selectorId == id;
            })
        && std::all_of(selector.classNames.begin(), selector.classNames.end(),
            [&classes](const std::string& className) {
                return classes.find(className) != classes.end();
            })
        && std::all_of(selector.pseudoClasses.begin(), selector.pseudoClasses.end(),
            [target, matchingPseudoClasses](CSS::PseudoClass pseudoClass) {
                return matchesPseudoClass(pseudoClass, target, matchingPseudoClasses);
            })
        && std::all_of(selector.selectorFunctions.begin(), selector.selectorFunctions.end(),
            [&](const auto& selectorFunction) {
                return matchesSelectorFunction(*selectorFunction, element, id, classes, matchingPseudoClasses, pseudoElement, target,
                    inlineAncestors);
            })
        && selector.pseudoElement == pseudoElement;
}

const Element* structuralParent(const Element* element) {
    if (!element || element->idScopeRoot())
        return nullptr;
    return element->parentElement();
}

bool matchesStructuralSelector(const StyleSelector& selector, const Element& element) {
    return matchesSelector(selector, element.elementName(), element.id(), element.classes(), {}, {}, &element, nullptr);
}

const Element* previousElementSibling(const Element* element) {
    for (const Node* sibling = element ? element->previousSibling() : nullptr; sibling; sibling = sibling->previousSibling())
        if (const Element* result = sibling->asElement())
            return result;
    return nullptr;
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<CSS::PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target,
    const std::vector<std::string>* inlineAncestors) {
    if (rule.selectors.empty()
        || !matchesSelector(rule.selectors.back(), element, id, classes, matchingPseudoClasses, pseudoElement, target, inlineAncestors))
        return false;
    if (rule.selectors.size() == 1)
        return true;
    if (!target || rule.combinators.size() + 1 != rule.selectors.size())
        return false;

    static const std::set<std::string> sNoClasses;
    const auto matchRemaining = [&](auto&& self, std::size_t selectorIndex, const Element* current, const Element* ancestor,
                                    std::size_t inlineIndex) -> bool {
        if (selectorIndex == 0)
            return true;

        const StyleSelector& selector = rule.selectors[selectorIndex - 1];
        switch (rule.combinators[selectorIndex - 1]) {
        case SelectorCombinator::Child:
            if (inlineAncestors && inlineIndex) {
                const std::string& inlineElement = (*inlineAncestors)[inlineIndex - 1];
                if (!matchesSelector(selector, inlineElement, {}, sNoClasses, {}, {}, nullptr, inlineAncestors))
                    return false;
                return self(self, selectorIndex - 1, current, ancestor, inlineIndex - 1);
            }
            if (!ancestor || !matchesStructuralSelector(selector, *ancestor))
                return false;
            return self(self, selectorIndex - 1, ancestor, structuralParent(ancestor), inlineIndex);

        case SelectorCombinator::Descendant:
            if (inlineAncestors && inlineIndex) {
                const std::string& inlineElement = (*inlineAncestors)[inlineIndex - 1];
                if (!matchesSelector(selector, inlineElement, {}, sNoClasses, {}, {}, nullptr, inlineAncestors))
                    return false;
                return self(self, selectorIndex - 1, current, ancestor, inlineIndex - 1);
            }
            for (const Element* candidate = ancestor; candidate; candidate = structuralParent(candidate)) {
                if (!matchesStructuralSelector(selector, *candidate))
                    continue;
                if (self(self, selectorIndex - 1, candidate, structuralParent(candidate), inlineIndex))
                    return true;
            }
            return false;

        case SelectorCombinator::NextSibling: {
            if (inlineAncestors)
                return false;
            const Element* sibling = previousElementSibling(current);
            if (!sibling || !matchesStructuralSelector(selector, *sibling))
                return false;
            return self(self, selectorIndex - 1, sibling, structuralParent(sibling), inlineIndex);
        }

        case SelectorCombinator::SubsequentSibling: {
            if (inlineAncestors)
                return false;
            for (const Element* sibling = previousElementSibling(current); sibling; sibling = previousElementSibling(sibling)) {
                if (!matchesStructuralSelector(selector, *sibling))
                    continue;
                if (self(self, selectorIndex - 1, sibling, structuralParent(sibling), inlineIndex))
                    return true;
            }
            return false;
        }

        case SelectorCombinator::Column:
            return false;
        }
        return false;
    };
    return matchRemaining(matchRemaining, rule.selectors.size() - 1, target, inlineAncestors ? target : structuralParent(target),
        inlineAncestors ? inlineAncestors->size() : 0);
}

std::optional<std::string> varName(const detail::TokenStream& stream, detail::TokenRange range,
    std::optional<detail::TokenRange>& fallback) {
    const std::vector<detail::TokenRange> arguments = detail::splitOnDelimiter(stream, range, ',');
    if (arguments.empty() || arguments.size() > 2)
        return std::nullopt;
    const detail::TokenRange nameRange = detail::trimRange(stream, arguments.front());
    const std::vector<std::size_t> significant = [&] {
        std::vector<std::size_t> result;
        for (std::size_t index = nameRange.begin; index < nameRange.end; ++index)
            if (!detail::isTrivia(stream.tokens()[index].kind))
                result.push_back(index);
        return result;
    }();
    if (significant.size() != 1 || stream.tokens()[significant.front()].kind != detail::TokenKind::Ident)
        return std::nullopt;
    const std::string name = detail::decodeIdentifier(stream.text(significant.front()));
    if (name.size() < 3 || name.rfind("--", 0) != 0)
        return std::nullopt;
    if (arguments.size() == 2)
        fallback = detail::trimRange(stream, arguments[1]);
    return name;
}

bool isGuaranteedInvalidCustomPropertyValue(std::string_view source) {
    const std::string keyword = detail::normalizeKeyword(source);
    return keyword == "initial" || keyword == "inherit" || keyword == "unset" || keyword == "revert" || keyword == "revert-layer";
}

class CustomPropertyResolver {
public:
    explicit CustomPropertyResolver(const CustomPropertyMap& properties)
        : mProperties(properties) {}

    std::optional<CustomPropertyValue> substituteValue(std::string_view source) {
        const std::optional<std::string> value = substitute(source);
        return value ? std::optional<CustomPropertyValue>(CustomPropertyValue {*value}) : std::nullopt;
    }

    std::optional<CustomPropertyValue> resolve(std::string_view name) {
        const std::string key(name);
        if (mCycleNames.find(key) != mCycleNames.end())
            return std::nullopt;
        const auto cached = mResolved.find(key);
        if (cached != mResolved.end())
            return cached->second;
        const auto active = std::find(mResolving.begin(), mResolving.end(), key);
        if (active != mResolving.end()) {
            mCycleNames.insert(active, mResolving.end());
            return std::nullopt;
        }
        const auto found = mProperties.find(key);
        if (found == mProperties.end() || isGuaranteedInvalidCustomPropertyValue(found->second.source))
            return std::nullopt;
        mResolving.push_back(key);
        const std::optional<CustomPropertyValue> result = substituteValue(found->second.source);
        mResolving.pop_back();
        if (!result)
            return std::nullopt;
        mResolved.emplace(key, *result);
        return result;
    }

    CustomPropertyMap all() {
        CustomPropertyMap result;
        for (const auto& [name, value] : mProperties)
            if (const std::optional<CustomPropertyValue> resolved = resolve(name))
                result.emplace(name, *resolved);
        return result;
    }

private:
    static void appendSegment(std::string& result, std::string_view segment, bool& preserveBoundary) {
        if (segment.empty())
            return;
        if (preserveBoundary && !result.empty())
            result += "/**/";
        result += segment;
        preserveBoundary = false;
    }

    std::optional<std::string> substitute(std::string_view source) {
        const detail::TokenStream stream(source);
        return substituteRange(stream, {0, stream.tokens().size()}, 0, stream.source().size());
    }

    std::optional<std::string> substituteRange(const detail::TokenStream& stream, detail::TokenRange range, std::size_t sourceBegin,
        std::size_t sourceEnd) {
        std::string result;
        std::size_t sourceOffset = sourceBegin;
        bool preserveBoundary = false;
        for (std::size_t index = range.begin; index < range.end;) {
            const detail::Token& token = stream.tokens()[index];
            const bool opening = token.kind == detail::TokenKind::Function || token.kind == detail::TokenKind::OpenParen
                || token.kind == detail::TokenKind::OpenBracket || token.kind == detail::TokenKind::OpenBrace;
            if (!opening) {
                ++index;
                continue;
            }
            if (token.matching == detail::kNoMatchingToken || token.matching >= range.end)
                return std::nullopt;

            const std::string_view text = stream.text(index);
            const bool isVar = token.kind == detail::TokenKind::Function && !text.empty() && text.back() == '('
                && detail::lower(detail::decodeIdentifier(text.substr(0, text.size() - 1))) == "var";
            if (isVar) {
                std::optional<detail::TokenRange> fallback;
                const std::optional<std::string> name = varName(stream, {index + 1, token.matching}, fallback);
                if (!name)
                    return std::nullopt;
                const std::optional<CustomPropertyValue> replacement = resolve(*name);
                std::optional<CustomPropertyValue> value = replacement;
                const bool cycle = !replacement && std::any_of(mResolving.begin(), mResolving.end(), [this](const std::string& resolving) {
                    return mCycleNames.find(resolving) != mCycleNames.end();
                });
                if (!value && !cycle && fallback)
                    value = substituteValue(detail::serializeRange(stream, *fallback));
                if (!value)
                    return std::nullopt;
                appendSegment(result, stream.source().substr(sourceOffset, token.begin - sourceOffset), preserveBoundary);
                appendSegment(result, value->source, preserveBoundary);
                preserveBoundary = !value->source.empty();
                sourceOffset = stream.tokens()[token.matching].end;
                index = token.matching + 1;
                continue;
            }

            appendSegment(result, stream.source().substr(sourceOffset, token.end - sourceOffset), preserveBoundary);
            const std::optional<std::string> body =
                substituteRange(stream, {index + 1, token.matching}, token.end, stream.tokens()[token.matching].begin);
            if (!body)
                return std::nullopt;
            result += *body;
            result.append(stream.source(), stream.tokens()[token.matching].begin,
                stream.tokens()[token.matching].end - stream.tokens()[token.matching].begin);
            sourceOffset = stream.tokens()[token.matching].end;
            index = token.matching + 1;
        }
        if (sourceOffset > sourceEnd)
            return std::nullopt;
        appendSegment(result, stream.source().substr(sourceOffset, sourceEnd - sourceOffset), preserveBoundary);
        return result;
    }

    const CustomPropertyMap& mProperties;
    CustomPropertyMap mResolved;
    std::vector<std::string> mResolving;
    std::set<std::string> mCycleNames;
};

CustomPropertyMap resolveCustomProperties(const CustomPropertyMap* inherited, const std::vector<const StyleRule*>& matchedRules) {
    CustomPropertyMap properties = inherited ? *inherited : CustomPropertyMap {};
    for (const StyleRule* rule : matchedRules)
        for (const CustomPropertyDeclaration& declaration : rule->customProperties)
            properties[declaration.name] = declaration.value;
    return CustomPropertyResolver(properties).all();
}

struct ResourceUsage {
    bool optional = false;
    bool cursor = false;
};

std::optional<ResourceUsage> resourceUsage(std::string_view property) {
    if (property == "background" || property == "background-image" || property == "border-image-source")
        return ResourceUsage {false, false};
    if (property == "mask" || property == "mask-image")
        return ResourceUsage {true, false};
    if (property == "cursor")
        return ResourceUsage {false, true};
    return std::nullopt;
}
} // namespace
} // namespace Core::Style

namespace Core::CSS {
using Style::BackgroundLayer;
using Style::BuilderState;
using Style::ColorScheme;
using Style::ColorSchemeContext;
using Style::ComputedStyle;
using Style::CursorImage;
using Style::CursorValue;
using Style::CustomPropertyMap;
using Style::CustomPropertyResolver;
using Style::CustomPropertyValue;
using Style::Image;
using Style::isGuaranteedInvalidCustomPropertyValue;
using Style::MaskLayer;
using Style::matchesRule;
using Style::normalizeOverflow;
using Style::resolveCustomProperties;
using Style::ResourceUsage;
using Style::resourceUsage;
using Style::selectorCanBeOwnedBy;
using Style::selectorHasNestedDescendantPseudoClass;
using Style::selectorHasNestedFollowingSiblingPseudoClass;
using Style::selectorHasPseudoClass;
using Style::selectorPseudoClassAffectsDescendants;
using Style::selectorPseudoClassAffectsFollowingSiblings;
using Style::selectorPseudoClasses;
using Style::specificity;
using Style::varName;

void StyleModel::addRule(const StyleRule& rule) {
    StyleRule copy = rule;
    copy.sourceOrder = rules.size();
    rules.push_back(std::move(copy));
}

StyleRuleSet::StyleRuleSet(StyleModel&& model)
    : mDependencies(std::move(model.dependencies))
    , mRules(std::move(model.rules)) {
    std::stable_sort(mRules.begin(), mRules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        if (lhs.origin != rhs.origin)
            return static_cast<std::uint8_t>(lhs.origin) < static_cast<std::uint8_t>(rhs.origin);
        const StyleSpecificity left = specificity(lhs);
        const StyleSpecificity right = specificity(rhs);
        return left == right ? lhs.sourceOrder < rhs.sourceOrder : left < right;
    });
    const auto addResource = [this](const std::string& value, bool optional, bool cursor) {
        if (value.empty())
            return;
        const auto found =
            std::find_if(mResourceReferences.begin(), mResourceReferences.end(), [&value](const StyleResourceReference& reference) {
                return reference.value == value;
            });
        if (found == mResourceReferences.end())
            mResourceReferences.push_back({value, optional, cursor});
        else {
            found->optional = found->optional && optional;
            found->cursor = found->cursor && cursor;
        }
    };
    std::map<std::string, std::vector<std::string_view>> customPropertyValues;
    for (const StyleRule& rule : mRules)
        for (const CustomPropertyDeclaration& declaration : rule.customProperties)
            customPropertyValues[declaration.name].push_back(declaration.value.source);

    std::map<std::string, ResourceUsage> pendingUsage;
    std::vector<std::string> pendingNames;
    const auto enqueueVariable = [&](std::string_view name, ResourceUsage usage) {
        const auto [found, inserted] = pendingUsage.emplace(std::string(name), usage);
        if (inserted) {
            pendingNames.push_back(found->first);
            return;
        }
        const ResourceUsage merged {found->second.optional && usage.optional, found->second.cursor && usage.cursor};
        if (merged.optional == found->second.optional && merged.cursor == found->second.cursor)
            return;
        found->second = merged;
        pendingNames.push_back(found->first);
    };
    const auto collectDeferredResources = [&](std::string_view source, ResourceUsage usage) {
        const detail::TokenStream stream(source);
        const auto& tokens = stream.tokens();
        for (std::size_t index = 0; index < tokens.size(); ++index) {
            const detail::Token& token = tokens[index];
            if (token.kind == detail::TokenKind::Url) {
                if (const std::optional<std::string> url = detail::parseUrl(stream, {index, index + 1}))
                    addResource(*url, usage.optional, usage.cursor);
                continue;
            }
            if (token.kind != detail::TokenKind::Function || token.matching == detail::kNoMatchingToken)
                continue;
            const std::optional<detail::FunctionRange> function = detail::parseFunction(stream, {index, token.matching + 1});
            if (!function)
                continue;
            if (function->name == "url") {
                if (const std::optional<std::string> url = detail::parseUrl(stream, {index, token.matching + 1}))
                    addResource(*url, usage.optional, usage.cursor);
            } else if (function->name == "var") {
                std::optional<detail::TokenRange> fallback;
                if (const std::optional<std::string> name = varName(stream, {index + 1, token.matching}, fallback))
                    enqueueVariable(*name, usage);
            }
        }
    };
    for (const StyleRule& rule : mRules) {
        for (const StyleDeclaration& declaration : rule.declarations) {
            if (const auto* images = std::get_if<StyleImageLayers>(&declaration.value)) {
                for (const BackgroundLayer& layer : images->layers)
                    if (const std::string* resource = layer.image.resource())
                        addResource(*resource, false, false);
            } else if (const auto* image = std::get_if<Image>(&declaration.value)) {
                if (declaration.property == "border-image-source") {
                    if (const std::string* resource = image->resource())
                        addResource(*resource, false, false);
                }
            } else if (const auto* masks = std::get_if<StyleMaskLayers>(&declaration.value)) {
                for (const MaskLayer& layer : masks->layers)
                    if (const std::string* resource = layer.image.image.resource())
                        addResource(*resource, true, false);
            } else if (const auto* cursor = std::get_if<CursorValue>(&declaration.value)) {
                for (const CursorImage& image : cursor->images)
                    addResource(image.resource, false, true);
            } else if (const auto* deferred = std::get_if<DeferredStyleValue>(&declaration.value)) {
                if (const std::optional<ResourceUsage> usage = resourceUsage(declaration.property))
                    collectDeferredResources(deferred->source, *usage);
            }
        }
    }
    for (std::size_t index = 0; index < pendingNames.size(); ++index) {
        const std::string& name = pendingNames[index];
        const auto found = customPropertyValues.find(name);
        if (found == customPropertyValues.end())
            continue;
        for (const std::string_view value : found->second)
            collectDeferredResources(value, pendingUsage[name]);
    }
    buildIndexes();
}

void StyleRuleSet::buildIndexes() {
    const auto addIndex = [](auto& index, const std::string& key, std::size_t ruleIndex) {
        if (!key.empty())
            index[key].push_back(ruleIndex);
    };
    const auto addUnique = [](auto& values, std::size_t value) {
        if (std::find(values.begin(), values.end(), value) == values.end())
            values.push_back(value);
    };
    const auto addPseudoClassRules = [&](const std::set<PseudoClass>& pseudoClasses, auto& buckets, std::size_t ruleIndex) {
        for (const PseudoClass pseudoClass : pseudoClasses)
            addUnique(buckets[pseudoClass], ruleIndex);
    };

    for (std::size_t ruleIndex = 0; ruleIndex < mRules.size(); ++ruleIndex) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!rule.selectors.empty()) {
            const StyleSelector& selector = rule.selectors.back();
            if (selector.element.empty() && selector.ids.empty() && selector.classNames.empty())
                mUniversalRuleIndices.push_back(ruleIndex);
            addIndex(mElementRuleIndices, selector.element, ruleIndex);
            for (const std::string& id : selector.ids)
                addIndex(mIdRuleIndices, id, ruleIndex);
            for (const std::string& className : selector.classNames)
                addIndex(mClassRuleIndices, className, ruleIndex);
        }

        const bool layoutDeclaration = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                   return !Style::detail::isPropertyPaintOnly(declaration.property);
               });
        const bool hitTestDeclaration = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                   return Style::detail::propertyAffectsHitTesting(declaration.property);
               });
        const bool inherits = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                   return Style::detail::propertyPropagatesToDescendants(declaration.property);
               });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            const std::set<PseudoClass> pseudoClasses = selectorPseudoClasses(selector);
            if (layoutDeclaration)
                addPseudoClassRules(pseudoClasses, mLayoutPseudoClassRules, ruleIndex);
            if (hitTestDeclaration)
                addPseudoClassRules(pseudoClasses, mHitTestPseudoClassRules, ruleIndex);
            if (selectorIndex + 1 < rule.selectors.size() || inherits || selectorHasNestedDescendantPseudoClass(selector)
                || selectorHasNestedFollowingSiblingPseudoClass(selector))
                addPseudoClassRules(pseudoClasses, mDescendantPseudoClassRules, ruleIndex);
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
    sortUnique(mLayoutPseudoClassRules);
    sortUnique(mHitTestPseudoClassRules);
    sortUnique(mDescendantPseudoClassRules);
}

bool StyleRuleSet::pseudoClassAffectsLayout(PseudoClass pseudoClass) const { return mLayoutPseudoClassRules.contains(pseudoClass); }

bool StyleRuleSet::pseudoClassAffectsLayout(const Element& element, PseudoClass pseudoClass) const {
    if (pseudoClass == PseudoClass::Disabled && element.elementName() == HTMLTagName(HTMLTag::Fieldset))
        return pseudoClassAffectsLayout(pseudoClass);
    const auto candidates = mLayoutPseudoClassRules.find(pseudoClass);
    if (candidates == mLayoutPseudoClassRules.end())
        return false;
    const auto hasLayoutDeclaration = [](const StyleRule& rule) {
        return !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                   return !Style::detail::isPropertyPaintOnly(declaration.property);
               });
    };
    for (const std::size_t ruleIndex : candidates->second) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!hasLayoutDeclaration(rule))
            continue;
        for (const StyleSelector& selector : rule.selectors)
            if (selectorHasPseudoClass(selector, pseudoClass) && selectorCanBeOwnedBy(selector, element, pseudoClass))
                return true;
    }
    return false;
}

bool StyleRuleSet::pseudoClassAffectsHitTesting(PseudoClass pseudoClass) const { return mHitTestPseudoClassRules.contains(pseudoClass); }

bool StyleRuleSet::pseudoClassAffectsHitTesting(const Element& element, PseudoClass pseudoClass) const {
    if (pseudoClass == PseudoClass::Disabled && element.elementName() == HTMLTagName(HTMLTag::Fieldset))
        return pseudoClassAffectsHitTesting(pseudoClass);
    const auto candidates = mHitTestPseudoClassRules.find(pseudoClass);
    if (candidates == mHitTestPseudoClassRules.end())
        return false;
    for (const std::size_t ruleIndex : candidates->second)
        for (const StyleSelector& selector : mRules[ruleIndex].selectors)
            if (selectorHasPseudoClass(selector, pseudoClass) && selectorCanBeOwnedBy(selector, element, pseudoClass))
                return true;
    return false;
}

bool StyleRuleSet::pseudoClassAffectsDescendants(const Element& element, PseudoClass pseudoClass) const {
    if (pseudoClass == PseudoClass::Disabled && element.elementName() == HTMLTagName(HTMLTag::Fieldset))
        return true;
    const auto candidates = mDescendantPseudoClassRules.find(pseudoClass);
    if (candidates == mDescendantPseudoClassRules.end())
        return false;
    for (const std::size_t ruleIndex : candidates->second) {
        const StyleRule& rule = mRules[ruleIndex];
        const bool inherits = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(), [](const StyleDeclaration& declaration) {
                   return Style::detail::propertyPropagatesToDescendants(declaration.property);
               });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if (selectorHasPseudoClass(selector, pseudoClass) && selectorCanBeOwnedBy(selector, element, pseudoClass)
                && (selectorIndex + 1 < rule.selectors.size() || inherits || selectorPseudoClassAffectsDescendants(selector, pseudoClass)))
                return true;
        }
    }
    return false;
}

bool StyleRuleSet::pseudoClassAffectsFollowingSiblings(const Element& element, PseudoClass pseudoClass) const {
    const auto candidates = mDescendantPseudoClassRules.find(pseudoClass);
    if (candidates == mDescendantPseudoClassRules.end())
        return false;
    for (const std::size_t ruleIndex : candidates->second) {
        const StyleRule& rule = mRules[ruleIndex];
        for (const StyleSelector& selector : rule.selectors)
            if (selectorHasPseudoClass(selector, pseudoClass) && selectorCanBeOwnedBy(selector, element, pseudoClass)
                && selectorPseudoClassAffectsFollowingSiblings(selector, pseudoClass))
                return true;
        for (std::size_t selectorIndex = 0; selectorIndex + 1 < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if (!selectorHasPseudoClass(selector, pseudoClass) || !selectorCanBeOwnedBy(selector, element, pseudoClass))
                continue;
            const SelectorCombinator combinator = rule.combinators[selectorIndex];
            if (combinator == SelectorCombinator::NextSibling || combinator == SelectorCombinator::SubsequentSibling
                || selectorPseudoClassAffectsFollowingSiblings(selector, pseudoClass))
                return true;
        }
    }
    return false;
}

ComputedStyle StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<PseudoClass> matchingPseudoClasses) const {
    ComputedStyle style =
        mImpl->ruleSet.resolveInternal(element, id, classes, matchingPseudoClasses, {}, nullptr, nullptr, nullptr, {}, nullptr);
    style.textDecorationPropagation = style.textDecoration();
    return style;
}

ComputedStyle StyleSheet::resolveElement(const Element& element) const {
    return resolveElement(element, nullptr, {}, nullptr, nullptr, ComputedStyle::initialStyle());
}

ComputedStyle StyleSheet::resolveElement(const Element& element, const CustomPropertyMap* inheritedCustomProperties,
    ColorSchemeContext colorSchemeContext, const ColorScheme* inheritedColorScheme, const ComputedStyle* parentStyle,
    const ComputedStyle& rootStyle) const {
    ComputedStyle style = mImpl->ruleSet.resolveInternal(element.elementName(), element.id(), element.classes(), {}, {}, &element, nullptr,
        inheritedCustomProperties, colorSchemeContext, inheritedColorScheme, parentStyle, rootStyle);
    if (!inheritedCustomProperties)
        style.textDecorationPropagation = style.textDecoration();
    return style;
}

ComputedStyle StyleSheet::resolvePseudoElement(const Element& owner, std::string_view pseudoElementName) const {
    return resolvePseudoElement(owner, pseudoElementName, nullptr, {}, nullptr, nullptr, ComputedStyle::initialStyle());
}

ComputedStyle StyleSheet::resolvePseudoElement(const Element& owner, std::string_view pseudoElementName,
    const CustomPropertyMap* inheritedCustomProperties, ColorSchemeContext colorSchemeContext, const ColorScheme* inheritedColorScheme,
    const ComputedStyle* parentStyle, const ComputedStyle& rootStyle) const {
    ComputedStyle style = mImpl->ruleSet.resolveInternal(owner.elementName(), owner.id(), owner.classes(), {}, pseudoElementName, &owner,
        nullptr, inheritedCustomProperties, colorSchemeContext, inheritedColorScheme, parentStyle, rootStyle);
    if (!inheritedCustomProperties)
        style.textDecorationPropagation = style.textDecoration();
    return style;
}

ComputedStyle StyleSheet::resolveInline(const Element& owner, const std::string& element,
    const std::vector<std::string>& inlineAncestors) const {
    static const std::set<std::string> sNoClasses;
    ComputedStyle style = mImpl->ruleSet.resolveInternal(element, {}, sNoClasses, {}, {}, &owner, &inlineAncestors, nullptr, {}, nullptr,
        nullptr, ComputedStyle::initialStyle());
    style.textDecorationPropagation = style.textDecoration();
    return style;
}

ComputedStyle StyleRuleSet::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes,
    std::initializer_list<PseudoClass> matchingPseudoClasses, std::string_view pseudoElement, const Element* target,
    const std::vector<std::string>* inlineAncestors, const CustomPropertyMap* inheritedCustomProperties,
    ColorSchemeContext colorSchemeContext, const ColorScheme* inheritedColorScheme, const ComputedStyle* parentStyle,
    const ComputedStyle& rootStyle) const {
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

    std::vector<const StyleRule*> matchedRules;
    matchedRules.reserve(candidates.size());
    for (const std::size_t ruleIndex : candidates) {
        const StyleRule& rule = mRules[ruleIndex];
        if (matchesRule(rule, element, id, classes, matchingPseudoClasses, pseudoElement, target, inlineAncestors))
            matchedRules.push_back(&rule);
    }

    ComputedStyle style;
    BuilderState builderState {style, parentStyle, rootStyle};
    if (inheritedColorScheme)
        style.setColorScheme(*inheritedColorScheme);
    style.customProperties = resolveCustomProperties(inheritedCustomProperties, matchedRules);
    const auto applyDeclarations = [&](bool colorSchemeFirst) {
        for (const StyleRule* rule : matchedRules) {
            for (const StyleDeclaration& declaration : rule->declarations) {
                if ((declaration.property == "color-scheme") != colorSchemeFirst)
                    continue;
                if (const auto* deferred = std::get_if<DeferredStyleValue>(&declaration.value)) {
                    CustomPropertyResolver customProperties(style.customProperties);
                    const std::optional<CustomPropertyValue> substituted = customProperties.substituteValue(deferred->source);
                    if (!substituted) {
                        Style::detail::applyInvalidStyleDeclaration(builderState, declaration.property);
                        continue;
                    }
                    const detail::TokenStream stream(substituted->source);
                    StyleSheetLoadResult ignored;
                    const auto compiled = StyleModel::compileDeclaration(declaration.property, stream, {0, stream.tokens().size()}, {},
                        ignored, {}, style.usedColorScheme, true);
                    if (!compiled) {
                        Style::detail::applyInvalidStyleDeclaration(builderState, declaration.property);
                        continue;
                    }
                    for (const StyleDeclaration& resolved : *compiled)
                        Style::detail::applyStyleDeclaration(builderState, resolved);
                } else
                    Style::detail::applyStyleDeclaration(builderState, declaration);
            }
        }
    };
    applyDeclarations(true);
    style.usedColorScheme = style.colorScheme().used(colorSchemeContext);
    applyDeclarations(false);
    normalizeOverflow(style);
    return style;
}
} // namespace Core::CSS
