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
#include <type_traits>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>
#include "diagnostic.h"
#include "elements/element.h"
#include "elements/elementinternal.h"
#include "layout/buildresult.h"
#include "layout/document.h"
#include "layout/schema.h"
#include "llerror.h"
#include "localization.h"
#include "types.h"

namespace radia::ui {
class LabelElement;

class LayoutBuildContext {
public:
    LayoutBuildContext(const LocalizationCatalog& localization, std::string locale) : mLocalization(localization), mLocale(std::move(locale)) {}

    bool hasLocalizationKey(const std::string& key) const { return mLocalization.containsDefaultString(key); }

    std::string resolveText(const std::string& key) const { return mLocalization.resolveText(mLocale, key); }

    std::string resolveMarkup(const LocalizedText& text) const { return mLocalization.resolveMarkup(mLocale, text); }

    LocalizedText t(std::string key) const { return LocalizedText(std::move(key)); }

private:
    const LocalizationCatalog& mLocalization;
    std::string mLocale;
};

enum class ElementContentMode : uint8_t { Unsupported, ElementText, TextChildren };

struct ElementAttribute {
    std::string authoredName;
    std::string value;
    SourceRange source;
};

using ElementAttributeMap = std::unordered_map<std::string, ElementAttribute>;

struct ElementBuildInput {
    Tag tag = Tag::Unknown;
    std::string authoredName;
    SourceRange source;
    std::string sourceName;
    ElementAttributeMap attributes;

