/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "inputbridge.h"
#include "indra_constants.h"
#include "llkeyboard.h"

namespace radia::viewer::ui {
using radia::ui::CursorStyle;
using radia::ui::KeyEvent;
using radia::ui::kKeyReturn;
using radia::ui::kKeySpace;
using radia::ui::kKeyTab;
using radia::ui::kModifierAlt;
using radia::ui::kModifierControl;
using radia::ui::kModifierPlatformControl;
using radia::ui::kModifierShift;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::WheelEvent;

namespace {
constexpr float kDomWheelPixelsPerDetent = 40.f;

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
        case KEY_PAD_RETURN: return kKeyReturn;
        case ' ': return kKeySpace;
        case KEY_TAB: return kKeyTab;
        default: return key;
    }
}

std::uint32_t translateModifiers(std::uint32_t modifiers) {
    std::uint32_t result = 0;
    if (modifiers & MASK_SHIFT) result |= kModifierShift;
    if (modifiers & MASK_CONTROL) result |= kModifierControl;
    if (modifiers & MASK_ALT) result |= kModifierAlt;
    if (modifiers & MASK_MAC_CONTROL) result |= kModifierPlatformControl;
    return result;
}
} // namespace

PointerEvent translatePointerInput(const NativePointerInput& input) {
    return {{static_cast<float>(input.x), static_cast<float>(input.y)},
            translateButton(input.button),
            translateModifiers(input.modifiers),
            input.clickCount,
            {input.dx, input.dy}};
}

WheelEvent translateScrollInput(const NativeScrollInput& input) {
    return {{static_cast<float>(input.x), static_cast<float>(input.y)},
            input.horizontal * kDomWheelPixelsPerDetent,
            input.vertical * kDomWheelPixelsPerDetent,
            translateModifiers(input.modifiers)};
}

KeyEvent translateKeyInput(const NativeKeyInput& input) {
    return {translateKey(input.key), translateModifiers(input.modifiers), input.repeated};
}

ECursorType translateCursor(CursorStyle cursor) {
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
} // namespace radia::viewer::ui
