/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <map>
#include <string>
#include <string_view>
#include "eventcall.h"

namespace radia::ui::detail {
class AuthoredEventStore final {
public:
    static void set(Element& element, std::string_view type, EventCall call);
    static const EventCall* find(const Element& element, std::string_view type);

private:
    std::map<std::string, EventCall, std::less<>> mCalls;
};
} // namespace radia::ui::detail
