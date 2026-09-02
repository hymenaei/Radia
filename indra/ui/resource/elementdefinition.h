/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <initializer_list>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>
#include "diagnostic.h"
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "html/elementnames.h"
#include "layout/buildresult.h"
#include "layout/document.h"
#include "llerror.h"
#include "localization.h"
#include "types.h"

namespace radia::ui {
class HTMLLabelElement;

class ResourceBuildContext {
public:
    ResourceBuildContext(const LocalizationCatalog& localization, std::string locale) : mLocalization(localization), mLocale(std::move(locale)) {}

    bool hasLocalizationKey(const std::string& key) const { return mLocalization.containsDefaultString(key); }

    std::string resolveText(const std::string& key) const { return mLocalization.resolveText(mLocale, key); }

    std::string resolveHTML(const LocalizedText& text) const { return mLocalization.resolveHTML(mLocale, text); }

    LocalizedText t(std::string key) const { return LocalizedText(std::move(key)); }

private:
    const LocalizationCatalog& mLocalization;
    std::string mLocale;
};

class ElementBuildContext final {
public:
    ElementBuildContext(ResourceBuildResult& result, const ResourceBuildContext* resourceContext)
        : mResult(result), mResourceContext(resourceContext) {}

    bool hasLocalization() const { return mResourceContext != nullptr; }
    bool hasLocalizationKey(const std::string& key) const { return mResourceContext && mResourceContext->hasLocalizationKey(key); }
    std::string resolveText(const std::string& key) const { return mResourceContext ? mResourceContext->resolveText(key) : key; }
    std::string resolveHTML(const LocalizedText& text) const { return mResourceContext ? mResourceContext->resolveHTML(text) : text.key(); }
    LocalizedText t(std::string key) const { return mResourceContext ? mResourceContext->t(std::move(key)) : LocalizedText(std::move(key)); }

    void warning(std::string code, std::string message, std::string sourceName = {}, std::size_t line = 0, std::size_t column = 0) {
        mResult.warning(std::move(code), std::move(message), std::move(sourceName), line, column);
    }

    void error(std::string code, std::string message, std::string sourceName = {}, std::size_t line = 0, std::size_t column = 0) {
        mResult.error(std::move(code), std::move(message), std::move(sourceName), line, column);
    }

    std::size_t errorCount() const { return mResult.errors.size(); }

private:
    ResourceBuildResult& mResult;
    const ResourceBuildContext* mResourceContext;
};

enum class ElementContentMode : uint8_t { Unsupported, ElementText, TextChildren };

struct ElementAttribute {
    std::string authoredName;
    std::string value;
    SourceRange source;
};

using ElementAttributeMap = std::unordered_map<std::string, ElementAttribute>;

struct ElementBuildInput {
    HTMLTag tag = HTMLTag::Unknown;
    std::string authoredName;
    SourceRange source;
    std::string sourceName;
    ElementAttributeMap attributes;

    const ElementAttribute* find(std::string_view name) const {
        const auto found = attributes.find(canonicalizeHTMLName(name));
        return found == attributes.end() ? nullptr : &found->second;
    }
};

struct ScopedElementDefinition {
    std::string elementName;
    std::vector<HTMLTag> acceptedTags;
    std::function<Element*(Element&, ElementBuildContext&, const std::string&, std::size_t, std::size_t)> create;
};

struct ElementAttributeDefinition {
    std::vector<std::string> names;
    std::function<void(const ElementBuildInput&, Element&, ElementBuildContext&)> apply;
};

struct ResourceRootDefinition {
    std::string expectedElementName;
};

struct ChildClaim {
    enum class Kind : uint8_t { NotHandled, Handled, Container };

    static ChildClaim notHandled() { return ChildClaim(Kind::NotHandled); }
    static ChildClaim handled() { return ChildClaim(Kind::Handled); }
    static ChildClaim routeTo(Element& container) { return ChildClaim(Kind::Container, &container); }

    Kind kind() const { return mKind; }
    Element* container() const { return mContainer; }

private:
    explicit ChildClaim(Kind kind, Element* container = nullptr) : mKind(kind), mContainer(container) {}

    Kind mKind = Kind::NotHandled;
    Element* mContainer = nullptr;
};

struct ElementAttributeBehavior {
    std::function<void(const ElementBuildInput&, Element&, ElementBuildContext&)> apply;
};

struct ElementCompositionBehavior {
    std::function<void(const ElementBuildInput&, Element&, const class ElementScopeContext&, ElementBuildContext&)> validate;
};

struct ElementChildrenBehavior {
    std::unordered_map<std::string, std::vector<std::string>> partAttributes;
    std::function<ChildClaim(const ElementBuildInput&, Element&, ElementBuildContext&)> claim;
};

struct ElementContentBehavior {
    ElementContentMode mode = ElementContentMode::TextChildren;
    std::function<void(std::string, Element&, ElementBuildContext&, const std::string&, std::size_t)> applyText;
    std::function<std::unique_ptr<Element>(std::string)> createTextChild;
    std::unordered_map<std::string, ScopedElementDefinition> scopedElements;
};

class ElementScopeContext {
public:
    using LabelablePredicate = std::function<bool(const Element&)>;

