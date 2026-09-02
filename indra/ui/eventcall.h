/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <variant>
#include <vector>
#include "event.h"

namespace radia::ui {
struct SourceElementArgument {};
struct CurrentEventArgument {};

using EventArgument = std::variant<std::int64_t, std::string, bool, SourceElementArgument, CurrentEventArgument>;

struct AuthoredEventDescriptor {
    std::string_view attribute;
    std::string_view type;
};

inline constexpr AuthoredEventDescriptor kAuthoredEventDescriptors[] = {
    {"onClick", kClickEvent},
    {"onDoubleClick", kDoubleClickEvent},
    {"onInput", kInputEvent},
    {"onChange", kChangeEvent},
    {"onPointerDown", kPointerDownEvent},
    {"onPointerUp", kPointerUpEvent},
    {"onPointerMove", kPointerMoveEvent},
    {"onContextMenu", kContextMenuEvent},
    {"onWheel", kWheelEvent},
};

class EventCall {
public:
    explicit EventCall(std::string name, std::vector<EventArgument> arguments = {});

    const std::string& name() const { return mName; }
    const std::vector<EventArgument>& arguments() const { return mArguments; }

private:
    std::string mName;
    std::vector<EventArgument> mArguments;
};

class Element;
Element& setAuthoredEventCall(Element& element, std::string_view type, EventCall call);
const EventCall* authoredEventCall(const Element& element, std::string_view type);

enum class EventCallParseError : std::uint8_t { NoError, CallRequired, NameInvalid, SyntaxInvalid, LiteralUnsupported, IntegerOutOfRange };

struct EventCallParseResult {
    std::optional<EventCall> call;
    EventCallParseError error = EventCallParseError::NoError;
    std::size_t errorOffset = 0;

    bool ok() const { return call.has_value(); }
};

EventCallParseResult parseEventCall(std::string_view source);
bool isEventHandlerName(std::string_view value);
const char* eventCallParseErrorCode(EventCallParseError error);
const char* eventCallParseErrorMessage(EventCallParseError error);
} // namespace radia::ui
