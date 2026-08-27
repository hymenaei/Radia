/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>

namespace radia::ui {
std::string schemaNameKey(std::string_view name);
bool isElementIdentifier(std::string_view value);
} // namespace radia::ui
