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
#include "html/elementnames.h"

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

struct SourceAttribute {
    std::string authoredName;
    std::string value;
    bool hasValue = false;
    SourceRange source;
};

using SourceAttributeMap = std::unordered_map<std::string, SourceAttribute>;

struct SourceNode;

struct SourceContent {
    SourceRange source;
    std::string text;
    std::unique_ptr<const SourceNode> node;

    bool isText() const { return !node; }
};

struct SourceNode {
    HTMLTag tag = HTMLTag::Unknown;
    std::string authoredName;
    SourceRange source;
    SourceAttributeMap attributes;
    std::vector<SourceContent> content;
};

struct SourceDocument {
    std::string sourceName;
    std::unique_ptr<const SourceNode> root;
};

struct SourceDocumentParseResult : DiagnosticResult {
    std::shared_ptr<const SourceDocument> document;
    bool ok() const { return !hasErrors() && document && document->root; }
};

class SourceDocumentParser final {
public:
    SourceDocumentParseResult parse(const std::string& html, const std::string& sourceName = {}) const;
};
} // namespace radia::ui
