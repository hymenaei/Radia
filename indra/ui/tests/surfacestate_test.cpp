/**
 * @file surfacestate_test.cpp
 * @brief Tests state-driven Surface layout, hit testing, and visibility.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public License
 * as published by the Free Software Foundation; version 2.1 of the
 * License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <memory>
#include "render/recordingpaintcontext.h"
#include "style/stylesheet.h"
#include "surface/surface.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/label.h"
#include "widgets/panel.h"

namespace {
using radia::ui::Button;
using radia::ui::Floater;
using radia::ui::Label;
using radia::ui::Panel;
using radia::ui::RecordingPaintContext;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::Visibility;
using radia::ui::WidgetState;
} // namespace

TEST(SurfaceStateTest, ReflowsWhenHoveredStateChangesLayout) {
    StyleSheet styleSheet;
    constexpr char kStateLayout[] = "button { width: 20px; height: 10px; } button:hover { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateLayout).ok());
    EXPECT_TRUE(styleSheet.stateAffectsLayout(WidgetState::Hovered));
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hasState(WidgetState::Hovered));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceStateTest, RefreshesHitTestingWhenHoveredPolicyChanges) {
    StyleSheet styleSheet;
    constexpr char kStateHitTest[] = "button { pointer-events: auto; } button:hover { pointer-events: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateHitTest).ok());
    EXPECT_TRUE(styleSheet.stateAffectsHitTesting(WidgetState::Hovered));
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));

    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hasState(WidgetState::Hovered));
    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));
}

TEST(SurfaceStateTest, InvalidatesDescendantLayoutForOwnerState) {
    StyleSheet styleSheet;
    constexpr char kDescendantState[] = "panel { flow: row; } label { width: 20px; height: 10px; } "
                                        "panel:hover > label { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDescendantState).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = std::make_unique<Panel>();
    Panel* parent = panel.get();
    ASSERT_NE(parent, nullptr);
    panel->setRect({0.f, 0.f, 100.f, 20.f}).setPointerEvents(true);
    auto label = std::make_unique<Label>("descendant");
    Label* target = label.get();
    ASSERT_NE(target, nullptr);
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));

    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(parent->hasState(WidgetState::Hovered));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceStateTest, RemovesUnavailableWidgetsFromStationaryHitTesting) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<Button>();
    Button* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hasState(WidgetState::Hovered));

    RecordingPaintContext recording;
    target->setVisibility(Visibility::Hidden);
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));

    target->setVisibility(Visibility::Visible);
    surface.paint(recording);
    EXPECT_TRUE(target->hasState(WidgetState::Hovered));

    target->setDisabled(true);
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(WidgetState::Hovered));
    target->setDisabled(false);
    surface.paint(recording);
    EXPECT_TRUE(target->hasState(WidgetState::Hovered));
}

TEST(SurfaceStateTest, RestylesCompositePartsWhenOwnerStateChanges) {
    StyleSheet styleSheet;
    constexpr char kCompositeOwnerState[] = "floater { flow: column; width: 100px; height: 100px; "
                                            "&:minimized::header { height: 40px; } } "
                                            "floater::header { height: 20px; } floater::content { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCompositeOwnerState).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    ASSERT_NE(target, nullptr);
    floater->setCanMinimize(true);
    surface.mountFloater(std::move(floater));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->header()->rect().h, 20.f);

    target->setMinimized(true);
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->header()->rect().h, 40.f);
}
