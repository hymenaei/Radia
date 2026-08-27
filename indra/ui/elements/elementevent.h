/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string_view>
#include "event.h"

namespace radia::ui {
struct AuthoredEventDescriptor {
    std::string_view attribute;
    std::string_view type;
};

inline constexpr AuthoredEventDescriptor kAuthoredEventDescriptors[] = {
    {"onClick", kClickEvent},
    {"onDoubleClick", kDoubleClickEvent},
    {"onInput", kInputEvent},
    {"onChange", kChangeEvent},
    {"onPointerDown", kPointerDownEvent},
    {"onPointerUp", kPointerUpEvent},
    {"onPointerMove", kPointerMoveEvent},
    {"onContextMenu", kContextMenuEvent},
    {"onWheel", kWheelEvent},
};
} // namespace radia::ui
