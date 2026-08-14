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
#include <memory>
#include "../test/lltut.h"
#include "render/recordingpaintcontext.h"
#include "style/stylesheet.h"
#include "surface/surface.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/label.h"
#include "widgets/panel.h"

namespace tut {
struct surfaceStateData {};
using surfaceStateTest = test_group<surfaceStateData>;
using surfaceStateObject = surfaceStateTest::object;
surfaceStateTest surfaceStateTestCase("surface state");

template<> template<> void surfaceStateObject::test<1>() {
    rdui::StyleSheet styleSheet;
    const char* kStateLayout = "button { width: 20px; height: 10px; } button:hover { width: 40px; }";
    ensure("state layout stylesheet compiles", styleSheet.loadRadia(kStateLayout).ok());
    ensure("state layout dependency is detected", styleSheet.stateAffectsLayout(rdui::WidgetState::Hovered));
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.updateLayout();
    ensure_equals("state layout starts with base width", target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ensure("hover state is applied", target->hasState(rdui::WidgetState::Hovered));
    surface.updateLayout();
    ensure_equals("state layout declarations trigger reflow", target->rect().w, 40.f);
}

template<> template<> void surfaceStateObject::test<2>() {
    rdui::StyleSheet styleSheet;
    const char* kStateHitTest = "button { pointer-events: auto; } button:hover { pointer-events: none; }";
    ensure("state hit-test stylesheet compiles", styleSheet.loadRadia(kStateHitTest).ok());
    ensure("state hit-test dependency is detected", styleSheet.stateAffectsHitTesting(rdui::WidgetState::Hovered));
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));

    surface.pointerMove({{5.f, 5.f}});
    ensure("pointer enters target before state policy changes", target->hasState(rdui::WidgetState::Hovered));
    rdui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure("stationary pointer refreshes state-driven hit policy", !target->hasState(rdui::WidgetState::Hovered));
    surface.paint(recording);
    ensure("state-driven hit policy settles without hover oscillation", !target->hasState(rdui::WidgetState::Hovered));
}

template<> template<> void surfaceStateObject::test<3>() {
    rdui::StyleSheet styleSheet;
    const char* kDescendantState = "panel { flow: row; } label { width: 20px; height: 10px; } panel:hover > label { width: 40px; }";
    ensure("descendant state stylesheet compiles", styleSheet.loadRadia(kDescendantState).ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = std::make_unique<rdui::Panel>();
    rdui::Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 20.f}).setPointerEvents(true);
    auto label = std::make_unique<rdui::Label>("descendant");
    rdui::Label* target = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));

    surface.updateLayout();
    ensure_equals("descendant starts with base width", target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ensure("owner hover is applied", parent->hasState(rdui::WidgetState::Hovered));
    surface.updateLayout();
    ensure_equals("owner state invalidates descendant geometry", target->rect().w, 40.f);
}

template<> template<> void surfaceStateObject::test<4>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<rdui::Button>();
    rdui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.pointerMove({{5.f, 5.f}});
    ensure("visibility test starts hovered", target->hasState(rdui::WidgetState::Hovered));

    rdui::RecordingPaintContext recording;
    target->setVisibility(rdui::Visibility::Hidden);
    surface.paint(recording);
    ensure("hidden target is removed from stationary hit testing", !target->hasState(rdui::WidgetState::Hovered));

    target->setVisibility(rdui::Visibility::Visible);
    surface.paint(recording);
    ensure("restored target is found by stationary hit testing", target->hasState(rdui::WidgetState::Hovered));

    target->setDisabled(true);
    surface.paint(recording);
    ensure("disabled target is removed from stationary hit testing", !target->hasState(rdui::WidgetState::Hovered));
    target->setDisabled(false);
    surface.paint(recording);
    ensure("re-enabled target is found by stationary hit testing", target->hasState(rdui::WidgetState::Hovered));
}

template<> template<> void surfaceStateObject::test<5>() {
    rdui::StyleSheet styleSheet;
    const char* kCompositeOwnerState =
        "floater { flow: column; width: 100px; height: 100px; &:minimized::header { height: 40px; } } floater::header { height: 20px; } floater::content { flex-grow: 1; }";
    ensure("composite owner-state stylesheet compiles", styleSheet.loadRadia(kCompositeOwnerState).ok());
    rdui::Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanMinimize(true);
    surface.mountFloater(std::move(floater));
    surface.updateLayout();
    ensure_equals("composite header starts with base height", target->header()->rect().h, 20.f);

    target->setMinimized(true);
    surface.updateLayout();
    ensure_equals("owner state invalidates cached composite part style", target->header()->rect().h, 40.f);
}
} // namespace tut
