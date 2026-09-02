/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <gtest/gtest.h>
#include <memory>
#include <optional>
#include "binding/binder.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "render/recordingpaintcontext.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace {
using radia::ui::CursorStyle;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::Node;
using radia::ui::PaintCommandKind;
using radia::ui::PointerButton;
using radia::ui::RecordingPaintContext;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::SurfaceFloaterDelegate;
using radia::ui::SurfaceLayer;
using radia::ui::Text;
using radia::ui::Vec2;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using radia::ui::detail::nodes;
} // namespace

namespace {
class FloaterDelegateProbe final : public SurfaceFloaterDelegate {
public:
    void floaterClosed(Surface&, HTMLFloaterElement&) override { ++closes; }
    void floaterMinimizedChanged(Surface&, HTMLFloaterElement&) override { ++minimizeChanges; }
    void floaterMoveEnded(Surface&, HTMLFloaterElement&) override { ++moveCompletions; }
    void floaterResizeEnded(Surface&, HTMLFloaterElement&) override { ++resizeCompletions; }

    int closes = 0;
    int minimizeChanges = 0;
    int moveCompletions = 0;
    int resizeCompletions = 0;
};

TEST(FloatersTest, ReportsFloaterVisibilityFromResolvedDisplayAndVisibility) {
    StyleSheet styleSheet;
    constexpr char kVisibility[] = ".hidden { visibility: hidden; } .none { display: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibility).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 100.f);

    auto visible = makeElement<HTMLFloaterElement>();
    surface.mountFloater(std::move(visible));
    EXPECT_TRUE(surface.hasVisibleFloater());

    auto hidden = makeElement<HTMLFloaterElement>();
    hidden->addClass("hidden");
    surface.mountFloater(std::move(hidden));
    auto none = makeElement<HTMLFloaterElement>();
    none->addClass("none");
    surface.mountFloater(std::move(none));
    EXPECT_TRUE(surface.hasVisibleFloater());

    surface.clearLayer(SurfaceLayer::Floater);
    EXPECT_FALSE(surface.hasVisibleFloater());
}

TEST(FloatersTest, RestoresFloaterWithinViewportAfterMinimization) {
    StyleSheet styleSheet;
    constexpr char kMinimizedFloaterStyle[] = "floater { display: flex; flex-direction: column; } "
                                              "floater > head { height: 30px; display: flex; flex-direction: row; } "
                                              "floater > body { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMinimizedFloaterStyle).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = radia::ui::test::makeFloater(false, true);
    HTMLFloaterElement* target = floater.get();
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    target->setMinimized(true);
    surface.updateLayout();
    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    EXPECT_TRUE(surface.pointerDown({dragStart, PointerButton::Left}));
    surface.pointerMove({{199.f, dragStart.y}, PointerButton::Left});
    surface.pointerUp({{199.f, dragStart.y}, PointerButton::Left});
    const float minimizedLeft = target->rect().left();

    target->setMinimized(false);
    EXPECT_EQ(target->rect().w, 100.f);
    EXPECT_EQ(target->rect().right(), 200.f);
    EXPECT_LT(target->rect().left(), minimizedLeft);
}

TEST(FloatersTest, TransfersFloaterBetweenSurfacesAndReportsLifecycle) {
    Surface first;
    Surface second;
    FloaterDelegateProbe firstDelegate;
    FloaterDelegateProbe secondDelegate;
    first.setFloaterDelegate(&firstDelegate);
    second.setFloaterDelegate(&secondDelegate);
    first.setViewport(100.f, 100.f);
    second.setViewport(80.f, 60.f);

    auto floater = radia::ui::test::makeFloater(true, true);
    HTMLFloaterElement* target = floater.get();
    floater->setRect({90.f, 90.f, 30.f, 30.f});
    first.mountFloater(std::move(floater));
    EXPECT_EQ(target->rect().right(), 100.f);
    EXPECT_EQ(target->rect().top(), 100.f);
    std::unique_ptr<HTMLFloaterElement> transferred = first.unmountFloater(*target);
    ASSERT_TRUE(transferred);
    EXPECT_EQ(transferred.get(), target);
    second.mountFloater(std::move(transferred));
    EXPECT_EQ(target->rect().right(), 80.f);
    EXPECT_EQ(target->rect().top(), 60.f);
    target->setMinimized(true);
    EXPECT_EQ(secondDelegate.minimizeChanges, 1);
    EXPECT_EQ(firstDelegate.minimizeChanges, 0);
    target->setMinimized(false);
    target->close();
    EXPECT_EQ(secondDelegate.closes, 1);
    EXPECT_EQ(firstDelegate.closes, 0);
}

TEST(FloatersTest, ReportsMoveCompletionAfterPointerUp) {
    StyleSheet styleSheet;
    constexpr char kMove[] = "floater { display: flex; flex-direction: column; } "
                             "floater > head { height: 30px; display: flex; flex-direction: row; } "
                             "floater > body { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMove).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = radia::ui::test::makeFloater(false, true);
    HTMLFloaterElement* target = floater.get();
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    surface.updateLayout();
    target->setMinimized(true);
    surface.updateLayout();
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);

    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    EXPECT_TRUE(surface.pointerDown({dragStart, PointerButton::Left}));
    EXPECT_EQ(delegate.moveCompletions, 0);
    const float initialLeft = target->rect().left();
    surface.pointerMove({{199.f, dragStart.y}, PointerButton::Left});
    EXPECT_GT(target->rect().left(), initialLeft);
    EXPECT_EQ(delegate.moveCompletions, 0);
    surface.pointerUp({{199.f, dragStart.y}, PointerButton::Left});
    EXPECT_EQ(delegate.moveCompletions, 1);
}

