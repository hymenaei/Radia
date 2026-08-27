/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/schema.h"

namespace radia::ui {
std::string schemaNameKey(std::string_view name) {
    std::string key;
    key.reserve(name.size());
    for (char character : name) {
        if (character >= 'A' && character <= 'Z') character = static_cast<char>(character - 'A' + 'a');
        key.push_back(character);
    }
    return key;
}

bool isElementIdentifier(std::string_view value) {
    if (value.empty()) return false;
    for (char character : value)
        if ((character < 'a' || character > 'z')
            && (character < 'A' || character > 'Z')
            && (character < '0' || character > '9')
            && character != '-'
            && character != '_')
            return false;
    return true;
}
} // namespace radia::ui
