/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "event.h"
#include "llcursortypes.h"
#include "nativeinput.h"
#include "style/computedstyle.h"

namespace radia::viewer::ui {
using radia::ui::CursorStyle;
using radia::ui::KeyEvent;
using radia::ui::PointerEvent;
using radia::ui::WheelEvent;

PointerEvent translatePointerInput(const NativePointerInput& input);
WheelEvent translateScrollInput(const NativeScrollInput& input);
KeyEvent translateKeyInput(const NativeKeyInput& input);
ECursorType translateCursor(CursorStyle cursor);
} // namespace radia::viewer::ui