TEST(FloatersTest, MovesRetainedTextWithFloater) {
    StyleSheet styleSheet;
    constexpr char kMove[] = "floater { display: flex; flex-direction: column; } "
                             "floater > head { height: 30px; display: flex; flex-direction: row; } "
                             "floater > body { flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMove).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    target->body()->textContent("body");
    surface.updateLayout();

    Text* text = nullptr;
    for (Node& child : nodes(*target->body()))
        if ((text = child.asText())) break;
    ASSERT_NE(text, nullptr);
    const Rect initialFloaterRect = target->rect();
    const Rect initialTextRect = text->rect();

    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    ASSERT_TRUE(surface.pointerDown({dragStart, PointerButton::Left}));
    const Vec2 dragEnd{dragStart.x + 20.f, dragStart.y + 10.f};
    ASSERT_TRUE(surface.pointerMove({dragEnd, PointerButton::Left}));

    EXPECT_FLOAT_EQ(target->rect().x - initialFloaterRect.x, 20.f);
    EXPECT_FLOAT_EQ(target->rect().y - initialFloaterRect.y, 10.f);
    EXPECT_FLOAT_EQ(text->rect().x - initialTextRect.x, 20.f);
    EXPECT_FLOAT_EQ(text->rect().y - initialTextRect.y, 10.f);
}

TEST(FloatersTest, MovesScrollableBodyClipWithFloater) {
    StyleSheet styleSheet;
    constexpr char kMove[] = "floater { display: flex; flex-direction: column; } "
                             "floater > head { height: 30px; display: flex; flex-direction: row; } "
                             "floater > body { flex-grow: 1; overflow: auto; scrollbar-mode: overlay; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMove).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 200.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    floater->setRect({20.f, 20.f, 100.f, 100.f});
    surface.mountFloater(std::move(floater));
    target->body()->textContent("body");
    surface.updateLayout();

    const Rect initialBodyRect = target->body()->rect();
    RecordingPaintContext initialRecording;
    surface.paint(initialRecording);
    const auto* initialClip = initialRecording.last(PaintCommandKind::PushClip);
    ASSERT_NE(initialClip, nullptr);
    EXPECT_FLOAT_EQ(initialClip->rect.x, initialBodyRect.x);
    EXPECT_FLOAT_EQ(initialClip->rect.y, initialBodyRect.y);

    const Vec2 dragStart{target->rect().left() + 2.f, target->rect().top() - 15.f};
    ASSERT_TRUE(surface.pointerDown({dragStart, PointerButton::Left}));
    const Vec2 dragEnd{dragStart.x + 20.f, dragStart.y + 10.f};
    ASSERT_TRUE(surface.pointerMove({dragEnd, PointerButton::Left}));

    RecordingPaintContext movedRecording;
    surface.paint(movedRecording);
    const auto* movedClip = movedRecording.last(PaintCommandKind::PushClip);
    ASSERT_NE(movedClip, nullptr);
    EXPECT_FLOAT_EQ(movedClip->rect.x - initialClip->rect.x, 20.f);
    EXPECT_FLOAT_EQ(movedClip->rect.y - initialClip->rect.y, 10.f);
    EXPECT_FLOAT_EQ(movedClip->rect.w, initialClip->rect.w);
    EXPECT_FLOAT_EQ(movedClip->rect.h, initialClip->rect.h);
}

