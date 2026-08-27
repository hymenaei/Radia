/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <string>
#include <utility>
#include "event.h"
#include "eventcall.h"

namespace radia::ui {
struct EventRegistrationDescriptor {
    using Invoke = std::function<void(Event&, const EventCall&)>;
    using ArgumentError = std::function<const char*(const EventCall&)>;

    std::string name;
    Invoke invoke;
    ArgumentError argumentError;
};

class Binder;
class EventHandlerRegistration;

namespace detail {
EventHandlerRegistration makeEventRegistration(EventRegistrationDescriptor descriptor);
EventHandlerRegistration makeEventRegistration(std::string name, EventRegistrationDescriptor::Invoke invoke,
                                               EventRegistrationDescriptor::ArgumentError argumentError);
} // namespace detail

class EventHandlerRegistration final {
public:
    using Invoke = EventRegistrationDescriptor::Invoke;
    using ArgumentError = EventRegistrationDescriptor::ArgumentError;

private:
    friend class Binder;
    friend EventHandlerRegistration detail::makeEventRegistration(EventRegistrationDescriptor descriptor);
    friend EventHandlerRegistration detail::makeEventRegistration(std::string name, Invoke invoke, ArgumentError argumentError);

    explicit EventHandlerRegistration(EventRegistrationDescriptor descriptor) : mDescriptor(std::move(descriptor)) {}

    bool valid() const noexcept {
        return !mDescriptor.name.empty() && static_cast<bool>(mDescriptor.invoke) && static_cast<bool>(mDescriptor.argumentError);
    }
    const std::string& name() const noexcept { return mDescriptor.name; }
    std::string takeName() && { return std::move(mDescriptor.name); }
    Invoke takeInvoke() && { return std::move(mDescriptor.invoke); }
    ArgumentError takeArgumentError() && { return std::move(mDescriptor.argumentError); }
    EventHandlerRegistration copy() const { return EventHandlerRegistration(mDescriptor); }

    EventRegistrationDescriptor mDescriptor;
};

namespace detail {
inline EventHandlerRegistration makeEventRegistration(EventRegistrationDescriptor descriptor) {
    return EventHandlerRegistration(std::move(descriptor));
}

inline EventHandlerRegistration makeEventRegistration(std::string name, EventHandlerRegistration::Invoke invoke,
                                                      EventHandlerRegistration::ArgumentError argumentError) {
    return makeEventRegistration({std::move(name), std::move(invoke), std::move(argumentError)});
}
} // namespace detail
} // namespace radia::ui
