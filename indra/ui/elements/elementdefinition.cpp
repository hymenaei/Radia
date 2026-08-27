/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/elementdefinition.h"
#include <algorithm>
#include <cctype>
#include <sstream>
#include <unordered_map>
#include <unordered_set>
#include "elements/button.h"
#include "elements/elementevent.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"

namespace radia::ui {
namespace {
ElementDefinition genericElementDefinition(Tag tag) {
    ElementDefinition result;
    result.elementName = sourceTagName(tag);
    result.create = [tag] { return std::make_unique<Element>(sourceTagName(tag)); };
    if (tag == Tag::Br || tag == Tag::Kbd) result.contentBehavior.mode = ElementContentMode::Unsupported;
    if (tag == Tag::Kbd) result.attributes.push_back("shortcut");
    return result;
}

using CompositeInstances = std::unordered_map<std::string, Element*>;

CompositeTopology makeCompositeTopology(const ElementDefinition& definition) {
    CompositeTopology topology;
    topology.indices.reserve(definition.compositeParts.size());
    for (std::size_t index = 0; index < definition.compositeParts.size(); ++index) {
        const CompositePartDefinition& part = definition.compositeParts[index];
        if (part.path.empty() || !part.create || !topology.indices.emplace(part.path, index).second) topology.valid = false;
    }
    if (!topology.valid) return topology;

    std::vector<uint8_t> state(definition.compositeParts.size(), 0);
    std::function<void(std::size_t)> visit = [&](std::size_t index) {
        if (!topology.valid || state[index] == 2) return;
        if (state[index] == 1) {
            topology.valid = false;
            return;
        }
        state[index] = 1;
        const CompositePartDefinition& part = definition.compositeParts[index];
        if (!part.parentPath.empty()) {
            const auto parent = topology.indices.find(part.parentPath);
            if (parent == topology.indices.end()) {
                topology.valid = false;
                return;
            }
            visit(parent->second);
        }
        state[index] = 2;
        topology.order.push_back(index);
    };
    for (std::size_t index = 0; index < definition.compositeParts.size() && topology.valid; ++index) visit(index);
    if (!topology.valid) topology.order.clear();
    return topology;
}

const CompositeTopology* topologyFor(const ElementDefinition& definition, CompositeTopology& fallback) {
    if (definition.compositeTopology) return definition.compositeTopology.get();
    fallback = makeCompositeTopology(definition);
    return &fallback;
}

void collectCompositeInstances(Element& owner, const ElementDefinition& definition, const CompositeTopology& topology,
                               CompositeInstances& instances) {
    for (const std::size_t index : topology.order) {
        const CompositePartDefinition& part = definition.compositeParts[index];
        Element* parent = &owner;
        if (!part.parentPath.empty()) {
            const auto found = instances.find(part.parentPath);
            if (found == instances.end()) continue;
            parent = found->second;
        }
        const ElementList children = parent->children();
        const auto existing = std::find_if(children.begin(), children.end(), [&owner, &part](Element* child) {
            return child->styleElement() == owner.styleElement() && child->part() == part.path;
        });
        if (existing != children.end()) instances.emplace(part.path, *existing);
    }
}

Element* instantiateCompositePartImpl(Element& owner, const ElementDefinition& definition, const CompositeTopology& topology, std::size_t partIndex,
                                      CompositeInstances& instances, std::unordered_set<std::string>& constructing) {
    if (!topology.valid || partIndex >= definition.compositeParts.size()) return nullptr;
    const CompositePartDefinition& part = definition.compositeParts[partIndex];
    const auto existing = instances.find(part.path);
    if (existing != instances.end()) return existing->second;
    if (!constructing.insert(part.path).second || part.path.empty() || !part.create) return nullptr;

    Element* parent = &owner;
    if (!part.parentPath.empty()) {
        const auto parentIndex = topology.indices.find(part.parentPath);
        if (parentIndex == topology.indices.end()) {
            constructing.erase(part.path);
            return nullptr;
        }
        parent = instantiateCompositePartImpl(owner, definition, topology, parentIndex->second, instances, constructing);
        if (!parent) {
            constructing.erase(part.path);
            return nullptr;
        }
    }

    std::unique_ptr<Element> child = part.create();
    if (!child) {
        constructing.erase(part.path);
        return nullptr;
    }
    detail::ElementCompilerAccess::setStyleIdentity(*child, owner.styleElement(), part.path);
    Element* instance = child.get();
    parent->append(std::move(child));
    instances.emplace(part.path, instance);
    constructing.erase(part.path);
    if (part.bind) part.bind(owner, *instance);
    return instance;
}
} // namespace

void detail::ElementCompilerAccess::setStyleIdentity(Element& element, std::string elementName, std::string part) {
    element.setStyleElement(std::move(elementName));
    element.setPart(std::move(part));
}

void detail::ElementCompilerAccess::setStyleAttribute(Element& element, std::string name, std::string value) {
    element.mStyleAttributes[std::move(name)] = std::move(value);
    element.invalidateStyleTree(false, false);
}

void detail::ElementCompilerAccess::removeStyleAttribute(Element& element, std::string_view name) {
    element.mStyleAttributes.erase(std::string(name));
    element.invalidateStyleTree(false, false);
}

void detail::ElementCompilerAccess::setIdScopeRoot(Element& element) {
    element.setIdScopeRoot(true);
}

void detail::ElementCompilerAccess::setState(Element& element, ElementState state, bool enabled) {
    element.setState(state, enabled);
}

const std::string& detail::ElementCompilerAccess::labelTargetId(const LabelElement& label) {
    return label.mTargetId;
}

Element* detail::ElementCompilerAccess::labelTarget(const LabelElement& label) {
    return label.mTarget;
}

void detail::ElementCompilerAccess::setLabelTarget(LabelElement& label, Element* target) {
    label.mTarget = target;
}

void detail::ElementCompilerAccess::setFlowBreakBefore(Element& element, bool enabled) {
    NodeAccess::setFlowBreakBefore(element, enabled);
}

void detail::ElementCompilerAccess::setKeybinding(Element& element, std::string keybindingId) {
    element.setKeybinding(std::move(keybindingId));
}

void detail::prepareCompositeTopology(ElementDefinition& definition) {
    definition.compositeTopology = std::make_shared<const CompositeTopology>(makeCompositeTopology(definition));
}

const CompositePartDefinition* findCompositePartDefinition(const ElementDefinition& element, const std::vector<std::string>& parts) {
    if (parts.empty()) return nullptr;
    std::string path;
    for (const std::string& part : parts) {
        if (!path.empty()) path += "::";
        path += part;
    }
    if (element.compositeTopology) {
        const auto index = element.compositeTopology->indices.find(path);
        if (index != element.compositeTopology->indices.end() && index->second < element.compositeParts.size())
            return &element.compositeParts[index->second];
    }
    const auto composite = std::find_if(element.compositeParts.begin(), element.compositeParts.end(),
                                        [&path](const CompositePartDefinition& part) { return part.path == path; });
    return composite == element.compositeParts.end() ? nullptr : &*composite;
}

bool producesState(const ElementDefinition& definition, ElementState state) {
    return std::find(definition.producedStates.begin(), definition.producedStates.end(), state) != definition.producedStates.end();
}

bool producesState(const CompositePartDefinition& part, ElementState state) {
    return std::find(part.producedStates.begin(), part.producedStates.end(), state) != part.producedStates.end();
}

void detail::instantiateCompositeParts(Element& owner, const ElementDefinition& definition) {
    CompositeTopology fallback;
    const CompositeTopology& topology = *topologyFor(definition, fallback);
    if (!topology.valid) return;
    CompositeInstances instances;
    instances.reserve(definition.compositeParts.size());
    collectCompositeInstances(owner, definition, topology, instances);
    std::unordered_set<std::string> constructing;
    for (const std::size_t index : topology.order)
        if (definition.compositeParts[index].eager) instantiateCompositePartImpl(owner, definition, topology, index, instances, constructing);
}

Element* detail::instantiateCompositePart(Element& owner, const ElementDefinition& definition, const std::string& path) {
    CompositeTopology fallback;
    const CompositeTopology& topology = *topologyFor(definition, fallback);
    if (!topology.valid) return nullptr;
    const auto part = topology.indices.find(path);
    if (part == topology.indices.end()) return nullptr;
    CompositeInstances instances;
    instances.reserve(definition.compositeParts.size());
    collectCompositeInstances(owner, definition, topology, instances);
    std::unordered_set<std::string> constructing;
    return instantiateCompositePartImpl(owner, definition, topology, part->second, instances, constructing);
}

bool readElementAttribute(const ElementBuildInput& input, std::string_view name, std::string& value) {
    const ElementAttribute* attribute = input.find(name);
    if (!attribute) return false;
    value = attribute->value;
    return true;
}

bool readElementBoolean(const ElementBuildInput& input, std::string_view name, bool& value, LayoutBuildResult& result) {
    std::string text;
    if (!readElementAttribute(input, name, text)) return false;
    if (text.empty() || text == "true" || text == "1") value = true;
    else if (text == "false" || text == "0") value = false;
    else {
        result.error("layout.attribute.boolean_invalid", "Invalid boolean value for " + std::string(name) + ": " + text + ".", input.sourceName,
                     input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}

namespace {
bool readElementVisibility(const ElementBuildInput& input, Visibility& value, LayoutBuildResult& result) {
    std::string text;
    if (!readElementAttribute(input, "visibility", text)) return false;
    if (text == "visible") value = Visibility::Visible;
    else if (text == "hidden") value = Visibility::Hidden;
    else if (text == "collapse") value = Visibility::Collapse;
    else {
        result.error("layout.attribute.visibility_invalid", "Invalid visibility value: " + text + ". Expected visible, hidden, or collapse.",
                     input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}
} // namespace

ResolvedLayoutText localizedLayoutText(std::string value, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext* context,
                                       std::size_t line) {
    ResolvedLayoutText resolved;
    if (!context) {
        resolved.literal = std::move(value);
        return resolved;
    }

    const auto first = std::find_if_not(value.begin(), value.end(), [](unsigned char character) { return std::isspace(character); });
    const auto last = std::find_if_not(value.rbegin(), value.rend(), [](unsigned char character) { return std::isspace(character); }).base();
    if (first == value.end()) return resolved;
    const std::string trimmed(first, last);
    if (trimmed.size() < 4 || trimmed.rfind("{{", 0) != 0 || trimmed.substr(trimmed.size() - 2) != "}}") {
        resolved.literal = std::move(value);
        return resolved;
    }

    const std::string key = trimmed.substr(2, trimmed.size() - 4);
    if (key.empty() || key.find_first_of("{}<>\"' \t\r\n") != std::string::npos) {
        result.error("layout.localization.invalid", "Invalid localization key: " + key + ".", source, line);
        resolved.literal = std::move(value);
        return resolved;
    }
    if (!context->hasLocalizationKey(key)) result.error("layout.localization.missing", "Unknown localization key: " + key + ".", source, line);
    resolved.prefix.assign(value.begin(), first);
    resolved.suffix.assign(last, value.end());
    resolved.text = context->t(key);
    return resolved;
}

void validateElementAttributes(const ElementBuildInput& input, const std::vector<std::string>& elementAttributes, LayoutBuildResult& result) {
    static const std::unordered_set<std::string> sCommonAttributes = {
        "id", "class", "visibility", "disabled", "x", "y", "width", "height", "interactive", "blocksPointer",
    };
    std::unordered_set<std::string> allowed;
    for (const std::string& name : elementAttributes) allowed.insert(schemaNameKey(name));
    for (const std::string& name : sCommonAttributes) allowed.insert(schemaNameKey(name));
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) allowed.insert(schemaNameKey(descriptor.attribute));
    const std::string elementName = input.authoredName.empty() ? std::string(sourceTagName(input.tag)) : input.authoredName;
    for (const auto& [attributeName, attribute] : input.attributes)
        if (!allowed.count(attributeName))
            result.error("layout.attribute.unknown", "Unknown attribute on <" + elementName + ">: " + attribute.authoredName + ".", input.sourceName,
                         attribute.source.begin.line, attribute.source.begin.column);
}

void applyCommonElementAttributes(const ElementBuildInput& input, Element& element, LayoutBuildResult& result) {
    static const char* sUnsupportedAttributes[] = {
        "x", "y", "width", "height", "interactive", "blocksPointer",
    };
    for (const char* name : sUnsupportedAttributes) {
        std::string ignored;
        if (readElementAttribute(input, name, ignored))
            result.error("layout.attribute.unsupported", std::string("Unsupported HTML attribute: ") + name + ".", input.sourceName,
                         input.source.begin.line, input.source.begin.column);
    }

    std::string value;
    if (readElementAttribute(input, "id", value)) {
        const ElementAttribute* attribute = input.find("id");
        if (!isElementIdentifier(value))
            result.error("layout.id.invalid", "Element id must be a valid identifier.", input.sourceName, attribute->source.begin.line,
                         attribute->source.begin.column);
        else element.setId(value);
    }
    if (readElementAttribute(input, "class", value)) {
        std::stringstream classes(value);
        while (classes >> value)
            if (!isElementIdentifier(value)) {
                const ElementAttribute* attribute = input.find("class");
                result.error("layout.class.invalid", "Element class must be a valid identifier.", input.sourceName, attribute->source.begin.line,
                             attribute->source.begin.column);
            } else element.addClass(value);
    }
    Visibility visibility = Visibility::Visible;
    if (readElementVisibility(input, visibility, result)) element.setVisibility(visibility);
    bool boolean = false;
    if (readElementBoolean(input, "disabled", boolean, result)) element.disabled(boolean);

    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        if (!readElementAttribute(input, descriptor.attribute, value)) continue;
        const ElementAttribute* attribute = input.find(descriptor.attribute);

        EventCallParseResult parsed = parseEventCall(value);
        if (!parsed.ok()) {
            result.warning(eventCallParseErrorCode(parsed.error), eventCallParseErrorMessage(parsed.error), input.sourceName,
                           attribute->source.begin.line, attribute->source.begin.column + parsed.errorOffset);
            continue;
        }
        const std::string& handlerName = parsed.call->name();
        if (handlerName == "postBuild" || handlerName == "onOpen" || handlerName == "onClose") {
            result.warning("layout.event.handler_reserved", "Controller lifecycle name cannot be used as an Event Handler: " + handlerName + ".",
                           input.sourceName, attribute->source.begin.line, attribute->source.begin.column);
            continue;
        }
        element.setEventCall(descriptor.type, std::move(*parsed.call));
    }
}

const ElementDefinition* findElementDefinition(Tag tag, std::string_view) {
    switch (tag) {
        case Tag::Input: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::input();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Button: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::button();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Minimize: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::minimize();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Close: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::close();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Fieldset: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::fieldset();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Legend: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::legend();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Floater: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::floater();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Icon: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::icon();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Label: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::label();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        case Tag::Panel: {
            static const ElementDefinition definition = [] {
                ElementDefinition result = detail::ElementDefinitionFactory::panel();
                detail::prepareCompositeTopology(result);
                return result;
            }();
            return &definition;
        }
        default: break;
    }

    if (!isGenericElementTag(tag)) return nullptr;
    using GenericDefinitions = std::unordered_map<Tag, ElementDefinition>;
    static const GenericDefinitions definitions = [] {
        GenericDefinitions result;
        for (const Tag genericTag : genericElementTags()) result.emplace(genericTag, genericElementDefinition(genericTag));
        return result;
    }();
    const auto found = definitions.find(tag);
    return found == definitions.end() ? nullptr : &found->second;
}

namespace detail {
std::unique_ptr<Element> createRuntimeElement(std::string_view elementName) {
    const ElementDefinition* definition = findElementDefinition(sourceTagFromName(elementName));
    if (!definition || !definition->create) return nullptr;

    std::unique_ptr<Element> element = definition->create();
    llassert_always(element);
    return element;
}
} // namespace detail

ElementSelectorMetadata inspectElementSelector(Tag tag, std::string_view inputType, const std::vector<std::string>& parts,
                                               std::optional<ElementState> elementState, std::optional<ElementState> partState) {
    ElementSelectorMetadata result;
    const ElementDefinition* owner = findElementDefinition(tag);
    if (!owner) return result;

    const CompositePartDefinition* part = parts.empty() ? nullptr : findCompositePartDefinition(*owner, parts);

    result.elementName = owner->elementName;
    result.known = true;
    result.partKnown = parts.empty() || part != nullptr;
    result.elementProducesState = elementState && producesState(*owner, *elementState);
    result.partProducesState = partState && part && producesState(*part, *partState);
    return result;
}
} // namespace radia::ui
