/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>

namespace Core::CSS {
struct StyleRule;

namespace detail {
StyleRule expandNestedSelector(const StyleRule& parent, const std::string& selector);
} // namespace detail
} // namespace Core::CSS
