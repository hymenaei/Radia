/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <functional>
#include <limits>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <variant>
#include "binding/eventregistration.h"
#include "documentcontroller.h"
#include "event.h"
#include "eventcall.h"

namespace radia::viewer::ui { using ControllerHandlerRegistration = radia::ui::EventRegistrationDescriptor; }

namespace radia::viewer::ui::detail {
using radia::ui::CurrentEventArgument;
using radia::ui::Element;
using radia::ui::Event;
using radia::ui::EventArgument;
using radia::ui::EventCall;
using radia::ui::SourceElementArgument;
using radia::viewer::ui::ControllerHandlerRegistration;

template<typename T> using ControllerEventParameterBase = std::remove_cv_t<std::remove_reference_t<T>>;

template<typename T> struct ControllerEventArgumentAdapter {
    static constexpr bool sSupported = false;
};

template<> struct ControllerEventArgumentAdapter<Element*> {
    static constexpr bool sSupported = true;
    static bool matches(const EventArgument& argument) { return std::holds_alternative<SourceElementArgument>(argument); }
    static Element* value(const EventArgument&, Event& event) { return &event.target(); }
};

template<> struct ControllerEventArgumentAdapter<Event> {
    static constexpr bool sSupported = true;
    static bool matches(const EventArgument& argument) { return std::holds_alternative<CurrentEventArgument>(argument); }
    static Event& value(const EventArgument&, Event& event) { return event; }
};

template<typename T> bool controllerEventArgumentMatches(const EventArgument& argument) {
    using Base = ControllerEventParameterBase<T>;
    if constexpr (ControllerEventArgumentAdapter<Base>::sSupported) return ControllerEventArgumentAdapter<Base>::matches(argument);
    else if constexpr (std::is_same_v<Base, bool>) return std::holds_alternative<bool>(argument);
    else if constexpr (std::is_integral_v<Base>) {
        if (!std::holds_alternative<std::int64_t>(argument)) return false;
        const std::int64_t value = std::get<std::int64_t>(argument);
        if constexpr (std::is_signed_v<Base>)
            return value >= static_cast<std::int64_t>(std::numeric_limits<Base>::min())
                && value <= static_cast<std::int64_t>(std::numeric_limits<Base>::max());
        else return value >= 0 && static_cast<std::uint64_t>(value) <= std::numeric_limits<Base>::max();
    } else if constexpr (std::is_same_v<Base, std::string> || std::is_same_v<Base, std::string_view>)
        return std::holds_alternative<std::string>(argument);
    else return false;
}

template<typename T> inline constexpr bool kIsSupportedControllerEventParameter = !std::is_rvalue_reference_v<T>
    && (!std::is_lvalue_reference_v<T> || std::is_const_v<std::remove_reference_t<T>>)
    && (ControllerEventArgumentAdapter<ControllerEventParameterBase<T>>::sSupported
        || std::is_same_v<ControllerEventParameterBase<T>, bool>
        || std::is_integral_v<ControllerEventParameterBase<T>>
        || std::is_same_v<ControllerEventParameterBase<T>, std::string>
        || std::is_same_v<ControllerEventParameterBase<T>, std::string_view>);

template<typename T> const char* controllerEventArgumentError(const EventArgument& argument) {
    if (controllerEventArgumentMatches<T>(argument)) return nullptr;
    if (std::holds_alternative<SourceElementArgument>(argument)) return "binding.event.this_type_mismatch";
    if (std::holds_alternative<CurrentEventArgument>(argument)) return "binding.event.event_type_mismatch";
    return "binding.event.argument_type_mismatch";
}

template<typename... Args, std::size_t... Indices>
const char* controllerEventCallArgumentErrorImpl(const EventCall& call, std::index_sequence<Indices...>) {
    const char* errors[] = {controllerEventArgumentError<Args>(call.arguments()[Indices])...};
    for (const char* error : errors)
        if (error) return error;
    return nullptr;
}

template<typename... Args> const char* controllerEventCallArgumentError(const EventCall& call) {
    if (call.arguments().size() != sizeof...(Args)) return "binding.event.arity_mismatch";
    if constexpr (sizeof...(Args) == 0) return nullptr;
    else return controllerEventCallArgumentErrorImpl<Args...>(call, std::index_sequence_for<Args...>());
}

template<typename T> decltype(auto) controllerEventArgumentValue(const EventArgument& argument, Event& event) {
    static_assert(kIsSupportedControllerEventParameter<T>, "Unsupported DocumentController Event parameter.");
    using Base = ControllerEventParameterBase<T>;
    if constexpr (ControllerEventArgumentAdapter<Base>::sSupported) return ControllerEventArgumentAdapter<Base>::value(argument, event);
    else if constexpr (std::is_same_v<Base, bool>) return std::get<bool>(argument);
    else if constexpr (std::is_integral_v<Base>) return static_cast<Base>(std::get<std::int64_t>(argument));
    else if constexpr (std::is_same_v<Base, std::string>) return std::get<std::string>(argument);
    else return std::string_view(std::get<std::string>(argument));
}

template<typename Controller, typename Method, typename... Args, std::size_t... Indices>
void invokeControllerEvent(Controller* object, Method method, Event& event, const EventCall& call, std::index_sequence<Indices...>) {
    std::invoke(method, object, controllerEventArgumentValue<Args>(call.arguments()[Indices], event)...);
}

template<typename Callback> ControllerHandlerRegistration makeControllerHandlerRegistration(std::string handlerName, Callback callback) {
    using CallbackT = std::decay_t<Callback>;
    static_assert(std::is_invocable_v<CallbackT>,
                  "DocumentController handlers must be callable without arguments; use the member Event overload for event arguments.");
    return {std::move(handlerName), [callback = CallbackT(std::move(callback))](Event&, const EventCall&) mutable { callback(); },
            controllerEventCallArgumentError<>};
}

template<typename T> inline constexpr bool kIsControllerEventParameter = ControllerEventArgumentAdapter<ControllerEventParameterBase<T>>::sSupported
    || std::is_same_v<ControllerEventParameterBase<T>, bool>
    || std::is_integral_v<ControllerEventParameterBase<T>>
    || std::is_same_v<ControllerEventParameterBase<T>, std::string>
    || std::is_same_v<ControllerEventParameterBase<T>, std::string_view>;

template<typename Controller, typename... Args>
ControllerHandlerRegistration makeControllerHandlerRegistration(std::string handlerName, Controller* object,
                                                                         void (Controller::*method)(Args...)) {
    static_assert((kIsControllerEventParameter<Args> && ...), "Unsupported DocumentController Event parameter.");
    static_assert((kIsSupportedControllerEventParameter<Args> && ...), "Unsupported DocumentController Event parameter.");
    if (!object) return {std::move(handlerName), {}, controllerEventCallArgumentError<Args...>};
    return {std::move(handlerName),
            [object, method](Event& event, const EventCall& call) {
                invokeControllerEvent<Controller, decltype(method), Args...>(object, method, event, call, std::index_sequence_for<Args...>());
            },
            controllerEventCallArgumentError<Args...>};
}

template<typename Controller, typename... Args>
ControllerHandlerRegistration makeControllerHandlerRegistration(std::string handlerName, Controller* object,
                                                                         void (Controller::*method)(Args...) const) {
    static_assert((kIsControllerEventParameter<Args> && ...), "Unsupported DocumentController Event parameter.");
    static_assert((kIsSupportedControllerEventParameter<Args> && ...), "Unsupported DocumentController Event parameter.");
    if (!object) return {std::move(handlerName), {}, controllerEventCallArgumentError<Args...>};
    return {std::move(handlerName),
            [object, method](Event& event, const EventCall& call) {
                invokeControllerEvent<Controller, decltype(method), Args...>(object, method, event, call, std::index_sequence_for<Args...>());
            },
            controllerEventCallArgumentError<Args...>};
}
} // namespace radia::viewer::ui::detail

namespace radia::viewer::ui {
template<typename Callback> void DocumentController::handler(std::string handlerName, Callback callback) {
    static_assert(std::is_invocable_v<Callback>,
                  "DocumentController callbacks must be zero-argument; use handler(name, &Controller::method) for typed Event handlers.");
    addHandlerRegistration(detail::makeControllerHandlerRegistration(std::move(handlerName), std::move(callback)));
}

template<typename ControllerT, typename... Args> void DocumentController::handler(std::string handlerName, void (ControllerT::*method)(Args...)) {
    static_assert(std::is_base_of_v<DocumentController, ControllerT>, "Event Handler member must belong to the document controller.");
    addHandlerRegistration(detail::makeControllerHandlerRegistration(std::move(handlerName), dynamic_cast<ControllerT*>(this), method));
}

template<typename ControllerT, typename... Args>
void DocumentController::handler(std::string handlerName, void (ControllerT::*method)(Args...) const) {
    static_assert(std::is_base_of_v<DocumentController, ControllerT>, "Event Handler member must belong to the document controller.");
    addHandlerRegistration(detail::makeControllerHandlerRegistration(std::move(handlerName), dynamic_cast<ControllerT*>(this), method));
}
} // namespace radia::viewer::ui
