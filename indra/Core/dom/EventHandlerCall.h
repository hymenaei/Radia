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

namespace Core {
struct CurrentTargetArgument {
    bool operator==(const CurrentTargetArgument&) const = default;
};
struct CurrentEventArgument {
    bool operator==(const CurrentEventArgument&) const = default;
};

using EventCallArgument = std::variant<std::int64_t, std::string, bool, CurrentTargetArgument, CurrentEventArgument>;

class EventHandlerCall {
public:
    explicit EventHandlerCall(std::string name, std::vector<EventCallArgument> arguments = {});

    const std::string& name() const { return mName; }
    const std::vector<EventCallArgument>& arguments() const { return mArguments; }
    bool operator==(const EventHandlerCall&) const = default;

private:
    std::string mName;
    std::vector<EventCallArgument> mArguments;
};

class Element;
Element& setEventHandlerCall(Element& element, std::string_view type, EventHandlerCall call);
const EventHandlerCall* eventHandlerCall(const Element& element, std::string_view type);

enum class EventHandlerCallParseError : std::uint8_t {
    NoError,
    CallRequired,
    NameInvalid,
    SyntaxInvalid,
    LiteralUnsupported,
    IntegerOutOfRange
};

struct EventHandlerCallParseResult {
    std::optional<EventHandlerCall> call;
    EventHandlerCallParseError error = EventHandlerCallParseError::NoError;
    std::size_t errorOffset = 0;

    bool ok() const { return call.has_value(); }
};

EventHandlerCallParseResult parseEventHandlerCall(std::string_view source);
bool isEventHandlerName(std::string_view value);
const char* eventHandlerCallParseErrorCode(EventHandlerCallParseError error);
const char* eventHandlerCallParseErrorMessage(EventHandlerCallParseError error);
} // namespace Core
