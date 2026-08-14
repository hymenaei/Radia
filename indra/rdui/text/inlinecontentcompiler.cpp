/**
 * @file inlinecontentcompiler.cpp
 * @brief Compiles authored Layout Resource content into typed inline text sources.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "text/inlinecontentcompiler.h"
#include <algorithm>
#include <cctype>
#include <functional>
#include <iterator>
#include "layout/schema.h"
#include "widgets/widgetcontract.h"

namespace rdui {
namespace {
std::string trimmedText(const std::string& text) {
    const auto first = std::find_if_not(text.begin(), text.end(), [](unsigned char character) { return std::isspace(character); });
    if (first == text.end()) return {};
    const auto last = std::find_if_not(text.rbegin(), text.rend(), [](unsigned char character) { return std::isspace(character); }).base();
    return std::string(first, last);
}

bool hasAuthoredContent(const LayoutNode& node) {
    for (const LayoutContent& content : node.content)
        if (content.node || !trimmedText(content.text).empty()) return true;
    return false;
}
} // namespace

TextSource compileInlineContent(const std::vector<LayoutContent>& contentItems, const std::string& hostElementName,
                                const std::vector<InlineContentKind>& accepted, LayoutBuildResult& result, const std::string& source,
                                const LayoutBuildContext* context) {
    bool hasEmittedContent = false;
    bool hasPendingSpace = false;
    const auto inlineText = [&](const std::string& authored, std::size_t line) {
        std::vector<TextSourceNode> nodes;
        if (context) {
            const std::string key = trimmedText(authored);
            const bool hasLeadingSpace = !authored.empty() && std::isspace(static_cast<unsigned char>(authored.front()));
            const bool hasTrailingSpace = !authored.empty() && std::isspace(static_cast<unsigned char>(authored.back()));
            if (key.empty()) {
                if (hasEmittedContent && (hasLeadingSpace || hasTrailingSpace)) hasPendingSpace = true;
                return nodes;
            }
            if (hasEmittedContent && (hasPendingSpace || hasLeadingSpace)) nodes.push_back(TextSourceNode::text(" "));
            if (!context->hasLocalizationKey(key))
                result.error("layout.localization.missing", "Unknown localization key: " + key + ".", source, line);
            TextSource localized = context->localizeContent(key);
            nodes.insert(nodes.end(), localized.nodes().begin(), localized.nodes().end());
            hasEmittedContent = true;
            hasPendingSpace = hasTrailingSpace;
            return nodes;
        }

        std::string value;
        for (unsigned char character : authored) {
            if (std::isspace(character)) {
                if (hasEmittedContent) hasPendingSpace = true;
                continue;
            }
            if (hasPendingSpace) {
                value += ' ';
                hasPendingSpace = false;
            }
            value += static_cast<char>(character);
            hasEmittedContent = true;
        }
        if (!value.empty()) nodes.push_back(TextSourceNode::text(std::move(value)));
        return nodes;
    };
    const auto accepts = [&accepted](InlineContentKind kind) { return std::find(accepted.begin(), accepted.end(), kind) != accepted.end(); };
    std::function<std::vector<TextSourceNode>(const std::vector<LayoutContent>&)> buildInline;
    buildInline = [&](const std::vector<LayoutContent>& content) {
        std::vector<TextSourceNode> nodes;
        for (const LayoutContent& item : content) {
            if (item.isText()) {
                std::vector<TextSourceNode> textNodes = inlineText(item.text, item.source.begin.line);
                nodes.insert(nodes.end(), std::make_move_iterator(textNodes.begin()), std::make_move_iterator(textNodes.end()));
                continue;
            }

            const LayoutNode& inlineNode = *item.node;
            InlineContentKind kind = InlineContentKind::Text;
            if (!tryGetInlineContentKind(inlineNode.name, kind)) {
                result.error("layout.inline.element_unknown", "Unsupported inline element in <" + hostElementName + ">: " + inlineNode.name + ".",
                             source, inlineNode.source.begin.line, inlineNode.source.begin.column);
                continue;
            }
            if (!accepts(kind)) {
                result.error("layout.inline.unsupported",
                             "Inline <" + std::string(inlineContentElement(kind)) + "> is not supported in <" + hostElementName + ">.", source,
                             inlineNode.source.begin.line, inlineNode.source.begin.column);
                continue;
            }
            if (kind == InlineContentKind::Link) {
                result.error("layout.inline.not_implemented", "Inline <" + std::string(inlineContentElement(kind)) + "> is not implemented yet.",
                             source, inlineNode.source.begin.line, inlineNode.source.begin.column);
                continue;
            }
            if (kind == InlineContentKind::Kbd) {
                const auto shortcut = inlineNode.attributes.find("shortcut");
                for (const auto& [name, attribute] : inlineNode.attributes)
                    if (name != "shortcut")
                        result.error("layout.inline.attribute_unknown", "Unknown attribute on inline <kbd>: " + attribute.authoredName + ".", source,
                                     attribute.source.begin.line, attribute.source.begin.column);
                if (shortcut == inlineNode.attributes.end())
                    result.error("layout.inline.kbd.shortcut_required", "Inline <kbd> requires a shortcut attribute.", source,
                                 inlineNode.source.begin.line, inlineNode.source.begin.column);
                else if (!isKebabCaseIdentifier(shortcut->second.value))
                    result.error("layout.inline.kbd.shortcut_invalid", "Inline <kbd> shortcut must be a lowercase kebab-case command id.", source,
                                 shortcut->second.source.begin.line, shortcut->second.source.begin.column);
                if (hasAuthoredContent(inlineNode))
                    result.error("layout.inline.children_unsupported", "Inline <kbd> cannot contain authored content.", source,
                                 inlineNode.source.begin.line, inlineNode.source.begin.column);
                if (shortcut != inlineNode.attributes.end() && isKebabCaseIdentifier(shortcut->second.value)) {
                    if (hasEmittedContent && hasPendingSpace) nodes.push_back(TextSourceNode::text(" "));
                    nodes.push_back(TextSourceNode::kbd(shortcut->second.value));
                    hasEmittedContent = true;
                    hasPendingSpace = false;
                }
                continue;
            }
            for (const auto& [name, attribute] : inlineNode.attributes)
                result.error("layout.inline.attribute_unknown",
                             "Unknown attribute on inline <" + std::string(inlineContentElement(kind)) + ">: " + attribute.authoredName + ".", source,
                             attribute.source.begin.line, attribute.source.begin.column);

            if (kind == InlineContentKind::Br) {
                if (hasAuthoredContent(inlineNode))
                    result.error("layout.inline.children_unsupported", "Inline <br> cannot contain content.", source, inlineNode.source.begin.line,
                                 inlineNode.source.begin.column);
                nodes.push_back(TextSourceNode::br());
                hasEmittedContent = false;
                hasPendingSpace = false;
            } else {
                if (hasEmittedContent && hasPendingSpace) {
                    nodes.push_back(TextSourceNode::text(" "));
                    hasPendingSpace = false;
                }
                nodes.push_back(TextSourceNode::container(kind, buildInline(inlineNode.content)));
            }
        }
        return nodes;
    };
    return TextSource(buildInline(contentItems));
}
} // namespace rdui
