#include "llviewerprecompiledheaders.h"
#include "rduiinputbridge.h"

#include "indra_constants.h"
#include "llkeyboard.h"

namespace rdui::viewer
{
    namespace
    {
        PointerButton translateButton(NativePointerButton button)
        {
            switch (button)
            {
                case NativePointerButton::Left: return PointerButton::Left;
                case NativePointerButton::Right: return PointerButton::Right;
                case NativePointerButton::Middle: return PointerButton::Middle;
                case NativePointerButton::Auxiliary1: return PointerButton::Auxiliary1;
                case NativePointerButton::Auxiliary2: return PointerButton::Auxiliary2;
                default: return PointerButton::NoButton;
            }
        }

        int translateKey(std::int32_t key)
        {
            return key == KEY_RETURN || key == KEY_PAD_RETURN ? rdui::KEY_RETURN
                 : key == ' ' ? rdui::KEY_SPACE
                 : key == KEY_TAB ? rdui::KEY_TAB
                 : key;
        }

        std::uint32_t translateModifiers(std::uint32_t modifiers)
        {
            std::uint32_t result = 0;
            if (modifiers & MASK_SHIFT) result |= MODIFIER_SHIFT;
            if (modifiers & MASK_CONTROL) result |= MODIFIER_CONTROL;
            if (modifiers & MASK_ALT) result |= MODIFIER_ALT;
            if (modifiers & MASK_MAC_CONTROL) result |= MODIFIER_PLATFORM_CONTROL;
            return result;
        }
    }

    SurfaceInputEvent RduiInputBridge::translate(const NativeInputEvent& event) const
    {
        if (const auto* pointer = std::get_if<NativePointerInput>(&event))
        {
            return SurfacePointerInput{
                pointer->phase,
                {{static_cast<float>(pointer->x), static_cast<float>(pointer->y)},
                 translateButton(pointer->button), translateModifiers(pointer->modifiers),
                 pointer->clickCount, {pointer->dx, pointer->dy}}};
        }
        if (const auto* scroll = std::get_if<NativeScrollInput>(&event))
        {
            return ScrollEvent{{static_cast<float>(scroll->x), static_cast<float>(scroll->y)},
                               scroll->horizontal, scroll->vertical,
                               translateModifiers(scroll->modifiers)};
        }
        if (const auto* key = std::get_if<NativeKeyInput>(&event))
        {
            return SurfaceKeyInput{
                key->down,
                {translateKey(key->key), translateModifiers(key->modifiers), key->repeated}};
        }
        if (const auto* character = std::get_if<NativeCharacterInput>(&event))
            return SurfaceCharacterInput{character->codepoint};
        return std::get<NativeInteractionLoss>(event);
    }

    ECursorType RduiInputBridge::translateCursor(CursorStyle cursor) const
    {
        switch (cursor)
        {
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
}
