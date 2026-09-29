/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "EventHandlerCall.h"
#include <charconv>
#include <limits>
#include <memory>
#include "Element.h"
#include "EventHandlerCallInternal.h"

namespace Core {
EventHandlerCall::EventHandlerCall(std::string name, std::vector<EventCallArgument> arguments)
    : mName(std::move(name))
    , mArguments(std::move(arguments)) {}

namespace detail {
void EventHandlerCallStore::set(Element& element, std::string_view type, EventHandlerCall call) {
    if (!element.mEventHandlerCallStore)
        element.mEventHandlerCallStore = std::make_unique<EventHandlerCallStore>();
    element.mEventHandlerCallStore->mCalls.insert_or_assign(std::string(type), std::move(call));
}

const EventHandlerCall* EventHandlerCallStore::find(const Element& element, std::string_view type) {
    if (!element.mEventHandlerCallStore)
        return nullptr;
    const auto found = element.mEventHandlerCallStore->mCalls.find(type);
    return found == element.mEventHandlerCallStore->mCalls.end() ? nullptr : &found->second;
}
} // namespace detail

Element& setEventHandlerCall(Element& element, std::string_view type, EventHandlerCall call) {
    detail::EventHandlerCallStore::set(element, type, std::move(call));
    return element;
}

const EventHandlerCall* eventHandlerCall(const Element& element, std::string_view type) {
    return detail::EventHandlerCallStore::find(element, type);
}

namespace {
bool isLowercaseAscii(char value) { return value >= 'a' && value <= 'z'; }

bool isAsciiAlpha(char value) { return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z'); }

bool isAsciiDigit(char value) { return value >= '0' && value <= '9'; }

bool isNameContinuation(char value) { return isAsciiAlpha(value) || isAsciiDigit(value); }

bool isArgumentBoundary(char value) {
    return value == ',' || value == ')' || value == ' ' || value == '\t' || value == '\r' || value == '\n';
}

class EventHandlerCallParser {
public:
    explicit EventHandlerCallParser(std::string_view source)
        : mSource(source) {}

    EventHandlerCallParseResult parse() {
        skipWhitespace();
        const std::size_t nameStart = mOffset;
        if (atEnd())
            return failure(EventHandlerCallParseError::NameInvalid, nameStart);
        if (!isLowercaseAscii(current()))
            return failure(EventHandlerCallParseError::NameInvalid, nameStart);

        ++mOffset;
        while (!atEnd() && isNameContinuation(current()))
            ++mOffset;
        const std::string name(mSource.substr(nameStart, mOffset - nameStart));

        if (!atEnd() && !isWhitespace(current()) && current() != '(')
            return failure(EventHandlerCallParseError::NameInvalid, mOffset);

        skipWhitespace();
        if (atEnd())
            return failure(EventHandlerCallParseError::CallRequired, mOffset);
        if (current() != '(')
            return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);
        ++mOffset;

        std::vector<EventCallArgument> arguments;
        skipWhitespace();
        if (!atEnd() && current() == ')') {
            ++mOffset;
            return finish(std::move(name), std::move(arguments));
        }

        while (true) {
            EventHandlerCallParseResult argumentResult = parseArgument(arguments);
            if (argumentResult.error != EventHandlerCallParseError::NoError)
                return argumentResult;

            skipWhitespace();
            if (atEnd())
                return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);
            if (current() == ')') {
                ++mOffset;
                return finish(std::move(name), std::move(arguments));
            }
            if (current() != ',')
                return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);
            ++mOffset;
            skipWhitespace();
            if (atEnd() || current() == ')')
                return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);
        }
    }

