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
    radia::ui::StyleSheet styleSheet;
    const char* kStateLayout = "button { width: 20px; height: 10px; } button:hover { width: 40px; }";
    ensure("state layout stylesheet compiles", styleSheet.loadRadia(kStateLayout).ok());
    ensure("state layout dependency is detected", styleSheet.stateAffectsLayout(radia::ui::WidgetState::Hovered));
    radia::ui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<radia::ui::Button>();
    radia::ui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.updateLayout();
    ensure_equals("state layout starts with base width", target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ensure("hover state is applied", target->hasState(radia::ui::WidgetState::Hovered));
    surface.updateLayout();
    ensure_equals("state layout declarations trigger reflow", target->rect().w, 40.f);
}

template<> template<> void surfaceStateObject::test<2>() {
    radia::ui::StyleSheet styleSheet;
    const char* kStateHitTest = "button { pointer-events: auto; } button:hover { pointer-events: none; }";
    ensure("state hit-test stylesheet compiles", styleSheet.loadRadia(kStateHitTest).ok());
    ensure("state hit-test dependency is detected", styleSheet.stateAffectsHitTesting(radia::ui::WidgetState::Hovered));
    radia::ui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<radia::ui::Button>();
    radia::ui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));

    surface.pointerMove({{5.f, 5.f}});
    ensure("pointer enters target before state policy changes", target->hasState(radia::ui::WidgetState::Hovered));
    radia::ui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure("stationary pointer refreshes state-driven hit policy", !target->hasState(radia::ui::WidgetState::Hovered));
    surface.paint(recording);
    ensure("state-driven hit policy settles without hover oscillation", !target->hasState(radia::ui::WidgetState::Hovered));
}

template<> template<> void surfaceStateObject::test<3>() {
    radia::ui::StyleSheet styleSheet;
    const char* kDescendantState = "panel { flow: row; } label { width: 20px; height: 10px; } panel:hover > label { width: 40px; }";
    ensure("descendant state stylesheet compiles", styleSheet.loadRadia(kDescendantState).ok());
    radia::ui::Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panel = std::make_unique<radia::ui::Panel>();
    radia::ui::Panel* parent = panel.get();
    panel->setRect({0.f, 0.f, 100.f, 20.f}).setPointerEvents(true);
    auto label = std::make_unique<radia::ui::Label>("descendant");
    radia::ui::Label* target = label.get();
    panel->addChild(std::move(label));
    surface.root().addChild(std::move(panel));

    surface.updateLayout();
    ensure_equals("descendant starts with base width", target->rect().w, 20.f);
    surface.pointerMove({{5.f, 5.f}});
    ensure("owner hover is applied", parent->hasState(radia::ui::WidgetState::Hovered));
    surface.updateLayout();
    ensure_equals("owner state invalidates descendant geometry", target->rect().w, 40.f);
}

template<> template<> void surfaceStateObject::test<4>() {
    radia::ui::Surface surface;
    surface.setViewport(100.f, 100.f);
    auto button = std::make_unique<radia::ui::Button>();
    radia::ui::Button* target = button.get();
    button->setRect({0.f, 0.f, 20.f, 10.f}).setPointerEvents(true);
    surface.root().addChild(std::move(button));
    surface.pointerMove({{5.f, 5.f}});
    ensure("visibility test starts hovered", target->hasState(radia::ui::WidgetState::Hovered));

    radia::ui::RecordingPaintContext recording;
    target->setVisibility(radia::ui::Visibility::Hidden);
    surface.paint(recording);
    ensure("hidden target is removed from stationary hit testing", !target->hasState(radia::ui::WidgetState::Hovered));

    target->setVisibility(radia::ui::Visibility::Visible);
    surface.paint(recording);
    ensure("restored target is found by stationary hit testing", target->hasState(radia::ui::WidgetState::Hovered));

    target->setDisabled(true);
    surface.paint(recording);
    ensure("disabled target is removed from stationary hit testing", !target->hasState(radia::ui::WidgetState::Hovered));
    target->setDisabled(false);
    surface.paint(recording);
    ensure("re-enabled target is found by stationary hit testing", target->hasState(radia::ui::WidgetState::Hovered));
}

template<> template<> void surfaceStateObject::test<5>() {
    radia::ui::StyleSheet styleSheet;
    const char* kCompositeOwnerState =
        "floater { flow: column; width: 100px; height: 100px; &:minimized::header { height: 40px; } } floater::header { height: 20px; } floater::content { flex-grow: 1; }";
    ensure("composite owner-state stylesheet compiles", styleSheet.loadRadia(kCompositeOwnerState).ok());
    radia::ui::Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<radia::ui::Floater>();
    radia::ui::Floater* target = floater.get();
    floater->setCanMinimize(true);
    surface.mountFloater(std::move(floater));
    surface.updateLayout();
    ensure_equals("composite header starts with base height", target->header()->rect().h, 20.f);

    target->setMinimized(true);
    surface.updateLayout();
    ensure_equals("owner state invalidates cached composite part style", target->header()->rect().h, 40.f);
}
} // namespace tut
