/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include "types.h"

namespace radia::ui {
std::optional<Color> parseColor(const std::string& value);
bool isColorSyntax(const std::string& value);
} // namespace radia::ui
