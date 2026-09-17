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
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"
#include "dom/element.h"
#include "html/elementnames.h"
#include "style/property.h"

namespace radia::ui {
namespace {
bool matchesState(ElementState state, uint16_t states, const Element* target) {
    if (state == ElementState::Disabled) return target ? target->disabled() : hasState(states, state);
    return hasState(states, state);
}

bool matchesStateMask(std::uint16_t mask, uint16_t states, const Element* target) {
    if ((mask & static_cast<std::uint16_t>(ElementState::FocusVisible)) != 0
        && (!hasState(states, ElementState::Focused) || !hasState(states, ElementState::FocusVisible)))
        return false;
    for (const ElementState state : {ElementState::Hovered, ElementState::Active, ElementState::Focused, ElementState::Disabled,
                                     ElementState::Checked, ElementState::Minimized, ElementState::Invalid, ElementState::Indeterminate})
        if ((mask & static_cast<std::uint16_t>(state)) != 0 && !matchesState(state, states, target)) return false;
    return true;
}

bool matchesDirection(const std::optional<LayoutDirection>& selectorDirection, LayoutDirection direction) {
    return !selectorDirection || *selectorDirection == direction;
}

bool matchesAttribute(const StyleSelector& selector, const Element* element) {
    if (selector.attributes.empty()) return true;
    if (!element) return false;
    return std::all_of(selector.attributes.begin(), selector.attributes.end(), [element](const StyleAttributeSelector& attribute) {
        const Element::Attribute* serialized = element->attribute(attribute.name);
        if (attribute.presence) return serialized != nullptr;
        if (!serialized || !serialized->value) return false;

        std::string actual = *serialized->value;
        std::string expected = attribute.value;
        if (attribute.caseInsensitive || (!attribute.caseSensitivitySpecified && (attribute.name == "type" || attribute.name == "switch"))) {
            actual = detail::lower(std::move(actual));
            expected = detail::lower(std::move(expected));
        }
        switch (attribute.match) {
            case StyleAttributeSelector::Match::Exact: return actual == expected;
            case StyleAttributeSelector::Match::Prefix: return actual.rfind(expected, 0) == 0;
            case StyleAttributeSelector::Match::Suffix:
                return actual.size() >= expected.size() && actual.compare(actual.size() - expected.size(), expected.size(), expected) == 0;
            case StyleAttributeSelector::Match::Substring: return actual.find(expected) != std::string::npos;
            case StyleAttributeSelector::Match::IncludesHyphen:
                return actual == expected || (actual.size() > expected.size() && actual.rfind(expected + '-', 0) == 0);
            case StyleAttributeSelector::Match::IncludesWord: {
                std::size_t start = 0;
                while (start < actual.size()) {
                    while (start < actual.size() && detail::isCSSWhitespace(actual[start])) ++start;
                    const std::size_t end = actual.find_first_of(" \t\r\n\f", start);
                    if (actual.substr(start, end == std::string::npos ? std::string::npos : end - start) == expected) return true;
                    if (end == std::string::npos) break;
                    start = end;
                }
                return false;
            }
        }
        return false;
    });
}

bool matchesRoot(const StyleSelector& selector, const Element* element) {
    return !selector.root || !element || element->parentElement() == nullptr || element->idScopeRoot();
}

bool selectorCanBeOwnedBy(const StyleSelector& selector, const Element& element) {
    if ((selector.element.empty() || selector.element == element.elementName())
        && matchesRoot(selector, &element)
        && matchesAttribute(selector, &element)
        && std::all_of(selector.ids.begin(), selector.ids.end(), [&element](const std::string& id) { return id == element.id(); })
        && std::all_of(selector.classNames.begin(), selector.classNames.end(),
                       [&element](const std::string& name) { return element.classes().find(name) != element.classes().end(); })
        && matchesStateMask(selector.stateMask, element.states(), &element))
        return true;
    return false;
}

std::optional<std::size_t> stateIndex(ElementState state) {
    const std::uint16_t bit = static_cast<std::uint16_t>(state);
    if (bit == 0 || (bit & static_cast<std::uint16_t>(bit - 1)) != 0) return std::nullopt;
    std::size_t index = 0;
    for (std::uint16_t value = bit; value > 1; value >>= 1) ++index;
    return index;
}

StyleSpecificity specificity(const StyleSelector& selector) {
    return {static_cast<std::uint32_t>(selector.ids.size()),
            static_cast<std::uint32_t>(selector.classNames.size()
                                       + selector.attributes.size()
                                       + selector.stateSpecificity
                                       + selector.rootSpecificity
                                       + selector.directionSpecificity),
            static_cast<std::uint32_t>((selector.element.empty() || selector.universal ? 0 : 1) + (!selector.pseudoElement.empty() ? 1 : 0))};
}

StyleSpecificity specificity(const StyleRule& rule) {
    StyleSpecificity result;
    for (const StyleSelector& selector : rule.selectors) {
        const StyleSpecificity value = specificity(selector);
        result.ids += value.ids;
        result.classesAttributesAndStates += value.classesAttributesAndStates;
        result.elements += value.elements;
    }
    return result;
}

bool matchesSelector(const StyleSelector& selector, const std::string& element, const std::string& id, const std::set<std::string>& classes,
                     uint16_t ownerStates, std::string_view pseudoElement, const Element* target, LayoutDirection direction) {
    return (selector.element.empty() || selector.element == element)
        && matchesRoot(selector, target)
        && matchesAttribute(selector, target)
        && std::all_of(selector.ids.begin(), selector.ids.end(), [&id](const std::string& selectorId) { return selectorId == id; })
        && std::all_of(selector.classNames.begin(), selector.classNames.end(),
                       [&classes](const std::string& className) { return classes.find(className) != classes.end(); })
        && matchesDirection(selector.direction, direction)
        && matchesStateMask(selector.stateMask, ownerStates, target)
        && selector.pseudoElement == pseudoElement;
}

const Element* structuralParent(const Element* element) {
    if (!element || element->idScopeRoot()) return nullptr;
    return element->parentElement();
}

bool matchesStructuralSelector(const StyleSelector& selector, const Element& element, LayoutDirection direction) {
    return matchesSelector(selector, element.elementName(), element.id(), element.classes(), element.states(), {}, &element, direction);
}

const Element* previousElementSibling(const Element* element) {
    for (const Node* sibling = element ? element->previousSibling() : nullptr; sibling; sibling = sibling->previousSibling())
        if (const Element* result = sibling->asElement()) return result;
    return nullptr;
}

bool matchesRule(const StyleRule& rule, const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t ownerStates,
                 std::string_view pseudoElement, const Element* target, const std::vector<std::string>* inlineAncestors, LayoutDirection direction) {
    if (rule.selectors.empty() || !matchesSelector(rule.selectors.back(), element, id, classes, ownerStates, pseudoElement, target, direction))
        return false;
    if (rule.selectors.size() == 1) return true;
    if (!target || rule.combinators.size() + 1 != rule.selectors.size()) return false;

    static const std::set<std::string> sNoClasses;
    const auto matchRemaining = [&](auto&& self, std::size_t selectorIndex, const Element* current, const Element* ancestor,
                                    std::size_t inlineIndex) -> bool {
        if (selectorIndex == 0) return true;

        const StyleSelector& selector = rule.selectors[selectorIndex - 1];
        switch (rule.combinators[selectorIndex - 1]) {
            case SelectorCombinator::Child:
                if (inlineAncestors && inlineIndex) {
                    const std::string& inlineElement = (*inlineAncestors)[inlineIndex - 1];
                    if (!matchesSelector(selector, inlineElement, {}, sNoClasses, 0, {}, nullptr, direction)) return false;
                    return self(self, selectorIndex - 1, current, ancestor, inlineIndex - 1);
                }
                if (!ancestor || !matchesStructuralSelector(selector, *ancestor, direction)) return false;
                return self(self, selectorIndex - 1, ancestor, structuralParent(ancestor), inlineIndex);

            case SelectorCombinator::Descendant:
                if (inlineAncestors && inlineIndex) {
                    const std::string& inlineElement = (*inlineAncestors)[inlineIndex - 1];
                    if (!matchesSelector(selector, inlineElement, {}, sNoClasses, 0, {}, nullptr, direction)) return false;
                    return self(self, selectorIndex - 1, current, ancestor, inlineIndex - 1);
                }
                for (const Element* candidate = ancestor; candidate; candidate = structuralParent(candidate)) {
                    if (!matchesStructuralSelector(selector, *candidate, direction)) continue;
                    if (self(self, selectorIndex - 1, candidate, structuralParent(candidate), inlineIndex)) return true;
                }
                return false;

            case SelectorCombinator::NextSibling: {
                if (inlineAncestors) return false;
                const Element* sibling = previousElementSibling(current);
                if (!sibling || !matchesStructuralSelector(selector, *sibling, direction)) return false;
                return self(self, selectorIndex - 1, sibling, structuralParent(sibling), inlineIndex);
            }

            case SelectorCombinator::SubsequentSibling: {
                if (inlineAncestors) return false;
                for (const Element* sibling = previousElementSibling(current); sibling; sibling = previousElementSibling(sibling)) {
                    if (!matchesStructuralSelector(selector, *sibling, direction)) continue;
                    if (self(self, selectorIndex - 1, sibling, structuralParent(sibling), inlineIndex)) return true;
                }
                return false;
            }

            case SelectorCombinator::Column: return false;
        }
        return false;
    };
    return matchRemaining(matchRemaining, rule.selectors.size() - 1, target, inlineAncestors ? target : structuralParent(target),
                          inlineAncestors ? inlineAncestors->size() : 0);
}

std::optional<std::string> varName(const detail::CSSTokenStream& stream, detail::CSSTokenRange range,
                                   std::optional<detail::CSSTokenRange>& fallback) {
    const std::vector<detail::CSSTokenRange> arguments = detail::splitCSSOnDelimiter(stream, range, ',');
    if (arguments.empty() || arguments.size() > 2) return std::nullopt;
    const detail::CSSTokenRange nameRange = detail::trimCSSRange(stream, arguments.front());
    const std::vector<std::size_t> significant = [&] {
        std::vector<std::size_t> result;
        for (std::size_t index = nameRange.begin; index < nameRange.end; ++index)
            if (!detail::isCSSTrivia(stream.tokens()[index].kind)) result.push_back(index);
        return result;
    }();
    if (significant.size() != 1 || stream.tokens()[significant.front()].kind != detail::CSSTokenKind::Ident) return std::nullopt;
    const std::string name = detail::decodeCSSIdentifier(stream.text(significant.front()));
    if (name.size() < 3 || name.rfind("--", 0) != 0) return std::nullopt;
    if (arguments.size() == 2) fallback = detail::trimCSSRange(stream, arguments[1]);
    return name;
}

bool isGuaranteedInvalidCustomPropertyValue(std::string_view source) {
    const std::string keyword = detail::normalizeCSSKeyword(source);
    return keyword == "initial" || keyword == "inherit" || keyword == "unset" || keyword == "revert" || keyword == "revert-layer";
}

class CustomPropertyResolver {
public:
    explicit CustomPropertyResolver(const CustomPropertyMap& properties) : mProperties(properties) {}

