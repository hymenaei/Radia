/**
 * @file rduiinlinecontent.cpp
 * @brief
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
#include "rduiinlinecontent.h"
#include "rduischema.h"

namespace rdui {
InlineContentNode InlineContentNode::text(std::string value) {
    InlineContentNode result(InlineContentKind::Text);
    result.mValue = std::move(value);
    return result;
}

InlineContentNode InlineContentNode::container(InlineContentKind kind, std::vector<InlineContentNode> children) {
    llassert_always(kind == InlineContentKind::B || kind == InlineContentKind::I || kind == InlineContentKind::S);
    InlineContentNode result(kind);
    result.mChildren = std::move(children);
    return result;
}

InlineContentNode InlineContentNode::kbd(std::string binding, KeybindingPresentation presentation) {
    InlineContentNode result(InlineContentKind::Kbd);
    result.mMetadata = std::move(binding);
    result.mKeybindingPresentation = std::move(presentation);
    return result;
}

InlineContentNode InlineContentNode::br() {
    return InlineContentNode(InlineContentKind::Br);
}

InlineContentNode InlineContentNode::link(std::string destination, std::vector<InlineContentNode> children) {
    InlineContentNode result(InlineContentKind::Link);
    result.mMetadata = std::move(destination);
    result.mChildren = std::move(children);
    return result;
}

InlineContent InlineContent::text(std::string value) {
    return InlineContent({InlineContentNode::text(std::move(value))});
}

namespace {
void appendPlainText(const InlineContentNode& node, std::string& result) {
    switch (node.kind()) {
        case InlineContentKind::Text: result += node.value(); return;
        case InlineContentKind::Kbd: return;
        case InlineContentKind::Br: result += '\n'; return;
        case InlineContentKind::B:
        case InlineContentKind::I:
        case InlineContentKind::S:
        case InlineContentKind::Link:
            for (const InlineContentNode& child : node.children()) appendPlainText(child, result);
            return;
    }
}
} // namespace

std::string InlineContent::plainText() const {
    std::string result;
    for (const InlineContentNode& node : mNodes) appendPlainText(node, result);
    return result;
}

InlineContent InlineContent::resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve) const {
    const auto resolve_node = [&](auto&& self, const InlineContentNode& node) -> InlineContentNode {
        std::vector<InlineContentNode> children;
        children.reserve(node.children().size());
        for (const InlineContentNode& child : node.children()) children.push_back(self(self, child));

        switch (node.kind()) {
            case InlineContentKind::Text: return InlineContentNode::text(node.value());
            case InlineContentKind::B:
            case InlineContentKind::I:
            case InlineContentKind::S: return InlineContentNode::container(node.kind(), std::move(children));
            case InlineContentKind::Kbd: return InlineContentNode::kbd(node.metadata(), resolve(node.metadata()));
            case InlineContentKind::Br: return InlineContentNode::br();
            case InlineContentKind::Link: return InlineContentNode::link(node.metadata(), std::move(children));
        }
        llassert(false);
        return InlineContentNode::br();
    };

    std::vector<InlineContentNode> nodes;
    nodes.reserve(mNodes.size());
    for (const InlineContentNode& node : mNodes) nodes.push_back(resolve_node(resolve_node, node));
    return InlineContent(std::move(nodes));
}

const char* inlineContentElement(InlineContentKind kind) {
    switch (kind) {
        case InlineContentKind::Text: return "";
        case InlineContentKind::B: return "b";
        case InlineContentKind::I: return "i";
        case InlineContentKind::S: return "s";
        case InlineContentKind::Kbd: return "kbd";
        case InlineContentKind::Br: return "br";
        case InlineContentKind::Link: return "link";
    }
    return "";
}

bool inlineContentKind(const std::string& element, InlineContentKind& kind) {
    const std::string lookup = schemaNameKey(element);
    for (InlineContentKind candidate : {
             InlineContentKind::B,
             InlineContentKind::I,
             InlineContentKind::S,
             InlineContentKind::Kbd,
             InlineContentKind::Br,
             InlineContentKind::Link,
         }) {
        if (lookup != schemaNameKey(inlineContentElement(candidate))) continue;
        kind = candidate;
        return true;
    }
    return false;
}

bool isInlineStyleElement(const std::string& element) {
    return schemaNameKey(element) == schemaNameKey(inlineContentElement(InlineContentKind::Kbd));
}
} // namespace rdui
