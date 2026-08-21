/**
 * @file eventregistration.h
 * @brief Defines the type-erased Event Handler registration boundary.
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

#ifndef RD_BINDING_EVENTREGISTRATION_H
#define RD_BINDING_EVENTREGISTRATION_H

#include <functional>
#include <optional>
#include <string>
#include <utility>
#include "eventdescriptor.h"

namespace radia::ui {
class Binder;
class EventHandlerRegistration;

namespace detail {
EventHandlerRegistration makeEventRegistration(EventRegistrationDescriptor descriptor);
EventHandlerRegistration makeEventRegistration(std::string name, std::optional<WidgetEventKind> kind,
                                               std::function<void(const WidgetEvent&, const EventCall&)> invoke,
                                               std::function<const char*(const EventCall&, WidgetEventKind)> argumentError);
} // namespace detail

class EventHandlerRegistration final {
public:
    using Invoke = EventRegistrationDescriptor::Invoke;
    using ArgumentError = EventRegistrationDescriptor::ArgumentError;

private:
    friend class Binder;
    friend EventHandlerRegistration detail::makeEventRegistration(EventRegistrationDescriptor descriptor);
    friend EventHandlerRegistration detail::makeEventRegistration(std::string name, std::optional<WidgetEventKind> kind, Invoke invoke,
                                                                  ArgumentError argumentError);

    explicit EventHandlerRegistration(EventRegistrationDescriptor descriptor) : mDescriptor(std::move(descriptor)) {}

    bool valid() const noexcept {
        return !mDescriptor.name.empty() && static_cast<bool>(mDescriptor.invoke) && static_cast<bool>(mDescriptor.argumentError);
    }
    const std::string& name() const noexcept { return mDescriptor.name; }
    std::optional<WidgetEventKind> kind() const noexcept { return mDescriptor.kind; }
    std::string takeName() && { return std::move(mDescriptor.name); }
    std::optional<WidgetEventKind> takeKind() && { return mDescriptor.kind; }
    Invoke takeInvoke() && { return std::move(mDescriptor.invoke); }
    ArgumentError takeArgumentError() && { return std::move(mDescriptor.argumentError); }
    EventHandlerRegistration copy() const { return EventHandlerRegistration(mDescriptor); }

    EventRegistrationDescriptor mDescriptor;
};

namespace detail {
inline EventHandlerRegistration makeEventRegistration(EventRegistrationDescriptor descriptor) {
    return EventHandlerRegistration(std::move(descriptor));
}

inline EventHandlerRegistration makeEventRegistration(std::string name, std::optional<WidgetEventKind> kind, EventHandlerRegistration::Invoke invoke,
                                                      EventHandlerRegistration::ArgumentError argumentError) {
    return makeEventRegistration({std::move(name), kind, std::move(invoke), std::move(argumentError)});
}
} // namespace detail
} // namespace radia::ui
#endif // RD_BINDING_EVENTREGISTRATION_H
