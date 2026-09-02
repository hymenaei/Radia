/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include "diagnostic.h"
#include "dom/document.h"

namespace radia::ui {
struct ResourceBuildResult : DiagnosticResult {
    std::unique_ptr<Document> document;
    bool ok() const { return !hasErrors() && document != nullptr; }

    template<typename ElementT> ElementT* rootAs() { return document ? dynamic_cast<ElementT*>(document->documentElement()) : nullptr; }

    template<typename ElementT> const ElementT* rootAs() const {
        return document ? dynamic_cast<const ElementT*>(document->documentElement()) : nullptr;
    }
};
} // namespace radia::ui
