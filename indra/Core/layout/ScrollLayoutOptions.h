/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "NativeAppearance.h"

namespace Core::Layout {
struct ScrollLayoutOptions {
    ScrollbarMode scrollbarMode = ScrollbarMode::Classic;
    NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics();
};
} // namespace Core::Layout