    ElementScopeContext(const std::unordered_map<std::string, Element*>& elements, const std::unordered_set<std::string>& ambiguousIds,
                        LabelablePredicate isLabelable)
        : mElements(elements), mAmbiguousIds(ambiguousIds), mIsLabelable(std::move(isLabelable)) {}

    Element* find(const std::string& id) const {
        if (id.empty()) return nullptr;
        const auto found = mElements.find(id);
        return found == mElements.end() ? nullptr : found->second;
    }

    bool ambiguous(const std::string& id) const { return mAmbiguousIds.find(id) != mAmbiguousIds.end(); }
    bool labelable(const Element& element) const { return mIsLabelable(element); }

private:
    const std::unordered_map<std::string, Element*>& mElements;
    const std::unordered_set<std::string>& mAmbiguousIds;
    LabelablePredicate mIsLabelable;
};

struct ResourceElementDefinition {
    std::string elementName;
    std::vector<std::string> attributes;
    ElementAttributeBehavior attributeBehavior;
    ElementCompositionBehavior compositionBehavior;
    ElementChildrenBehavior childrenBehavior;
    ElementContentBehavior contentBehavior;
    std::optional<ResourceRootDefinition> resourceRoot;
    std::vector<ElementState> producedStates;
    std::vector<std::string> pseudoElementNames;
    bool labelable = false;
    bool scopedOnly = false;
};

namespace detail {
class ElementDefinitions final {
public:
    ElementDefinitions() = delete;

    static ResourceElementDefinition button();
    static ResourceElementDefinition fieldset();
    static ResourceElementDefinition floater();
    static ResourceElementDefinition icon();
    static ResourceElementDefinition input();
    static ResourceElementDefinition label();
    static ResourceElementDefinition legend();
    static ResourceElementDefinition panel();
    static ResourceElementDefinition minimize();
    static ResourceElementDefinition close();
};
} // namespace detail

struct ElementSelectorMetadata {
    std::string elementName;
    bool known = false;
    bool pseudoElementKnown = false;
    bool elementProducesState = false;
};

const ResourceElementDefinition* findElementDefinition(HTMLTag tag);
ElementSelectorMetadata inspectElementSelector(HTMLTag tag, std::string_view pseudoElement, std::optional<ElementState> elementState = std::nullopt);

bool readElementAttribute(const ElementBuildInput& input, std::string_view name, std::string& value);
bool readElementBoolean(const ElementBuildInput& input, std::string_view name, bool& value, ElementBuildContext& context);
bool producesState(const ResourceElementDefinition& element, ElementState state);
namespace detail {
class ElementCompilerAccess {
public:
    static void setStyleAttribute(Element& element, std::string name, std::string value);
    static void removeStyleAttribute(Element& element, std::string_view name);
    static void setIdScopeRoot(Element& element);
    static void setState(Element& element, ElementState state, bool enabled);
    static const std::string& labelTargetId(const HTMLLabelElement& label);
    static Element* labelTarget(const HTMLLabelElement& label);
    static void setFlowBreakBefore(Element& element, bool enabled);
};

} // namespace detail

struct ResolvedLayoutText {
    std::string literal;
    std::string prefix;
    std::string suffix;
    std::optional<LocalizedText> text;
};

ResolvedLayoutText localizedLayoutText(std::string value, ElementBuildContext& context, const std::string& sourceName, std::size_t line = 0);
void validateElementAttributes(const ElementBuildInput& input, const std::vector<std::string>& elementAttributes, ElementBuildContext& context);
void applyCommonElementAttributes(const ElementBuildInput& input, Element& element, ElementBuildContext& context);

inline ElementAttributeDefinition allowedAttribute(std::string name) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    return definition;
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition stringAttribute(std::initializer_list<std::string> names, SetterT setter) {
    ElementAttributeDefinition definition;
    definition.names.assign(names.begin(), names.end());
    definition.apply = [names = definition.names, setter](const ElementBuildInput& input, Element& element, ElementBuildContext&) {
        std::string value;
        for (const std::string& name : names) {
            if (!readElementAttribute(input, name, value)) continue;
            (void)std::invoke(setter, static_cast<ElementT&>(element), std::move(value));
            return;
        }
    };
    return definition;
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition stringAttribute(std::string name, SetterT setter) {
    return stringAttribute<ElementT>({std::move(name)}, setter);
}

template<typename ElementT>
ElementAttributeDefinition stringAttribute(std::initializer_list<std::string> names, ElementT& (ElementT::*setter)(std::string)) {
    return stringAttribute<ElementT, decltype(setter)>(names, setter);
}

template<typename ElementT> ElementAttributeDefinition stringAttribute(std::string name, ElementT& (ElementT::*setter)(std::string)) {
    return stringAttribute<ElementT, decltype(setter)>(std::move(name), setter);
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition booleanAttribute(std::string name, SetterT setter) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
        bool value = false;
        if (readElementBoolean(input, name, value, context)) (void)std::invoke(setter, static_cast<ElementT&>(element), value);
    };
    return definition;
}

template<typename ElementT> ElementAttributeDefinition booleanAttribute(std::string name, ElementT& (ElementT::*setter)(bool)) {
    return booleanAttribute<ElementT, decltype(setter)>(std::move(name), setter);
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition localizedStringAttribute(std::string name, SetterT setter) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
        std::string key;
        if (!readElementAttribute(input, name, key)) return;
        LocalizedText value(key);
        if (context.hasLocalization()) {
            if (!context.hasLocalizationKey(key))
                context.error("layout.localization.missing", "Unknown localization key: " + key + ".", input.sourceName, input.source.begin.line,
                              input.source.begin.column);
            value = context.t(key);
        }
        (void)std::invoke(setter, static_cast<ElementT&>(element), std::move(value));
    };
    return definition;
}