TEST(FloatersTest, KeepsWrappedBodyContentInsideFloaterWidth) {
    StyleSheet styleSheet;
    constexpr char kScrollableBody[] = "floater { display: flex; flex-direction: column; } "
                                       "floater > head { height: 30px; display: flex; flex-direction: row; } "
                                       "floater > body { display: flex; flex-direction: column; flex-grow: 1; min-size: 0px; margin: 8px; gap: 8px; "
                                       "overflow: auto; scrollbar-mode: classic; scrollbar-gutter: stable; } "
                                       "#sections { display: flex; flex-direction: column; width: 100%; } "
                                       "#section { display: flex; flex-direction: column; width: 100%; padding: 8px; box-sizing: border-box; } "
                                       "#copy { text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kScrollableBody).ok());

    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    target->setRect({40.f, 40.f, 160.f, 140.f});
    auto sections = makeElement<HTMLPanelElement>();
    sections->setId("sections");
    auto section = makeElement<HTMLPanelElement>();
    section->setId("section");
    auto copy = makeElement<HTMLLabelElement>(
        "This wrapped text is deliberately long enough to exercise the flex column's cross-axis sizing without creating horizontal overflow.");
    copy->setId("copy");
    section->append(std::move(copy));
    sections->append(std::move(section));
    target->body()->append(std::move(sections));
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    ASSERT_GT(target->body()->clientHeight(), 0.f);
    EXPECT_FLOAT_EQ(target->body()->scrollWidth(), target->body()->clientWidth());

    RecordingPaintContext recording;
    surface.paint(recording);
    const auto* scrollbar = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(scrollbar, nullptr);
    ASSERT_TRUE(scrollbar->scrollbar.has_value());
    EXPECT_FALSE(scrollbar->scrollbar->geometry.horizontal.visible);
    ASSERT_TRUE(scrollbar->scrollbar->geometry.vertical.visible);
    EXPECT_GE(scrollbar->scrollbar->geometry.vertical.bounds.left(), target->body()->rect().left());
    EXPECT_LE(scrollbar->scrollbar->geometry.vertical.bounds.right(), target->body()->rect().right());
}

TEST(FloatersTest, KeepsHorizontalScrollbarLocalToFloaterBody) {
    StyleSheet styleSheet;
    constexpr char kScrollableBody[] =
        "floater { display: flex; flex-direction: column; } "
        "floater > head { height: 30px; display: flex; flex-direction: row; } "
        "floater > body { display: flex; flex-direction: column; flex-grow: 1; min-size: 0px; margin: 8px; overflow: auto; "
        "scrollbar-mode: classic; } #wide { display: block; width: 500px; height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kScrollableBody).ok());

    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    target->setRect({40.f, 40.f, 160.f, 140.f});
    auto wide = makeElement<HTMLPanelElement>();
    wide->setId("wide");
    target->body()->append(std::move(wide));
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    EXPECT_GT(target->body()->scrollWidth(), target->body()->clientWidth());
    RecordingPaintContext recording;
    surface.paint(recording);
    const auto* scrollbar = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(scrollbar, nullptr);
    ASSERT_TRUE(scrollbar->scrollbar.has_value());
    ASSERT_TRUE(scrollbar->scrollbar->geometry.horizontal.visible);
    EXPECT_GE(scrollbar->scrollbar->geometry.horizontal.bounds.left(), target->body()->rect().left());
    EXPECT_LE(scrollbar->scrollbar->geometry.horizontal.bounds.right(), target->body()->rect().right());
}

