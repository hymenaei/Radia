/**
 * @file schema.h
 * @brief Provides Layout Resource identifier and attribute-name validation helpers.
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

#ifndef RD_LAYOUT_SCHEMA_H
#define RD_LAYOUT_SCHEMA_H

#include <string>
#include <string_view>

namespace radia::ui {
std::string schemaNameKey(std::string_view name);
bool isWidgetIdentifier(std::string_view value);
bool isKebabCaseIdentifier(std::string_view value);
} // namespace radia::ui
#endif // RD_LAYOUT_SCHEMA_H