    std::optional<CustomPropertyValue> substituteValue(std::string_view source) {
        const std::optional<std::string> value = substitute(source);
        return value ? std::optional<CustomPropertyValue>(CustomPropertyValue{*value}) : std::nullopt;
    }

    std::optional<CustomPropertyValue> resolve(std::string_view name) {
        const std::string key(name);
        if (mCycleNames.find(key) != mCycleNames.end()) return std::nullopt;
        const auto cached = mResolved.find(key);
        if (cached != mResolved.end()) return cached->second;
        const auto active = std::find(mResolving.begin(), mResolving.end(), key);
        if (active != mResolving.end()) {
            mCycleNames.insert(active, mResolving.end());
            return std::nullopt;
        }
        const auto found = mProperties.find(key);
        if (found == mProperties.end() || isGuaranteedInvalidCustomPropertyValue(found->second.source)) return std::nullopt;
        mResolving.push_back(key);
        const std::optional<CustomPropertyValue> result = substituteValue(found->second.source);
        mResolving.pop_back();
        if (!result) return std::nullopt;
        mResolved.emplace(key, *result);
        return result;
    }

    CustomPropertyMap all() {
        CustomPropertyMap result;
        for (const auto& [name, value] : mProperties)
            if (const std::optional<CustomPropertyValue> resolved = resolve(name)) result.emplace(name, *resolved);
        return result;
    }

private:
    static void appendSegment(std::string& result, std::string_view segment, bool& preserveBoundary) {
        if (segment.empty()) return;
        if (preserveBoundary && !result.empty()) result += "/**/";
        result += segment;
        preserveBoundary = false;
    }

