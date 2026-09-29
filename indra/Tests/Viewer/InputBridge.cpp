/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Viewer/InputBridge.h>
#include <cstdint>
#include <gtest/gtest.h>
#include <optional>
#include "indra_constants.h"
#include "llkeyboard.h"

namespace {
using Core::KeyEvent;
using Core::kKeyReturn;
using Core::kModifierAlt;
using Core::kModifierControl;
using Core::kModifierPlatformControl;
using Core::kModifierShift;
using Core::PointerButton;
using Core::PointerEvent;
using Core::WheelEvent;
using Core::Style::CursorStyle;
using Viewer::NativeKeyInput;
using Viewer::NativePointerButton;
using Viewer::NativePointerInput;
using Viewer::NativeScrollInput;
using Viewer::takePointerMoveForFrame;
using Viewer::translateCursor;
using Viewer::translateKeyInput;
using Viewer::translatePointerInput;
using Viewer::translateScrollInput;
} // namespace

TEST(InputBridge, TranslatesPointerInput) {
    const NativePointerInput input {
        12.5f,
        34.25f,
        NativePointerButton::Auxiliary1,
        MASK_SHIFT | MASK_ALT,
        2,
        3.5f,
        -4.f,
    };

    const PointerEvent translated = translatePointerInput(input);
    EXPECT_FLOAT_EQ(translated.position.x, 12.5f);
    EXPECT_FLOAT_EQ(translated.position.y, 34.25f);
    EXPECT_EQ(translated.button, PointerButton::Auxiliary1);
    EXPECT_EQ(translated.modifiers, kModifierShift | kModifierAlt);
    EXPECT_EQ(translated.clickCount, std::uint8_t {2});
    EXPECT_FLOAT_EQ(translated.delta.x, 3.5f);
    EXPECT_FLOAT_EQ(translated.delta.y, -4.f);
}

TEST(InputBridge, TranslatesKeyInput) {
    const NativeKeyInput input {
        KEY_PAD_RETURN,
        MASK_CONTROL | MASK_MAC_CONTROL,
        true,
    };

    const KeyEvent translated = translateKeyInput(input);
    EXPECT_EQ(translated.key, kKeyReturn);
    EXPECT_EQ(translated.modifiers, kModifierControl | kModifierPlatformControl);
    EXPECT_TRUE(translated.repeated);
}

TEST(InputBridge, NormalizesWheelInput) {
    const WheelEvent scroll = translateScrollInput({8, 9, -1.f, 2.f, MASK_CONTROL});
    EXPECT_FLOAT_EQ(scroll.dx, -40.f);
    EXPECT_FLOAT_EQ(scroll.dy, 80.f);
    EXPECT_EQ(scroll.modifiers, kModifierControl);
}

TEST(InputBridge, MapsCursorStyles) {
    EXPECT_EQ(translateCursor(Core::Style::CursorStyle::Pointer), UI_CURSOR_HAND);
    EXPECT_EQ(translateCursor(Core::Style::CursorStyle::EastWestResize), UI_CURSOR_SIZEWE);
    EXPECT_EQ(translateCursor(Core::Style::CursorStyle::Auto), UI_CURSOR_ARROW);
}

TEST(InputBridge, CoalescesPointerMoves) {
    std::optional<NativePointerInput> pending;
    pending = NativePointerInput {10.f, 20.f, NativePointerButton::NoButton, MASK_SHIFT, 1, 0.f, 0.f};
    pending = NativePointerInput {12.f, 24.f, NativePointerButton::NoButton, MASK_CONTROL, 1, 0.f, 0.f};

    const std::optional<NativePointerInput> sample = takePointerMoveForFrame(pending, true, true, false, MASK_CONTROL, 3.5f, -4.f);
    ASSERT_TRUE(sample.has_value());
    EXPECT_FLOAT_EQ(sample->x, 12.f);
    EXPECT_FLOAT_EQ(sample->y, 24.f);
    EXPECT_EQ(sample->modifiers, MASK_CONTROL);
    EXPECT_FLOAT_EQ(sample->dx, 3.5f);
    EXPECT_FLOAT_EQ(sample->dy, -4.f);
    EXPECT_FALSE(pending.has_value());
}

TEST(InputBridge, DropsUncapturedOutsideMoves) {
    std::optional<NativePointerInput> pending;
    pending = NativePointerInput {12.f, 24.f, NativePointerButton::NoButton, 0, 1, 0.f, 0.f};

    EXPECT_FALSE(takePointerMoveForFrame(pending, true, false, false, 0, 1.f, 2.f).has_value());
    EXPECT_FALSE(pending.has_value());
}
