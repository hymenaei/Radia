/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <vector>

namespace radia::ui {
struct KeybindingPresentation {
    std::vector<std::string> keys;

    bool operator==(const KeybindingPresentation& other) const { return keys == other.keys; }
};
} // namespace radia::ui
