/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/ElementInternal.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/HTMLInputElement.h>
#include <Core/HTMLLabelElement.h>
#include <Core/HTMLPanelElement.h>
#include <Core/RecordingPaintContext.h>
#include <Core/StylePass.h>
#include <Core/StyleSheet.h>
#include <Core/Surface.h>
#include <Core/TextMeasurer.h>
#include <gtest/gtest.h>
#include <memory>
#include "FloaterTestHelpers.h"

namespace {
using Core::FixedTextMeasurer;
using Core::HTMLButtonElement;
using Core::HTMLFloaterElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::RecordingPaintContext;
using Core::Surface;
using Core::CSS::PseudoClass;
using Core::CSS::StyleSheet;
using Core::detail::ElementInternalAccess;
using Core::detail::makeElement;
using Core::Style::Pass;
using Core::Style::Visibility;
using CoreTests::makeFloater;
} // namespace

TEST(SurfaceState, ReflowsOnHover) {
    StyleSheet styleSheet;
    constexpr char kStateLayout[] = "button { width: 20px; height: 10px; } button:hover { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateLayout).ok());
    EXPECT_TRUE(styleSheet.pseudoClassAffectsLayout(PseudoClass::Hover));
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
    ASSERT_TRUE(target->hovered());
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
    ElementInternalAccess::setHovered(*target, false);
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 20.f);
}

TEST(SurfaceState, InvalidatesFollowingSiblingLayout) {
    StyleSheet styleSheet;
    constexpr char kSiblingStateLayout[] = "panel { display: flex; flex-direction: row; } button { width: 20px; height: 10px; } "
                                           "label { width: 10px; height: 10px; } button:hover + label { width: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSiblingStateLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* buttonTarget = button.get();
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelTarget = label.get();
    panel->append(std::move(button));
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    surface.updateLayout();
    EXPECT_FLOAT_EQ(labelTarget->rect().w, 10.f);
    ElementInternalAccess::setHovered(*buttonTarget, true);
    surface.updateLayout();
    EXPECT_FLOAT_EQ(labelTarget->rect().w, 40.f);
    ElementInternalAccess::setHovered(*buttonTarget, false);
    surface.updateLayout();
    EXPECT_FLOAT_EQ(labelTarget->rect().w, 10.f);
}

TEST(SurfaceState, InvalidatesTextDecorationDescendants) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel:hover { text-decoration: underline; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    HTMLPanelElement* panelTarget = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 20.f}).setPointerEvents(true);
    auto label = makeElement<HTMLLabelElement>("text");
    HTMLLabelElement* labelTarget = label.get();
    panel->append(std::move(label));
    surface.mount(std::move(panel));

    Pass styles(styleSheet, FixedTextMeasurer {});
    EXPECT_EQ(styles.style(*labelTarget).textDecorationPropagation, Core::Style::TextDecoration::NoneValue);
    ElementInternalAccess::setHovered(*panelTarget, true);
    EXPECT_EQ(styles.style(*labelTarget).textDecorationPropagation, Core::Style::TextDecoration::Underline);
}

TEST(SurfaceState, RefreshesHitTestingOnHover) {
    StyleSheet styleSheet;
    constexpr char kStateHitTest[] = "button { pointer-events: auto; } button:hover { pointer-events: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kStateHitTest).ok());
    EXPECT_TRUE(styleSheet.pseudoClassAffectsHitTesting(PseudoClass::Hover));
    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.mount(std::move(button));

    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hovered());
    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_FALSE(target->hovered());
    surface.paint(recording);
    EXPECT_FALSE(target->hovered());
}

TEST(SurfaceState, InvalidatesDescendantLayout) {
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
    ASSERT_TRUE(parent->hovered());
    surface.updateLayout();
    EXPECT_FLOAT_EQ(target->rect().w, 40.f);
}

TEST(SurfaceState, ReflowsOnSelectorChange) {
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

TEST(SurfaceState, RemovesUnavailableHitTargets) {
    Surface surface;
    surface.setViewport(100.f, 100.f);
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    ASSERT_NE(target, nullptr);
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.mount(std::move(button));
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(target->hovered());

    RecordingPaintContext recording;
    target->setVisibility(Visibility::Hidden);
    surface.paint(recording);
    EXPECT_FALSE(target->hovered());

    target->setVisibility(Visibility::Visible);
    surface.paint(recording);
    EXPECT_TRUE(target->hovered());

    target->disabled(true);
    surface.paint(recording);
    EXPECT_FALSE(target->hovered());
    target->disabled(false);
    surface.paint(recording);
    EXPECT_TRUE(target->hovered());
}

TEST(SurfaceState, RestylesPartsOnOwnerChange) {
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
