/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string_view>

namespace radia::ui {
class Element;

namespace detail {
class HTMLElementFactory final {
public:
    static std::unique_ptr<Element> Create(std::string_view localName);
};
} // namespace detail
} // namespace radia::ui