    std::optional<std::string> substitute(std::string_view source) {
        const detail::CSSTokenStream stream(source);
        return substituteRange(stream, {0, stream.tokens().size()}, 0, stream.source().size());
    }

    std::optional<std::string> substituteRange(const detail::CSSTokenStream& stream, detail::CSSTokenRange range, std::size_t sourceBegin,
                                               std::size_t sourceEnd) {
        std::string result;
        std::size_t sourceOffset = sourceBegin;
        bool preserveBoundary = false;
        for (std::size_t index = range.begin; index < range.end;) {
            const detail::CSSToken& token = stream.tokens()[index];
            const bool opening = token.kind == detail::CSSTokenKind::Function
                || token.kind == detail::CSSTokenKind::OpenParen
                || token.kind == detail::CSSTokenKind::OpenBracket
                || token.kind == detail::CSSTokenKind::OpenBrace;
            if (!opening) {
                ++index;
                continue;
            }
            if (token.matching == detail::kNoMatchingCSSToken || token.matching >= range.end) return std::nullopt;

            const std::string_view text = stream.text(index);
            const bool isVar = token.kind == detail::CSSTokenKind::Function
                && !text.empty()
                && text.back() == '('
                && detail::lower(detail::decodeCSSIdentifier(text.substr(0, text.size() - 1))) == "var";
            if (isVar) {
                std::optional<detail::CSSTokenRange> fallback;
                const std::optional<std::string> name = varName(stream, {index + 1, token.matching}, fallback);
                if (!name) return std::nullopt;
                const std::optional<CustomPropertyValue> replacement = resolve(*name);
                std::optional<CustomPropertyValue> value = replacement;
                const bool cycle = !replacement && std::any_of(mResolving.begin(), mResolving.end(), [this](const std::string& resolving) {
                    return mCycleNames.find(resolving) != mCycleNames.end();
                });
                if (!value && !cycle && fallback) value = substituteValue(detail::serializeCSSRange(stream, *fallback));
                if (!value) return std::nullopt;
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
            if (!body) return std::nullopt;
            result += *body;
            result.append(stream.source(), stream.tokens()[token.matching].begin,
                          stream.tokens()[token.matching].end - stream.tokens()[token.matching].begin);
            sourceOffset = stream.tokens()[token.matching].end;
            index = token.matching + 1;
        }
        if (sourceOffset > sourceEnd) return std::nullopt;
        appendSegment(result, stream.source().substr(sourceOffset, sourceEnd - sourceOffset), preserveBoundary);
        return result;
    }

    const CustomPropertyMap& mProperties;
    CustomPropertyMap mResolved;
    std::vector<std::string> mResolving;
    std::set<std::string> mCycleNames;
};

CustomPropertyMap resolveCustomProperties(const CustomPropertyMap* inherited, const std::vector<const StyleRule*>& matchedRules) {
    CustomPropertyMap properties = inherited ? *inherited : CustomPropertyMap{};
    for (const StyleRule* rule : matchedRules)
        for (const CustomPropertyDeclaration& declaration : rule->customProperties) properties[declaration.name] = declaration.value;
    return CustomPropertyResolver(properties).all();
}

struct ResourceUsage {
    bool optional = false;
    bool cursor = false;
};

std::optional<ResourceUsage> resourceUsage(std::string_view property) {
    if (property == "background" || property == "background-image") return ResourceUsage{false, false};
    if (property == "mask" || property == "mask-image") return ResourceUsage{true, false};
    if (property == "cursor") return ResourceUsage{false, true};
    return std::nullopt;
}
} // namespace

void StyleModel::addRule(const StyleRule& rule) {
    StyleRule copy = rule;
    copy.sourceOrder = static_cast<int>(rules.size());
    rules.push_back(std::move(copy));
}

StyleRuleSet::StyleRuleSet(StyleModel&& model) : mDependencies(std::move(model.dependencies)), mRules(std::move(model.rules)) {
    std::stable_sort(mRules.begin(), mRules.end(), [](const StyleRule& lhs, const StyleRule& rhs) {
        if (lhs.origin != rhs.origin) return static_cast<std::uint8_t>(lhs.origin) < static_cast<std::uint8_t>(rhs.origin);
        const StyleSpecificity left = specificity(lhs);
        const StyleSpecificity right = specificity(rhs);
        return left == right ? lhs.sourceOrder < rhs.sourceOrder : left < right;
    });
    const auto addResource = [this](const std::string& value, bool optional, bool cursor) {
        if (value.empty()) return;
        const auto found = std::find_if(mResourceReferences.begin(), mResourceReferences.end(),
                                        [&value](const StyleResourceReference& reference) { return reference.value == value; });
        if (found == mResourceReferences.end()) mResourceReferences.push_back({value, optional, cursor});
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
        const ResourceUsage merged{found->second.optional && usage.optional, found->second.cursor && usage.cursor};
        if (merged.optional == found->second.optional && merged.cursor == found->second.cursor) return;
        found->second = merged;
        pendingNames.push_back(found->first);
    };
    const auto collectDeferredResources = [&](std::string_view source, ResourceUsage usage) {
        const detail::CSSTokenStream stream(source);
        const auto& tokens = stream.tokens();
        for (std::size_t index = 0; index < tokens.size(); ++index) {
            const detail::CSSToken& token = tokens[index];
            if (token.kind == detail::CSSTokenKind::Url) {
                if (const std::optional<std::string> url = detail::parseCSSUrl(stream, {index, index + 1}))
                    addResource(*url, usage.optional, usage.cursor);
                continue;
            }
            if (token.kind != detail::CSSTokenKind::Function || token.matching == detail::kNoMatchingCSSToken) continue;
            const std::optional<detail::CSSFunctionRange> function = detail::parseCSSFunction(stream, {index, token.matching + 1});
            if (!function) continue;
            if (function->name == "url") {
                if (const std::optional<std::string> url = detail::parseCSSUrl(stream, {index, token.matching + 1}))
                    addResource(*url, usage.optional, usage.cursor);
            } else if (function->name == "var") {
                std::optional<detail::CSSTokenRange> fallback;
                if (const std::optional<std::string> name = varName(stream, {index + 1, token.matching}, fallback)) enqueueVariable(*name, usage);
            }
        }
    };
    for (const StyleRule& rule : mRules)
        for (const StyleDeclaration& declaration : rule.declarations) {
            if (const auto* images = std::get_if<StyleImageLayers>(&declaration.value))
                for (const BackgroundLayer& layer : images->layers) addResource(layer.resource, false, false);
            else if (const auto* masks = std::get_if<StyleMaskLayers>(&declaration.value))
                for (const MaskLayer& layer : masks->layers) addResource(layer.image.resource, true, false);
            else if (const auto* cursor = std::get_if<CursorValue>(&declaration.value))
                for (const CursorImage& image : cursor->images) addResource(image.resource, false, true);
            else if (const auto* deferred = std::get_if<DeferredStyleValue>(&declaration.value))
                if (const std::optional<ResourceUsage> usage = resourceUsage(declaration.property.get().name))
                    collectDeferredResources(deferred->source, *usage);
        }
    for (std::size_t index = 0; index < pendingNames.size(); ++index) {
        const std::string& name = pendingNames[index];
        const auto found = customPropertyValues.find(name);
        if (found == customPropertyValues.end()) continue;
        for (const std::string_view value : found->second) collectDeferredResources(value, pendingUsage[name]);
    }
    buildIndexes();
}

void StyleRuleSet::buildIndexes() {
    const auto addIndex = [](auto& index, const std::string& key, std::size_t ruleIndex) {
        if (!key.empty()) index[key].push_back(ruleIndex);
    };
    const auto addUnique = [](auto& values, std::size_t value) {
        if (std::find(values.begin(), values.end(), value) == values.end()) values.push_back(value);
    };
    const auto addStateRule = [&](std::uint16_t stateMask, auto& mask, auto& buckets, std::size_t ruleIndex) {
        for (const ElementState state :
             {ElementState::Hovered, ElementState::Active, ElementState::Focused, ElementState::Disabled, ElementState::Checked,
              ElementState::FocusVisible, ElementState::Minimized, ElementState::Invalid, ElementState::Indeterminate}) {
            if ((stateMask & static_cast<std::uint16_t>(state)) == 0) continue;
            const std::optional<std::size_t> index = stateIndex(state);
            if (!index) continue;
            mask |= static_cast<std::uint16_t>(state);
            addUnique(buckets[*index], ruleIndex);
        }
    };
    const auto addStateBucket = [&](std::uint16_t stateMask, auto& buckets, std::size_t ruleIndex) {
        for (const ElementState state :
             {ElementState::Hovered, ElementState::Active, ElementState::Focused, ElementState::Disabled, ElementState::Checked,
              ElementState::FocusVisible, ElementState::Minimized, ElementState::Invalid, ElementState::Indeterminate}) {
            if ((stateMask & static_cast<std::uint16_t>(state)) == 0) continue;
            const std::optional<std::size_t> index = stateIndex(state);
            if (index) addUnique(buckets[*index], ruleIndex);
        }
    };

    for (std::size_t ruleIndex = 0; ruleIndex < mRules.size(); ++ruleIndex) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!rule.selectors.empty()) {
            const StyleSelector& selector = rule.selectors.back();
            if (selector.element.empty() && selector.ids.empty() && selector.classNames.empty()) mUniversalRuleIndices.push_back(ruleIndex);
            addIndex(mElementRuleIndices, selector.element, ruleIndex);
            for (const std::string& id : selector.ids) addIndex(mIdRuleIndices, id, ruleIndex);
            for (const std::string& className : selector.classNames) addIndex(mClassRuleIndices, className, ruleIndex);
        }

