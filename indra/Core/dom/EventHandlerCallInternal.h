/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <map>
#include <string>
#include <string_view>
#include "EventHandlerCall.h"

namespace Core::detail {
class EventHandlerCallStore final {
public:
    static void set(Element& element, std::string_view type, EventHandlerCall call);
    static const EventHandlerCall* find(const Element& element, std::string_view type);

private:
    std::map<std::string, EventHandlerCall, std::less<>> mCalls;
};
} // namespace Core::detail
