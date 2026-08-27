/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "text/inlineelements.h"
#include <algorithm>
#include <cctype>
#include <iterator>
#include "elements/element.h"
#include "elements/elementdefinition.h"
#include "elements/elementinternal.h"
#include "layout/schema.h"

namespace radia::ui {
namespace {
bool hasAuthoredContent(const SourceNode& node) {
    for (const SourceContent& content : node.content) {
        if (content.node) return true;
        if (std::any_of(content.text.begin(), content.text.end(), [](unsigned char character) { return !std::isspace(character); })) return true;
    }
    return false;
}

struct AppendState {
    Element& target;
    const std::string& hostElementName;
    const std::vector<Tag>& acceptedElements;
    LayoutBuildResult& result;
    const std::string& source;
    const LayoutBuildContext* context;
    bool hasEmittedContent = false;
    bool hasPendingSpace = false;
};

bool accepts(const std::vector<Tag>& acceptedElements, Tag tag) {
    return std::find(acceptedElements.begin(), acceptedElements.end(), tag) != acceptedElements.end();
}

void addFinding(InlineValidationResult& result, InlineValidationKind kind, const SourceNode& node, const SourceLocation& location = {}) {
    result.findings.push_back({kind, node.tag, node.source, {location, location}, node.authoredName, {}});
}

void validateNode(const SourceNode& node, const std::vector<Tag>& acceptedElements, InlineValidationResult& result) {
    if (!accepts(acceptedElements, node.tag)) {
        addFinding(result, InlineValidationKind::UnsupportedElement, node, node.source.begin);
        return;
    }

    if (node.tag == Tag::Link) {
        addFinding(result, InlineValidationKind::NotImplemented, node, node.source.begin);
        return;
    }

    if (node.tag == Tag::Kbd) {
        const auto shortcut = node.attributes.find("shortcut");
        if (shortcut == node.attributes.end()) addFinding(result, InlineValidationKind::KbdShortcutRequired, node, node.source.begin);
        else if (!isElementIdentifier(shortcut->second.value))
            addFinding(result, InlineValidationKind::KbdShortcutInvalid, node, shortcut->second.source.begin);

        for (const auto& [name, attribute] : node.attributes)
            if (name != "shortcut")
                result.findings.push_back(
                    {InlineValidationKind::AttributeUnknown, node.tag, node.source, attribute.source, node.authoredName, attribute.authoredName});
    } else {
        for (const auto& [name, attribute] : node.attributes)
            result.findings.push_back(
                {InlineValidationKind::AttributeUnknown, node.tag, node.source, attribute.source, node.authoredName, attribute.authoredName});
    }

    if (node.tag == Tag::Br || node.tag == Tag::Kbd) {
        if (hasAuthoredContent(node)) addFinding(result, InlineValidationKind::ChildrenUnsupported, node, node.source.begin);
        return;
    }

    for (const SourceContent& content : node.content)
        if (content.node) validateNode(*content.node, acceptedElements, result);
}

void appendValidationDiagnostics(const InlineValidationResult& validation, LayoutBuildResult& result, const std::string& source,
                                 const std::string& hostElementName) {
    for (const InlineValidationFinding& finding : validation.findings) {
        const SourceLocation& elementLocation = finding.elementSource.begin;
        const SourceLocation& location = finding.source.begin;
        switch (finding.kind) {
            case InlineValidationKind::UnsupportedElement:
                result.error("layout.inline.unsupported", "Inline <" + finding.elementName + "> is not supported in <" + hostElementName + ">.",
                             source, elementLocation.line, elementLocation.column);
                break;
            case InlineValidationKind::NotImplemented:
                result.error("layout.inline.not_implemented", "Inline <link> is not implemented yet.", source, elementLocation.line,
                             elementLocation.column);
                break;
            case InlineValidationKind::AttributeUnknown:
                result.error("layout.inline.attribute_unknown",
                             "Unknown attribute on inline <" + finding.elementName + ">: " + finding.attributeName + ".", source, location.line,
                             location.column);
                break;
            case InlineValidationKind::KbdShortcutRequired:
                result.error("layout.inline.kbd.shortcut_required", "Inline <kbd> requires a shortcut attribute.", source, elementLocation.line,
                             elementLocation.column);
                break;
            case InlineValidationKind::KbdShortcutInvalid:
                result.error("layout.inline.kbd.shortcut_invalid", "Inline <kbd> shortcut must be a valid identifier.", source, location.line,
                             location.column);
                break;
            case InlineValidationKind::ChildrenUnsupported:
                result.error("layout.inline.children_unsupported", "Inline <" + finding.elementName + "> cannot contain authored content.", source,
                             elementLocation.line, elementLocation.column);
                break;
        }
    }
}

void appendLiteral(AppendState& state, const std::string& authored) {
    for (const unsigned char character : authored) {
        if (std::isspace(character)) {
            if (state.hasEmittedContent) state.hasPendingSpace = true;
            continue;
        }
        if (state.hasPendingSpace) {
            detail::appendText(state.target, " ");
            state.hasPendingSpace = false;
        }
        detail::appendText(state.target, std::string(1, static_cast<char>(character)));
        state.hasEmittedContent = true;
    }
}

void appendText(AppendState& state, const std::string& authored, std::size_t line) {
    const ResolvedLayoutText resolved = localizedLayoutText(authored, state.result, state.source, state.context, line);
    if (!resolved.text) {
        appendLiteral(state, resolved.literal);
        return;
    }

    appendLiteral(state, resolved.prefix);
    if (state.hasPendingSpace) {
        detail::appendText(state.target, " ");
        state.hasPendingSpace = false;
    }
    detail::appendLocalizedText(state.target, *resolved.text, state.context->resolveMarkup(*resolved.text));
    state.hasEmittedContent = true;
    appendLiteral(state, resolved.suffix);
}

void appendChildren(AppendState& state, const std::vector<SourceContent>& contentItems);

void appendElement(AppendState& state, const SourceNode& node) {
    const std::string elementName = sourceTagName(node.tag);
    if (!accepts(state.acceptedElements, node.tag)) return;

    if (node.tag == Tag::Link) return;

    if (node.tag == Tag::Kbd) {
        const auto shortcut = node.attributes.find("shortcut");
        if (shortcut == node.attributes.end() || !isElementIdentifier(shortcut->second.value)) return;

        if (state.hasPendingSpace) {
            detail::appendText(state.target, " ");
            state.hasPendingSpace = false;
        }
        auto element = std::make_unique<Element>("kbd");
        detail::ElementCompilerAccess::setKeybinding(*element, shortcut->second.value);
        state.target.append(std::move(element));
        state.hasEmittedContent = true;
        return;
    }

    if (node.tag == Tag::Br) {
        auto element = std::make_unique<Element>("br");
        Element* added = element.get();
        state.target.append(std::move(element));
        detail::NodeAccess::setFlowBreakBefore(*added, false);
        state.hasEmittedContent = false;
        state.hasPendingSpace = false;
        return;
    }

    if (state.hasPendingSpace) {
        detail::appendText(state.target, " ");
        state.hasPendingSpace = false;
    }
    auto element = std::make_unique<Element>(elementName.c_str());
    Element* added = element.get();
    AppendState nested{*element, state.hostElementName, state.acceptedElements, state.result, state.source, state.context};
    appendChildren(nested, node.content);
    state.target.append(std::move(element));
    detail::NodeAccess::setFlowBreakBefore(*added, false);
    state.hasEmittedContent = true;
}

void appendChildren(AppendState& state, const std::vector<SourceContent>& contentItems) {
    bool pendingFlowBreak = false;
    for (const SourceContent& item : contentItems) {
        if (item.isText()) {
            const std::size_t before = detail::nodes(state.target).size();
            appendText(state, item.text, item.source.begin.line);
            if (pendingFlowBreak && detail::nodes(state.target).size() > before) {
                Node& child = *std::next(detail::nodes(state.target).begin(), static_cast<std::ptrdiff_t>(before));
                detail::NodeAccess::setFlowBreakBefore(child, true);
                pendingFlowBreak = false;
            }
            continue;
        }

        const std::size_t before = detail::nodes(state.target).size();
        appendElement(state, *item.node);
        if (item.node->tag == Tag::Br) {
            pendingFlowBreak = true;
            continue;
        }
        if (detail::nodes(state.target).size() > before) {
            Node& child = *std::next(detail::nodes(state.target).begin(), static_cast<std::ptrdiff_t>(before));
            detail::NodeAccess::setFlowBreakBefore(child, pendingFlowBreak);
            pendingFlowBreak = false;
        }
    }
}
} // namespace

InlineValidationResult validateInlineContent(const std::vector<SourceContent>& contentItems, const std::vector<Tag>& acceptedElements) {
    InlineValidationResult result;
    for (const SourceContent& content : contentItems)
        if (content.node) validateNode(*content.node, acceptedElements, result);
    return result;
}

void appendInlineElements(Element& target, const std::vector<SourceContent>& contentItems, const std::string& hostElementName,
                          const std::vector<Tag>& acceptedElements, LayoutBuildResult& result, const std::string& source,
                          const LayoutBuildContext* context) {
    appendValidationDiagnostics(validateInlineContent(contentItems, acceptedElements), result, source, hostElementName);
    AppendState state{target, hostElementName, acceptedElements, result, source, context};
    appendChildren(state, contentItems);
}
} // namespace radia::ui