template<typename ElementT> ElementAttributeDefinition localizedStringAttribute(std::string name, ElementT& (ElementT::*setter)(LocalizedText)) {
    return localizedStringAttribute<ElementT, decltype(setter)>(std::move(name), setter);
}

template<typename ElementT>
ElementAttributeDefinition resolvedLocalizedStringAttribute(std::string name, ElementT& (ElementT::*setter)(LocalizedText, std::string)) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
        std::string key;
        if (!readElementAttribute(input, name, key)) return;
        LocalizedText value(key);
        std::string resolved = key;
        if (context.hasLocalization()) {
            if (!context.hasLocalizationKey(key))
                context.error("layout.localization.missing", "Unknown localization key: " + key + ".", input.sourceName, input.source.begin.line,
                              input.source.begin.column);
            value = context.t(key);
            resolved = context.resolveText(key);
        }
        (void)std::invoke(setter, static_cast<ElementT&>(element), std::move(value), std::move(resolved));
    };
    return definition;
}

template<typename ElementT> class ElementDefinitionBuilder final {
public:
    explicit ElementDefinitionBuilder(std::string_view elementName) { mDefinition.elementName = elementName; }
    ElementDefinitionBuilder(const ElementDefinitionBuilder&) = delete;
    ElementDefinitionBuilder& operator=(const ElementDefinitionBuilder&) = delete;
    ElementDefinitionBuilder(ElementDefinitionBuilder&&) noexcept = default;
    ElementDefinitionBuilder& operator=(ElementDefinitionBuilder&&) noexcept = default;

    ElementDefinitionBuilder&& attributes(std::initializer_list<ElementAttributeDefinition> attributes) {
        mAttributes.insert(mAttributes.end(), attributes.begin(), attributes.end());
        return std::move(*this);
    }

    template<typename ValidatorT> ElementDefinitionBuilder&& validate(ValidatorT validator) {
        mValidators.emplace_back(std::move(validator));
        return std::move(*this);
    }

    template<typename ValidatorT> ElementDefinitionBuilder&& composition(ValidatorT validator) {
        mDefinition.compositionBehavior.validate =
            [validator = std::move(validator)](const ElementBuildInput& input, Element& element, const ElementScopeContext& scope,
                                               ElementBuildContext& context) { validator(input, static_cast<ElementT&>(element), scope, context); };
        return std::move(*this);
    }

    ElementDefinitionBuilder&& state(ElementState state) {
        mDefinition.producedStates.push_back(state);
        return std::move(*this);
    }

    ElementDefinitionBuilder&& labelable() {
        mDefinition.labelable = true;
        return std::move(*this);
    }

    ElementDefinitionBuilder&& scopedOnly() {
        mDefinition.scopedOnly = true;
        return std::move(*this);
    }

    ElementDefinitionBuilder&& resourceRoot(std::string expectedElementName = {}) {
        if (expectedElementName.empty()) expectedElementName = mDefinition.elementName;
        mDefinition.resourceRoot = ResourceRootDefinition{std::move(expectedElementName)};
        return std::move(*this);
    }

    ElementDefinitionBuilder&& pseudoElement(std::string name) {
        mDefinition.pseudoElementNames.push_back(canonicalizeHTMLName(name));
        return std::move(*this);
    }

    ElementDefinitionBuilder&& textChildren() {
        mDefinition.contentBehavior.mode = ElementContentMode::TextChildren;
        return std::move(*this);
    }

    template<typename CreateT> ElementDefinitionBuilder&& textChildren(CreateT create) {
        mDefinition.contentBehavior.mode = ElementContentMode::TextChildren;
        mDefinition.contentBehavior.createTextChild = [create = std::move(create)](std::string content) { return create(std::move(content)); };
        return std::move(*this);
    }

    template<typename ApplyT> ElementDefinitionBuilder&& elementText(ApplyT apply) {
        mDefinition.contentBehavior.mode = ElementContentMode::ElementText;
        mDefinition.contentBehavior.applyText = [apply = std::move(apply)](std::string value, Element& element, ElementBuildContext& context,
                                                                           const std::string& sourceName, std::size_t line) {
            apply(std::move(value), static_cast<ElementT&>(element), context, sourceName, line);
        };
        return std::move(*this);
    }

    template<typename ApplyT>
    ElementDefinitionBuilder&& scopedElement(std::string_view elementName, std::initializer_list<HTMLTag> accepted, ApplyT apply) {
        ScopedElementDefinition definition;
        definition.elementName = elementName;
        definition.acceptedTags.assign(accepted.begin(), accepted.end());
        definition.create = [apply = std::move(apply)](Element& element, ElementBuildContext& context, const std::string& sourceName,
                                                       std::size_t line, std::size_t column) -> Element* {
            return apply(static_cast<ElementT&>(element), context, sourceName, line, column);
        };
        mDefinition.contentBehavior.scopedElements.emplace(canonicalizeHTMLName(definition.elementName), std::move(definition));
        return std::move(*this);
    }

    template<typename ClaimT> ElementDefinitionBuilder&& childContainer(std::string name, std::vector<std::string> attributes, ClaimT claim) {
        ChildContainer container;
        container.name = std::move(name);
        container.attributes = std::move(attributes);
        container.claim = std::move(claim);
        mChildContainers.push_back(std::move(container));
        return std::move(*this);
    }

    ResourceElementDefinition build() && {
        llassert_always(!mBuilt);
        if (mBuilt) return {};
        mBuilt = true;
        for (const ElementAttributeDefinition& attribute : mAttributes)
            mDefinition.attributes.insert(mDefinition.attributes.end(), attribute.names.begin(), attribute.names.end());
        if (!mAttributes.empty() || !mValidators.empty()) {
            mDefinition.attributeBehavior.apply = [attributes = std::move(mAttributes), validators = std::move(mValidators)](
                                                      const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
                for (const ElementAttributeDefinition& attribute : attributes)
                    if (attribute.apply) attribute.apply(input, element, context);
                for (const Validator& validator : validators) validator(input, static_cast<ElementT&>(element), context);
            };
        }
        for (const ChildContainer& container : mChildContainers)
            mDefinition.childrenBehavior.partAttributes.emplace(canonicalizeHTMLName(container.name), container.attributes);
        if (!mChildContainers.empty()) {
            mDefinition.childrenBehavior.claim = [containers = std::move(mChildContainers)](const ElementBuildInput& input, Element& element,
                                                                                            ElementBuildContext& context) -> ChildClaim {
                for (const ChildContainer& container : containers) {
                    if (canonicalizeHTMLName(container.name) != canonicalizeHTMLName(htmlTagName(input.tag))) continue;
                    Element* target = container.claim(input, static_cast<ElementT&>(element), context);
                    return target ? ChildClaim::routeTo(*target) : ChildClaim::handled();
                }
                return ChildClaim::notHandled();
            };
        }
        return std::move(mDefinition);
    }

private:
    using Validator = std::function<void(const ElementBuildInput&, ElementT&, ElementBuildContext&)>;
    struct ChildContainer {
        std::string name;
        std::vector<std::string> attributes;
        std::function<Element*(const ElementBuildInput&, ElementT&, ElementBuildContext&)> claim;
    };

    ResourceElementDefinition mDefinition;
    std::vector<ElementAttributeDefinition> mAttributes;
    std::vector<Validator> mValidators;
    std::vector<ChildContainer> mChildContainers;
    bool mBuilt = false;
};

template<typename ElementT> ElementDefinitionBuilder<ElementT> defineElement(std::string_view elementName) {
    return ElementDefinitionBuilder<ElementT>(elementName);
}
} // namespace radia::ui
