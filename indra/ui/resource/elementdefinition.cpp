/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "resource/elementdefinition.h"
#include <algorithm>
#include <cctype>
#include <unordered_map>
#include <unordered_set>
#include "eventcall.h"
#include "html/label.h"

namespace radia::ui {
using detail::ElementCompilerAccess;

namespace {
std::optional<bool> parseBooleanValue(std::string_view value) {
    if (value.empty() || value == "true" || value == "1") return true;
    if (value == "false" || value == "0") return false;
    return std::nullopt;
}

ResourceElementDefinition htmlContentDefinition(HTMLTag tag) {
    ResourceElementDefinition result;
    result.elementName = htmlTagName(tag);
    switch (tag) {
        case HTMLTag::Br: result.contentBehavior.mode = ElementContentMode::Unsupported; break;
        case HTMLTag::Kbd:
            result.attributes.push_back("shortcut");
            result.contentBehavior.mode = ElementContentMode::Unsupported;
            break;
        default: break;
    }
    return result;
}

} // namespace

void detail::ElementCompilerAccess::setStyleAttribute(Element& element, std::string name, std::string value) {
    element.mStyleAttributes[std::move(name)] = std::move(value);
    element.invalidateStyleTree(true, true);
}

void detail::ElementCompilerAccess::removeStyleAttribute(Element& element, std::string_view name) {
    element.mStyleAttributes.erase(std::string(name));
    element.invalidateStyleTree(true, true);
}

void detail::ElementCompilerAccess::setIdScopeRoot(Element& element) {
    element.setIdScopeRoot(true);
}

void detail::ElementCompilerAccess::setState(Element& element, ElementState state, bool enabled) {
    element.setState(state, enabled);
}

const std::string& detail::ElementCompilerAccess::labelTargetId(const HTMLLabelElement& label) {
    return label.mTargetId;
}

Element* detail::ElementCompilerAccess::labelTarget(const HTMLLabelElement& label) {
    return label.target();
}

void detail::ElementCompilerAccess::setFlowBreakBefore(Element& element, bool enabled) {
    NodeAccess::setFlowBreakBefore(element, enabled);
}

bool producesState(const ResourceElementDefinition& definition, ElementState state) {
    return std::find(definition.producedStates.begin(), definition.producedStates.end(), state) != definition.producedStates.end();
}

bool readElementAttribute(const ElementBuildInput& input, std::string_view name, std::string& value) {
    const ElementAttribute* attribute = input.find(name);
    if (!attribute) return false;
    value = attribute->value;
    return true;
}

bool readElementBoolean(const ElementBuildInput& input, std::string_view name, bool& value, ElementBuildContext& context) {
    std::string text;
    if (!readElementAttribute(input, name, text)) return false;
    const std::optional<bool> parsed = parseBooleanValue(text);
    if (parsed) value = *parsed;
    else {
        context.error("layout.attribute.boolean_invalid", "Invalid boolean value for " + std::string(name) + ": " + text + ".", input.sourceName,
                      input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}

namespace {
bool readElementVisibility(const ElementBuildInput& input, Visibility& value, ElementBuildContext& context) {
    std::string text;
    if (!readElementAttribute(input, "visibility", text)) return false;
    if (text == "visible") value = Visibility::Visible;
    else if (text == "hidden") value = Visibility::Hidden;
    else if (text == "collapse") value = Visibility::Collapse;
    else {
        context.error("layout.attribute.visibility_invalid", "Invalid visibility value: " + text + ". Expected visible, hidden, or collapse.",
                      input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}
} // namespace

ResolvedLayoutText localizedLayoutText(std::string value, ElementBuildContext& context, const std::string& sourceName, std::size_t line) {
    ResolvedLayoutText resolved;
    if (!context.hasLocalization()) {
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
        context.error("layout.localization.invalid", "Invalid localization key: " + key + ".", sourceName, line);
        resolved.literal = std::move(value);
        return resolved;
    }
    if (!context.hasLocalizationKey(key)) context.error("layout.localization.missing", "Unknown localization key: " + key + ".", sourceName, line);
    resolved.prefix.assign(value.begin(), first);
    resolved.suffix.assign(last, value.end());
    resolved.text = context.t(key);
    return resolved;
}

void validateElementAttributes(const ElementBuildInput& input, const std::vector<std::string>& elementAttributes, ElementBuildContext& context) {
    static const std::unordered_set<std::string> sCommonAttributes = {
        "id", "class", "visibility", "disabled", "x", "y", "width", "height", "interactive", "blocksPointer",
    };
    std::unordered_set<std::string> allowed;
    for (const std::string& name : elementAttributes) allowed.insert(canonicalizeHTMLName(name));
    for (const std::string& name : sCommonAttributes) allowed.insert(canonicalizeHTMLName(name));
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) allowed.insert(canonicalizeHTMLName(descriptor.attribute));
    const std::string elementName = input.authoredName.empty() ? std::string(htmlTagName(input.tag)) : input.authoredName;
    for (const auto& [attributeName, attribute] : input.attributes)
        if (!allowed.count(attributeName))
            context.error("layout.attribute.unknown", "Unknown attribute on <" + elementName + ">: " + attribute.authoredName + ".", input.sourceName,
                          attribute.source.begin.line, attribute.source.begin.column);
}

void applyCommonElementAttributes(const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
    static const char* sUnsupportedAttributes[] = {
        "x", "y", "width", "height", "interactive", "blocksPointer",
    };
    for (const char* name : sUnsupportedAttributes) {
        std::string ignored;
        if (readElementAttribute(input, name, ignored))
            context.error("layout.attribute.unsupported", std::string("Unsupported HTML attribute: ") + name + ".", input.sourceName,
                          input.source.begin.line, input.source.begin.column);
    }

    std::string value;
    if (readElementAttribute(input, "id", value)) {
        const ElementAttribute* attribute = input.find("id");
        if (value.empty() || containsHTMLWhitespace(value))
            context.error("layout.id.invalid", "Element id must be non-empty and contain no ASCII whitespace.", input.sourceName, attribute->source.begin.line,
                          attribute->source.begin.column);
        else element.setId(value);
    }
    if (readElementAttribute(input, "class", value)) {
        std::string classes;
        if (const Element::Attribute* existing = element.attribute("class"); existing && existing->value) classes = *existing->value;
        if (!classes.empty() && !value.empty()) classes += ' ';
        classes += value;
        element.setAttribute("class", std::move(classes));
    }
    Visibility visibility = Visibility::Visible;
    if (readElementVisibility(input, visibility, context)) element.setVisibility(visibility);
    bool boolean = false;
    if (readElementBoolean(input, "disabled", boolean, context)) element.disabled(boolean);

    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        if (!readElementAttribute(input, descriptor.attribute, value)) continue;
        const ElementAttribute* attribute = input.find(descriptor.attribute);

        EventCallParseResult parsed = parseEventCall(value);
        if (!parsed.ok()) {
            context.warning(eventCallParseErrorCode(parsed.error), eventCallParseErrorMessage(parsed.error), input.sourceName,
                            attribute->source.begin.line, attribute->source.begin.column + parsed.errorOffset);
            continue;
        }
        const std::string& handlerName = parsed.call->name();
        if (handlerName == "postBuild" || handlerName == "onOpen" || handlerName == "onClose") {
            context.warning("layout.event.handler_reserved", "Controller lifecycle name cannot be used as an Event Handler: " + handlerName + ".",
                            input.sourceName, attribute->source.begin.line, attribute->source.begin.column);
            continue;
        }
        setAuthoredEventCall(element, descriptor.type, std::move(*parsed.call));
    }
}

const ResourceElementDefinition* findElementDefinition(HTMLTag tag) {
    using detail::ElementDefinitions;

    using Definitions = std::unordered_map<HTMLTag, ResourceElementDefinition>;
    static const Definitions definitions = [] {
        Definitions result;
        const auto add = [&result](HTMLTag tag, ResourceElementDefinition definition) { result.emplace(tag, std::move(definition)); };
        add(HTMLTag::Button, ElementDefinitions::button());
        add(HTMLTag::Fieldset, ElementDefinitions::fieldset());
        add(HTMLTag::Floater, ElementDefinitions::floater());
        add(HTMLTag::Icon, ElementDefinitions::icon());
        add(HTMLTag::Input, ElementDefinitions::input());
        add(HTMLTag::Label, ElementDefinitions::label());
        add(HTMLTag::Legend, ElementDefinitions::legend());
        add(HTMLTag::Minimize, ElementDefinitions::minimize());
        add(HTMLTag::Close, ElementDefinitions::close());
        add(HTMLTag::Panel, ElementDefinitions::panel());
        const HTMLTag registeredTags[] = {
            HTMLTag::Abbr,  HTMLTag::B,        HTMLTag::Button, HTMLTag::Br,       HTMLTag::Cite,      HTMLTag::Code,   HTMLTag::Dfn,
            HTMLTag::Del,   HTMLTag::Div,      HTMLTag::Em,     HTMLTag::Fieldset, HTMLTag::Floater,   HTMLTag::Head,   HTMLTag::Header,
            HTMLTag::I,     HTMLTag::Icon,     HTMLTag::Ins,    HTMLTag::Kbd,      HTMLTag::Label,     HTMLTag::Legend, HTMLTag::Link,
            HTMLTag::Mark,  HTMLTag::Minimize, HTMLTag::Close,  HTMLTag::Panel,    HTMLTag::Paragraph, HTMLTag::Q,      HTMLTag::S,
            HTMLTag::Small, HTMLTag::Strong,   HTMLTag::Title,  HTMLTag::U,        HTMLTag::Input,     HTMLTag::Body,
        };
        for (const HTMLTag tag : registeredTags)
            if (!result.contains(tag)) add(tag, htmlContentDefinition(tag));
        return result;
    }();
    const auto found = definitions.find(tag);
    return found == definitions.end() ? nullptr : &found->second;
}

ElementSelectorMetadata inspectElementSelector(HTMLTag tag, std::string_view pseudoElement, std::optional<ElementState> elementState) {
    ElementSelectorMetadata result;
    const ResourceElementDefinition* owner = findElementDefinition(tag);
    if (!owner) return result;

    result.elementName = owner->elementName;
    result.known = true;
    result.pseudoElementKnown = pseudoElement.empty()
        || std::find(owner->pseudoElementNames.begin(), owner->pseudoElementNames.end(), pseudoElement) != owner->pseudoElementNames.end();
    result.elementProducesState = elementState && producesState(*owner, *elementState);
    return result;
}
} // namespace radia::ui
