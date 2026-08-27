/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <vector>
#include "layout/buildresult.h"
#include "layout/document.h"

namespace radia::ui {
class Element;
class LayoutBuildContext;

enum class InlineValidationKind {
    UnsupportedElement,
    NotImplemented,
    AttributeUnknown,
    KbdShortcutRequired,
    KbdShortcutInvalid,
    ChildrenUnsupported
};

struct InlineValidationFinding {
    InlineValidationKind kind;
    Tag tag = Tag::Unknown;
    SourceRange elementSource;
    SourceRange source;
    std::string elementName;
    std::string attributeName;
};

struct InlineValidationResult {
    std::vector<InlineValidationFinding> findings;
};

InlineValidationResult validateInlineContent(const std::vector<SourceContent>& contentItems, const std::vector<Tag>& acceptedElements);

void appendInlineElements(Element& target, const std::vector<SourceContent>& contentItems, const std::string& hostElementName,
                          const std::vector<Tag>& acceptedElements, LayoutBuildResult& result, const std::string& source,
                          const LayoutBuildContext* context);
} // namespace radia::ui
