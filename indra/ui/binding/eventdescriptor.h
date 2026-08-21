/**
 * @file eventdescriptor.h
 * @brief Defines the type-erased data used to prepare an Event Handler.
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

#ifndef RD_BINDING_EVENTDESCRIPTOR_H
#define RD_BINDING_EVENTDESCRIPTOR_H

#include <functional>
#include <optional>
#include <string>
#include "eventcall.h"
#include "widgetevent.h"

namespace radia::ui {
struct EventRegistrationDescriptor {
    using Invoke = std::function<void(const WidgetEvent&, const EventCall&)>;
    using ArgumentError = std::function<const char*(const EventCall&, WidgetEventKind)>;

    std::string name;
    std::optional<WidgetEventKind> kind;
    Invoke invoke;
    ArgumentError argumentError;
};
} // namespace radia::ui
#endif // RD_BINDING_EVENTDESCRIPTOR_H