TEST(FloatersTest, KeepsScrollbarsInsideFloaterBorder) {
    StyleSheet styleSheet;
    constexpr char kBorderedScrollableFloater[] = "floater { display: flex; flex-direction: column; border: 2px #ffffff; border-radius: 12px; } "
                                                  "floater > head { height: 30px; display: flex; flex-direction: row; } "
                                                  "floater > body { flex-grow: 1; min-size: 0px; overflow: auto; "
                                                  "scrollbar-mode: classic; } #tall { display: block; height: 500px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kBorderedScrollableFloater).ok());

    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    target->setRect({40.f, 40.f, 160.f, 140.f});
    auto tall = makeElement<HTMLPanelElement>();
    tall->setId("tall");
    target->body()->append(std::move(tall));
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const auto* scrollbar = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(scrollbar, nullptr);
    ASSERT_TRUE(scrollbar->scrollbar.has_value());
    ASSERT_TRUE(scrollbar->scrollbar->geometry.vertical.visible);
    EXPECT_GE(scrollbar->scrollbar->geometry.vertical.bounds.left(), target->rect().left() + 2.f);
    EXPECT_LE(scrollbar->scrollbar->geometry.vertical.bounds.right(), target->rect().right() - 2.f);
    ASSERT_TRUE(scrollbar->scrollbar->clip.enabled);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderBox.x, target->rect().x);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderBox.y, target->rect().y);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderBox.w, target->rect().w);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderBox.h, target->rect().h);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderWidth.left, 2.f);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderWidth.right, 2.f);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderRadius.bottomLeft.horizontal.pixels, 12.f);
    EXPECT_FLOAT_EQ(scrollbar->scrollbar->clip.borderRadius.bottomRight.vertical.pixels, 12.f);

    const auto& commands = recording.commands();
    const auto scrollbarIndex = std::distance(commands.begin(), std::find_if(commands.begin(), commands.end(), [](const auto& command) {
                                                  return command.kind == PaintCommandKind::Scrollbar;
                                              }));
    ASSERT_LT(scrollbarIndex, commands.size());
    ASSERT_GT(scrollbarIndex, 0u);
    ASSERT_LT(scrollbarIndex + 1, commands.size());
    EXPECT_EQ(commands[scrollbarIndex - 1].kind, PaintCommandKind::PushClip);
    EXPECT_EQ(commands[scrollbarIndex + 1].kind, PaintCommandKind::PopClip);
    EXPECT_FLOAT_EQ(commands[scrollbarIndex - 1].rect.x, target->body()->rect().x);
    EXPECT_FLOAT_EQ(commands[scrollbarIndex - 1].rect.y, target->body()->rect().y);
    EXPECT_FLOAT_EQ(commands[scrollbarIndex - 1].rect.w, target->body()->rect().w);
    EXPECT_FLOAT_EQ(commands[scrollbarIndex - 1].rect.h, target->body()->rect().h);
}

TEST(FloatersTest, KeepsFloaterResizeCursorOverBorderedScrollbar) {
    StyleSheet styleSheet;
    constexpr char kBorderedScrollableFloater[] = "floater { display: flex; flex-direction: column; border: 2px #ffffff; border-radius: 12px; } "
                                                  "floater > head { height: 30px; display: flex; flex-direction: row; } "
                                                  "floater > body { flex-grow: 1; min-size: 0px; overflow: auto; "
                                                  "scrollbar-mode: classic; } #tall { display: block; height: 500px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kBorderedScrollableFloater).ok());

    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = radia::ui::test::makeFloater();
    HTMLFloaterElement* target = floater.get();
    target->setResizeable(true).setRect({40.f, 40.f, 160.f, 140.f});
    auto tall = makeElement<HTMLPanelElement>();
    tall->setId("tall");
    target->body()->append(std::move(tall));
    surface.mountFloater(std::move(floater));
    surface.updateLayout();

    RecordingPaintContext recording;
    surface.paint(recording);
    const auto* scrollbar = recording.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(scrollbar, nullptr);
    ASSERT_TRUE(scrollbar->scrollbar.has_value());
    const Rect verticalBounds = scrollbar->scrollbar->geometry.vertical.bounds;
    const Vec2 point{verticalBounds.right() - 1.f, verticalBounds.y + verticalBounds.h * .5f};
    ASSERT_TRUE(verticalBounds.contains(point));

    ASSERT_TRUE(surface.pointerMove({point}));
    EXPECT_EQ(surface.cursor(), CursorStyle::EastWestResize);

    const Rect initialFloaterRect = target->rect();
    ASSERT_TRUE(surface.pointerDown({point, PointerButton::Left}));
    ASSERT_TRUE(surface.hasPointerCapture());
    const Vec2 dragPoint{point.x + 20.f, point.y};
    ASSERT_TRUE(surface.pointerMove({dragPoint, PointerButton::Left}));
    EXPECT_GT(target->rect().w, initialFloaterRect.w);
    EXPECT_FLOAT_EQ(target->body()->scrollTop(), 0.f);
    EXPECT_TRUE(surface.pointerUp({dragPoint, PointerButton::Left}));
}