        const bool layoutDeclaration = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
        const bool hitTestDeclaration = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return declaration.property.get().affectsHitTesting(); });
        const bool inherits = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return declaration.property.get().propagatesToDescendants(); });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if (layoutDeclaration) addStateRule(selector.stateMask, mLayoutStateMask, mLayoutStateRules, ruleIndex);
            if (hitTestDeclaration) addStateRule(selector.stateMask, mHitTestStateMask, mHitTestStateRules, ruleIndex);
            if (selectorIndex + 1 < rule.selectors.size() || inherits) addStateBucket(selector.stateMask, mDescendantStateRules, ruleIndex);
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
    if (state == ElementState::Disabled && element.elementName() == kFieldsetTag.localName) return stateAffectsLayout(state);
    if (!stateAffectsLayout(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    const auto hasLayoutDeclaration = [](const StyleRule& rule) {
        return !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return !declaration.property.get().isPaintOnly(); });
    };
    for (const std::size_t ruleIndex : mLayoutStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        if (!hasLayoutDeclaration(rule)) continue;
        for (const StyleSelector& selector : rule.selectors)
            if ((selector.stateMask & static_cast<std::uint16_t>(state)) != 0 && selectorCanBeOwnedBy(selector, element)) return true;
    }
    return false;
}

