/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstddef>
#include <cstdint>
#include <gtest/gtest.h>
#include <string>
#include <variant>
#include "eventcall.h"

namespace {
using radia::ui::CurrentEventArgument;
using radia::ui::EventCallParseError;
using radia::ui::EventCallParseResult;
using radia::ui::parseEventCall;
using radia::ui::SourceElementArgument;
using ::testing::Message;
}

TEST(EventCallTest, ParsesCallWithoutArguments) {
    const EventCallParseResult parsed = parseEventCall("press()");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "press");
    EXPECT_TRUE(parsed.call->arguments().empty());
}

TEST(EventCallTest, ParsesSignedIntegerArguments) {
    struct IntegerCase {
        const char* source;
        std::int64_t expected;
    };

    for (const IntegerCase& test : {IntegerCase{"selectLocale(+1)", std::int64_t{1}}, IntegerCase{"selectLocale(-1)", std::int64_t{-1}}}) {
        SCOPED_TRACE(Message() << "signed integer call: " << test.source);
        const EventCallParseResult parsed = parseEventCall(test.source);
        ASSERT_TRUE(parsed.ok());
        ASSERT_EQ(parsed.call->arguments().size(), std::size_t{1});

        const auto& argument = parsed.call->arguments().front();
        ASSERT_TRUE(std::holds_alternative<std::int64_t>(argument));
        EXPECT_EQ(std::get<std::int64_t>(argument), test.expected);
    }
}

TEST(EventCallTest, ParsesSupportedArgumentKinds) {
    const EventCallParseResult parsed = parseEventCall("inspect('settings', true, false, this, event)");
    ASSERT_TRUE(parsed.ok());
    const auto& arguments = parsed.call->arguments();
    ASSERT_EQ(arguments.size(), std::size_t{5});
    EXPECT_EQ(std::get<std::string>(arguments[0]), "settings");
    EXPECT_TRUE(std::get<bool>(arguments[1]));
    EXPECT_FALSE(std::get<bool>(arguments[2]));
    EXPECT_TRUE(std::holds_alternative<SourceElementArgument>(arguments[3]));
    EXPECT_TRUE(std::holds_alternative<CurrentEventArgument>(arguments[4]));
}

TEST(EventCallTest, ParsesWhitespaceAroundCallAndArguments) {
    const EventCallParseResult parsed = parseEventCall("  open ( 'settings' , true )  ");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "open");
    ASSERT_EQ(parsed.call->arguments().size(), std::size_t{2});
    EXPECT_EQ(std::get<std::string>(parsed.call->arguments()[0]), "settings");
    EXPECT_TRUE(std::get<bool>(parsed.call->arguments()[1]));
}

TEST(EventCallTest, RejectsBareHandlerName) {
    const EventCallParseResult parsed = parseEventCall("press");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, EventCallParseError::CallRequired);
}

TEST(EventCallTest, RejectsNamesOutsideLowerCamelCase) {
    struct RejectionCase {
        const char* source;
        std::size_t errorOffset;
    };

    for (const RejectionCase& test :
         {RejectionCase{"Save()", 0}, RejectionCase{"save-profile()", 4}, RejectionCase{"save_profile()", 4}, RejectionCase{"save.profile()", 4}}) {
        SCOPED_TRACE(Message() << "invalid handler call: " << test.source);
        const EventCallParseResult parsed = parseEventCall(test.source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventCallParseError::NameInvalid);
        EXPECT_EQ(parsed.errorOffset, test.errorOffset);
    }
}

TEST(EventCallTest, RejectsMalformedCallSyntax) {
    for (const char* source : {"press(true,)", "press(true false)", "press() close()", "press('open)", "press(1 + 2)"}) {
        SCOPED_TRACE(Message() << "malformed call: " << source);
        const EventCallParseResult parsed = parseEventCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventCallParseError::SyntaxInvalid);
    }
}

TEST(EventCallTest, RejectsUnsupportedArgumentForms) {
    for (const char* source : {"press(,)", "press(other)", "press(this.id)", "press(select(1))", "press(\"settings\")", "press('a\\'b')"}) {
        SCOPED_TRACE(Message() << "unsupported argument call: " << source);
        const EventCallParseResult parsed = parseEventCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, EventCallParseError::LiteralUnsupported);
    }
}

TEST(EventCallTest, RejectsOutOfRangeIntegerArguments) {
    const EventCallParseResult parsed = parseEventCall("select(9223372036854775808)");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, EventCallParseError::IntegerOutOfRange);
}