TEST(FloatersTest, ResizesFloaterWithinSurfaceBounds) {
    StyleSheet styleSheet;
    constexpr char kResize[] = "floater { size: 80px 100px; min-size: 40px 50px; display: flex; flex-direction: column; } "
                               "floater > head { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kResize).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    FloaterDelegateProbe delegate;
    surface.setFloaterDelegate(&delegate);
    auto floater = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* target = floater.get();
    floater->setResizeable(true);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ASSERT_TRUE(prepared.has_value());
    surface.placeFloater(*target, *prepared);
    surface.updateLayout();

    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::EastWestResize);
    EXPECT_TRUE(surface.pointerDown({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}, PointerButton::Left}));
    surface.pointerMove({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    EXPECT_EQ(target->rect().right(), 200.f);
    surface.pointerUp({{250.f, target->rect().bottom() + 30.f}, PointerButton::Left});
    EXPECT_EQ(delegate.resizeCompletions, 1);
}

TEST(FloatersTest, HidesResizeCursorWhenResizingIsUnavailable) {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = radia::ui::test::makeFloater(false, true);
    HTMLFloaterElement* target = floater.get();
    floater->setResizeable(false).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater));

    surface.pointerMove({{119.f, 60.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
    target->setResizeable(true).setMinimized(true);
    surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 2.f}});
    EXPECT_EQ(surface.cursor(), CursorStyle::Default);
}

