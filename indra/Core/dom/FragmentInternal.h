/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>
#include "Node.h"

namespace Core {
class Fragment;
class Element;

namespace detail {
FragmentPtr parseFragment(std::string_view html, const Element* context = nullptr);
std::string serializeChildren(const Node& parent);
} // namespace detail
} // namespace Core
