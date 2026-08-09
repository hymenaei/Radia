/**
 * @file syntax.h
 * @brief Private top-level stylesheet syntax scanners.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef RD_STYLE_SYNTAX_H
#define RD_STYLE_SYNTAX_H

#include <optional>
#include <string>
#include <vector>

namespace rdui::detail {
std::string trim(const std::string& value);
std::string lower(std::string value);
bool startsWith(const std::string& value, const std::string& prefix);
bool endsWith(const std::string& value, const std::string& suffix);
std::vector<std::string> tokenizeTopLevel(const std::string& value, bool split_slash = false);
std::vector<std::string> splitTopLevel(const std::string& value, char delimiter);
std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open);
} // namespace rdui::detail
#endif // RD_STYLE_SYNTAX_H