TEST(FloatersTest, UsesFrozenWidthForPercentageMinimumSize) {
    StyleSheet styleSheet;
    constexpr char kPercentageMinimumLayout[] = "floater { size: 80px 100px; min-size: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageMinimumLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* target = floater.get();
    floater->setResizeable(true);
    surface.mountFloater(std::move(floater));
    const std::optional<Rect> prepared = surface.prepareFloater(*target);
    ASSERT_TRUE(prepared.has_value());
    surface.placeFloater(*target, *prepared);

    const Vec2 leftEdge{target->rect().left() + 1.f, target->rect().bottom() + 30.f};
    surface.pointerDown({leftEdge, PointerButton::Left});
    surface.pointerMove({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    surface.pointerUp({{target->rect().right() + 500.f, leftEdge.y}, PointerButton::Left});
    EXPECT_EQ(target->rect().w, 50.f);
}

TEST(FloatersTest, ResizesFloatersMountedInModalLayer) {
    Surface surface;
    surface.setViewport(200.f, 160.f);
    auto floater = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* target = floater.get();
    floater->setResizeable(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(floater), SurfaceLayer::Modal);

    EXPECT_TRUE(surface.pointerDown({{119.f, 60.f}, PointerButton::Left}));
    surface.pointerMove({{159.f, 60.f}, PointerButton::Left});
    surface.pointerUp({{159.f, 60.f}, PointerButton::Left});
    EXPECT_EQ(target->rect().w, 140.f);
}

TEST(FloatersTest, KeepsFixedOuterSizeWhileTrackingContentGeometry) {
    StyleSheet styleSheet;
    constexpr char kFixedOuter[] = "floater { size: 300px; display: flex; flex-direction: column; } "
                                   "floater > head { height: 20px; } floater > body { display: flex; flex-direction: column; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFixedOuter).ok());
    Surface surface(styleSheet);
    surface.setViewport(500.f, 400.f);
    auto floater = radia::ui::test::makeFloater();
    floater->body()->append(makeElement<HTMLLabelElement>("first"));

    const std::optional<Rect> firstPrepared = surface.prepareFloater(*floater);
    ASSERT_TRUE(firstPrepared.has_value());
    const Rect firstOuter = *firstPrepared;
    const Vec2 firstContent = floater->authoredContentSize();
    floater->body()->append(makeElement<HTMLLabelElement>("second"));
    const std::optional<Rect> secondPrepared = surface.prepareFloater(*floater);
    ASSERT_TRUE(secondPrepared.has_value());
    const Rect secondOuter = *secondPrepared;
    const Vec2 secondContent = floater->authoredContentSize();

    EXPECT_EQ(firstOuter.x, 100.f);
    EXPECT_EQ(firstOuter.y, 50.f);
    EXPECT_EQ(firstOuter.h, secondOuter.h);
    EXPECT_GT(secondContent.y, firstContent.y);
}

TEST(FloatersTest, ResolvesPercentageGeometryAgainstViewport) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "floater { width: 50%; height: 25%; left: 10%; bottom: 10%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(400.f, 300.f);
    auto floater = makeElementValue<HTMLFloaterElement>();
    const std::optional<Rect> prepared = surface.prepareFloater(floater);
    ASSERT_TRUE(prepared.has_value());
    const Rect rect = *prepared;
    EXPECT_FLOAT_EQ(rect.w, 200.f);
    EXPECT_FLOAT_EQ(rect.h, 75.f);
    EXPECT_FLOAT_EQ(rect.x, 40.f);
    EXPECT_FLOAT_EQ(rect.y, 30.f);
}

TEST(FloatersTest, RoutesResizeThroughPointerTransparentFloater) {
    StyleSheet styleSheet;
    constexpr char kPointerTransparentLayout[] = "floater.pass-through { pointer-events: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPointerTransparentLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);
    auto lower = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* lowerTarget = lower.get();
    lower->setResizeable(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));
    auto upper = radia::ui::test::makeFloater();
    HTMLFloaterElement* upperTarget = upper.get();
    upper->setResizeable(false).setRect({70.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(upper));
    auto upperChild = makeElement<HTMLButtonElement>();
    upperChild->setRect({0.f, 0.f, 100.f, 80.f}).setPointerEvents(true);
    upperTarget->body()->append(std::move(upperChild));

    const Vec2 lowerEdgeUnderUpper{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    EXPECT_TRUE(surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});

    upperTarget->addClass("pass-through");
    EXPECT_TRUE(surface.pointerDown({lowerEdgeUnderUpper, PointerButton::Left}));
    EXPECT_TRUE(surface.hasPointerCapture());
    surface.pointerUp({lowerEdgeUnderUpper, PointerButton::Left});
}

TEST(FloatersTest, KeepsOverflowVisibleDescendantAsPointerTarget) {
    StyleSheet styleSheet;
    constexpr char kOverflowVisiblePassThroughStyle[] = "floater.pass-through { pointer-events: none; "
                                                        "overflow: visible; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOverflowVisiblePassThroughStyle).ok());
    Surface surface(styleSheet);
    surface.setViewport(200.f, 160.f);

    auto lower = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* lowerTarget = lower.get();
    lower->setResizeable(true).setRect({20.f, 20.f, 100.f, 80.f});
    surface.mountFloater(std::move(lower));

    auto upper = radia::ui::test::makeFloater();
    HTMLFloaterElement* upperTarget = upper.get();
    upper->addClass("pass-through").setRect({70.f, 20.f, 40.f, 80.f});
    surface.mountFloater(std::move(upper));

    auto overflowChild = makeElement<HTMLButtonElement>();
    overflowChild->setRect({110.f, 20.f, 40.f, 80.f}).setPointerEvents(true);
    upperTarget->body()->append(std::move(overflowChild));

    const Vec2 point{lowerTarget->rect().right() - 1.f, lowerTarget->rect().y + 40.f};
    EXPECT_TRUE(surface.pointerDown({point, PointerButton::Left}));
    EXPECT_FALSE(surface.hasPointerCapture());
}

TEST(FloatersTest, ReplacesFloaterAndReturnsRetiredRoot) {
    Surface surface;
    surface.setViewport(240.f, 180.f);
    auto original = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* originalPointer = original.get();
    original->setRect({20.f, 25.f, 100.f, 80.f});
    surface.mountFloater(std::move(original));
    radia::ui::ElementRef<HTMLFloaterElement> originalHandle(originalPointer);

    auto replacement = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* replacementPointer = replacement.get();
    replacement->setRect({30.f, 35.f, 120.f, 90.f});
    std::unique_ptr<HTMLFloaterElement> retired = surface.replaceFloater(*originalPointer, std::move(replacement));
    radia::ui::ElementRef<HTMLFloaterElement> replacementHandle(replacementPointer);

    ASSERT_NE(retired, nullptr);
    EXPECT_EQ(retired.get(), originalPointer);
    EXPECT_TRUE(surface.ownsFloater(*replacementPointer));
    EXPECT_EQ(retired->parentElement(), nullptr);
    EXPECT_EQ(replacementPointer->parentElement(), nullptr);
    EXPECT_EQ(originalHandle.get(), originalPointer);
    EXPECT_EQ(originalHandle.getMounted(), nullptr);
    EXPECT_EQ(replacementHandle.getMounted(), replacementPointer);
}
} // namespace
