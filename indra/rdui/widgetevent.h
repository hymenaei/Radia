/**
 * @file widgetevent.h
 * @brief Cross-cutting Widget Event vocabulary and typed Event declarations.
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

#ifndef RD_WIDGETEVENT_H
#define RD_WIDGETEVENT_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <optional>
#include <utility>
#include "event.h"

namespace rdui {
class Widget;
class EventCall;

enum class WidgetEventKind : uint8_t {
#define WIDGET_EVENT_ENTRY(name, attribute) name,
#include "widgetevents.def"
#undef WIDGET_EVENT_ENTRY
};

inline constexpr WidgetEventKind kAllWidgetEventKinds[] = {
#define WIDGET_EVENT_ENTRY(name, attribute) WidgetEventKind::name,
#include "widgetevents.def"
#undef WIDGET_EVENT_ENTRY
};

struct WidgetEvent {
    WidgetEvent(Widget& source, WidgetEventKind kind) : source(source), kind(kind) {}

    Widget& source;
    WidgetEventKind kind;
};

struct ClickEvent final : WidgetEvent {
    explicit ClickEvent(Widget& source) : WidgetEvent(source, WidgetEventKind::Click) {}
};

struct ChangeEvent final : WidgetEvent {
    ChangeEvent(Widget& source, bool checked) : WidgetEvent(source, WidgetEventKind::Change), checked(checked) {}

    bool checked;
};

struct MouseWidgetEvent final : WidgetEvent {
    MouseWidgetEvent(Widget& source, WidgetEventKind kind, MouseEvent mouse) : WidgetEvent(source, kind), mouse(std::move(mouse)) {}

    MouseEvent mouse;
};

struct LongClickEvent final : WidgetEvent {
    LongClickEvent(Widget& source, std::chrono::milliseconds heldFor) : WidgetEvent(source, WidgetEventKind::LongClick), heldFor(heldFor) {}

    std::chrono::milliseconds heldFor;
};

namespace detail {
struct EventHandler {
    std::optional<WidgetEventKind> kind;
    std::function<void(const WidgetEvent&, const EventCall&)> invoke;
};
} // namespace detail
} // namespace rdui
#endif // RD_WIDGETEVENT_H