    const ElementAttribute* find(std::string_view name) const {
        const auto found = attributes.find(schemaNameKey(name));
        return found == attributes.end() ? nullptr : &found->second;
    }
};

struct CompositePartDefinition {
    std::string path;
    std::string parentPath;
    std::function<std::unique_ptr<Element>()> create;
    std::function<void(Element&, Element&)> bind;
    std::vector<ElementState> producedStates;
    bool eager = true;
};

struct CompositeTopology {
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<std::size_t> order;
    bool valid = true;
};

struct ScopedElementDefinition {
    std::string elementName;
    std::vector<Tag> acceptedElements;
    std::function<Element*(Element&, LayoutBuildResult&, const std::string&, std::size_t, std::size_t)> create;
};

struct ElementAttributeDefinition {
    std::vector<std::string> names;
    std::function<void(const ElementBuildInput&, Element&, LayoutBuildResult&, const LayoutBuildContext*)> apply;
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
    std::function<void(const ElementBuildInput&, Element&, LayoutBuildResult&, const LayoutBuildContext*)> apply;
};

struct ElementCompositionBehavior {
    std::function<void(const ElementBuildInput&, Element&, const class ElementScopeContext&, LayoutBuildResult&)> validate;
};

struct ElementChildrenBehavior {
    std::unordered_map<std::string, std::vector<std::string>> partAttributes;
    std::function<ChildClaim(const ElementBuildInput&, Element&, LayoutBuildResult&)> claim;
};

struct ElementContentBehavior {
    ElementContentMode mode = ElementContentMode::TextChildren;
    std::function<void(std::string, Element&, LayoutBuildResult&, const std::string&, const LayoutBuildContext*, std::size_t)> applyText;
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
        if (id.empty() || ambiguous(id)) return nullptr;
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

struct ElementDefinition {
    std::string elementName;
    std::function<std::unique_ptr<Element>()> create;
    std::vector<std::string> attributes;
    ElementAttributeBehavior attributeBehavior;
    ElementCompositionBehavior compositionBehavior;
    ElementChildrenBehavior childrenBehavior;
    ElementContentBehavior contentBehavior;
    std::optional<ResourceRootDefinition> resourceRoot;
    std::vector<ElementState> producedStates;
    std::vector<CompositePartDefinition> compositeParts;
    std::shared_ptr<const CompositeTopology> compositeTopology;
    bool labelable = false;
    bool scopedOnly = false;
};

namespace detail {
class ElementDefinitionFactory {
public:
    static ElementDefinition button();
    static ElementDefinition fieldset();
    static ElementDefinition floater();
    static ElementDefinition icon();
    static ElementDefinition input();
    static ElementDefinition label();
    static ElementDefinition legend();
    static ElementDefinition panel();
    static ElementDefinition minimize();
    static ElementDefinition close();
};
} // namespace detail

struct ElementSelectorMetadata {
    std::string elementName;
    bool known = false;
    bool partKnown = false;
    bool elementProducesState = false;
    bool partProducesState = false;
};

const ElementDefinition* findElementDefinition(Tag tag, std::string_view inputType = {});
ElementSelectorMetadata inspectElementSelector(Tag tag, std::string_view inputType, const std::vector<std::string>& parts,
                                               std::optional<ElementState> elementState = std::nullopt,
                                               std::optional<ElementState> partState = std::nullopt);

bool readElementAttribute(const ElementBuildInput& input, std::string_view name, std::string& value);
bool readElementBoolean(const ElementBuildInput& input, std::string_view name, bool& value, LayoutBuildResult& result);
const CompositePartDefinition* findCompositePartDefinition(const ElementDefinition& element, const std::vector<std::string>& parts);
bool producesState(const ElementDefinition& element, ElementState state);
bool producesState(const CompositePartDefinition& part, ElementState state);
namespace detail {
class ElementCompilerAccess {
public:
    static void setStyleIdentity(Element& element, std::string elementName, std::string part);
    static void setStyleAttribute(Element& element, std::string name, std::string value);
    static void removeStyleAttribute(Element& element, std::string_view name);
    static void setIdScopeRoot(Element& element);
    static void setState(Element& element, ElementState state, bool enabled);
    static const std::string& labelTargetId(const LabelElement& label);
    static Element* labelTarget(const LabelElement& label);
    static void setLabelTarget(LabelElement& label, Element* target);
    static void setFlowBreakBefore(Element& element, bool enabled);
    static void setKeybinding(Element& element, std::string keybindingId);
};

void prepareCompositeTopology(ElementDefinition& definition);
void instantiateCompositeParts(Element& owner, const ElementDefinition& definition);
Element* instantiateCompositePart(Element& owner, const ElementDefinition& definition, const std::string& path);
} // namespace detail

struct ResolvedLayoutText {
    std::string literal;
    std::string prefix;
    std::string suffix;
    std::optional<LocalizedText> text;
};

ResolvedLayoutText localizedLayoutText(std::string value, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext* context,
                                       std::size_t line = 0);
void validateElementAttributes(const ElementBuildInput& input, const std::vector<std::string>& elementAttributes, LayoutBuildResult& result);
void applyCommonElementAttributes(const ElementBuildInput& input, Element& element, LayoutBuildResult& result);

namespace detail {
template<typename PartT, typename OwnerT, typename SlotT> CompositePartDefinition makeElementPart(std::string path, SlotT* OwnerT::* slot) {
    static_assert(std::is_base_of_v<Element, OwnerT>);
    static_assert(std::is_base_of_v<Element, PartT>);
    static_assert(std::is_base_of_v<SlotT, PartT>);

    CompositePartDefinition definition;
    const std::size_t separator = path.rfind("::");
    if (separator != std::string::npos) definition.parentPath = path.substr(0, separator);
    definition.path = std::move(path);
    definition.create = [] { return std::make_unique<PartT>(); };
    definition.bind = [slot](Element& owner, Element& part) {
        OwnerT& typedOwner = static_cast<OwnerT&>(owner);
        PartT& typedPart = static_cast<PartT&>(part);
        typedOwner.*slot = &typedPart;
    };
    return definition;
}
} // namespace detail

inline ElementAttributeDefinition allowedAttribute(std::string name) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    return definition;
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition stringAttribute(std::initializer_list<std::string> names, SetterT setter) {
    ElementAttributeDefinition definition;
    definition.names.assign(names.begin(), names.end());
    definition.apply = [names = definition.names, setter](const ElementBuildInput& input, Element& element, LayoutBuildResult&,
                                                          const LayoutBuildContext*) {
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
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, LayoutBuildResult& result,
                                                                 const LayoutBuildContext*) {
        bool value = false;
        if (readElementBoolean(input, name, value, result)) (void)std::invoke(setter, static_cast<ElementT&>(element), value);
    };
    return definition;
}

template<typename ElementT> ElementAttributeDefinition booleanAttribute(std::string name, ElementT& (ElementT::*setter)(bool)) {
    return booleanAttribute<ElementT, decltype(setter)>(std::move(name), setter);
}

template<typename ElementT, typename SetterT> ElementAttributeDefinition localizedStringAttribute(std::string name, SetterT setter) {
    ElementAttributeDefinition definition;
    definition.names.push_back(std::move(name));
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, LayoutBuildResult& result,
                                                                 const LayoutBuildContext* context) {
        std::string key;
        if (!readElementAttribute(input, name, key)) return;
        LocalizedText value(key);
        if (context) {
            if (!context->hasLocalizationKey(key))
                result.error("layout.localization.missing", "Unknown localization key: " + key + ".", input.sourceName, input.source.begin.line,
                             input.source.begin.column);
            value = context->t(key);
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
    definition.apply = [name = definition.names.front(), setter](const ElementBuildInput& input, Element& element, LayoutBuildResult& result,
                                                                 const LayoutBuildContext* context) {
        std::string key;
        if (!readElementAttribute(input, name, key)) return;
        LocalizedText value(key);
        std::string resolved = key;
        if (context) {
            if (!context->hasLocalizationKey(key))
                result.error("layout.localization.missing", "Unknown localization key: " + key + ".", input.sourceName, input.source.begin.line,
                             input.source.begin.column);
            value = context->t(key);
            resolved = context->resolveText(key);
        }
        (void)std::invoke(setter, static_cast<ElementT&>(element), std::move(value), std::move(resolved));
    };
    return definition;
}

template<typename ElementT> class ElementDefinitionBuilder final {
public:
    explicit ElementDefinitionBuilder(std::string elementName) {
        mDefinition.elementName = std::move(elementName);
        mDefinition.create = [] { return std::make_unique<ElementT>(); };
    }
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
        mDefinition.compositionBehavior.validate = [validator = std::move(validator)](const ElementBuildInput& input, Element& element,
                                                                                      const ElementScopeContext& scope, LayoutBuildResult& result) {
            validator(input, static_cast<ElementT&>(element), scope, result);
        };
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

    template<typename PartT, typename OwnerT, typename SlotT>
    ElementDefinitionBuilder&& part(std::string path, SlotT* OwnerT::* slot, bool eager = true) {
        static_assert(std::is_same_v<OwnerT, ElementT>);
        CompositePartDefinition part = detail::makeElementPart<PartT>(std::move(path), slot);
        part.eager = eager;
        mDefinition.compositeParts.push_back(std::move(part));
        return std::move(*this);
    }

    template<typename OwnerT, typename PartT> ElementDefinitionBuilder&& part(std::string path, PartT* OwnerT::* slot, bool eager = true) {
        return part<PartT>(std::move(path), slot, eager);
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
        mDefinition.contentBehavior.applyText = [apply = std::move(apply)](std::string value, Element& element, LayoutBuildResult& result,
                                                                           const std::string& source, const LayoutBuildContext* context,
                                                                           std::size_t line) {
            apply(std::move(value), static_cast<ElementT&>(element), result, source, context, line);
        };
        return std::move(*this);
    }

    template<typename ApplyT> ElementDefinitionBuilder&& scopedElement(std::string elementName, std::initializer_list<Tag> accepted, ApplyT apply) {
        ScopedElementDefinition definition;
        definition.elementName = std::move(elementName);
        definition.acceptedElements.assign(accepted.begin(), accepted.end());
        definition.create = [apply = std::move(apply)](Element& element, LayoutBuildResult& result, const std::string& source, std::size_t line,
                                                       std::size_t column) -> Element* {
            return apply(static_cast<ElementT&>(element), result, source, line, column);
        };
        mDefinition.contentBehavior.scopedElements.emplace(schemaNameKey(definition.elementName), std::move(definition));
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

    ElementDefinition build() && {
        llassert_always(!mBuilt);
        if (mBuilt) return {};
        mBuilt = true;
        for (const ElementAttributeDefinition& attribute : mAttributes)
            mDefinition.attributes.insert(mDefinition.attributes.end(), attribute.names.begin(), attribute.names.end());
        if (!mAttributes.empty() || !mValidators.empty()) {
            mDefinition.attributeBehavior.apply = [attributes = std::move(mAttributes), validators = std::move(mValidators)](
                                                      const ElementBuildInput& input, Element& element, LayoutBuildResult& result,
                                                      const LayoutBuildContext* context) {
                for (const ElementAttributeDefinition& attribute : attributes)
                    if (attribute.apply) attribute.apply(input, element, result, context);
                for (const Validator& validator : validators) validator(input, static_cast<ElementT&>(element), result, context);
            };
        }
        for (const ChildContainer& container : mChildContainers)
            mDefinition.childrenBehavior.partAttributes.emplace(schemaNameKey(container.name), container.attributes);
        if (!mChildContainers.empty()) {
            mDefinition.childrenBehavior.claim = [containers = std::move(mChildContainers)](const ElementBuildInput& input, Element& element,
                                                                                            LayoutBuildResult& result) -> ChildClaim {
                for (const ChildContainer& container : containers) {
                    if (schemaNameKey(container.name) != schemaNameKey(sourceTagName(input.tag))) continue;
                    Element* target = container.claim(input, static_cast<ElementT&>(element), result);
                    return target ? ChildClaim::routeTo(*target) : ChildClaim::handled();
                }
                return ChildClaim::notHandled();
            };
        }
        return std::move(mDefinition);
    }

private:
    using Validator = std::function<void(const ElementBuildInput&, ElementT&, LayoutBuildResult&, const LayoutBuildContext*)>;
    struct ChildContainer {
        std::string name;
        std::vector<std::string> attributes;
        std::function<Element*(const ElementBuildInput&, ElementT&, LayoutBuildResult&)> claim;
    };

    ElementDefinition mDefinition;
    std::vector<ElementAttributeDefinition> mAttributes;
    std::vector<Validator> mValidators;
    std::vector<ChildContainer> mChildContainers;
    bool mBuilt = false;
};

template<typename ElementT> ElementDefinitionBuilder<ElementT> defineElement(std::string elementName) {
    return ElementDefinitionBuilder<ElementT>(std::move(elementName));
}
} // namespace radia::ui
