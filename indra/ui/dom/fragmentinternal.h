/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>
#include "dom/node.h"

namespace radia::ui {
class Fragment;

namespace dom_detail {
FragmentPtr parseFragment(std::string_view html);
std::string serializeChildren(const Node& parent);
} // namespace dom_detail
} // namespace radia::ui
