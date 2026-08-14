/**
 * @file componentcontrollereventadapter.h
 * @brief Defines the viewer ComponentController event registration boundary.
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

#ifndef RD_COMPONENTCONTROLLER_EVENTADAPTER_H
#define RD_COMPONENTCONTROLLER_EVENTADAPTER_H

#include <cstdint>
#include <functional>
#include <limits>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include "componentcontrollerevents.h"
#include "eventcall.h"

namespace rdui::detail {
template<typename T> using ControllerEventParameterBase = std::remove_cv_t<std::remove_reference_t<T>>;

template<typename T> struct ControllerEventArgumentAdapter {
    static constexpr bool sSupported = false;
    static constexpr std::optional<WidgetEventKind> sKind = std::nullopt;
};

template<> struct ControllerEventArgumentAdapter<viewer::Widget> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = std::nullopt;
    static bool matches(const EventArgument& argument, WidgetEventKind) { return std::holds_alternative<SourceWidgetArgument>(argument); }
    static viewer::Widget value(const EventArgument&, const WidgetEvent& event) { return viewer::Widget::fromEventSource(event.source); }
};

template<> struct ControllerEventArgumentAdapter<viewer::Event> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = std::nullopt;
    static bool matches(const EventArgument& argument, WidgetEventKind) { return std::holds_alternative<CurrentEventArgument>(argument); }
    static viewer::Event value(const EventArgument&, const WidgetEvent& event) { return viewer::Event(event); }
};

template<> struct ControllerEventArgumentAdapter<viewer::ClickEvent> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = WidgetEventKind::Click;
    static bool matches(const EventArgument& argument, WidgetEventKind eventKind) {
        return std::holds_alternative<CurrentEventArgument>(argument) && eventKind == WidgetEventKind::Click;
    }
    static viewer::ClickEvent value(const EventArgument&, const WidgetEvent& event) { return viewer::ClickEvent(event); }
};

template<> struct ControllerEventArgumentAdapter<viewer::ChangeEvent> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = WidgetEventKind::Change;
    static bool matches(const EventArgument& argument, WidgetEventKind eventKind) {
        return std::holds_alternative<CurrentEventArgument>(argument) && eventKind == WidgetEventKind::Change;
    }
    static viewer::ChangeEvent value(const EventArgument&, const WidgetEvent& event) {
        return viewer::ChangeEvent(static_cast<const rdui::ChangeEvent&>(event));
    }
};

template<> struct ControllerEventArgumentAdapter<viewer::MouseWidgetEvent> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = std::nullopt;
    static bool matches(const EventArgument& argument, WidgetEventKind eventKind) {
        return std::holds_alternative<CurrentEventArgument>(argument)
            && (eventKind == WidgetEventKind::MouseDown
                || eventKind == WidgetEventKind::MouseUp
                || eventKind == WidgetEventKind::MouseMove
                || eventKind == WidgetEventKind::DoubleClick
                || eventKind == WidgetEventKind::ContextMenu);
    }
    static viewer::MouseWidgetEvent value(const EventArgument&, const WidgetEvent& event) {
        return viewer::MouseWidgetEvent(static_cast<const rdui::MouseWidgetEvent&>(event));
    }
};

template<> struct ControllerEventArgumentAdapter<viewer::LongClickEvent> {
    static constexpr bool sSupported = true;
    static constexpr std::optional<WidgetEventKind> sKind = WidgetEventKind::LongClick;
    static bool matches(const EventArgument& argument, WidgetEventKind eventKind) {
        return std::holds_alternative<CurrentEventArgument>(argument) && eventKind == WidgetEventKind::LongClick;
    }
    static viewer::LongClickEvent value(const EventArgument&, const WidgetEvent& event) {
        return viewer::LongClickEvent(static_cast<const rdui::LongClickEvent&>(event));
    }
};

template<typename T> constexpr std::optional<WidgetEventKind> controllerEventKindForParameter() {
    return ControllerEventArgumentAdapter<ControllerEventParameterBase<T>>::sKind;
}

template<typename... Args> constexpr std::optional<WidgetEventKind> controllerEventKindForParameters() {
    std::optional<WidgetEventKind> result;
    auto add = [&result](const std::optional<WidgetEventKind> kind) {
        if (kind && !result) result = kind;
    };
    (add(controllerEventKindForParameter<Args>()), ...);
    return result;
}

template<typename... Args> consteval bool controllerEventKindsConflict() {
    std::optional<WidgetEventKind> first;
    bool conflict = false;
    auto add = [&first, &conflict](const std::optional<WidgetEventKind> kind) {
        if (!kind) return;
        if (!first) first = *kind;
        else if (*first != *kind) conflict = true;
    };
    (add(controllerEventKindForParameter<Args>()), ...);
    return conflict;
}

template<typename T> bool controllerEventArgumentMatches(const EventArgument& argument, WidgetEventKind kind) {
    using Base = ControllerEventParameterBase<T>;
    if constexpr (ControllerEventArgumentAdapter<Base>::sSupported) return ControllerEventArgumentAdapter<Base>::matches(argument, kind);
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

template<typename T> const char* controllerEventArgumentError(const EventArgument& argument, WidgetEventKind kind) {
    if (controllerEventArgumentMatches<T>(argument, kind)) return nullptr;
    if (std::holds_alternative<SourceWidgetArgument>(argument)) return "binding.event.this_type_mismatch";
    if (std::holds_alternative<CurrentEventArgument>(argument)) return "binding.event.event_type_mismatch";
    return "binding.event.argument_type_mismatch";
}

template<typename... Args, std::size_t... Indices>
const char* controllerEventCallArgumentErrorImpl(const EventCall& call, WidgetEventKind kind, std::index_sequence<Indices...>) {
    const char* errors[] = {controllerEventArgumentError<Args>(call.arguments()[Indices], kind)...};
    for (const char* error : errors)
        if (error) return error;
    return nullptr;
}

template<typename... Args> const char* controllerEventCallArgumentError(const EventCall& call, WidgetEventKind kind) {
    if (call.arguments().size() != sizeof...(Args)) return "binding.event.arity_mismatch";
    if constexpr (sizeof...(Args) == 0) return nullptr;
    else return controllerEventCallArgumentErrorImpl<Args...>(call, kind, std::index_sequence_for<Args...>());
}

template<typename T> decltype(auto) controllerEventArgumentValue(const EventArgument& argument, const WidgetEvent& event) {
    static_assert(kIsSupportedControllerEventParameter<T>, "Unsupported ComponentController Event parameter.");
    using Base = ControllerEventParameterBase<T>;
    if constexpr (ControllerEventArgumentAdapter<Base>::sSupported) return ControllerEventArgumentAdapter<Base>::value(argument, event);
    else if constexpr (std::is_same_v<Base, bool>) return std::get<bool>(argument);
    else if constexpr (std::is_integral_v<Base>) return static_cast<Base>(std::get<std::int64_t>(argument));
    else if constexpr (std::is_same_v<Base, std::string>) return std::get<std::string>(argument);
    else return std::string_view(std::get<std::string>(argument));
}

template<typename Controller, typename Method, typename... Args, std::size_t... Indices>
void invokeControllerEvent(Controller* object, Method method, const WidgetEvent& event, const EventCall& call, std::index_sequence<Indices...>) {
    std::invoke(method, object, controllerEventArgumentValue<Args>(call.arguments()[Indices], event)...);
}

template<typename Callback> viewer::ComponentControllerEventRegistration makeControllerEventRegistration(std::string name, Callback callback) {
    using CallbackT = std::decay_t<Callback>;
    static_assert(std::is_invocable_v<CallbackT>,
                  "ComponentController callbacks must be callable without arguments; use the member Event overload for typed arguments.");
    return {std::move(name), std::nullopt, [callback = CallbackT(std::move(callback))](const WidgetEvent&, const EventCall&) mutable { callback(); },
            controllerEventCallArgumentError<>};
}

template<typename T> inline constexpr bool kIsControllerEventParameter =
    !std::is_base_of_v<WidgetEvent, ControllerEventParameterBase<T>> || ControllerEventArgumentAdapter<ControllerEventParameterBase<T>>::sSupported;

template<typename Controller, typename... Args>
viewer::ComponentControllerEventRegistration makeControllerEventRegistration(std::string name, Controller* object,
                                                                             void (Controller::*method)(Args...)) {
    static_assert((kIsControllerEventParameter<Args> && ...),
                  "ComponentController Event parameters must use the viewer Event facades, not core rdui Events.");
    static_assert((kIsSupportedControllerEventParameter<Args> && ...), "Unsupported ComponentController Event parameter.");
    static_assert(!controllerEventKindsConflict<Args...>(), "ComponentController Event Handler parameters must use one typed Event kind.");
    const std::optional<WidgetEventKind> kind = controllerEventKindForParameters<Args...>();
    if (!object) return {std::move(name), kind, {}, controllerEventCallArgumentError<Args...>};
    return {std::move(name), kind,
            [object, method](const WidgetEvent& event, const EventCall& call) {
                invokeControllerEvent<Controller, decltype(method), Args...>(object, method, event, call, std::index_sequence_for<Args...>());
            },
            controllerEventCallArgumentError<Args...>};
}

template<typename Controller, typename... Args>
viewer::ComponentControllerEventRegistration makeControllerEventRegistration(std::string name, Controller* object,
                                                                             void (Controller::*method)(Args...) const) {
    static_assert((kIsControllerEventParameter<Args> && ...),
                  "ComponentController Event parameters must use the viewer Event facades, not core rdui Events.");
    static_assert((kIsSupportedControllerEventParameter<Args> && ...), "Unsupported ComponentController Event parameter.");
    static_assert(!controllerEventKindsConflict<Args...>(), "ComponentController Event Handler parameters must use one typed Event kind.");
    const std::optional<WidgetEventKind> kind = controllerEventKindForParameters<Args...>();
    if (!object) return {std::move(name), kind, {}, controllerEventCallArgumentError<Args...>};
    return {std::move(name), kind,
            [object, method](const WidgetEvent& event, const EventCall& call) {
                invokeControllerEvent<Controller, decltype(method), Args...>(object, method, event, call, std::index_sequence_for<Args...>());
            },
            controllerEventCallArgumentError<Args...>};
}
} // namespace rdui::detail
#endif // RD_COMPONENTCONTROLLER_EVENTADAPTER_H
