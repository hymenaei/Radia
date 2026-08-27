/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "skinpreparation.h"
#include <memory>

namespace radia::viewer::ui {
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;

SkinGenerationPrepareResult prepareSkinGeneration(SkinSnapshotResult captured) {
    SkinGenerationPrepareResult result;
    ResourceSnapshot snapshot = std::move(captured.snapshot);
    result.append(std::move(captured));
    if (result.hasErrors()) return result;

    SkinGenerationPrepareResult compiled = SkinCompiler().prepare(std::move(snapshot));
    auto generation = std::move(compiled.generation);
    result.append(std::move(compiled));
    result.generation = std::move(generation);
    return result;
}
} // namespace radia::viewer::ui