private:
    EventHandlerCallParseResult parseArgument(std::vector<EventCallArgument>& arguments) {
        skipWhitespace();
        if (atEnd())
            return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);

        if (current() == '\'')
            return parseString(arguments);
        if (current() == '+' || current() == '-' || isAsciiDigit(current()))
            return parseInteger(arguments);
        if (isAsciiAlpha(current()))
            return parseWord(arguments);
        return failure(EventHandlerCallParseError::LiteralUnsupported, mOffset);
    }

    EventHandlerCallParseResult parseString(std::vector<EventCallArgument>& arguments) {
        const std::size_t quote = mOffset++;
        const std::size_t begin = mOffset;
        while (!atEnd() && current() != '\'') {
            if (current() == '\\')
                return failure(EventHandlerCallParseError::LiteralUnsupported, mOffset);
            ++mOffset;
        }
        if (atEnd())
            return failure(EventHandlerCallParseError::SyntaxInvalid, quote);
        arguments.emplace_back(std::string(mSource.substr(begin, mOffset - begin)));
        ++mOffset;
        return {};
    }

    EventHandlerCallParseResult parseInteger(std::vector<EventCallArgument>& arguments) {
        const std::size_t begin = mOffset;
        bool positiveSign = false;
        if (current() == '+' || current() == '-') {
            positiveSign = current() == '+';
            ++mOffset;
        }
        const std::size_t digits = mOffset;
        while (!atEnd() && isAsciiDigit(current()))
            ++mOffset;
        if (digits == mOffset)
            return failure(EventHandlerCallParseError::LiteralUnsupported, begin);
        if (!atEnd() && !isArgumentBoundary(current()))
            return failure(EventHandlerCallParseError::LiteralUnsupported, mOffset);

        const std::string_view token =
            positiveSign ? mSource.substr(begin + 1, mOffset - begin - 1) : mSource.substr(begin, mOffset - begin);
        std::int64_t value = 0;
        const auto converted = std::from_chars(token.data(), token.data() + token.size(), value);
        if (converted.ec == std::errc::result_out_of_range)
            return failure(EventHandlerCallParseError::IntegerOutOfRange, begin);
        if (converted.ec != std::errc() || converted.ptr != token.data() + token.size())
            return failure(EventHandlerCallParseError::LiteralUnsupported, begin);
        arguments.emplace_back(value);
        return {};
    }

    EventHandlerCallParseResult parseWord(std::vector<EventCallArgument>& arguments) {
        const std::size_t begin = mOffset;
        while (!atEnd() && isNameContinuation(current()))
            ++mOffset;
        const std::string_view word = mSource.substr(begin, mOffset - begin);
        if (!atEnd() && !isArgumentBoundary(current()))
            return failure(EventHandlerCallParseError::LiteralUnsupported, begin);

        if (word == "true")
            arguments.emplace_back(true);
        else if (word == "false")
            arguments.emplace_back(false);
        else if (word == "this")
            arguments.emplace_back(CurrentTargetArgument {});
        else if (word == "event")
            arguments.emplace_back(CurrentEventArgument {});
        else
            return failure(EventHandlerCallParseError::LiteralUnsupported, begin);
        return {};
    }

    EventHandlerCallParseResult finish(std::string name, std::vector<EventCallArgument> arguments) {
        skipWhitespace();
        if (!atEnd())
            return failure(EventHandlerCallParseError::SyntaxInvalid, mOffset);
        EventHandlerCallParseResult result;
        result.call.emplace(std::move(name), std::move(arguments));
        return result;
    }

    EventHandlerCallParseResult failure(EventHandlerCallParseError error, std::size_t errorOffset) const {
        EventHandlerCallParseResult result;
        result.error = error;
        result.errorOffset = errorOffset;
        return result;
    }

    static bool isWhitespace(char value) { return value == ' ' || value == '\t' || value == '\r' || value == '\n'; }

    void skipWhitespace() {
        while (!atEnd() && isWhitespace(current()))
            ++mOffset;
    }

    bool atEnd() const { return mOffset == mSource.size(); }
    char current() const { return mSource[mOffset]; }

    std::string_view mSource;
    std::size_t mOffset = 0;
};
} // namespace

bool isEventHandlerName(std::string_view value) {
    if (value.empty() || !isLowercaseAscii(value.front()))
        return false;
    for (std::size_t index = 1; index < value.size(); ++index)
        if (!isNameContinuation(value[index]))
            return false;
    return true;
}

EventHandlerCallParseResult parseEventHandlerCall(std::string_view source) { return EventHandlerCallParser(source).parse(); }

const char* eventHandlerCallParseErrorCode(EventHandlerCallParseError error) {
    switch (error) {
    case EventHandlerCallParseError::NoError:
        return "";
    case EventHandlerCallParseError::CallRequired:
        return "layout.event.call_required";
    case EventHandlerCallParseError::NameInvalid:
        return "layout.event.name_invalid";
    case EventHandlerCallParseError::SyntaxInvalid:
        return "layout.event.syntax_invalid";
    case EventHandlerCallParseError::LiteralUnsupported:
        return "layout.event.literal_unsupported";
    case EventHandlerCallParseError::IntegerOutOfRange:
        return "layout.event.integer_out_of_range";
    }
    return "layout.event.syntax_invalid";
}

const char* eventHandlerCallParseErrorMessage(EventHandlerCallParseError error) {
    switch (error) {
    case EventHandlerCallParseError::NoError:
        return "";
    case EventHandlerCallParseError::CallRequired:
        return "Event Handler Calls require parentheses.";
    case EventHandlerCallParseError::NameInvalid:
        return "Event Handler names must use lower-camel-case.";
    case EventHandlerCallParseError::SyntaxInvalid:
        return "Invalid Event Handler Call syntax.";
    case EventHandlerCallParseError::LiteralUnsupported:
        return "Event Handler arguments support only integers, single-quoted strings, booleans, this, and event.";
    case EventHandlerCallParseError::IntegerOutOfRange:
        return "Event Handler integer argument is outside the signed 64-bit range.";
    }
    return "Invalid Event Handler Call syntax.";
}
} // namespace Core
