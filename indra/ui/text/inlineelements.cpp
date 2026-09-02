/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "text/inlineelements.h"
#include <algorithm>
#include <iterator>
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "html/element.h"
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "resource/elementdefinition.h"

namespace radia::ui {
using detail::appendLocalizedText;
using detail::appendText;
using detail::HTMLElementFactory;
using detail::NodeAccess;
using detail::nodes;

namespace {
bool hasAuthoredContent(const SourceNode& node) {
    for (const SourceContent& content : node.content) {
        if (content.node) return true;
        if (std::any_of(content.text.begin(), content.text.end(), [](char character) { return !isHTMLWhitespace(character); })) return true;
    }
    return false;
}

struct AppendState {
    Element& target;
    const std::string& hostName;
    const std::vector<HTMLTag>& acceptedTags;
    ElementBuildContext& context;
    const std::string& sourceName;
    bool hasEmittedContent = false;
    bool hasPendingSpace = false;
};

bool accepts(const std::vector<HTMLTag>& acceptedTags, HTMLTag tag) {
    return std::find(acceptedTags.begin(), acceptedTags.end(), tag) != acceptedTags.end();
}

void addFinding(InlineValidationResult& result, InlineValidationKind kind, const SourceNode& node, const SourceLocation& location = {}) {
    result.findings.push_back({kind, node.tag, node.source, {location, location}, node.authoredName, {}});
}

void validateNode(const SourceNode& node, const std::vector<HTMLTag>& acceptedTags, InlineValidationResult& result) {
    if (!accepts(acceptedTags, node.tag)) {
        addFinding(result, InlineValidationKind::UnsupportedElement, node, node.source.begin);
        return;
    }

    if (node.tag == HTMLTag::Link) {
        addFinding(result, InlineValidationKind::NotImplemented, node, node.source.begin);
        return;
    }

    if (node.tag == HTMLTag::Kbd) {
        const auto shortcut = node.attributes.find("shortcut");
        if (shortcut == node.attributes.end()) addFinding(result, InlineValidationKind::KbdShortcutRequired, node, node.source.begin);
        else if (shortcut->second.value.empty())
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

    if (node.tag == HTMLTag::Br || node.tag == HTMLTag::Kbd) {
        if (hasAuthoredContent(node)) addFinding(result, InlineValidationKind::ChildrenUnsupported, node, node.source.begin);
        return;
    }

    for (const SourceContent& content : node.content)
        if (content.node) validateNode(*content.node, acceptedTags, result);
}

void appendValidationDiagnostics(const InlineValidationResult& validation, ElementBuildContext& context, const std::string& sourceName,
                                 const std::string& hostName) {
    for (const InlineValidationFinding& finding : validation.findings) {
        const SourceLocation& elementLocation = finding.elementSource.begin;
        const SourceLocation& location = finding.source.begin;
        switch (finding.kind) {
            case InlineValidationKind::UnsupportedElement:
                context.error("layout.inline.unsupported", "Inline <" + finding.elementName + "> is not supported in <" + hostName + ">.", sourceName,
                              elementLocation.line, elementLocation.column);
                break;
            case InlineValidationKind::NotImplemented:
                context.error("layout.inline.not_implemented", "Inline <link> is not implemented yet.", sourceName, elementLocation.line,
                              elementLocation.column);
                break;
            case InlineValidationKind::AttributeUnknown:
                context.error("layout.inline.attribute_unknown",
                              "Unknown attribute on inline <" + finding.elementName + ">: " + finding.attributeName + ".", sourceName, location.line,
                              location.column);
                break;
            case InlineValidationKind::KbdShortcutRequired:
                context.error("layout.inline.kbd.shortcut_required", "Inline <kbd> requires a shortcut attribute.", sourceName, elementLocation.line,
                              elementLocation.column);
                break;
            case InlineValidationKind::KbdShortcutInvalid:
                context.error("layout.inline.kbd.shortcut_invalid", "Inline <kbd> shortcut must be non-empty.", sourceName, location.line,
                              location.column);
                break;
            case InlineValidationKind::ChildrenUnsupported:
                context.error("layout.inline.children_unsupported", "Inline <" + finding.elementName + "> cannot contain authored content.",
                              sourceName, elementLocation.line, elementLocation.column);
                break;
        }
    }
}

void appendLiteral(AppendState& state, const std::string& authored) {
    for (const unsigned char character : authored) {
        if (isHTMLWhitespace(static_cast<char>(character))) {
            if (state.hasEmittedContent) state.hasPendingSpace = true;
            continue;
        }
        if (state.hasPendingSpace) {
            appendText(state.target, " ");
            state.hasPendingSpace = false;
        }
        appendText(state.target, std::string(1, static_cast<char>(character)));
        state.hasEmittedContent = true;
    }
}

void appendText(AppendState& state, const std::string& authored, std::size_t line) {
    const ResolvedLayoutText resolved = localizedLayoutText(authored, state.context, state.sourceName, line);
    if (!resolved.text) {
        appendLiteral(state, resolved.literal);
        return;
    }

    appendLiteral(state, resolved.prefix);
    if (state.hasPendingSpace) {
        appendText(state.target, " ");
        state.hasPendingSpace = false;
    }
    appendLocalizedText(state.target, *resolved.text, state.context.resolveHTML(*resolved.text));
    state.hasEmittedContent = true;
    appendLiteral(state, resolved.suffix);
}

void appendChildren(AppendState& state, const std::vector<SourceContent>& contentItems);

void appendElement(AppendState& state, const SourceNode& node) {
    const std::string_view elementName = htmlTagName(node.tag);
    if (!accepts(state.acceptedTags, node.tag)) return;

    if (node.tag == HTMLTag::Link) return;

    if (node.tag == HTMLTag::Kbd) {
        const auto shortcut = node.attributes.find("shortcut");
        if (shortcut == node.attributes.end() || shortcut->second.value.empty()) return;

        if (state.hasPendingSpace) {
            appendText(state.target, " ");
            state.hasPendingSpace = false;
        }
        auto element = HTMLElementFactory::Create(kKbdTag.localName);
        static_cast<HTMLElement&>(*element).setKeybinding(shortcut->second.value);
        state.target.append(std::move(element));
        state.hasEmittedContent = true;
        return;
    }

    if (node.tag == HTMLTag::Br) {
        auto element = HTMLElementFactory::Create(kBrTag.localName);
        Element* added = element.get();
        state.target.append(std::move(element));
        NodeAccess::setFlowBreakBefore(*added, false);
        state.hasEmittedContent = false;
        state.hasPendingSpace = false;
        return;
    }

    if (state.hasPendingSpace) {
        appendText(state.target, " ");
        state.hasPendingSpace = false;
    }
    auto element = HTMLElementFactory::Create(elementName);
    Element* added = element.get();
    AppendState nested{*element, state.hostName, state.acceptedTags, state.context, state.sourceName};
    appendChildren(nested, node.content);
    state.target.append(std::move(element));
    NodeAccess::setFlowBreakBefore(*added, false);
    state.hasEmittedContent = true;
}

void appendChildren(AppendState& state, const std::vector<SourceContent>& contentItems) {
    bool pendingFlowBreak = false;
    for (const SourceContent& item : contentItems) {
        if (item.isText()) {
            const std::size_t before = nodes(state.target).size();
            appendText(state, item.text, item.source.begin.line);
            if (pendingFlowBreak && nodes(state.target).size() > before) {
                Node& child = *std::next(nodes(state.target).begin(), static_cast<std::ptrdiff_t>(before));
                NodeAccess::setFlowBreakBefore(child, true);
                pendingFlowBreak = false;
            }
            continue;
        }

        const std::size_t before = nodes(state.target).size();
        appendElement(state, *item.node);
        if (item.node->tag == HTMLTag::Br) {
            pendingFlowBreak = true;
            continue;
        }
        if (nodes(state.target).size() > before) {
            Node& child = *std::next(nodes(state.target).begin(), static_cast<std::ptrdiff_t>(before));
            NodeAccess::setFlowBreakBefore(child, pendingFlowBreak);
            pendingFlowBreak = false;
        }
    }
}
} // namespace

InlineValidationResult validateInlineContent(const std::vector<SourceContent>& contentItems, const std::vector<HTMLTag>& acceptedTags) {
    InlineValidationResult result;
    for (const SourceContent& content : contentItems)
        if (content.node) validateNode(*content.node, acceptedTags, result);
    return result;
}

void appendInlineElements(Element& target, const std::vector<SourceContent>& contentItems, const std::string& hostName,
                          const std::vector<HTMLTag>& acceptedTags, ElementBuildContext& context, const std::string& sourceName) {
    appendValidationDiagnostics(validateInlineContent(contentItems, acceptedTags), context, sourceName, hostName);
    AppendState state{target, hostName, acceptedTags, context, sourceName};
    appendChildren(state, contentItems);
}
} // namespace radia::ui
