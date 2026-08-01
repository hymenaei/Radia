/**
 * @file rduiinputbridge.cpp
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

#include "llviewerprecompiledheaders.h"
#include "rduiinputbridge.h"
#include "indra_constants.h"
#include "llkeyboard.h"

namespace rdui::viewer {
namespace {
PointerButton translateButton(NativePointerButton button) {
    switch (button) {
        case NativePointerButton::Left: return PointerButton::Left;
        case NativePointerButton::Right: return PointerButton::Right;
        case NativePointerButton::Middle: return PointerButton::Middle;
        case NativePointerButton::Auxiliary1: return PointerButton::Auxiliary1;
        case NativePointerButton::Auxiliary2: return PointerButton::Auxiliary2;
        default: return PointerButton::NoButton;
    }
}

int translateKey(std::int32_t key) {
    switch (key) {
        case KEY_RETURN:
        case KEY_PAD_RETURN: return rdui::KEY_RETURN;
        case ' ': return rdui::KEY_SPACE;
        case KEY_TAB: return rdui::KEY_TAB;
        default: return key;
    }
}

std::uint32_t translateModifiers(std::uint32_t modifiers) {
    std::uint32_t result = 0;
    if (modifiers & MASK_SHIFT) result |= MODIFIER_SHIFT;
    if (modifiers & MASK_CONTROL) result |= MODIFIER_CONTROL;
    if (modifiers & MASK_ALT) result |= MODIFIER_ALT;
    if (modifiers & MASK_MAC_CONTROL) result |= MODIFIER_PLATFORM_CONTROL;
    return result;
}
} // namespace

SurfaceInputEvent RduiInputBridge::translate(const NativeInputEvent& event) const {
    if (const auto* pointer = std::get_if<NativePointerInput>(&event)) {
        return SurfacePointerInput{pointer->phase,
                                   {{static_cast<float>(pointer->x), static_cast<float>(pointer->y)},
                                    translateButton(pointer->button),
                                    translateModifiers(pointer->modifiers),
                                    pointer->clickCount,
                                    {pointer->dx, pointer->dy}}};
    }
    if (const auto* scroll = std::get_if<NativeScrollInput>(&event)) {
        return ScrollEvent{{static_cast<float>(scroll->x), static_cast<float>(scroll->y)},
                           scroll->horizontal,
                           scroll->vertical,
                           translateModifiers(scroll->modifiers)};
    }
    if (const auto* key = std::get_if<NativeKeyInput>(&event))
        return SurfaceKeyInput{key->down, {translateKey(key->key), translateModifiers(key->modifiers), key->repeated}};
    if (const auto* character = std::get_if<NativeCharacterInput>(&event)) return SurfaceCharacterInput{character->codepoint};
    return std::get<NativeInteractionLoss>(event);
}

ECursorType RduiInputBridge::translateCursor(CursorStyle cursor) const {
    switch (cursor) {
        case CursorStyle::Pointer: return UI_CURSOR_HAND;
        case CursorStyle::Progress: return UI_CURSOR_WORKING;
        case CursorStyle::Wait: return UI_CURSOR_WAIT;
        case CursorStyle::Crosshair:
        case CursorStyle::Cell: return UI_CURSOR_CROSS;
        case CursorStyle::Text:
        case CursorStyle::VerticalText: return UI_CURSOR_IBEAM;
        case CursorStyle::Alias:
        case CursorStyle::Copy: return UI_CURSOR_ARROWCOPY;
        case CursorStyle::Move:
        case CursorStyle::AllScroll: return UI_CURSOR_SIZEALL;
        case CursorStyle::NoDrop:
        case CursorStyle::NotAllowed: return UI_CURSOR_NO;
        case CursorStyle::Grab: return UI_CURSOR_TOOLGRAB;
        case CursorStyle::Grabbing: return UI_CURSOR_TOOLGRABBING;
        case CursorStyle::ColumnResize:
        case CursorStyle::EastWestResize: return UI_CURSOR_SIZEWE;
        case CursorStyle::RowResize:
        case CursorStyle::NorthSouthResize: return UI_CURSOR_SIZENS;
        case CursorStyle::NortheastSouthwestResize: return UI_CURSOR_SIZENESW;
        case CursorStyle::NorthwestSoutheastResize: return UI_CURSOR_SIZENWSE;
        case CursorStyle::ZoomIn: return UI_CURSOR_TOOLZOOMIN;
        case CursorStyle::ZoomOut: return UI_CURSOR_TOOLZOOMOUT;
        default: return UI_CURSOR_ARROW;
    }
}
} // namespace rdui::viewer
