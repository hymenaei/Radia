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
#include <optional>
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

namespace {
using radia::ui::Button;
using radia::ui::CursorStyle;
using radia::ui::Floater;
using radia::ui::Label;
using radia::ui::PointerButton;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::SurfaceFloaterDelegate;
using radia::ui::SurfaceLayer;
using radia::ui::Vec2;
} // namespace

namespace tut {
class FloaterDelegateProbe final : public SurfaceFloaterDelegate {
public:
    bool canDetachFloater(const Surface&, const Floater&) const override { return allowDetach; }
    void floaterClosed(Surface&, Floater&) override { ++closes; }
    void floaterMinimizedChanged(Surface&, Floater&, bool) override { ++minimizeChanges; }
    void floaterMoved(Surface&, Floater&) override { ++moves; }
    void floaterMoveEnded(Surface&, Floater&) override { ++moveCompletions; }
    bool beginNativeFloaterResize(Surface& surface, Floater& floater) override {
        ++resizeStarts;
        if (unmountOnNativeResize) unmountedFloater = surface.unmountFloater(floater);
        return nativeResize;
    }
    void floaterResized(Surface&, Floater&, bool complete) override {
        ++resizeChanges;
        if (complete) ++resizeCompletions;
    }
    void floaterDetachRequested(Surface&, Floater&, const Vec2& desiredPosition, const Vec2&) override {
        ++detachRequests;
        requestedPosition = desiredPosition;
    }

