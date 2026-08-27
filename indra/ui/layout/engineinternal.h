/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "layout/engine.h"

namespace radia::ui {
class StylePass;

LayoutStatistics layoutTreeUsingStylePass(Element& root, StylePass& styles, ScrollLayoutOptions scrollOptions = {});
} // namespace radia::ui
