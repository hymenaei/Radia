/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "resource/elementdefinition.h"
#include <algorithm>
#include <cctype>
#include <unordered_map>
#include "event/eventcall.h"
#include "html/element.h"

namespace radia::ui {
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
            result.attributeBehavior.apply = [](const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
                const ElementAttribute* shortcut = input.find("shortcut");
                if (!shortcut) {
                    context.error("layout.kbd.shortcut_required", "<kbd> requires a shortcut attribute.", input.sourceName, input.source.begin.line,
                                  input.source.begin.column);
                } else if (!shortcut->hasValue || shortcut->value.empty() || containsHTMLWhitespace(shortcut->value)) {
                    context.error("layout.kbd.shortcut_invalid", "<kbd> shortcut must be non-empty and contain no ASCII whitespace.",
                                  input.sourceName, shortcut->source.begin.line, shortcut->source.begin.column);
                } else {
                    static_cast<HTMLElement&>(element).setKeybinding(shortcut->value);
                }
            };
            break;
        default: break;
    }
    return result;
}

bool isKnownHTMLAttribute(HTMLTag tag, std::string_view name) {
    const std::string canonicalName = canonicalizeHTMLName(name);
    if (tag == HTMLTag::Br) return false;
    if (canonicalName == "id" || canonicalName == "class" || canonicalName == "disabled") return true;
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors)
        if (canonicalizeHTMLName(descriptor.attribute) == canonicalName) return true;

    const ResourceElementDefinition* definition = findElementDefinition(tag);
    if (!definition) return false;
    return std::any_of(definition->attributes.begin(), definition->attributes.end(),
                       [&](const std::string& attribute) { return canonicalizeHTMLName(attribute) == canonicalName; });
}
} // namespace

bool producesState(const ResourceElementDefinition& definition, ElementState state) {
    return std::find(definition.producedStates.begin(), definition.producedStates.end(), state) != definition.producedStates.end();
}

bool isRegisteredHTMLAttribute(HTMLTag tag, std::string_view name) {
    return isKnownHTMLAttribute(tag, name);
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

void validateElementAttributes(const ElementBuildInput& input, ElementBuildContext& context) {
    const std::string elementName = input.authoredName.empty() ? std::string(htmlTagName(input.tag)) : input.authoredName;
    for (const auto& [attributeName, attribute] : input.attributes)
        if (!isRegisteredHTMLAttribute(input.tag, attributeName))
            context.error("layout.attribute.unknown", "Unknown attribute on <" + elementName + ">: " + attribute.authoredName + ".", input.sourceName,
                          attribute.source.begin.line, attribute.source.begin.column);
}

void applyElementDefinitionAttributes(const ResourceElementDefinition& definition, const ElementBuildInput& input, Element& element,
                                      ElementBuildContext& context) {
    if (definition.attributeBehavior.apply) definition.attributeBehavior.apply(input, element, context);
}

void applyCommonElementAttributes(const ElementBuildInput& input, Element& element, ElementBuildContext& context) {
    std::string value;
    if (readElementAttribute(input, "id", value)) {
        const ElementAttribute* attribute = input.find("id");
        if (value.empty() || containsHTMLWhitespace(value))
            context.error("layout.id.invalid", "Element id must be non-empty and contain no ASCII whitespace.", input.sourceName,
                          attribute->source.begin.line, attribute->source.begin.column);
        else element.setId(value);
    }
    if (readElementAttribute(input, "class", value)) {
        std::string classes;
        if (const Element::Attribute* existing = element.attribute("class"); existing && existing->value) classes = *existing->value;
        if (!classes.empty() && !value.empty()) classes += ' ';
        classes += value;
        element.setAttribute("class", std::move(classes));
    }
    bool boolean = false;
    if (readElementBoolean(input, "disabled", boolean, context)) element.disabled(boolean);

    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        if (!readElementAttribute(input, descriptor.attribute, value)) continue;
        const ElementAttribute* attribute = input.find(descriptor.attribute);

        AuthoredEventCallParseResult parsed = parseAuthoredEventCall(value);
        if (!parsed.ok()) {
            context.warning(authoredEventCallParseErrorCode(parsed.error), authoredEventCallParseErrorMessage(parsed.error), input.sourceName,
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
        add(HTMLTag::Input, ElementDefinitions::input());
        add(HTMLTag::Label, ElementDefinitions::label());
        add(HTMLTag::Legend, ElementDefinitions::legend());
        add(HTMLTag::Minimize, ElementDefinitions::minimize());
        add(HTMLTag::Close, ElementDefinitions::close());
        add(HTMLTag::Panel, ElementDefinitions::panel());
        const HTMLTag registeredTags[] = {
            HTMLTag::Abbr,     HTMLTag::B,     HTMLTag::Button, HTMLTag::Br,        HTMLTag::Cite,    HTMLTag::Code, HTMLTag::Dfn,
            HTMLTag::Del,      HTMLTag::Div,   HTMLTag::Em,     HTMLTag::Fieldset,  HTMLTag::Floater, HTMLTag::Head, HTMLTag::Header,
            HTMLTag::I,        HTMLTag::Ins,   HTMLTag::Kbd,    HTMLTag::Label,     HTMLTag::Legend,  HTMLTag::Link, HTMLTag::Mark,
            HTMLTag::Minimize, HTMLTag::Close, HTMLTag::Panel,  HTMLTag::Paragraph, HTMLTag::Q,       HTMLTag::S,    HTMLTag::Small,
            HTMLTag::Strong,   HTMLTag::Title, HTMLTag::U,      HTMLTag::Input,     HTMLTag::Body,
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
