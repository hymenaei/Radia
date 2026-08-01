/**
 * @file action.h
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

#ifndef RD_ACTION_H
#define RD_ACTION_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <utility>
#include "event.h"

namespace rdui {
class Widget;

enum class ActionEventKind : uint8_t { Click, DoubleClick, Change, MouseDown, MouseUp, MouseMove, LongClick, ContextMenu };

struct ActionEvent {
    ActionEvent(Widget& source, ActionEventKind kind) : source(source), kind(kind) {}

    Widget& source;
    ActionEventKind kind;
};

struct ClickActionEvent final : ActionEvent {
    explicit ClickActionEvent(Widget& source) : ActionEvent(source, ActionEventKind::Click) {}
};

struct ChangeActionEvent final : ActionEvent {
    ChangeActionEvent(Widget& source, bool checked) : ActionEvent(source, ActionEventKind::Change), checked(checked) {}

    bool checked;
};

struct MouseActionEvent final : ActionEvent {
    MouseActionEvent(Widget& source, ActionEventKind kind, MouseEvent mouse) : ActionEvent(source, kind), mouse(std::move(mouse)) {}

    MouseEvent mouse;
};

struct LongClickActionEvent final : ActionEvent {
    LongClickActionEvent(Widget& source, std::chrono::milliseconds held_for) : ActionEvent(source, ActionEventKind::LongClick), heldFor(held_for) {}

    std::chrono::milliseconds heldFor;
};

namespace detail {
struct ActionHandler {
    ActionEventKind kind;
    std::function<void(const ActionEvent&)> invoke;
};
} // namespace detail
} // namespace rdui
#endif // RD_ACTION_H
