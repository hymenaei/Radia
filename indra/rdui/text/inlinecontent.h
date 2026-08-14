/**
 * @file inlinecontent.h
 * @brief Defines immutable inline-content values and semantic text nodes.
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

#ifndef RD_TEXT_INLINECONTENT_H
#define RD_TEXT_INLINECONTENT_H

#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace rdui {
struct KeybindingPresentation {
    std::vector<std::string> keys;

    bool operator==(const KeybindingPresentation& other) const { return keys == other.keys; }
};

enum class InlineContentKind : uint8_t {
#define INLINE_CONTENT_ENTRY(name, element, authored) name,
#include "text/inlinecontent.def"
#undef INLINE_CONTENT_ENTRY
};

class InlineContentNode {
public:
    static InlineContentNode text(std::string value);
    static InlineContentNode container(InlineContentKind kind, std::vector<InlineContentNode> children);
    static InlineContentNode kbd(std::string shortcutId, KeybindingPresentation presentation = {});
    static InlineContentNode br();
    static InlineContentNode link(std::string destination, std::vector<InlineContentNode> children);

    InlineContentKind kind() const { return mKind; }
    const std::string& value() const { return mValue; }
    const std::string& shortcutId() const { return mShortcutId; }
    const std::string& destination() const { return mDestination; }
    const KeybindingPresentation& keybindingPresentation() const { return mKeybindingPresentation; }
    const std::vector<InlineContentNode>& children() const { return mChildren; }

private:
    explicit InlineContentNode(InlineContentKind kind) : mKind(kind) {}

    InlineContentKind mKind;
    std::string mValue;
    std::string mShortcutId;
    std::string mDestination;
    KeybindingPresentation mKeybindingPresentation;
    std::vector<InlineContentNode> mChildren;
};

class InlineContent {
public:
    InlineContent() = default;
    explicit InlineContent(std::vector<InlineContentNode> nodes) : mNodes(std::move(nodes)) {}

    static InlineContent text(std::string value);

    const std::vector<InlineContentNode>& nodes() const { return mNodes; }
    bool empty() const { return mNodes.empty(); }
    std::string plainText() const;
    InlineContent resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve) const;

private:
    std::vector<InlineContentNode> mNodes;
};

const char* inlineContentElement(InlineContentKind kind);
bool tryGetInlineContentKind(const std::string& element, InlineContentKind& kind);
bool isInlineStyleElement(const std::string& element);
} // namespace rdui
#endif // RD_TEXT_INLINECONTENT_H
