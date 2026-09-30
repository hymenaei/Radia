/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <vector>
#include "HTMLName.h"
#include "ResourceBuildResult.h"
#include "SourceDocument.h"

namespace Core {
class Element;
class ElementBuildContext;

enum class InlineValidationKind {
    UnsupportedElement,
    AttributeUnknown,
    KbdShortcutRequired,
    KbdShortcutInvalid,
    ChildrenUnsupported
};

struct InlineValidationFinding {
    InlineValidationKind kind;
    HTMLTag tag = HTMLTag::Unknown;
    SourceRange elementSource;
    SourceRange source;
    std::string elementName;
    std::string attributeName;
};

struct InlineValidationResult {
    std::vector<InlineValidationFinding> findings;
};

InlineValidationResult validateInlineContent(const std::vector<SourceContent>& contentItems, const std::vector<HTMLTag>& acceptedTags);

void appendInlineElements(Element& target, const std::vector<SourceContent>& contentItems, const std::string& hostName,
    const std::vector<HTMLTag>& acceptedTags, ElementBuildContext& context, const std::string& sourceName);
} // namespace Core
