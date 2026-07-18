#ifndef LL_RDUI_INPUT_BRIDGE_H
#define LL_RDUI_INPUT_BRIDGE_H

#include "rduievent.h"
#include "rduinativeinput.h"
#include "rduistyle.h"

#include <cstdint>
#include <variant>

namespace rdui::viewer
{
    struct SurfacePointerInput
    {
        NativePointerPhase phase = NativePointerPhase::Move;
        PointerEvent event;
    };

    struct SurfaceKeyInput
    {
        bool down = true;
        KeyEvent event;
    };

    struct SurfaceCharacterInput
    {
        std::uint32_t codepoint = 0;
    };

    using SurfaceInputEvent = std::variant<SurfacePointerInput, ScrollEvent, SurfaceKeyInput,
                                           SurfaceCharacterInput, NativeInteractionLoss>;

    class RduiInputBridge final
    {
        public:
            SurfaceInputEvent translate(const NativeInputEvent& event) const;
            ECursorType translateCursor(CursorStyle cursor) const;
    };
}

#endif // LL_RDUI_INPUT_BRIDGE_H
