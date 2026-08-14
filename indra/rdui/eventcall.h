/**
 * @file eventcall.h
 * @brief Defines and parses the restricted Event Handler Call language used by Layout Resources.
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

#ifndef RD_EVENTCALL_H
#define RD_EVENTCALL_H

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <variant>
#include <vector>

namespace rdui {
struct SourceWidgetArgument {};
struct CurrentEventArgument {};

using EventArgument = std::variant<std::int64_t, std::string, bool, SourceWidgetArgument, CurrentEventArgument>;

class EventCall {
public:
    explicit EventCall(std::string name, std::vector<EventArgument> arguments = {});

    const std::string& name() const { return mName; }
    const std::vector<EventArgument>& arguments() const { return mArguments; }

private:
    std::string mName;
    std::vector<EventArgument> mArguments;
};

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
} // namespace rdui
#endif // RD_EVENTCALL_H
