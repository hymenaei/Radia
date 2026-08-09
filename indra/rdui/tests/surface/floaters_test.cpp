/**
 * @file floaters_test.cpp
 * @brief Tests Surface floater mounting, ordering, and lifecycle behavior.
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
#include <algorithm>
#include "../test/lltut.h"
#include "binding/binder.h"
#include "render/recordingpaintcontext.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"

namespace tut {
class FloaterDelegateProbe final : public rdui::SurfaceFloaterDelegate {
public:
    bool canDetachFloater(const rdui::Surface&, const rdui::Floater&) const override { return allowDetach; }
    void floaterClosed(rdui::Surface&, rdui::Floater&) override { ++closes; }
    void floaterMinimizedChanged(rdui::Surface&, rdui::Floater&, bool) override { ++minimizeChanges; }
    void floaterMoved(rdui::Surface&, rdui::Floater&) override { ++moves; }
    bool beginNativeFloaterResize(rdui::Surface& surface, rdui::Floater& floater) override {
        ++resizeStarts;
        if (unmountOnNativeResize) unmountedFloater = surface.unmountFloater(floater);
        return nativeResize;
    }
    void floaterResized(rdui::Surface&, rdui::Floater&, bool complete) override {
        ++resizeChanges;
        if (complete) ++resizeCompletions;
    }
    void floaterDetachRequested(rdui::Surface&, rdui::Floater&, const rdui::Vec2& desired, const rdui::Vec2&) override {
        ++detachRequests;
        requested = desired;
    }

    bool allowDetach = true;
    int closes = 0;
    int minimizeChanges = 0;
    int moves = 0;
    int detachRequests = 0;
    int resizeStarts = 0;
    int resizeChanges = 0;
    int resizeCompletions = 0;
    bool nativeResize = false;
    bool unmountOnNativeResize = false;
    std::unique_ptr<rdui::Floater> unmountedFloater;
    rdui::Vec2 requested;
};

struct surfacefloaters_data {};
typedef test_group<surfacefloaters_data> surfacefloaters_test;
typedef surfacefloaters_test::object surfacefloaters_object;
using rduisurfacefloater_object = surfacefloaters_object;
surfacefloaters_test surfacefloaters_testcase("surfacefloaters");

template<> template<> void rduisurfacefloater_object::test<1>() {
    rdui::StyleSheet style_sheet;
    ensure("floater drag style compiles",
           style_sheet.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(200.f, 200.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanClose(false);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    surface.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left});
    surface.pointerMove({{-99.f, 110.f}, rdui::PointerButton::Left});
    ensure_equals("99 logical pixels beyond the Surface remain attached", delegate.detachRequests, 0);
    surface.pointerMove({{-100.f, 110.f}, rdui::PointerButton::Left});
    ensure_equals("100 logical pixels beyond the Surface request live detachment", delegate.detachRequests, 1);
    ensure_equals("request preserves desired unclamped x", delegate.requested.x, -110.f);
    surface.pointerMove({{-80.f, 110.f}, rdui::PointerButton::Left});
    ensure_equals("one drag emits one detach request", delegate.detachRequests, 1);
    surface.pointerUp({{-80.f, 110.f}, rdui::PointerButton::Left});

    target->setCanDetach(false);
    surface.pointerDown({{10.f, 110.f}, rdui::PointerButton::Left});
    surface.pointerMove({{-80.f, 110.f}, rdui::PointerButton::Left});
    ensure_equals("attached-only Floater resists every overshoot", delegate.detachRequests, 1);
    surface.pointerUp({{-80.f, 110.f}, rdui::PointerButton::Left});
}

template<> template<> void rduisurfacefloater_object::test<2>() {
    rdui::StyleSheet style_sheet;
    ensure("minimized restore style compiles",
           style_sheet.loadRadia("floater { flow: column; } floater::header { height: 30px; flow: row; } floater::content { flex-grow: 1; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setTitle("title").setCanClose(false).setCanMinimize(true);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    target->setMinimized(true);
    surface.updateLayout();
    const rdui::Vec2 drag_start{target->rect().left() + 2.f, target->rect().top() - 15.f};
    ensure("minimized header starts drag", surface.pointerDown({drag_start, rdui::PointerButton::Left}));
    surface.pointerMove({{199.f, drag_start.y}, rdui::PointerButton::Left});
    surface.pointerUp({{199.f, drag_start.y}, rdui::PointerButton::Left});
    const float minimized_left = target->rect().left();

    target->setMinimized(false);
    ensure_equals("restored width is preserved", target->rect().w, 100.f);
    ensure_equals("restored Floater is moved inside right bound", target->rect().right(), 200.f);
    ensure("restoring a wider Floater moves it left", target->rect().left() < minimized_left);
}

template<> template<> void rduisurfacefloater_object::test<3>() {
    rdui::Surface first;
    rdui::Surface second;
    FloaterDelegateProbe first_delegate;
    FloaterDelegateProbe second_delegate;
    first.setFloaterDelegate(&first_delegate);
    second.setFloaterDelegate(&second_delegate);
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setTitle("title").setCanMinimize(true);
    floater->setRect({90.f, 90.f, 30.f, 30.f});
    first.mountFloater(std::move(floater));
    ensure_equals("mount clamps to first Surface right edge", target->rect().right(), 100.f);
    ensure_equals("mount clamps to first Surface top edge", target->rect().top(), 100.f);
    ensure_equals("first Surface observes its placement change", first_delegate.moves, 1);

    std::unique_ptr<rdui::Floater> transferred = first.unmountFloater(*target);
    ensure("typed Floater transfer returns ownership", transferred && transferred.get() == target);
    second.mountFloater(std::move(transferred));
    ensure_equals("transfer applies destination right edge", target->rect().right(), 80.f);
    ensure_equals("transfer applies destination top edge", target->rect().top(), 60.f);
    ensure_equals("destination Surface observes transferred placement", second_delegate.moves, 1);

    target->setMinimized(true);
    ensure_equals("destination Surface observes minimization", second_delegate.minimizeChanges, 1);
    ensure_equals("source Surface no longer observes transferred Floater", first_delegate.minimizeChanges, 0);
    target->setMinimized(false);
    target->close();
    ensure_equals("destination Surface observes close", second_delegate.closes, 1);
    ensure_equals("source Surface does not observe close", first_delegate.closes, 0);
}

template<> template<> void rduisurfacefloater_object::test<4>() {
    rdui::StyleSheet style_sheet;
    ensure("startup Floater stylesheet compiles",
           style_sheet.loadRadia("floater { width: 100px; flow: column; } floater::header { height: 30px; }").ok());
    rdui::Surface surface(style_sheet);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanClose(false);
    surface.mountFloater(std::move(floater));

    surface.updateLayout();
    ensure_equals("zero-viewport layout does not place managed Floater x", target->rect().x, 0.f);
    ensure_equals("zero-viewport layout does not place managed Floater y", target->rect().y, 0.f);
    surface.setViewport(200.f, 100.f);
    ensure_equals("viewport initialization does not report a synthetic Floater move", delegate.moves, 0);
}

template<> template<> void rduisurfacefloater_object::test<5>() {
    rdui::StyleSheet style_sheet;
    ensure("resize stylesheet compiles",
           style_sheet
               .loadRadia("floater { size: 80px 100px; min-size: 40px 50px; flow: column; } "
                          "floater::header { height: 20px; }")
               .ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(200.f, 160.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    surface.placeFloater(*target, surface.prepareFloater(*target));
    surface.updateLayout();

    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}});
    ensure("right border has intrinsic cursor", surface.cursor() == rdui::CursorStyle::EastWestResize);
    ensure("right border starts capture",
           surface.pointerDown({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left}));
    surface.pointerMove({{250.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left});
    ensure_equals("attached resize remains in Surface", target->rect().right(), 200.f);
    ensure_equals("resize never enters detach path", delegate.detachRequests, 0);
    surface.pointerUp({{250.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left});
    ensure_equals("resize completes once", delegate.resizeCompletions, 1);
}

template<> template<> void rduisurfacefloater_object::test<6>() {
    rdui::Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanResize(false).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    surface.pointerMove({{119.f, 60.f}});
    ensure("disabled resizing exposes no intrinsic cursor", surface.cursor() == rdui::CursorStyle::Default);
    target->setCanResize(true).setCanMinimize(true).setMinimized(true);
    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 2.f}});
    ensure("minimized Floater exposes no resize cursor", surface.cursor() == rdui::CursorStyle::Default);
}

template<> template<> void rduisurfacefloater_object::test<7>() {
    rdui::StyleSheet style_sheet;
    ensure("percentage minimum compiles", style_sheet.loadRadia("floater { size: 80px 100px; min-size: 50%; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(400.f, 300.f);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    surface.placeFloater(*target, surface.prepareFloater(*target));

    const rdui::Vec2 left_edge{target->rect().left() + 1.f, target->rect().bottom() + 30.f};
    surface.pointerDown({left_edge, rdui::PointerButton::Left});
    surface.pointerMove({{target->rect().right() + 500.f, left_edge.y}, rdui::PointerButton::Left});
    surface.pointerUp({{target->rect().right() + 500.f, left_edge.y}, rdui::PointerButton::Left});
    ensure_equals("percentage minimum uses frozen original width", target->rect().w, 50.f);
}

template<> template<> void rduisurfacefloater_object::test<8>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 80.f);
    FloaterDelegateProbe delegate;
    delegate.nativeResize = true;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanResize(true).setRect({0.f, 0.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    ensure("native-hosted resize starts", surface.pointerDown({{99.f, 40.f}, rdui::PointerButton::Left}));
    surface.pointerMove({{139.f, 40.f}, rdui::PointerButton::Left});
    ensure_equals("native-hosted logical geometry may grow beyond its old viewport", target->rect().w, 140.f);
    surface.pointerUp({{139.f, 40.f}, rdui::PointerButton::Left});
    ensure_equals("native resize seam entered once", delegate.resizeStarts, 1);
    ensure_equals("native resize publishes completion", delegate.resizeCompletions, 1);
}

template<> template<> void rduisurfacefloater_object::test<9>() {
    rdui::Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<rdui::Floater>();
    rdui::Floater* target = floater.get();
    floater->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater), rdui::SurfaceLayer::Modal);

    ensure("modal resize starts before modal child routing", surface.pointerDown({{119.f, 60.f}, rdui::PointerButton::Left}));
    surface.pointerMove({{159.f, 60.f}, rdui::PointerButton::Left});
    surface.pointerUp({{159.f, 60.f}, rdui::PointerButton::Left});
    ensure_equals("expanded modal Floater resizes", target->rect().w, 140.f);
}

template<> template<> void rduisurfacefloater_object::test<10>() {
    rdui::StyleSheet style_sheet;
    ensure("fixed outer floater stylesheet compiles",
           style_sheet
               .loadRadia("floater { size: 300px; flow: column; } "
                          "floater::header { height: 20px; } "
                          "floater::content { flow: column; }")
               .ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(500.f, 400.f);
    auto floater = std::make_unique<rdui::Floater>();
    floater->addChild(std::make_unique<rdui::Label>("first"));

    const rdui::Rect first_outer = surface.prepareFloater(*floater);
    const rdui::Vec2 first_content = floater->authoredContentSize();
    floater->addChild(std::make_unique<rdui::Label>("second"));
    const rdui::Rect second_outer = surface.prepareFloater(*floater);
    const rdui::Vec2 second_content = floater->authoredContentSize();

    ensure_equals("unpositioned Floater is centered horizontally", first_outer.x, 100.f);
    ensure_equals("unpositioned Floater is centered vertically", first_outer.y, 50.f);
    ensure_equals("fixed outer height masks the content edit", first_outer.h, second_outer.h);
    ensure("authored content geometry still records the edit", second_content.y > first_content.y);
}

template<> template<> void rduisurfacefloater_object::test<11>() {
    rdui::StyleSheet style_sheet;
    ensure("viewport percentage Floater stylesheet compiles",
           style_sheet.loadRadia("floater { width: 50%; height: 25%; left: 10%; bottom: 10%; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(400.f, 300.f);
    rdui::Floater floater;
    const rdui::Rect rect = surface.prepareFloater(floater);
    ensure_approximately_equals("percentage width resolves against viewport width", rect.w, 200.f, 6);
    ensure_approximately_equals("percentage height resolves against viewport height", rect.h, 75.f, 6);
    ensure_approximately_equals("percentage left resolves against viewport width", rect.x, 40.f, 6);
    ensure_approximately_equals("percentage bottom resolves against viewport height", rect.y, 30.f, 6);
}

template<> template<> void rduisurfacefloater_object::test<12>() {
    rdui::StyleSheet style_sheet;
    ensure("pointer-pass-through Floater stylesheet compiles", style_sheet.loadRadia("floater.pass-through { pointer-events: none; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(200.f, 160.f);
    auto lower = std::make_unique<rdui::Floater>();
    rdui::Floater* lower_target = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));
    auto upper = std::make_unique<rdui::Floater>();
    rdui::Floater* upper_target = upper.get();
    upper->setCanResize(false).setRect({70.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(upper));
    auto upper_child = std::make_unique<rdui::Button>();
    upper_child->setRect({0.f, 0.f, 100.f, 80.f}).setPointerEvents(true);
    upper_target->content()->addChild(std::move(upper_child));

    const rdui::Vec2 lower_edge_under_upper{lower_target->rect().right() - 1.f, lower_target->rect().y + 40.f};
    ensure("interactive child in pass-through upper Floater blocks lower resize hit testing",
           surface.pointerDown({lower_edge_under_upper, rdui::PointerButton::Left}));
    ensure("blocked lower resize does not capture the pointer", !surface.hasPointerCapture());
    surface.pointerUp({lower_edge_under_upper, rdui::PointerButton::Left});

    upper_target->addClass("pass-through");
    ensure("pointer-transparent upper Floater allows lower resize", surface.pointerDown({lower_edge_under_upper, rdui::PointerButton::Left}));
    ensure("lower resize captures through a pointer-transparent Floater", surface.hasPointerCapture());
    surface.pointerUp({lower_edge_under_upper, rdui::PointerButton::Left});
}

template<> template<> void rduisurfacefloater_object::test<13>() {
    rdui::Surface surface;
    surface.setViewport(100.f, 80.f);
    FloaterDelegateProbe delegate;
    delegate.nativeResize = true;
    delegate.unmountOnNativeResize = true;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<rdui::Floater>();
    floater->setCanResize(true).setRect({0.f, 0.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    ensure("native resize rejects a floater detached by its delegate", !surface.pointerDown({{99.f, 40.f}, rdui::PointerButton::Left}));
    ensure("detached native resize does not capture the pointer", !surface.hasPointerCapture());
    ensure("delegate retains the detached Floater for cleanup", delegate.unmountedFloater != nullptr);
}

template<> template<> void rduisurfacefloater_object::test<14>() {
    rdui::StyleSheet style_sheet;
    ensure("overflow-visible pass-through stylesheet compiles",
           style_sheet.loadRadia("floater.pass-through { pointer-events: none; overflow: visible; }").ok());
    rdui::Surface surface(style_sheet);
    surface.setViewport(200.f, 160.f);

    auto lower = std::make_unique<rdui::Floater>();
    rdui::Floater* lower_target = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));

    auto upper = std::make_unique<rdui::Floater>();
    rdui::Floater* upper_target = upper.get();
    upper->addClass("pass-through").setRect({70.f, 20.f, 40.f, 80.f});
    surface.mountFloater(std::move(upper));

    auto overflow_child = std::make_unique<rdui::Button>();
    overflow_child->setRect({110.f, 20.f, 40.f, 80.f}).setPointerEvents(true);
    upper_target->content()->addChild(std::move(overflow_child));

    const rdui::Vec2 point{lower_target->rect().right() - 1.f, lower_target->rect().y + 40.f};
    ensure("overflow-visible descendant remains the pointer target", surface.pointerDown({point, rdui::PointerButton::Left}));
    ensure("overflow-visible descendant does not capture resize", !surface.hasPointerCapture());
}
} // namespace tut
