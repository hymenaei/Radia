/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>
#include "HTMLNames.h"

namespace radia::ui {
bool isHTMLNameCharacter(char character);
bool isHTMLWhitespace(char character);
bool containsHTMLWhitespace(std::string_view value);
std::string canonicalizeHTMLName(std::string_view name);
std::string decodeHTMLReferences(std::string_view value);
} // namespace radia::ui
