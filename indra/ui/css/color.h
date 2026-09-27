/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include "syntax.h"
#include "types.h"

namespace radia::ui {
std::optional<Color> consumeColor(const detail::CSSTokenStream& stream, detail::CSSTokenRange range);
std::optional<Color> consumeColor(const std::string& value);
bool isColorSyntax(const detail::CSSTokenStream& stream, detail::CSSTokenRange range);
bool isColorSyntax(const std::string& value);
} // namespace radia::ui
