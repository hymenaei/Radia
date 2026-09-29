/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include "CSSTokenStream.h"
#include "Color.h"

namespace Core::CSS {
std::optional<Color> consumeColor(const detail::TokenStream& stream, detail::TokenRange range);
std::optional<Color> consumeColor(const std::string& value);
bool isColorSyntax(const detail::TokenStream& stream, detail::TokenRange range);
bool isColorSyntax(const std::string& value);
} // namespace Core::CSS
