/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <memory>
#include "css/stylesheet.h"
#include "dom/elementinternal.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "render/recordingpaintcontext.h"
#include "surface/surface.h"

namespace {
using radia::ui::ElementState;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::RecordingPaintContext;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::Visibility;
using radia::ui::detail::makeElement;
using radia::ui::test::makeFloater;
} // namespace

TEST(SurfaceStateTest, ReflowsWhenHoveredStateChangesLayout) {
    StyleSheet styleSheet;
    constexpr char kStateLayout[] = "button { width: 20px; height: 10px; } button:hover { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateLayout).ok());
    EXPECT_TRUE(styleSheet.stateAffectsLayout(ElementState::Hovered));
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setPointerEvents(true);
    surface.mount(std::move(button));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);
    surface.pointerMove({{5.f, 95.f}});
    ASSERT_TRUE(target->hasState(ElementState::Hovered));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceStateTest, RefreshesHitTestingWhenHoveredPolicyChanges) {
    StyleSheet styleSheet;
    constexpr char kStateHitTest[] = "button { pointer-events: auto; } button:hover { pointer-events: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateHitTest).ok());
    EXPECT_TRUE(styleSheet.stateAffectsHitTesting(ElementState::Hovered));
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.mount(std::move(button));

    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hasState(ElementState::Hovered));
    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(ElementState::Hovered));
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(ElementState::Hovered));
}

TEST(SurfaceStateTest, InvalidatesDescendantLayoutForOwnerState) {
    StyleSheet styleSheet;
    constexpr char kDescendantState[] = "panel { display: flex; flex-direction: row; } label { width: 20px; height: 10px; } "
                                        "panel:hover > label { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDescendantState).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    HTMLPanelElement* parent = panel.get();
    ASSERT_NE(parent, nullptr);
    panel->setRect({0.f, 0.f, 100.f, 20.f}).setPointerEvents(true);
    auto label = makeElement<HTMLLabelElement>("descendant");
    HTMLLabelElement* target = label.get();
    ASSERT_NE(target, nullptr);
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(parent->hasState(ElementState::Hovered));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceStateTest, ReflowsWhenStyleSelectorAttributeChanges) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("input { display: block; width: 20px; height: 10px; } "
                               "input[type=\"checkbox\"] { width: 40px; }")
                    .ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto input = makeElement<HTMLInputElement>();
    HTMLInputElement* target = input.get();
    surface.mount(std::move(input));

    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);

    target->type("checkbox");
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceStateTest, RemovesUnavailableElementsFromStationaryHitTesting) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.mount(std::move(button));
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hasState(ElementState::Hovered));

    RecordingPaintContext recording;
    target->setVisibility(Visibility::Hidden);
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(ElementState::Hovered));

    target->setVisibility(Visibility::Visible);
    surface.paint(recording);
    EXPECT_TRUE(target->hasState(ElementState::Hovered));

    target->disabled(true);
    surface.paint(recording);
    EXPECT_FALSE(target->hasState(ElementState::Hovered));
    target->disabled(false);
    surface.paint(recording);
    EXPECT_TRUE(target->hasState(ElementState::Hovered));
}

TEST(SurfaceStateTest, RestylesCompositePartsWhenOwnerStateChanges) {
    StyleSheet styleSheet;
    constexpr char kCompositeOwnerState[] = "floater { display: flex; flex-direction: column; width: 100px; height: 100px; } "
                                            "floater:minimized > head { height: 40px; } "
                                            "floater > head { height: 20px; } floater > body { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCompositeOwnerState).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = makeFloater(false, true);
    HTMLFloaterElement* target = floater.get();
    ASSERT_NE(target, nullptr);
    surface.mountFloater(std::move(floater));
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->head()->rect().h, 20.f);

    target->setMinimized(true);
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->head()->rect().h, 40.f);
}
