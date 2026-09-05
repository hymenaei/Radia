/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>
#include "dom/node.h"
#include "html/elementnames.h"

namespace radia::ui {
class Fragment;

namespace html_detail {
bool isValidHTMLAttribute(HTMLTag tag, std::string_view name, bool hasValue, std::string_view value);
FragmentPtr parseFragment(std::string_view html);
std::string serializeChildren(const Node& parent);
} // namespace html_detail
} // namespace radia::ui
