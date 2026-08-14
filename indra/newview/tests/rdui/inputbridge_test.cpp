/**
 * @file inputbridge_test.cpp
 * @brief Tests translation of native input events and cursor styles to UI surface input.
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
#include "indra_constants.h"
#include "inputbridge.h"
#include "llkeyboard.h"

namespace tut {
struct inputBridgeData {};
using inputBridgeTest = test_group<inputBridgeData>;
using inputBridgeObject = inputBridgeTest::object;
inputBridgeTest inputBridgeTestCase("NativeInputTranslation");

template<> template<> void inputBridgeObject::test<1>() {
    set_test_name("pointer input translates once into UI coordinates and flags");
    const rdui::viewer::NativePointerInput input{
        12.5f, 34.25f, rdui::viewer::NativePointerButton::Auxiliary1, MASK_SHIFT | MASK_ALT, 2, 3.5f, -4.f,
    };

    const rdui::PointerEvent translated = rdui::viewer::translatePointerInput(input);
    ensure_equals("fractional x remains canonical", translated.position.x, 12.5f);
    ensure_equals("fractional y remains canonical", translated.position.y, 34.25f);
    ensure("button translated", translated.button == rdui::PointerButton::Auxiliary1);
    ensure_equals("modifiers translated", translated.modifiers, rdui::MODIFIER_SHIFT | rdui::MODIFIER_ALT);
    ensure_equals("click count preserved", translated.clickCount, std::uint8_t{2});
    ensure_equals("x delta preserved", translated.delta.x, 3.5f);
    ensure_equals("y delta preserved", translated.delta.y, -4.f);
}

template<> template<> void inputBridgeObject::test<2>() {
    set_test_name("viewer keys and modifiers translate to UI meanings");
    const rdui::viewer::NativeKeyInput input{
        KEY_PAD_RETURN,
        MASK_CONTROL | MASK_MAC_CONTROL,
        false,
        true,
    };

    const rdui::KeyEvent translated = rdui::viewer::translateKeyInput(input);
    ensure_equals("key translated", translated.key, rdui::KEY_RETURN);
    ensure_equals("modifiers translated", translated.modifiers, rdui::MODIFIER_CONTROL | rdui::MODIFIER_PLATFORM_CONTROL);
    ensure("repeat preserved", translated.repeated);
}

template<> template<> void inputBridgeObject::test<3>() {
    set_test_name("scroll input retains its semantics");
    const auto scroll = rdui::viewer::translateScrollInput({8, 9, -1.f, 2.f, MASK_CONTROL});
    ensure_equals("horizontal delta preserved", scroll.dx, -1.f);
    ensure_equals("vertical delta preserved", scroll.dy, 2.f);
    ensure_equals("scroll modifiers translated", scroll.modifiers, rdui::MODIFIER_CONTROL);
}

template<> template<> void inputBridgeObject::test<4>() {
    set_test_name("cursor mapping is shared by main and detached windows");
    ensure("pointer cursor", rdui::viewer::translateCursor(rdui::CursorStyle::Pointer) == UI_CURSOR_HAND);
    ensure("horizontal resize cursor", rdui::viewer::translateCursor(rdui::CursorStyle::EastWestResize) == UI_CURSOR_SIZEWE);
    ensure("fallback cursor", rdui::viewer::translateCursor(rdui::CursorStyle::Auto) == UI_CURSOR_ARROW);
}
} // namespace tut
