/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstddef>
#include <cstdint>
#include <gtest/gtest.h>
#include <iterator>
#include <string>
#include <variant>
#include "event/eventcall.h"

namespace {
using radia::ui::AuthoredEventCallParseError;
using radia::ui::AuthoredEventCallParseResult;
using radia::ui::CurrentAuthoredEventArgument;
using radia::ui::kAuthoredEventDescriptors;
using radia::ui::parseAuthoredEventCall;
using radia::ui::SourceElementArgument;
using ::testing::Message;
}

TEST(AuthoredEventCallTest, ParsesCallWithoutArguments) {
    const AuthoredEventCallParseResult parsed = parseAuthoredEventCall("press()");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "press");
    EXPECT_TRUE(parsed.call->arguments().empty());
}

TEST(AuthoredEventCallTest, ParsesSignedIntegerArguments) {
    struct IntegerCase {
        const char* source;
        std::int64_t expected;
    };

    for (const IntegerCase& test : {IntegerCase{"selectLocale(+1)", std::int64_t{1}}, IntegerCase{"selectLocale(-1)", std::int64_t{-1}}}) {
        SCOPED_TRACE(Message() << "signed integer call: " << test.source);
        const AuthoredEventCallParseResult parsed = parseAuthoredEventCall(test.source);
        ASSERT_TRUE(parsed.ok());
        ASSERT_EQ(parsed.call->arguments().size(), std::size_t{1});

        const auto& argument = parsed.call->arguments().front();
        ASSERT_TRUE(std::holds_alternative<std::int64_t>(argument));
        EXPECT_EQ(std::get<std::int64_t>(argument), test.expected);
    }
}

TEST(AuthoredEventCallTest, ParsesSupportedArgumentKinds) {
    const AuthoredEventCallParseResult parsed = parseAuthoredEventCall("inspect('settings', true, false, this, event)");
    ASSERT_TRUE(parsed.ok());
    const auto& arguments = parsed.call->arguments();
    ASSERT_EQ(arguments.size(), std::size_t{5});
    EXPECT_EQ(std::get<std::string>(arguments[0]), "settings");
    EXPECT_TRUE(std::get<bool>(arguments[1]));
    EXPECT_FALSE(std::get<bool>(arguments[2]));
    EXPECT_TRUE(std::holds_alternative<SourceElementArgument>(arguments[3]));
    EXPECT_TRUE(std::holds_alternative<CurrentAuthoredEventArgument>(arguments[4]));
}

TEST(AuthoredEventCallTest, ParsesCallWhitespace) {
    const AuthoredEventCallParseResult parsed = parseAuthoredEventCall("  open ( 'settings' , true )  ");
    ASSERT_TRUE(parsed.ok());
    EXPECT_EQ(parsed.call->name(), "open");
    ASSERT_EQ(parsed.call->arguments().size(), std::size_t{2});
    EXPECT_EQ(std::get<std::string>(parsed.call->arguments()[0]), "settings");
    EXPECT_TRUE(std::get<bool>(parsed.call->arguments()[1]));
}

TEST(AuthoredEventCallTest, RejectsBareHandlerName) {
    const AuthoredEventCallParseResult parsed = parseAuthoredEventCall("press");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, AuthoredEventCallParseError::CallRequired);
}

TEST(AuthoredEventCallTest, RejectsNamesOutsideLowerCamelCase) {
    struct RejectionCase {
        const char* source;
        std::size_t errorOffset;
    };

    for (const RejectionCase& test :
         {RejectionCase{"Save()", 0}, RejectionCase{"save-profile()", 4}, RejectionCase{"save_profile()", 4}, RejectionCase{"save.profile()", 4}}) {
        SCOPED_TRACE(Message() << "invalid handler call: " << test.source);
        const AuthoredEventCallParseResult parsed = parseAuthoredEventCall(test.source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, AuthoredEventCallParseError::NameInvalid);
        EXPECT_EQ(parsed.errorOffset, test.errorOffset);
    }
}

TEST(AuthoredEventCallTest, RejectsMalformedCallSyntax) {
    for (const char* source : {"press(true,)", "press(true false)", "press() close()", "press('open)", "press(1 + 2)"}) {
        SCOPED_TRACE(Message() << "malformed call: " << source);
        const AuthoredEventCallParseResult parsed = parseAuthoredEventCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, AuthoredEventCallParseError::SyntaxInvalid);
    }
}

TEST(AuthoredEventCallTest, RejectsUnsupportedArgumentForms) {
    for (const char* source : {"press(,)", "press(other)", "press(this.id)", "press(select(1))", "press(\"settings\")", "press('a\\'b')"}) {
        SCOPED_TRACE(Message() << "unsupported argument call: " << source);
        const AuthoredEventCallParseResult parsed = parseAuthoredEventCall(source);
        EXPECT_FALSE(parsed.ok());
        EXPECT_EQ(parsed.error, AuthoredEventCallParseError::LiteralUnsupported);
    }
}

TEST(AuthoredEventCallTest, RejectsOutOfRangeIntegerArguments) {
    const AuthoredEventCallParseResult parsed = parseAuthoredEventCall("select(9223372036854775808)");
    EXPECT_FALSE(parsed.ok());
    EXPECT_EQ(parsed.error, AuthoredEventCallParseError::IntegerOutOfRange);
}

TEST(AuthoredEventCallTest, CoversAuthoredEventDescriptors) {
    constexpr radia::ui::AuthoredEventDescriptor expected[] = {
        {"onClick", radia::ui::kClickEvent},
        {"onDoubleClick", radia::ui::kDoubleClickEvent},
        {"onInput", radia::ui::kInputEvent},
        {"onChange", radia::ui::kChangeEvent},
        {"onPointerDown", radia::ui::kPointerDownEvent},
        {"onPointerUp", radia::ui::kPointerUpEvent},
        {"onPointerMove", radia::ui::kPointerMoveEvent},
        {"onContextMenu", radia::ui::kContextMenuEvent},
        {"onWheel", radia::ui::kWheelEvent},
    };

    ASSERT_EQ(std::size(kAuthoredEventDescriptors), std::size(expected));
    for (std::size_t index = 0; index < std::size(expected); ++index) {
        EXPECT_EQ(kAuthoredEventDescriptors[index].attribute, expected[index].attribute);
        EXPECT_EQ(kAuthoredEventDescriptors[index].type, expected[index].type);
    }
}
