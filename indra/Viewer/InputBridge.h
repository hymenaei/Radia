/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/ComputedStyle.h>
#include <Core/Event.h>
#include "NativeInput.h"
#include "llcursortypes.h"

namespace Viewer {
Core::PointerEvent translatePointerInput(const NativePointerInput& input);
Core::WheelEvent translateScrollInput(const NativeScrollInput& input);
Core::KeyEvent translateKeyInput(const NativeKeyInput& input);
ECursorType translateCursor(Core::Style::CursorStyle cursor);
} // namespace Viewer
