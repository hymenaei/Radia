/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
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

enum class Tag : uint8_t {
    Unknown,
    Abbr,
    B,
    Button,
    Br,
    Cite,
    Code,
    Dfn,
    Del,
    Div,
    Em,
    Fieldset,
    Floater,
    Head,
    Header,
    I,
    Icon,
    Ins,
    Kbd,
    Label,
    Legend,
    Link,
    Mark,
    Minimize,
    Close,
    Panel,
    Paragraph,
    Q,
    S,
    Small,
    Strong,
    Title,
    U,
    Input,
    Body
};

const char* sourceTagName(Tag tag);
Tag sourceTagFromName(std::string_view name);
bool isGenericElementTag(Tag tag);
bool isLocalizedInlineTag(Tag tag);
const std::vector<Tag>& genericElementTags();
const std::vector<Tag>& localizedInlineTags();

struct SourceAttribute {
    std::string authoredName;
    std::string value;
    SourceRange source;
};

using SourceAttributeMap = std::unordered_map<std::string, SourceAttribute>;

struct SourceNode;

struct SourceContent {
    SourceRange source;
    std::string text;
    std::unique_ptr<SourceNode> node;

    bool isText() const { return !node; }
};

struct SourceNode {
    Tag tag = Tag::Unknown;
    std::string authoredName;
    SourceRange source;
    SourceAttributeMap attributes;
    std::vector<SourceContent> content;
};

struct SourceDocument {
    std::string source;
    std::unique_ptr<SourceNode> root;
};

using SourceDocumentMap = std::unordered_map<std::string, std::shared_ptr<const SourceDocument>>;

struct SourceDocumentParseResult : DiagnosticResult {
    std::unique_ptr<SourceDocument> document;
    bool ok() const { return !hasErrors() && document && document->root; }
};

class SourceDocumentParser final {
public:
    SourceDocumentParseResult parse(const std::string& html, const std::string& source = {}) const;
};
} // namespace radia::ui
