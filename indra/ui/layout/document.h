/**
 * @file document.h
 * @brief Parses Layout Resource documents into validated node and attribute trees.
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

#ifndef RD_LAYOUT_DOCUMENT_H
#define RD_LAYOUT_DOCUMENT_H

#include <cstddef>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>
#include "diagnostic.h"

namespace radia::ui {
struct SourceLocation {
    std::size_t line = 0;
    std::size_t column = 0;
    std::size_t offset = 0;
};

struct SourceRange {
    SourceLocation begin;
    SourceLocation end;
};

struct LayoutAttribute {
    std::string authoredName;
    std::string value;
    SourceRange source;
};

struct LayoutNode;

struct LayoutContent {
    SourceRange source;
    std::string text;
    std::unique_ptr<LayoutNode> node;

    bool isText() const { return !node; }
};

struct LayoutNode {
    std::string name;
    SourceRange source;
    std::unordered_map<std::string, LayoutAttribute> attributes;
    std::vector<LayoutContent> content;
};

struct LayoutDocument {
    std::string source;
    std::unique_ptr<LayoutNode> root;
};

using LayoutDocumentMap = std::unordered_map<std::string, std::shared_ptr<const LayoutDocument>>;

class LayoutElement final {
public:
    explicit LayoutElement(const LayoutNode& node, const LayoutNode* defaults = nullptr) : mNode(node), mDefaults(defaults) {}

    const std::string& name() const { return mNode.name; }
    const SourceRange& source() const { return mNode.source; }
    const std::unordered_map<std::string, LayoutAttribute>& attributes() const { return mNode.attributes; }
    const std::vector<LayoutContent>& content() const { return mNode.content; }

    const LayoutAttribute* attribute(const std::string& name) const;

private:
    const LayoutNode& mNode;
    const LayoutNode* mDefaults = nullptr;
};

struct LayoutDocumentParseResult : DiagnosticResult {
    std::unique_ptr<LayoutDocument> document;
    bool ok() const { return !hasErrors() && document && document->root; }
};

class LayoutDocumentParser final {
public:
    LayoutDocumentParseResult parse(const std::string& xml, const std::string& source = {}) const;
};
} // namespace radia::ui
#endif // RD_LAYOUT_DOCUMENT_H
