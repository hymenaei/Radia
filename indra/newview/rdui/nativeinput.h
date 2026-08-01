/**
 * @file nativeinput.h
 * @brief
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef LL_RDUI_NATIVE_INPUT_H
#define LL_RDUI_NATIVE_INPUT_H

#include <cstdint>
#include <optional>
#include <variant>
#include "llcursortypes.h"

namespace rdui::viewer {
enum class NativePointerButton : std::uint8_t { NoButton, Left, Right, Middle, Auxiliary1, Auxiliary2 };
enum class NativePointerPhase : std::uint8_t { Move, Leave, Down, Up };

struct NativePointerInput {
    NativePointerPhase phase = NativePointerPhase::Move;
    float x = 0.f;
    float y = 0.f;
    NativePointerButton button = NativePointerButton::NoButton;
    std::uint32_t modifiers = 0;
    std::uint8_t clickCount = 1;
    float dx = 0.f;
    float dy = 0.f;
};

struct NativeScrollInput {
    std::int32_t x = 0;
    std::int32_t y = 0;
    float horizontal = 0.f;
    float vertical = 0.f;
    std::uint32_t modifiers = 0;
};

struct NativeKeyInput {
    std::int32_t key = 0;
    std::uint32_t modifiers = 0;
    bool down = true;
    bool repeated = false;
};

struct NativeCharacterInput {
    std::uint32_t codepoint = 0;
    std::uint32_t modifiers = 0;
};

enum class NativeInteractionLoss : std::uint8_t { Focus, Capture };

using NativeInputEvent = std::variant<NativePointerInput, NativeScrollInput, NativeKeyInput, NativeCharacterInput, NativeInteractionLoss>;

struct NativeInputDispatchResult {
    bool handled = false;
    std::optional<ECursorType> cursor;
};
} // namespace rdui::viewer
#endif // LL_RDUI_NATIVE_INPUT_H
