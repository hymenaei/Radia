/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <string_view>
#include "ComputedStyle.h"

namespace Core::detail {
struct SystemFontMatch {
    std::string path;
    int faceIndex = 0;
    std::string familyName;
    std::string fullName;
    std::string postScriptName;
};

std::optional<SystemFontMatch> matchSystemFont(const Style::FontFamily& family, const Style::FontSelectionRequest& request);
std::optional<SystemFontMatch> matchLocalFont(std::string_view name);
} // namespace Core::detail