bool StyleRuleSet::stateAffectsHitTesting(ElementState state) const {
    return (mHitTestStateMask & static_cast<std::uint16_t>(state)) != 0;
}

bool StyleRuleSet::stateAffectsHitTesting(const Element& element, ElementState state) const {
    if (state == ElementState::Disabled && element.elementName() == kFieldsetTag.localName) return stateAffectsHitTesting(state);
    if (!stateAffectsHitTesting(state)) return false;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : mHitTestStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        for (const StyleSelector& selector : rule.selectors)
            if ((selector.stateMask & static_cast<std::uint16_t>(state)) != 0 && selectorCanBeOwnedBy(selector, element)) return true;
    }
    return false;
}

bool StyleRuleSet::stateAffectsDescendants(const Element& element, ElementState state) const {
    if (state == ElementState::Disabled && element.elementName() == kFieldsetTag.localName) return true;
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    for (const std::size_t ruleIndex : mDescendantStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        const bool inherits = !rule.customProperties.empty()
            || std::any_of(rule.declarations.begin(), rule.declarations.end(),
                           [](const StyleDeclaration& declaration) { return declaration.property.get().propagatesToDescendants(); });
        for (std::size_t selectorIndex = 0; selectorIndex < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if ((selector.stateMask & static_cast<std::uint16_t>(state)) != 0
                && selectorCanBeOwnedBy(selector, element)
                && (selectorIndex + 1 < rule.selectors.size() || inherits))
                return true;
        }
    }
    return false;
}

