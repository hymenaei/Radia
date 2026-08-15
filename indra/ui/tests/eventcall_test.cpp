/**
 * @file eventcall_test.cpp
 * @brief Tests the restricted Event Handler Call language used by Layout Resources.
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
#include "../test/lltut.h"
#include "eventcall.h"

namespace tut {
struct eventCallData {};
using eventCallTest = test_group<eventCallData>;
using eventCallObject = eventCallTest::object;
eventCallTest eventCallTestCase("eventcall");

template<> template<> void eventCallObject::test<1>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("press()");
    ensure("zero-argument call parses", parsed.ok());
    ensure_equals("handler name retained", parsed.call->name(), std::string("press"));
    ensure("zero arguments retained", parsed.call->arguments().empty());
}

template<> template<> void eventCallObject::test<2>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("selectLocale(+1)");
    ensure("one-argument call parses", parsed.ok());
    ensure_equals("one argument retained", parsed.call->arguments().size(), 1U);
    ensure_equals("positive sign parsed", std::get<std::int64_t>(parsed.call->arguments()[0]), std::int64_t(1));
}

template<> template<> void eventCallObject::test<3>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("inspect('settings', true, false, this, event)");
    ensure("all closed literal kinds parse", parsed.ok());
    const auto& arguments = parsed.call->arguments();
    ensure_equals("five arguments retained", arguments.size(), 5U);
    ensure_equals("string retained", std::get<std::string>(arguments[0]), std::string("settings"));
    ensure("true retained", std::get<bool>(arguments[1]));
    ensure("false retained", !std::get<bool>(arguments[2]));
    ensure("this has a distinct type", std::holds_alternative<radia::ui::SourceWidgetArgument>(arguments[3]));
    ensure("event has a distinct type", std::holds_alternative<radia::ui::CurrentEventArgument>(arguments[4]));
}

template<> template<> void eventCallObject::test<4>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("  open ( 'settings' , true )  ");
    ensure("surrounding argument whitespace parses", parsed.ok());
    ensure_equals("whitespace does not alter the name", parsed.call->name(), std::string("open"));
}

template<> template<> void eventCallObject::test<5>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("press");
    ensure("bare Handler name is rejected", !parsed.ok());
    ensure_equals("bare Handler reports required call", static_cast<int>(parsed.error), static_cast<int>(radia::ui::EventCallParseError::CallRequired));
}

template<> template<> void eventCallObject::test<6>() {
    for (const char* source : {"Save()", "save-profile()", "save_profile()", "save.profile()"}) {
        const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall(source);
        ensure(std::string("invalid Handler name is rejected: ") + source, !parsed.ok());
        ensure_equals("invalid Handler name has a stable error", static_cast<int>(parsed.error),
                      static_cast<int>(radia::ui::EventCallParseError::NameInvalid));
    }
}

template<> template<> void eventCallObject::test<7>() {
    for (const char* source : {"press(,)", "press(true,)", "press(true false)", "press() close()", "press('open)"}) {
        const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall(source);
        ensure(std::string("invalid call syntax is rejected: ") + source, !parsed.ok());
    }
}

template<> template<> void eventCallObject::test<8>() {
    for (const char* source : {"press(other)", "press(this.id)", "press(select(1))", "press(1 + 2)", "press(\"settings\")", "press('a\\'b')"}) {
        const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall(source);
        ensure(std::string("expression or unsupported literal is rejected: ") + source, !parsed.ok());
    }
}

template<> template<> void eventCallObject::test<9>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("select(9223372036854775808)");
    ensure("out-of-range integer is rejected", !parsed.ok());
    ensure_equals("integer range has a stable error", static_cast<int>(parsed.error), static_cast<int>(radia::ui::EventCallParseError::IntegerOutOfRange));
}

template<> template<> void eventCallObject::test<10>() {
    const radia::ui::EventCallParseResult parsed = radia::ui::parseEventCall("selectLocale(-1)");
    ensure("negative integer parses", parsed.ok());
    ensure_equals("negative integer retained", std::get<std::int64_t>(parsed.call->arguments()[0]), std::int64_t(-1));
}
} // namespace tut
