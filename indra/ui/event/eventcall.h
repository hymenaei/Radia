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
#include "event/event.h"

namespace radia::ui {
struct SourceElementArgument {};
struct CurrentAuthoredEventArgument {};

using AuthoredEventArgument = std::variant<std::int64_t, std::string, bool, SourceElementArgument, CurrentAuthoredEventArgument>;

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

class AuthoredEventCall {
public:
    explicit AuthoredEventCall(std::string name, std::vector<AuthoredEventArgument> arguments = {});

    const std::string& name() const { return mName; }
    const std::vector<AuthoredEventArgument>& arguments() const { return mArguments; }

private:
    std::string mName;
    std::vector<AuthoredEventArgument> mArguments;
};

class Element;
Element& setAuthoredEventCall(Element& element, std::string_view type, AuthoredEventCall call);
const AuthoredEventCall* authoredEventCall(const Element& element, std::string_view type);

enum class AuthoredEventCallParseError : std::uint8_t { NoError, CallRequired, NameInvalid, SyntaxInvalid, LiteralUnsupported, IntegerOutOfRange };

struct AuthoredEventCallParseResult {
    std::optional<AuthoredEventCall> call;
    AuthoredEventCallParseError error = AuthoredEventCallParseError::NoError;
    std::size_t errorOffset = 0;

    bool ok() const { return call.has_value(); }
};

AuthoredEventCallParseResult parseAuthoredEventCall(std::string_view source);
bool isEventHandlerName(std::string_view value);
const char* authoredEventCallParseErrorCode(AuthoredEventCallParseError error);
const char* authoredEventCallParseErrorMessage(AuthoredEventCallParseError error);
} // namespace radia::ui