bool StyleRuleSet::stateAffectsFollowingSiblings(const Element& element, ElementState state) const {
    const std::optional<std::size_t> index = stateIndex(state);
    if (!index) return false;
    const std::uint16_t stateBit = static_cast<std::uint16_t>(state);
    for (const std::size_t ruleIndex : mDescendantStateRules[*index]) {
        const StyleRule& rule = mRules[ruleIndex];
        for (std::size_t selectorIndex = 0; selectorIndex + 1 < rule.selectors.size(); ++selectorIndex) {
            const StyleSelector& selector = rule.selectors[selectorIndex];
            if ((selector.stateMask & stateBit) == 0 || !selectorCanBeOwnedBy(selector, element)) continue;
            const SelectorCombinator combinator = rule.combinators[selectorIndex];
            if (combinator == SelectorCombinator::NextSibling || combinator == SelectorCombinator::SubsequentSibling) return true;
        }
    }
    return false;
}

ComputedStyle StyleSheet::resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t states,
                                  LayoutDirection direction) const {
    ComputedStyle style = mImpl->ruleSet.resolveInternal(element, id, classes, states, {}, nullptr, nullptr, direction, nullptr);
    resolveRelativeFontWeight(style);
    resolvePercentageLineHeight(style);
    style.textDecorationPropagation = style.textDecoration;
    return style;
}