    bool allowDetach = true;
    int closes = 0;
    int minimizeChanges = 0;
    int moves = 0;
    int detachRequests = 0;
    int moveCompletions = 0;
    int resizeStarts = 0;
    int resizeChanges = 0;
    int resizeCompletions = 0;
    bool nativeResize = false;
    bool unmountOnNativeResize = false;
    std::unique_ptr<Floater> unmountedFloater;
    Vec2 requestedPosition;
};

struct floatersData {};
using floatersTest = test_group<floatersData>;
using floatersObject = floatersTest::object;
floatersTest floatersTestCase("floaters");

template<> template<> void floatersObject::test<1>() {
    StyleSheet styleSheet;
    ensure("floater drag style compiles",
           styleSheet.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanClose(false);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    surface.pointerDown({{30.f, 110.f}, PointerButton::Left});
    surface.pointerMove({{-99.f, 110.f}, PointerButton::Left});
    ensure_equals("99 logical pixels beyond the Surface remain attached", delegate.detachRequests, 0);
    surface.pointerMove({{-100.f, 110.f}, PointerButton::Left});
    ensure_equals("100 logical pixels beyond the Surface request live detachment", delegate.detachRequests, 1);
    ensure_equals("request preserves desired unclamped x", delegate.requestedPosition.x, -110.f);
    surface.pointerMove({{-80.f, 110.f}, PointerButton::Left});
    ensure_equals("one drag emits one detach request", delegate.detachRequests, 1);
    surface.pointerUp({{-80.f, 110.f}, PointerButton::Left});
    ensure_equals("attached move publishes one completion", delegate.moveCompletions, 1);

    target->setCanDetach(false);
    surface.pointerDown({{10.f, 110.f}, PointerButton::Left});
    surface.pointerMove({{-80.f, 110.f}, PointerButton::Left});
    ensure_equals("attached-only Floater resists every overshoot", delegate.detachRequests, 1);
    surface.pointerUp({{-80.f, 110.f}, PointerButton::Left});
    ensure_equals("second attached move publishes one completion", delegate.moveCompletions, 2);
}

template<> template<> void floatersObject::test<2>() {
    StyleSheet styleSheet;
    ensure("minimized restore style compiles",
           styleSheet.loadRadia("floater { flow: column; } floater::header { height: 30px; flow: row; } floater::content { flex-grow: 1; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setTitle("title").setCanClose(false).setCanMinimize(true);
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    target->setMinimized(true);
    surface.updateLayout();
    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    ensure("minimized header starts drag", surface.pointerDown({dragStart, PointerButton::Left}));
    surface.pointerMove({{199.f, dragStart.y}, PointerButton::Left});
    surface.pointerUp({{199.f, dragStart.y}, PointerButton::Left});
    const float minimizedLeft = target->rect().left();

    target->setMinimized(false);
    ensure_equals("restored width is preserved", target->rect().w, 100.f);
    ensure_equals("restored Floater is moved inside right bound", target->rect().right(), 200.f);
    ensure("restoring a wider Floater moves it left", target->rect().left() < minimizedLeft);
}

template<> template<> void floatersObject::test<3>() {
    Surface first;
    Surface second;
    FloaterDelegateProbe firstDelegate;
    FloaterDelegateProbe secondDelegate;
    first.setFloaterDelegate(&firstDelegate);
    second.setFloaterDelegate(&secondDelegate);
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setTitle("title").setCanMinimize(true);
    floater->setRect({90.f, 90.f, 30.f, 30.f});
    first.mountFloater(std::move(floater));
    ensure_equals("mount clamps to first Surface right edge", target->rect().right(), 100.f);
    ensure_equals("mount clamps to first Surface top edge", target->rect().top(), 100.f);
    ensure_equals("first Surface observes its placement change", firstDelegate.moves, 1);

    std::unique_ptr<Floater> transferred = first.unmountFloater(*target);
    ensure("typed Floater transfer returns ownership", transferred && transferred.get() == target);
    second.mountFloater(std::move(transferred));
    ensure_equals("transfer applies destination right edge", target->rect().right(), 80.f);
    ensure_equals("transfer applies destination top edge", target->rect().top(), 60.f);
    ensure_equals("destination Surface observes transferred placement", secondDelegate.moves, 1);

    target->setMinimized(true);
    ensure_equals("destination Surface observes minimization", secondDelegate.minimizeChanges, 1);
    ensure_equals("source Surface no longer observes transferred Floater", firstDelegate.minimizeChanges, 0);
    target->setMinimized(false);
    target->close();
    ensure_equals("destination Surface observes close", secondDelegate.closes, 1);
    ensure_equals("source Surface does not observe close", firstDelegate.closes, 0);
}

template<> template<> void floatersObject::test<4>() {
    StyleSheet styleSheet;
    ensure("startup Floater stylesheet compiles",
           styleSheet.loadRadia("floater { width: 100px; flow: column; } floater::header { height: 30px; }").ok());
    Surface surface(styleSheet);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanClose(false);
    surface.mountFloater(std::move(floater));

    surface.updateLayout();
    ensure_equals("zero-viewport layout does not place managed Floater x", target->rect().x, 0.f);
    ensure_equals("zero-viewport layout does not place managed Floater y", target->rect().y, 0.f);
    surface.setViewport(200.f, 100.f);
    ensure_equals("viewport initialization does not report a synthetic Floater move", delegate.moves, 0);
}

template<> template<> void floatersObject::test<5>() {
    StyleSheet styleSheet;
    const char* kResize = "floater { size: 80px 100px; min-size: 40px 50px; flow: column; } floater::header { height: 20px; }";
    ensure("resize stylesheet compiles", styleSheet.loadRadia(kResize).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ensure("resize Floater preparation succeeds", prepared.has_value());
    if (prepared) surface.placeFloater(*target, *prepared);
    surface.updateLayout();

    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}});
    ensure("right border has intrinsic cursor", surface.cursor() == CursorStyle::EastWestResize);
    ensure("right border starts capture", surface.pointerDown({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}, PointerButton::Left}));
    surface.pointerMove({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    ensure_equals("attached resize remains in Surface", target->rect().right(), 200.f);
    ensure_equals("resize never enters detach path", delegate.detachRequests, 0);
    surface.pointerUp({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    ensure_equals("resize completes once", delegate.resizeCompletions, 1);
}

template<> template<> void floatersObject::test<6>() {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(false).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    surface.pointerMove({{119.f, 60.f}});
    ensure("disabled resizing exposes no intrinsic cursor", surface.cursor() == CursorStyle::Default);
    target->setCanResize(true).setCanMinimize(true).setMinimized(true);
    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 2.f}});
    ensure("minimized Floater exposes no resize cursor", surface.cursor() == CursorStyle::Default);
}

template<> template<> void floatersObject::test<7>() {
    StyleSheet styleSheet;
    ensure("percentage minimum compiles", styleSheet.loadRadia("floater { size: 80px 100px; min-size: 50%; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setCanClose(false);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ensure("percentage minimum Floater preparation succeeds", prepared.has_value());
    if (prepared) surface.placeFloater(*target, *prepared);

    const Vec2 leftEdge{target->rect().left() + 1.f, target->rect().bottom() + 30.f};
    surface.pointerDown({leftEdge, PointerButton::Left});
    surface.pointerMove({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    surface.pointerUp({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    ensure_equals("percentage minimum uses frozen original width", target->rect().w, 50.f);
}

template<> template<> void floatersObject::test<8>() {
    Surface surface;
    surface.setViewport(100.f, 80.f);
    FloaterDelegateProbe delegate;
    delegate.nativeResize = true;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setRect({0.f, 0.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    ensure("native-hosted resize starts", surface.pointerDown({{99.f, 40.f}, PointerButton::Left}));
    surface.pointerMove({{139.f, 40.f}, PointerButton::Left});
    ensure_equals("native-hosted logical geometry may grow beyond its old viewport", target->rect().w, 140.f);
    surface.pointerUp({{139.f, 40.f}, PointerButton::Left});
    ensure_equals("native resize seam entered once", delegate.resizeStarts, 1);
    ensure_equals("native resize publishes completion", delegate.resizeCompletions, 1);
}

template<> template<> void floatersObject::test<9>() {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = std::make_unique<Floater>();
    Floater* target = floater.get();
    floater->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater), SurfaceLayer::Modal);

    ensure("modal resize starts before modal child routing", surface.pointerDown({{119.f, 60.f}, PointerButton::Left}));
    surface.pointerMove({{159.f, 60.f}, PointerButton::Left});
    surface.pointerUp({{159.f, 60.f}, PointerButton::Left});
    ensure_equals("expanded modal Floater resizes", target->rect().w, 140.f);
}

template<> template<> void floatersObject::test<10>() {
    StyleSheet styleSheet;
    const char* kFixedOuter = "floater { size: 300px; flow: column; } floater::header { height: 20px; } floater::content { flow: column; }";
    ensure("fixed outer floater stylesheet compiles", styleSheet.loadRadia(kFixedOuter).ok());
    Surface surface(styleSheet);
    surface.setViewport(500.f, 400.f);
    auto floater = std::make_unique<Floater>();
    floater->addChild(std::make_unique<Label>("first"));

    const Rect firstOuter = surface.prepareFloater(*floater).value();
    const Vec2 firstContent = floater->authoredContentSize();
    floater->addChild(std::make_unique<Label>("second"));
    const Rect secondOuter = surface.prepareFloater(*floater).value();
    const Vec2 secondContent = floater->authoredContentSize();

    ensure_equals("unpositioned Floater is centered horizontally", firstOuter.x, 100.f);
    ensure_equals("unpositioned Floater is centered vertically", firstOuter.y, 50.f);
    ensure_equals("fixed outer height masks the content edit", firstOuter.h, secondOuter.h);
    ensure("authored content geometry still records the edit", secondContent.y > firstContent.y);
}

template<> template<> void floatersObject::test<11>() {
    StyleSheet styleSheet;
    ensure("viewport percentage Floater stylesheet compiles",
           styleSheet.loadRadia("floater { width: 50%; height: 25%; left: 10%; bottom: 10%; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    Floater floater;
    const Rect rect = surface.prepareFloater(floater).value();
    ensure_approximately_equals("percentage width resolves against viewport width", rect.w, 200.f, 6);
    ensure_approximately_equals("percentage height resolves against viewport height", rect.h, 75.f, 6);
    ensure_approximately_equals("percentage left resolves against viewport width", rect.x, 40.f, 6);
    ensure_approximately_equals("percentage bottom resolves against viewport height", rect.y, 30.f, 6);
}

template<> template<> void floatersObject::test<12>() {
    StyleSheet styleSheet;
    ensure("pointer-pass-through Floater stylesheet compiles", styleSheet.loadRadia("floater.pass-through { pointer-events: none; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    auto lower = std::make_unique<Floater>();
    Floater* lowerTarget = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));
    auto upper = std::make_unique<Floater>();
    Floater* upperTarget = upper.get();
    upper->setCanResize(false).setRect({70.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(upper));
    auto upperChild = std::make_unique<Button>();
    upperChild->setRect({0.f, 0.f, 100.f, 80.f}).setPointerEvents(true);
    upperTarget->content()->addChild(std::move(upperChild));

    const Vec2 lowerEdgeUnderUpper{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    ensure("interactive child in pass-through upper Floater blocks lower resize hit testing",
           surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    ensure("blocked lower resize does not capture the pointer", !surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});

    upperTarget->addClass("pass-through");
    ensure("pointer-transparent upper Floater allows lower resize", surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    ensure("lower resize captures through a pointer-transparent Floater", surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});
}

template<> template<> void floatersObject::test<13>() {
    Surface surface;
    surface.setViewport(100.f, 80.f);
    FloaterDelegateProbe delegate;
    delegate.nativeResize = true;
    delegate.unmountOnNativeResize = true;
    surface.setFloaterDelegate(&delegate);
    auto floater = std::make_unique<Floater>();
    floater->setCanResize(true).setRect({0.f, 0.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    ensure("native resize rejects a floater detached by its delegate", !surface.pointerDown({{99.f, 40.f}, PointerButton::Left}));
    ensure("detached native resize does not capture the pointer", !surface.hasPointerCapture());
    ensure("delegate retains the detached Floater for cleanup", delegate.unmountedFloater != nullptr);
}

template<> template<> void floatersObject::test<14>() {
    StyleSheet styleSheet;
    ensure("overflow-visible pass-through stylesheet compiles",
           styleSheet.loadRadia("floater.pass-through { pointer-events: none; overflow: visible; }").ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);

    auto lower = std::make_unique<Floater>();
    Floater* lowerTarget = lower.get();
    lower->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));

    auto upper = std::make_unique<Floater>();
    Floater* upperTarget = upper.get();
    upper->addClass("pass-through").setRect({70.f, 20.f, 40.f, 80.f});
    surface.mountFloater(std::move(upper));

    auto overflowChild = std::make_unique<Button>();
    overflowChild->setRect({110.f, 20.f, 40.f, 80.f}).setPointerEvents(true);
    upperTarget->content()->addChild(std::move(overflowChild));

    const Vec2 point{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    ensure("overflow-visible descendant remains the pointer target", surface.pointerDown({point, PointerButton::Left}));
    ensure("overflow-visible descendant does not capture resize", !surface.hasPointerCapture());
}

template<> template<> void floatersObject::test<15>() {
    set_test_name("Floater replacement swaps roots and returns the retired root");
    Surface surface;
    surface.setViewport(240.f, 180.f);
    auto original = std::make_unique<Floater>();
    Floater* originalPointer = original.get();
    original->setRect({20.f, 25.f, 100.f, 80.f});
    surface.mountFloater(std::move(original));

    auto replacement = std::make_unique<Floater>();
    Floater* replacementPointer = replacement.get();
    replacement->setRect({30.f, 35.f, 120.f, 90.f});
    std::unique_ptr<Floater> retired = surface.replaceFloater(*originalPointer, std::move(replacement));

    ensure("replacement returns the retired root", retired && retired.get() == originalPointer);
    ensure("replacement is owned by the Surface", surface.ownsFloater(*replacementPointer));
    ensure("retired root is detached", retired->parent() == nullptr);
    ensure("replacement keeps the Surface layer parent", replacementPointer->parent() != nullptr);
}
} // namespace tut
