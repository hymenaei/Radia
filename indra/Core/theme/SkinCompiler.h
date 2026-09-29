/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include "Diagnostic.h"
#include "ResourceProvider.h"
#include "SkinGeneration.h"

namespace Core {
struct SkinGenerationPrepareResult : DiagnosticResult {
    bool ok() const { return !hasErrors() && generation != nullptr; }
    std::shared_ptr<const SkinGeneration> generation;
};

class SkinCompiler final {
public:
    SkinGenerationPrepareResult prepare(ResourceSnapshot resources) const;
};
} // namespace Core
