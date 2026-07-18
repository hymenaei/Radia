#ifndef LL_RDUI_ACTION_H
#define LL_RDUI_ACTION_H

#include "rduievent.h"
#include <chrono>
#include <cstdint>
#include <functional>
#include <utility>

namespace rdui
{
    class Widget;

    enum class ActionEventKind : uint8_t
    {
        Click,
        DoubleClick,
        Change,
        MouseDown,
        MouseUp,
        MouseMove,
        LongClick,
        ContextMenu,
    };

    struct ActionEvent
    {
        ActionEvent(Widget& source, ActionEventKind kind) : source(source), kind(kind) {}

        Widget& source;
        ActionEventKind kind;
    };

    struct ClickActionEvent final : ActionEvent
    {
        explicit ClickActionEvent(Widget& source) : ActionEvent(source, ActionEventKind::Click) {}
    };

    struct ChangeActionEvent final : ActionEvent
    {
        ChangeActionEvent(Widget& source, bool checked) : ActionEvent(source, ActionEventKind::Change), checked(checked) {}

        bool checked;
    };

    struct MouseActionEvent final : ActionEvent
    {
        MouseActionEvent(Widget& source, ActionEventKind kind, MouseEvent mouse) : ActionEvent(source, kind), mouse(std::move(mouse)) {}

        MouseEvent mouse;
    };

    struct LongClickActionEvent final : ActionEvent
    {
        LongClickActionEvent(Widget& source, std::chrono::milliseconds held_for) : ActionEvent(source, ActionEventKind::LongClick), heldFor(held_for) {}

        std::chrono::milliseconds heldFor;
    };

    namespace detail
    {
        struct ActionHandler
        {
            ActionEventKind kind;
            std::function<void(const ActionEvent&)> invoke;
        };
    }
}

#endif // LL_RDUI_ACTION_H