ComputedStyle StyleSheet::resolveElement(const Element& element, LayoutDirection direction) const {
    return resolveElement(element, direction, nullptr);
}

ComputedStyle StyleSheet::resolveElement(const Element& element, LayoutDirection direction,
                                         const CustomPropertyMap* inheritedCustomProperties) const {
    ComputedStyle style = mImpl->ruleSet.resolveInternal(element.elementName(), element.id(), element.classes(), element.states(), {}, &element,
                                                         nullptr, direction, inheritedCustomProperties);
    if (!inheritedCustomProperties) resolveRelativeFontWeight(style);
    if (!inheritedCustomProperties) {
        resolvePercentageLineHeight(style);
        style.textDecorationPropagation = style.textDecoration;
    }
    return style;
}

ComputedStyle StyleSheet::resolvePseudoElement(const Element& owner, std::string_view pseudoElementName, LayoutDirection direction) const {
    return resolvePseudoElement(owner, pseudoElementName, direction, nullptr);
}

ComputedStyle StyleSheet::resolvePseudoElement(const Element& owner, std::string_view pseudoElementName, LayoutDirection direction,
                                               const CustomPropertyMap* inheritedCustomProperties) const {
    ComputedStyle style = mImpl->ruleSet.resolveInternal(owner.elementName(), owner.id(), owner.classes(), owner.states(), pseudoElementName, &owner,
                                                         nullptr, direction, inheritedCustomProperties);
    if (!inheritedCustomProperties) resolveRelativeFontWeight(style);
    if (!inheritedCustomProperties) {
        resolvePercentageLineHeight(style);
        style.textDecorationPropagation = style.textDecoration;
    }
    return style;
}

