/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "resolver.h"
#include "skin/compiler.h"

namespace radia::viewer::ui {
using radia::ui::SkinGenerationPrepareResult;

SkinGenerationPrepareResult prepareSkinGeneration(SkinSnapshotResult captured);
} // namespace radia::viewer::ui
