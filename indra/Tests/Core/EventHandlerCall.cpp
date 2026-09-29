/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/EventHandlerCall.h>
#include <cstddef>
#include <cstdint>
#include <gtest/gtest.h>
#include <iterator>
#include <string>
#include <string_view>
#include <variant>
#include "EventTypes.h"
#include "HTMLNames.h"

namespace {
using Core::CurrentEventArgument;
using Core::CurrentTargetArgument;
using Core::EventHandlerCallParseError;
using Core::EventHandlerCallParseResult;
using Core::parseEventHandlerCall;
using ::testing::Message;
} // namespace

TEST(EventHandlerCall, ParsesCallWithoutArguments) {
    const EventHandlerCallParseResult parsed = parseEventHandlerCall("press()");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "press");
    EXPECT_TRUE(parsed.call->arguments().empty());
}

TEST(EventHandlerCall, ParsesSignedIntegerArguments) {
    struct IntegerCase {
        const char* source;
        std::int64_t expected;
    };

    for (const IntegerCase& test :
        {IntegerCase {"selectLocale(+1)", std::int64_t {1}}, IntegerCase {"selectLocale(-1)", std::int64_t {-1}}}) {
        SCOPED_TRACE(Message() << "signed integer call: " << test.source);
        const EventHandlerCallParseResult parsed = parseEventHandlerCall(test.source);
        ASSERT_TRUE(parsed.ok());
        ASSERT_EQ(parsed.call->arguments().size(), std::size_t {1});

        const auto& argument = parsed.call->arguments().front();
        ASSERT_TRUE(std::holds_alternative<std::int64_t>(argument));
        EXPECT_EQ(std::get<std::int64_t>(argument), test.expected);
    }
}

TEST(EventHandlerCall, ParsesSupportedArgumentKinds) {
    const EventHandlerCallParseResult parsed = parseEventHandlerCall("inspect('settings', true, false, this, event)");
    ASSERT_TRUE(parsed.ok());
    const auto& arguments = parsed.call->arguments();
    ASSERT_EQ(arguments.size(), std::size_t {5});
    EXPECT_EQ(std::get<std::string>(arguments[0]), "settings");
    EXPECT_TRUE(std::get<bool>(arguments[1]));
    EXPECT_FALSE(std::get<bool>(arguments[2]));
    EXPECT_TRUE(std::holds_alternative<CurrentTargetArgument>(arguments[3]));
    EXPECT_TRUE(std::holds_alternative<CurrentEventArgument>(arguments[4]));
}

TEST(EventHandlerCall, ParsesCallWhitespace) {
    const EventHandlerCallParseResult parsed = parseEventHandlerCall("  open ( 'settings' , true )  ");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "open");
    ASSERT_EQ(parsed.call->arguments().size(), std::size_t {2});
    EXPECT_EQ(std::get<std::string>(parsed.call->arguments()[0]), "settings");
    EXPECT_TRUE(std::get<bool>(parsed.call->arguments()[1]));
}

TEST(EventHandlerCall, RejectsBareHandlerName) {
    const EventHandlerCallParseResult parsed = parseEventHandlerCall("press");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, EventHandlerCallParseError::CallRequired);
}

TEST(EventHandlerCall, RejectsNamesOutsideLowerCamelCase) {
    struct RejectionCase {
        const char* source;
        std::size_t errorOffset;
    };

    for (const RejectionCase& test : {RejectionCase {"Save()", 0}, RejectionCase {"save-profile()", 4}, RejectionCase {"save_profile()", 4},
             RejectionCase {"save.profile()", 4}}) {
        SCOPED_TRACE(Message() << "invalid handler call: " << test.source);
        const EventHandlerCallParseResult parsed = parseEventHandlerCall(test.source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventHandlerCallParseError::NameInvalid);
        EXPECT_EQ(parsed.errorOffset, test.errorOffset);
    }
}

TEST(EventHandlerCall, RejectsMalformedCallSyntax) {
    for (const char* source : {"press(true,)", "press(true false)", "press() close()", "press('open)", "press(1 + 2)"}) {
        SCOPED_TRACE(Message() << "malformed call: " << source);
        const EventHandlerCallParseResult parsed = parseEventHandlerCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventHandlerCallParseError::SyntaxInvalid);
    }
}

TEST(EventHandlerCall, RejectsUnsupportedArgumentForms) {
    for (const char* source : {"press(,)", "press(other)", "press(this.id)", "press(select(1))", "press(\"settings\")", "press('a\\'b')"}) {
        SCOPED_TRACE(Message() << "unsupported argument call: " << source);
        const EventHandlerCallParseResult parsed = parseEventHandlerCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventHandlerCallParseError::LiteralUnsupported);
    }
}

TEST(EventHandlerCall, RejectsOutOfRangeIntegerArguments) {
    const EventHandlerCallParseResult parsed = parseEventHandlerCall("select(9223372036854775808)");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, EventHandlerCallParseError::IntegerOutOfRange);
}

TEST(EventHandlerCall, UsesEventCatalogForAuthoredAttributes) {
    struct Expected {
        std::string_view eventType;
        Core::HTMLAttribute attribute;
    };
    constexpr Expected expected[] = {
        {"change", Core::HTMLAttribute::OnChange},
        {"click", Core::HTMLAttribute::OnClick},
        {"contextmenu", Core::HTMLAttribute::OnContextMenu},
        {"dblclick", Core::HTMLAttribute::OnDoubleClick},
        {"input", Core::HTMLAttribute::OnInput},
        {"mousedown", Core::HTMLAttribute::OnMouseDown},
        {"mousemove", Core::HTMLAttribute::OnMouseMove},
        {"mouseup", Core::HTMLAttribute::OnMouseUp},
        {"pointerdown", Core::HTMLAttribute::OnPointerDown},
        {"pointermove", Core::HTMLAttribute::OnPointerMove},
        {"pointerup", Core::HTMLAttribute::OnPointerUp},
        {"wheel", Core::HTMLAttribute::OnWheel},
    };

    std::size_t authoredEventCount = 0;
    for (const Core::EventTypeDescriptor& event : Core::eventTypes) {
        if (!event.htmlAttribute)
            continue;
        ASSERT_LT(authoredEventCount, std::size(expected));
        EXPECT_EQ(event.name, expected[authoredEventCount].eventType);
        EXPECT_EQ(*event.htmlAttribute, expected[authoredEventCount].attribute);
        ++authoredEventCount;
    }
    EXPECT_EQ(authoredEventCount, std::size(expected));
}