ComputedStyle StyleSheet::resolveInline(const Element& owner, const std::string& element, const std::vector<std::string>& inlineAncestors,
                                        LayoutDirection direction) const {
    static const std::set<std::string> sNoClasses;
    ComputedStyle style = mImpl->ruleSet.resolveInternal(element, {}, sNoClasses, 0, {}, &owner, &inlineAncestors, direction, nullptr);
    resolveRelativeFontWeight(style);
    resolvePercentageLineHeight(style);
    style.textDecorationPropagation = style.textDecoration;
    return style;
}

ComputedStyle StyleRuleSet::resolveInternal(const std::string& element, const std::string& id, const std::set<std::string>& classes,
                                            uint16_t ownerStates, std::string_view pseudoElement, const Element* target,
                                            const std::vector<std::string>* inlineAncestors, LayoutDirection direction,
                                            const CustomPropertyMap* inheritedCustomProperties) const {
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
        if (matchesRule(rule, element, id, classes, ownerStates, pseudoElement, target, inlineAncestors, direction)) matchedRules.push_back(&rule);
    }

    ComputedStyle style;
    style.customProperties = resolveCustomProperties(inheritedCustomProperties, matchedRules);
    for (const StyleRule* rule : matchedRules) {
        for (const StyleDeclaration& declaration : rule->declarations) {
            if (const auto* deferred = std::get_if<DeferredStyleValue>(&declaration.value)) {
                CustomPropertyResolver customProperties(style.customProperties);
                const std::optional<CustomPropertyValue> substituted = customProperties.substituteValue(deferred->source);
                // CSS computed-value invalidity discards lower cascaded values; reset only this property.
                if (!substituted) {
                    detail::applyInvalidStyleDeclaration(style, declaration.property.get());
                    continue;
                }
                const detail::CSSTokenStream stream(substituted->source);
                StyleSheetLoadResult ignored;
                const auto compiled =
                    StyleModel::compileDeclaration(declaration.property.get(), stream, {0, stream.tokens().size()}, {}, ignored, {});
                if (!compiled) {
                    detail::applyInvalidStyleDeclaration(style, declaration.property.get());
                    continue;
                }
                for (const StyleDeclaration& resolved : *compiled) detail::applyStyleDeclaration(style, resolved);
            } else detail::applyStyleDeclaration(style, declaration);
        }
    }
    normalizeOverflow(style);
    resolveLightDarkColors(style);
    return style;
}
} // namespace radia::ui
