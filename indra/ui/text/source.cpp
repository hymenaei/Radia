/**
 * @file source.cpp
 * @brief Defines lazy text sources and inline-content materialization.
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
#include "text/source.h"
#include <type_traits>

namespace radia::ui {
TextSourceNode TextSourceNode::text(std::string value) {
    return TextSourceNode(Literal{std::move(value)});
}

TextSourceNode TextSourceNode::fromLocalization(LocalizationRequest request, InlineContent fallback) {
    return TextSourceNode(Localized{std::move(request), std::move(fallback)});
}

TextSourceNode TextSourceNode::container(InlineContentKind kind, std::vector<TextSourceNode> children) {
    llassert_always(kind == InlineContentKind::B || kind == InlineContentKind::I || kind == InlineContentKind::S);
    return TextSourceNode(Container{kind, std::move(children)});
}

TextSourceNode TextSourceNode::kbd(std::string shortcutId, KeybindingPresentation presentation) {
    return TextSourceNode(Keybinding{std::move(shortcutId), std::move(presentation)});
}

TextSourceNode TextSourceNode::br() {
    return TextSourceNode(Break{});
}

TextSourceNode TextSourceNode::link(std::string destination, std::vector<TextSourceNode> children) {
    return TextSourceNode(Link{std::move(destination), std::move(children)});
}

namespace {
TextSourceNode sourceNode(const InlineContentNode& node) {
    std::vector<TextSourceNode> children;
    children.reserve(node.children().size());
    for (const InlineContentNode& child : node.children()) children.push_back(sourceNode(child));

    switch (node.kind()) {
        case InlineContentKind::Text: return TextSourceNode::text(node.value());
        case InlineContentKind::B:
        case InlineContentKind::I:
        case InlineContentKind::S: return TextSourceNode::container(node.kind(), std::move(children));
        case InlineContentKind::Kbd: return TextSourceNode::kbd(node.shortcutId(), node.keybindingPresentation());
        case InlineContentKind::Br: return TextSourceNode::br();
        case InlineContentKind::Link: return TextSourceNode::link(node.destination(), std::move(children));
    }
    llassert(false);
    return TextSourceNode::br();
}

std::vector<InlineContentNode> materializeNodes(const std::vector<TextSourceNode>& source,
                                                const std::function<InlineContent(const LocalizationRequest&)>* resolve) {
    std::vector<InlineContentNode> result;
    for (const TextSourceNode& node : source) {
        std::visit(
            [&](const auto& value) {
                using Value = std::decay_t<decltype(value)>;
                if constexpr (std::is_same_v<Value, TextSourceNode::Literal>) {
                    result.push_back(InlineContentNode::text(value.value));
                } else if constexpr (std::is_same_v<Value, TextSourceNode::Localized>) {
                    const InlineContent content = resolve ? (*resolve)(value.request) : value.fallback;
                    result.insert(result.end(), content.nodes().begin(), content.nodes().end());
                } else if constexpr (std::is_same_v<Value, TextSourceNode::Container>) {
                    result.push_back(InlineContentNode::container(value.kind, materializeNodes(value.children, resolve)));
                } else if constexpr (std::is_same_v<Value, TextSourceNode::Keybinding>) {
                    result.push_back(InlineContentNode::kbd(value.shortcutId, value.presentation));
                } else if constexpr (std::is_same_v<Value, TextSourceNode::Break>) {
                    result.push_back(InlineContentNode::br());
                } else if constexpr (std::is_same_v<Value, TextSourceNode::Link>) {
                    result.push_back(InlineContentNode::link(value.destination, materializeNodes(value.children, resolve)));
                }
            },
            node.value());
    }
    return result;
}
} // namespace

TextSource TextSource::literal(InlineContent content) {
    std::vector<TextSourceNode> nodes;
    nodes.reserve(content.nodes().size());
    for (const InlineContentNode& node : content.nodes()) nodes.push_back(sourceNode(node));
    return TextSource(std::move(nodes));
}

TextSource TextSource::text(std::string value) {
    return TextSource({TextSourceNode::text(std::move(value))});
}

TextSource TextSource::fromLocalization(LocalizationRequest request, InlineContent fallback) {
    return TextSource({TextSourceNode::fromLocalization(std::move(request), std::move(fallback))});
}

InlineContent TextSource::materialize() const {
    return InlineContent(materializeNodes(mNodes, nullptr));
}

InlineContent TextSource::materialize(const std::function<InlineContent(const LocalizationRequest&)>& resolve) const {
    return InlineContent(materializeNodes(mNodes, &resolve));
}
} // namespace radia::ui
