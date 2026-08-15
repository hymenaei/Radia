/**
 * @file eventcall.cpp
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

#include "linden_common.h"
#include "eventcall.h"
#include <charconv>
#include <limits>

namespace radia::ui {
EventCall::EventCall(std::string name, std::vector<EventArgument> arguments) : mName(std::move(name)), mArguments(std::move(arguments)) {}

namespace {
bool isLowercaseAscii(char value) {
    return value >= 'a' && value <= 'z';
}

bool isAsciiAlpha(char value) {
    return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z');
}

bool isAsciiDigit(char value) {
    return value >= '0' && value <= '9';
}

bool isNameContinuation(char value) {
    return isAsciiAlpha(value) || isAsciiDigit(value);
}

bool isArgumentBoundary(char value) {
    return value == ',' || value == ')' || value == ' ' || value == '\t' || value == '\r' || value == '\n';
}

class EventCallParser {
public:
    explicit EventCallParser(std::string_view source) : mSource(source) {}

    EventCallParseResult parse() {
        skipWhitespace();
        const std::size_t nameStart = mOffset;
        if (atEnd()) return failure(EventCallParseError::NameInvalid, nameStart);
        if (!isLowercaseAscii(current())) return failure(EventCallParseError::NameInvalid, nameStart);

        ++mOffset;
        while (!atEnd() && isNameContinuation(current())) ++mOffset;
        const std::string name(mSource.substr(nameStart, mOffset - nameStart));

        if (!atEnd() && !isWhitespace(current()) && current() != '(') return failure(EventCallParseError::NameInvalid, mOffset);

        skipWhitespace();
        if (atEnd()) return failure(EventCallParseError::CallRequired, mOffset);
        if (current() != '(') return failure(EventCallParseError::SyntaxInvalid, mOffset);
        ++mOffset;

        std::vector<EventArgument> arguments;
        skipWhitespace();
        if (!atEnd() && current() == ')') {
            ++mOffset;
            return finish(std::move(name), std::move(arguments));
        }

        while (true) {
            EventCallParseResult argumentResult = parseArgument(arguments);
            if (argumentResult.error != EventCallParseError::NoError) return argumentResult;

            skipWhitespace();
            if (atEnd()) return failure(EventCallParseError::SyntaxInvalid, mOffset);
            if (current() == ')') {
                ++mOffset;
                return finish(std::move(name), std::move(arguments));
            }
            if (current() != ',') return failure(EventCallParseError::SyntaxInvalid, mOffset);
            ++mOffset;
            skipWhitespace();
            if (atEnd() || current() == ')') return failure(EventCallParseError::SyntaxInvalid, mOffset);
        }
    }

private:
    EventCallParseResult parseArgument(std::vector<EventArgument>& arguments) {
        skipWhitespace();
        if (atEnd()) return failure(EventCallParseError::SyntaxInvalid, mOffset);

        if (current() == '\'') return parseString(arguments);
        if (current() == '+' || current() == '-' || isAsciiDigit(current())) return parseInteger(arguments);
        if (isAsciiAlpha(current())) return parseWord(arguments);
        return failure(EventCallParseError::LiteralUnsupported, mOffset);
    }

    EventCallParseResult parseString(std::vector<EventArgument>& arguments) {
        const std::size_t quote = mOffset++;
        const std::size_t begin = mOffset;
        while (!atEnd() && current() != '\'') {
            if (current() == '\\') return failure(EventCallParseError::LiteralUnsupported, mOffset);
            ++mOffset;
        }
        if (atEnd()) return failure(EventCallParseError::SyntaxInvalid, quote);
        arguments.emplace_back(std::string(mSource.substr(begin, mOffset - begin)));
        ++mOffset;
        return {};
    }

    EventCallParseResult parseInteger(std::vector<EventArgument>& arguments) {
        const std::size_t begin = mOffset;
        bool positiveSign = false;
        if (current() == '+' || current() == '-') {
            positiveSign = current() == '+';
            ++mOffset;
        }
        const std::size_t digits = mOffset;
        while (!atEnd() && isAsciiDigit(current())) ++mOffset;
        if (digits == mOffset) return failure(EventCallParseError::LiteralUnsupported, begin);
        if (!atEnd() && !isArgumentBoundary(current())) return failure(EventCallParseError::LiteralUnsupported, mOffset);

        const std::string_view token = positiveSign ? mSource.substr(begin + 1, mOffset - begin - 1) : mSource.substr(begin, mOffset - begin);
        std::int64_t value = 0;
        const auto converted = std::from_chars(token.data(), token.data() + token.size(), value);
        if (converted.ec == std::errc::result_out_of_range) return failure(EventCallParseError::IntegerOutOfRange, begin);
        if (converted.ec != std::errc() || converted.ptr != token.data() + token.size())
            return failure(EventCallParseError::LiteralUnsupported, begin);
        arguments.emplace_back(value);
        return {};
    }

    EventCallParseResult parseWord(std::vector<EventArgument>& arguments) {
        const std::size_t begin = mOffset;
        while (!atEnd() && isNameContinuation(current())) ++mOffset;
        const std::string_view word = mSource.substr(begin, mOffset - begin);
        if (!atEnd() && !isArgumentBoundary(current())) return failure(EventCallParseError::LiteralUnsupported, begin);

        if (word == "true") arguments.emplace_back(true);
        else if (word == "false") arguments.emplace_back(false);
        else if (word == "this") arguments.emplace_back(SourceWidgetArgument{});
        else if (word == "event") arguments.emplace_back(CurrentEventArgument{});
        else return failure(EventCallParseError::LiteralUnsupported, begin);
        return {};
    }

    EventCallParseResult finish(std::string name, std::vector<EventArgument> arguments) {
        skipWhitespace();
        if (!atEnd()) return failure(EventCallParseError::SyntaxInvalid, mOffset);
        EventCallParseResult result;
        result.call.emplace(std::move(name), std::move(arguments));
        return result;
    }

    EventCallParseResult failure(EventCallParseError error, std::size_t errorOffset) const {
        EventCallParseResult result;
        result.error = error;
        result.errorOffset = errorOffset;
        return result;
    }

    static bool isWhitespace(char value) { return value == ' ' || value == '\t' || value == '\r' || value == '\n'; }

    void skipWhitespace() {
        while (!atEnd() && isWhitespace(current())) ++mOffset;
    }

    bool atEnd() const { return mOffset == mSource.size(); }
    char current() const { return mSource[mOffset]; }

    std::string_view mSource;
    std::size_t mOffset = 0;
};
} // namespace

bool isEventHandlerName(std::string_view value) {
    if (value.empty() || !isLowercaseAscii(value.front())) return false;
    for (std::size_t index = 1; index < value.size(); ++index)
        if (!isNameContinuation(value[index])) return false;
    return true;
}

EventCallParseResult parseEventCall(std::string_view source) {
    return EventCallParser(source).parse();
}

const char* eventCallParseErrorCode(EventCallParseError error) {
    switch (error) {
        case EventCallParseError::NoError: return "";
        case EventCallParseError::CallRequired: return "layout.event.call_required";
        case EventCallParseError::NameInvalid: return "layout.event.name_invalid";
        case EventCallParseError::SyntaxInvalid: return "layout.event.syntax_invalid";
        case EventCallParseError::LiteralUnsupported: return "layout.event.literal_unsupported";
        case EventCallParseError::IntegerOutOfRange: return "layout.event.integer_out_of_range";
    }
    return "layout.event.syntax_invalid";
}

const char* eventCallParseErrorMessage(EventCallParseError error) {
    switch (error) {
        case EventCallParseError::NoError: return "";
        case EventCallParseError::CallRequired: return "Event Handler Calls require parentheses.";
        case EventCallParseError::NameInvalid: return "Event Handler names must use lower-camel-case.";
        case EventCallParseError::SyntaxInvalid: return "Invalid Event Handler Call syntax.";
        case EventCallParseError::LiteralUnsupported:
            return "Event Handler arguments support only integers, single-quoted strings, booleans, this, and event.";
        case EventCallParseError::IntegerOutOfRange: return "Event Handler integer argument is outside the signed 64-bit range.";
    }
    return "Invalid Event Handler Call syntax.";
}
} // namespace radia::ui
