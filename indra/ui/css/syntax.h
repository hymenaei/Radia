/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <vector>

namespace radia::ui::detail {
std::string trim(const std::string& value);
std::string lower(std::string value);
bool startsWith(const std::string& value, const std::string& prefix);
bool endsWith(const std::string& value, const std::string& suffix);
std::vector<std::string> tokenizeTopLevel(const std::string& value, bool splitSlash = false);
std::vector<std::string> splitTopLevel(const std::string& value, char delimiter);
std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open);
} // namespace radia::ui::detail
