/**
 * @file schema.cpp
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

#include "linden_common.h"
#include "layout/schema.h"

namespace rdui {
std::string schemaNameKey(std::string_view name) {
    std::string key;
    key.reserve(name.size());
    for (char character : name) {
        if (character >= 'A' && character <= 'Z') character = static_cast<char>(character - 'A' + 'a');
        key.push_back(character);
    }
    return key;
}

bool isWidgetIdentifier(std::string_view value) {
    if (value.empty()) return false;
    for (char character : value)
        if ((character < 'a' || character > 'z') && (character < 'A' || character > 'Z') && (character < '0' || character > '9') && character != '-')
            return false;
    return true;
}

bool isKebabCaseIdentifier(std::string_view value) {
    if (value.empty() || value.front() == '-' || value.back() == '-') return false;
    bool previous_hyphen = false;
    for (char character : value) {
        if (character == '-') {
            if (previous_hyphen) return false;
            previous_hyphen = true;
            continue;
        }
        previous_hyphen = false;
        if ((character < 'a' || character > 'z') && (character < '0' || character > '9')) return false;
    }
    return true;
}
} // namespace rdui
