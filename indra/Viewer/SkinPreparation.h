/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/SkinCompiler.h>
#include "SkinSnapshot.h"

namespace Viewer {
Core::SkinGenerationPrepareResult prepareSkinGeneration(SkinSnapshotResult captured);
} // namespace Viewer
