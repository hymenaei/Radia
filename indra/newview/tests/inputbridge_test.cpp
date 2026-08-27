/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstdint>
#include <gtest/gtest.h>
#include "indra_constants.h"
#include "inputbridge.h"
#include "llkeyboard.h"

namespace {
using radia::ui::CursorStyle;
using radia::ui::KeyEvent;
using radia::ui::kKeyReturn;
using radia::ui::kModifierAlt;
using radia::ui::kModifierControl;
using radia::ui::kModifierPlatformControl;
using radia::ui::kModifierShift;
using radia::ui::PointerButton;
using radia::ui::PointerEvent;
using radia::ui::WheelEvent;
using radia::viewer::ui::NativeKeyInput;
using radia::viewer::ui::NativePointerButton;
using radia::viewer::ui::NativePointerInput;
using radia::viewer::ui::NativeScrollInput;
using radia::viewer::ui::translateCursor;
using radia::viewer::ui::translateKeyInput;
using radia::viewer::ui::translatePointerInput;
using radia::viewer::ui::translateScrollInput;
} // namespace

TEST(InputBridgeTest, TranslatesPointerCoordinatesButtonModifiersAndDeltas) {
    const NativePointerInput input{
        12.5f, 34.25f, NativePointerButton::Auxiliary1, MASK_SHIFT | MASK_ALT, 2, 3.5f, -4.f,
    };

    const PointerEvent translated = translatePointerInput(input);
    EXPECT_FLOAT_EQ(translated.position.x, 12.5f);
    EXPECT_FLOAT_EQ(translated.position.y, 34.25f);
    EXPECT_EQ(translated.button, PointerButton::Auxiliary1);
    EXPECT_EQ(translated.modifiers, kModifierShift | kModifierAlt);
    EXPECT_EQ(translated.clickCount, std::uint8_t{2});
    EXPECT_FLOAT_EQ(translated.delta.x, 3.5f);
    EXPECT_FLOAT_EQ(translated.delta.y, -4.f);
}

TEST(InputBridgeTest, TranslatesKeyAndPlatformModifiers) {
    const NativeKeyInput input{
        KEY_PAD_RETURN,
        MASK_CONTROL | MASK_MAC_CONTROL,
        true,
    };

    const KeyEvent translated = translateKeyInput(input);
    EXPECT_EQ(translated.key, kKeyReturn);
    EXPECT_EQ(translated.modifiers, kModifierControl | kModifierPlatformControl);
    EXPECT_TRUE(translated.repeated);
}

TEST(InputBridgeTest, NormalizesWheelDeltasToDomPixels) {
    const WheelEvent scroll = translateScrollInput({8, 9, -1.f, 2.f, MASK_CONTROL});
    EXPECT_FLOAT_EQ(scroll.dx, -40.f);
    EXPECT_FLOAT_EQ(scroll.dy, 80.f);
    EXPECT_EQ(scroll.modifiers, kModifierControl);
}

TEST(InputBridgeTest, MapsCursorStylesToNativeCursors) {
    EXPECT_EQ(translateCursor(CursorStyle::Pointer), UI_CURSOR_HAND);
    EXPECT_EQ(translateCursor(CursorStyle::EastWestResize), UI_CURSOR_SIZEWE);
    EXPECT_EQ(translateCursor(CursorStyle::Auto), UI_CURSOR_ARROW);
}
