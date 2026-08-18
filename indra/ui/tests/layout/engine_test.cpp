/**
 * @file engine_test.cpp
 * @brief Tests retained layout measurement, flex allocation, and arrangement.
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
#include <gtest/gtest.h>
#include "layout/engine.h"
#include "style/stylesheet.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace {
using radia::ui::arrangeTree;
using radia::ui::Button;
using radia::ui::Field;
using radia::ui::FixedTextMetrics;
using radia::ui::Floater;
using radia::ui::Flow;
using radia::ui::JustifyContent;
using radia::ui::Label;
using radia::ui::LayoutDirection;
using radia::ui::LayoutStatistics;
using radia::ui::layoutTree;
using radia::ui::measureTree;
using radia::ui::measureWidget;
using radia::ui::Panel;
using radia::ui::Rect;
using radia::ui::resolveWidgetStyle;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::StyleSheetLoadResult;
using radia::ui::Switch;
using radia::ui::Text;
using radia::ui::TextMetrics;
using radia::ui::Vec2;
using radia::ui::Visibility;
using radia::ui::Widget;
using ::testing::Message;

} // namespace

namespace {
class LayoutEngineTest : public ::testing::Test {
protected:
    FixedTextMetrics text;
};

TEST_F(LayoutEngineTest, MeasuresButtonWithIconAndLabel) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { left: 10px; top: 10px; padding: 7px; gap: 6px; flow: row; "
                                     "font-size: 13px; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    Panel root;
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = std::make_unique<Button>();
    button->setIcon("search");
    button->setLabel("Apply");
    root.addChild(std::move(button));
    layoutTree(root, styleSheet, text);
    const Widget& result = *root.children().front();
    EXPECT_EQ(result.rect().w, 72.f);
    EXPECT_EQ(result.rect().h, 32.f);
    EXPECT_EQ(result.children()[1]->rect().x - result.children()[0]->rect().right(), 6.f);
}

TEST_F(LayoutEngineTest, CentersButtonContentWithinExplicitWidth) {
    StyleSheet styleSheet;
    constexpr char kCenteredButtonLayout[] = "button { width: 128px; height: 32px; padding: 7px; gap: 6px; flow: row; "
                                             "justify-content: center; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredButtonLayout).ok());
    Panel root;
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = std::make_unique<Button>();
    button->setIcon("search");
    button->setLabel("Apply");
    root.addChild(std::move(button));
    layoutTree(root, styleSheet, text);
    const Widget& result = *root.children().front();
    const float contentWidth = result.children()[1]->rect().right() - result.children()[0]->rect().left();
    EXPECT_EQ(result.children()[0]->rect().x - result.rect().x, (result.rect().w - contentWidth) * 0.5f);
}

TEST_F(LayoutEngineTest, LaysOutColumnChildrenWithPaddingAndGap) {
    StyleSheet styleSheet;
    constexpr char kColumnLayout[] = "panel { padding: 10px; flow: column; gap: 5px; } "
                                     "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnLayout).ok());
    Panel root;
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.addChild(std::make_unique<Label>("one"));
    root.addChild(std::make_unique<Label>("two"));
    layoutTree(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().w, 80.f);
    EXPECT_EQ(root.children()[0]->rect().bottom() - root.children()[1]->rect().top(), 5.f);
}

TEST_F(LayoutEngineTest, PositionsFreeFlowChildByRightAndBottom) {
    StyleSheet styleSheet;
    constexpr char kRightBottomLayout[] = "panel { width: 40px; height: 30px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRightBottomLayout).ok());
    Panel root;
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.addChild(std::make_unique<Panel>());
    layoutTree(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().right(), 95.f);
    EXPECT_EQ(root.children()[0]->rect().bottom(), 7.f);
}

TEST_F(LayoutEngineTest, LaysOutFloaterHeaderAndContentWithinPadding) {
    StyleSheet styleSheet;
    constexpr char kFloaterLayout[] = "floater { padding: 10px; flow: column; } floater::header { height: 30px; } "
                                      "floater::content { flex-grow: 1; } floater::header::title { left: 5px; top: 5px; height: 15px; } "
                                      "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterLayout).ok());
    Floater floater;
    floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
    floater.addChild(std::make_unique<Label>("content"));
    layoutTree(floater, styleSheet, text);
    EXPECT_EQ(floater.header()->rect().top(), 90.f);
    EXPECT_EQ(floater.children()[1]->rect().top(), 60.f);
}

TEST_F(LayoutEngineTest, CollapsedFloaterHeaderDoesNotReserveSpace) {
    StyleSheet styleSheet;
    constexpr char kCollapsedFloaterLayout[] = "floater { flow: column; } floater::header { height: 30px; } "
                                               "floater::content { flex-grow: 1; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCollapsedFloaterLayout).ok());
    Floater floater;
    floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
    floater.addChild(std::make_unique<Label>("content"));
    floater.header()->setVisibility(Visibility::Collapsed);
    layoutTree(floater, styleSheet, text);
    EXPECT_EQ(floater.children()[1]->rect().top(), 100.f);
}

TEST_F(LayoutEngineTest, DistributesAutoMarginAlongRow) {
    StyleSheet styleSheet;
    constexpr char kAutoMarginLayout[] = "panel { flow: row; } label { width: 10px; height: 10px; } "
                                         "#first { margin: 0px auto 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kAutoMarginLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto first = std::make_unique<Label>("first");
    first->setId("first");
    panel.addChild(std::move(first));
    panel.addChild(std::make_unique<Label>("second"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[1]->rect().x, 90.f);
}

TEST_F(LayoutEngineTest, CentersColumnChildWithAutoMargins) {
    StyleSheet styleSheet;
    constexpr char kCenteredColumnLayout[] = "panel { flow: column; } "
                                             "label { width: 20px; height: 10px; margin: 0px auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredColumnLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<Label>("center"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 40.f);
}

TEST_F(LayoutEngineTest, CentersIconInsideOversizedButtonPadding) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { size: 24px; padding: 20px; flow: row; justify-content: center; } "
                                     "button > icon { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    Button button;
    button.setRect({0.f, 0.f, 24.f, 24.f});
    button.setIcon("search");
    layoutTree(button, styleSheet, text);
    EXPECT_EQ(button.icon()->rect().x, 4.f);
    EXPECT_EQ(button.icon()->rect().y, 4.f);
}

TEST_F(LayoutEngineTest, AlignsColumnContentToEnd) {
    StyleSheet styleSheet;
    constexpr char kColumnContentLayout[] = "panel { flow: column; justify-content: end; } "
                                            "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnContentLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<Label>("bottom"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, DistributesRemainingWidthAcrossFlexChildren) {
    StyleSheet styleSheet;
    constexpr char kFlexDistributionLayout[] = "panel { flow: row; } "
                                               "label { width: 10px; height: 10px; flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexDistributionLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().w, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineTest, MeasuresAndArrangesRowChildren) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { flow: row; gap: 3px; padding: 2px; } "
                                  "label { width: 10px; height: 8px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Panel panel;
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));

    measureTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 27.f);
    EXPECT_EQ(panel.desiredSize().y, 12.f);

    panel.setRect({0.f, 0.f, 40.f, 20.f});
    arrangeTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().right(), 25.f);
}

TEST_F(LayoutEngineTest, PreservesExplicitGeometryInFreeFlow) {
    StyleSheet styleSheet;
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto button = std::make_unique<Button>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    panel.addChild(std::move(button));

    layoutTree(panel, styleSheet, text);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.x, 10.f);
    EXPECT_EQ(rect.y, 10.f);
    EXPECT_EQ(rect.w, 20.f);
    EXPECT_EQ(rect.h, 20.f);
}

TEST_F(LayoutEngineTest, AppliesSwitchIntrinsicAndPartLayout) {
    StyleSheet styleSheet;
    constexpr char kIntrinsicSwitchLayout[] = "switch { flow: column; justify-content: center; }";
    const StyleSheetLoadResult intrinsic = styleSheet.loadRadia(kIntrinsicSwitchLayout, "switch.radia");
    ASSERT_TRUE(intrinsic.ok());
    constexpr char kSwitch[] = "switch { width: 64px; height: 32px; padding: 3px 5px; flow: column; justify-content: center; } "
                               "switch::thumb { border-radius: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitch).ok());
    Switch control;
    control.setRect({10.f, 20.f, 64.f, 32.f});

    layoutTree(control, styleSheet, text);
    EXPECT_EQ(resolveWidgetStyle(styleSheet, control).flow, Flow::Row);
    EXPECT_EQ(control.thumb()->rect().left(), 15.f);
    EXPECT_EQ(control.thumb()->rect().bottom(), 23.f);
    EXPECT_EQ(control.thumb()->rect().h, 26.f);
    EXPECT_EQ(control.thumb()->rect().w, 26.f);
    EXPECT_EQ(resolveWidgetStyle(styleSheet, *control.thumb()).borderRadius, 7.f);

    control.setChecked(true);
    layoutTree(control, styleSheet, text);
    EXPECT_EQ(resolveWidgetStyle(styleSheet, control).justifyContent, JustifyContent::End);
    EXPECT_EQ(control.thumb()->rect().right(), 69.f);

    control.clearChildren();
    ASSERT_TRUE(control.thumb());
    EXPECT_EQ(control.thumb()->part(), "thumb");
    EXPECT_EQ(control.thumb()->parent(), &control);
}

TEST_F(LayoutEngineTest, UsesInjectedTextMetricsForMeasurement) {
    class ExactTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string&, const Style&) const override { return {47.f, 19.f}; }
    } exact;

    StyleSheet styleSheet;
    Label label("adapter-owned");
    measureTree(label, styleSheet, exact);
    EXPECT_EQ(label.desiredSize().x, 47.f);
    EXPECT_EQ(label.desiredSize().y, 19.f);
}

TEST_F(LayoutEngineTest, AppliesRightToLeftRowDirection) {
    StyleSheet styleSheet;
    constexpr char kDirection[] = "panel { flow: row; justify-content: start; gap: 5px; } label { width: 10px; height: 10px; } "
                                  "#physical { margin: 0px 7px 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDirection).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>("first"));
    auto physical = std::make_unique<Label>("second");
    physical->setId("physical");
    panel.addChild(std::move(physical));

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 78.f);
}

TEST_F(LayoutEngineTest, PreservesNegativeFreeFlowOffsets) {
    StyleSheet styleSheet;
    constexpr char kNegativeOffsetLayout[] = "label { width: 10px; height: 10px; left: -8px; bottom: -3px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNegativeOffsetLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<Label>("offset"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), -8.f);
    EXPECT_EQ(panel.children()[0]->rect().bottom(), -3.f);
}

TEST_F(LayoutEngineTest, PreservesFieldDefaultRowFlow) {
    StyleSheet styleSheet;
    constexpr char kField[] = "field { flow: column; } switch { width: 20px; height: 10px; } "
                              "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kField).ok());

    Field field;
    field.setRect({0.f, 0.f, 50.f, 10.f});
    field.addChild(std::make_unique<Label>("Label"));
    field.addChild(std::make_unique<Switch>());
    layoutTree(field, styleSheet, text);
    EXPECT_EQ(field.children()[1]->rect().left(), 30.f);
}

TEST_F(LayoutEngineTest, ResolvesPercentageGeometryAgainstContainingBlock) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "label { width: 50%; height: 25%; left: 10%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 200.f, 100.f});
    panel.addChild(std::make_unique<Label>("percentage"));

    layoutTree(panel, styleSheet, text);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_NEAR(rect.w, 100.f, 6);
    EXPECT_NEAR(rect.h, 25.f, 6);
    EXPECT_NEAR(rect.left(), 20.f, 6);
    EXPECT_NEAR(rect.top(), 80.f, 6);
}

TEST_F(LayoutEngineTest, DistributesAutomaticRowAndColumnGaps) {
    StyleSheet styleSheet;
    constexpr char kRowGapLayout[] = "panel { flow: row; gap: auto; } "
                                     "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowGapLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));
    panel.addChild(std::make_unique<Label>("third"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    constexpr char kColumnGapLayout[] = "panel { flow: column; gap: auto; } "
                                        "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnGapLayout).ok());
    panel.setRect({0.f, 0.f, 20.f, 100.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().top(), 55.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, MeasuresFloaterWithFixedHeightAndAutomaticWidth) {
    StyleSheet styleSheet;
    constexpr char kFloater[] = "floater { size: auto 100px; flow: column; } floater::header { height: 30px; } "
                                "floater::content { flow: column; gap: 5px; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloater).ok());
    Floater floater;
    floater.setCanClose(false);
    floater.addChild(std::make_unique<Label>("first"));
    floater.addChild(std::make_unique<Label>("second"));

    const Vec2 measured = measureWidget(floater, styleSheet, text);
    EXPECT_EQ(measured.x, 100.f);
    EXPECT_EQ(measured.y, 75.f);
}

TEST_F(LayoutEngineTest, AppliesCrossAxisAlignmentAndDirection) {
    StyleSheet styleSheet;
    constexpr char kRowAlignment[] = "panel { flow: row; align-items: start; } label { width: 10px; height: 10px; } "
                                     "label#center { align-self: center; } label#end { align-self: end; } "
                                     "label#stretch { height: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowAlignment).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end", "stretch"}) {
        auto label = std::make_unique<Label>(id);
        label->setId(id);
        panel.addChild(std::move(label));
    }

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 15.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[3]->rect().h, 40.f);

    constexpr char kColumnAlignment[] = "panel { flow: column; align-items: start; } label { width: 10px; height: 10px; } "
                                        "label#center { align-self: center; } label#end { align-self: end; } "
                                        "label#stretch { width: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnAlignment).ok());
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[3]->rect().w, 100.f);

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineTest, DistinguishesHiddenFromCollapsedLayout) {
    StyleSheet styleSheet;
    constexpr char kVisibilityLayout[] = "panel { flow: row; } "
                                         "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibilityLayout).ok());
    Panel panel;
    panel.addChild(std::make_unique<Label>("visible"));
    auto hidden = std::make_unique<Label>("hidden");
    hidden->setVisibility(Visibility::Hidden);
    panel.addChild(std::move(hidden));
    auto collapsed = std::make_unique<Label>("collapsed");
    collapsed->setVisibility(Visibility::Collapsed);
    panel.addChild(std::move(collapsed));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 10.f);
    EXPECT_EQ(measureWidget(*panel.children()[2], styleSheet, text).x, 0.f);

    panel.children()[1]->setVisibility(Visibility::Collapsed);
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 10.f);

    panel.children()[2]->setVisibility(Visibility::Visible);
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 10.f);
}

TEST_F(LayoutEngineTest, AppliesContainerVerticalAlignmentToContent) {
    StyleSheet styleSheet;
    constexpr char kVerticalAlignment[] = "panel { size: 40px 100px; flow: row; } label { size: 10px; } "
                                          "panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } "
                                          "panel.column { flow: column; vertical-align: bottom; } "
                                          "panel.free-bottom { flow: free; vertical-align: bottom; } "
                                          "button { size: 40px 100px; } button > * { size: 10px; } "
                                          "button.top { vertical-align: top; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVerticalAlignment).ok());

    auto addLabel = [](Widget& container) { container.addChild(std::make_unique<Label>("child")); };

    Panel top;
    top.setRect({0.f, 0.f, 100.f, 40.f});
    addLabel(top);
    layoutTree(top, styleSheet, text);
    EXPECT_EQ(top.children()[0]->rect().top(), 40.f);

    Panel middle;
    middle.setRect({0.f, 0.f, 100.f, 40.f});
    middle.addClass("middle");
    addLabel(middle);
    layoutTree(middle, styleSheet, text);
    EXPECT_EQ(middle.children()[0]->rect().bottom(), 15.f);

    Panel bottom;
    bottom.setRect({0.f, 0.f, 100.f, 40.f});
    bottom.addClass("bottom");
    addLabel(bottom);
    layoutTree(bottom, styleSheet, text);
    EXPECT_EQ(bottom.children()[0]->rect().bottom(), 0.f);

    Panel column;
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.addClass("column");
    addLabel(column);
    addLabel(column);
    layoutTree(column, styleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().top(), 20.f);
    EXPECT_EQ(column.children()[1]->rect().bottom(), 0.f);

    Panel freeBottom;
    freeBottom.setRect({0.f, 0.f, 100.f, 40.f});
    freeBottom.addClass("free-bottom");
    addLabel(freeBottom);
    layoutTree(freeBottom, styleSheet, text);
    EXPECT_EQ(freeBottom.children()[0]->rect().bottom(), 0.f);

    Button button;
    button.setRect({0.f, 0.f, 100.f, 40.f});
    button.setLabel("button");
    layoutTree(button, styleSheet, text);
    EXPECT_EQ(button.children()[0]->rect().bottom(), 15.f);
    EXPECT_EQ(button.children()[0]->rect().left(), 45.f);

    Button topButton;
    topButton.setRect({0.f, 0.f, 100.f, 40.f});
    topButton.addClass("top");
    topButton.setLabel("button");
    layoutTree(topButton, styleSheet, text);
    EXPECT_EQ(topButton.children()[0]->rect().top(), 40.f);
}

TEST_F(LayoutEngineTest, WrapsChildrenAcrossFlowBreaks) {
    StyleSheet styleSheet;
    constexpr char kFlowBreakLayout[] = "panel { flow: row; gap: 2px; } "
                                        "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlowBreakLayout).ok());

    Panel panel;
    auto first = std::make_unique<Label>("first");
    auto second = std::make_unique<Label>("second");
    auto third = std::make_unique<Label>("third");
    radia::ui::detail::WidgetCompilerAccess::setFlowBreakBefore(*second, true);
    panel.addChild(std::move(first));
    panel.addChild(std::move(second));
    panel.addChild(std::move(third));

    measureTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 22.f);
    EXPECT_EQ(panel.desiredSize().y, 22.f);

    panel.setRect({0.f, 0.f, 40.f, 40.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 28.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 12.f);
}

TEST_F(LayoutEngineTest, AppliesFlexBasisAndScaledShrink) {
    StyleSheet styleSheet;
    constexpr char kFlexBasisLayout[] = "panel { flow: row; } label { height: 10px; flex: 0 1 80px; } "
                                        "label.second { flex-basis: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexBasisLayout).ok());

    Panel intrinsic;
    intrinsic.addChild(std::make_unique<Label>());
    auto intrinsicSecond = std::make_unique<Label>();
    intrinsicSecond->addClass("second");
    intrinsic.addChild(std::move(intrinsicSecond));
    measureTree(intrinsic, styleSheet, text);
    EXPECT_EQ(intrinsic.desiredSize().x, 120.f);

    StyleSheet percentageTheme;
    constexpr char kPercentageFlexBasisLayout[] = "panel { flow: row; } "
                                                  "label { width: 30px; flex-basis: 50%; }";
    ASSERT_TRUE(percentageTheme.loadRadia(kPercentageFlexBasisLayout).ok());
    Panel indefinite;
    indefinite.addChild(std::make_unique<Label>());
    measureTree(indefinite, percentageTheme, text);
    EXPECT_EQ(indefinite.desiredSize().x, 30.f);

    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>());
    auto second = std::make_unique<Label>();
    second->addClass("second");
    panel.addChild(std::move(second));
    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().w, 200.f / 3.f, 5);
    EXPECT_NEAR(panel.children()[1]->rect().w, 100.f / 3.f, 5);
    EXPECT_NEAR(panel.children()[1]->rect().right(), 100.f, 5);
}

TEST_F(LayoutEngineTest, CentersOversizedFloaterHeaderChildren) {
    StyleSheet styleSheet;
    constexpr char kFloaterHeader[] = "floater::header { height: 48px; flow: row; padding: 12px; } "
                                      "floater::header::icon { size: 28px; } floater::header::title { line-height: 18px; } "
                                      "floater::header::custom { height: 24px; flex-grow: 1; } "
                                      "floater::header::close { size: 24px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterHeader).ok());

    Floater floater;
    floater.setIcon("search").setTitle("title");
    floater.header()->setRect({0.f, 0.f, 200.f, 48.f});
    layoutTree(*floater.header(), styleSheet, text);

    const float headerCenter = floater.header()->rect().y + floater.header()->rect().h * .5f;
    for (const auto& child : floater.header()->children()) {
        if (child->visibility() != Visibility::Visible) continue;
        const float childCenter = child->rect().y + child->rect().h * .5f;
        SCOPED_TRACE(Message() << "header part: " << child->part());
        EXPECT_FLOAT_EQ(childCenter, headerCenter);
    }
}

TEST_F(LayoutEngineTest, ReflowsWrappedTextInColumnLayout) {
    StyleSheet styleSheet;
    constexpr char kWrapping[] = "panel { flow: column; } "
                                 "text { font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapping).ok());

    Panel panel;
    panel.setRect({0.f, 0.f, 30.f, 100.f});
    panel.addChild(std::make_unique<Text>("alpha beta"));
    panel.addChild(std::make_unique<Text>("after"));

    layoutTree(panel, styleSheet, text);
    const Widget& wrapped = *panel.children()[0];
    const Widget& following = *panel.children()[1];
    EXPECT_EQ(wrapped.rect().h, 20.f);
    EXPECT_EQ(following.rect().top(), wrapped.rect().bottom());
}

TEST_F(LayoutEngineTest, RemeasuresWrappedTextAfterRowFlexShrink) {
    StyleSheet styleSheet;
    constexpr char kRowWrapping[] = "panel { width: 45px; flow: row; align-items: start; } "
                                    "text { min-width: 0px; flex: 1; font-size: 10px; line-height: 10px; text-wrap: wrap; } "
                                    "label { width: 10px; flex-shrink: 0; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowWrapping).ok());

    Panel panel;
    panel.addChild(std::make_unique<Text>("alpha beta"));
    panel.addChild(std::make_unique<Label>("x"));

    const Vec2 measured = measureWidget(panel, styleSheet, text);
    EXPECT_EQ(measured.y, 20.f);

    panel.setRect({0.f, 0.f, measured.x, measured.y});
    const LayoutStatistics layoutStats = layoutTree(panel, styleSheet, text);
    EXPECT_GT(layoutStats.constrainedRemeasures, std::size_t(0));
    EXPECT_EQ(panel.children()[0]->rect().h, 20.f);
    EXPECT_EQ(panel.children()[1]->rect().w, 10.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, ReappliesFlexBasisAfterTextReflow) {
    StyleSheet columnTheme;
    constexpr char kColumnBasis[] = "panel { width: 30px; flow: column; } "
                                    "text { flex-basis: 40px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(columnTheme.loadRadia(kColumnBasis).ok());
    Panel column;
    column.addChild(std::make_unique<Text>("alpha beta"));
    EXPECT_EQ(measureWidget(column, columnTheme, text).y, 40.f);

    StyleSheet rowTheme;
    constexpr char kRowMinimum[] = "panel { width: 80px; flow: row; } text { flex: 0 1 100px; font-size: 10px; "
                                   "line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; }";
    ASSERT_TRUE(rowTheme.loadRadia(kRowMinimum).ok());
    Panel row;
    row.addChild(std::make_unique<Text>("alpha beta"));
    row.addChild(std::make_unique<Label>("x"));
    row.setRect({0.f, 0.f, 80.f, 20.f});
    layoutTree(row, rowTheme, text);
    EXPECT_EQ(row.children()[0]->rect().w, 70.f);
}
TEST_F(LayoutEngineTest, ReusesCleanLayoutCache) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { flow: row; } "
                                  "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));

    const LayoutStatistics first = layoutTree(panel, styleSheet, text);
    const LayoutStatistics second = layoutTree(panel, styleSheet, text);
    EXPECT_GT(first.measuredNodes, std::size_t(0));
    EXPECT_GT(first.arrangedNodes, std::size_t(0));
    EXPECT_EQ(second.measuredNodes, std::size_t(0));
    EXPECT_EQ(second.arrangedNodes, std::size_t(0));
    EXPECT_GE(second.skippedNodes, std::size_t(2));
}

TEST_F(LayoutEngineTest, InvalidatesLayoutCacheForResizeAndDirection) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { flow: row; } "
                                  "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));
    layoutTree(panel, styleSheet, text);

    panel.setRect({0.f, 0.f, 140.f, 20.f});
    const LayoutStatistics resized = layoutTree(panel, styleSheet, text);
    EXPECT_GT(resized.measuredNodes, std::size_t(0));
    EXPECT_GT(resized.arrangedNodes, std::size_t(0));

    const LayoutStatistics directionChanged = layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(directionChanged.measuredNodes, std::size_t(0));
    EXPECT_GT(directionChanged.arrangedNodes, std::size_t(0));
}

TEST_F(LayoutEngineTest, InvalidatesTextMeasurementCacheWhenMetricsGenerationChanges) {
    class GenerationTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string&, const Style&) const override { return {32.f, 12.f}; }
        std::uint64_t generation() const override { return mGeneration; }
        void advance() { ++mGeneration; }

    private:
        std::uint64_t mGeneration = 1;
    } metrics;

    StyleSheet styleSheet;
    Label label("generation");
    const LayoutStatistics first = layoutTree(label, styleSheet, metrics);
    metrics.advance();
    const LayoutStatistics second = layoutTree(label, styleSheet, metrics);
    EXPECT_GT(first.measuredNodes, std::size_t(0));
    EXPECT_GT(second.measuredNodes, std::size_t(0));
}

TEST_F(LayoutEngineTest, ReallocatesColumnFlexChildrenUnderHeightConstraint) {
    StyleSheet styleSheet;
    constexpr char kColumnFlexLayout[] = "panel { flow: column; } "
                                         "label { height: 10px; flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnFlexLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));

    const LayoutStatistics stats = layoutTree(panel, styleSheet, text);
    EXPECT_GE(stats.constrainedRemeasures, std::size_t(2));
    EXPECT_EQ(panel.children()[0]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, ResolvesSiblingColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kSiblingColumnLayout[] = "panel { flow: column; } "
                                            "#quarter { height: 25%; } #half { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSiblingColumnLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto quarter = std::make_unique<Label>("quarter");
    quarter->setId("quarter");
    auto half = std::make_unique<Label>("half");
    half->setId("half");
    panel.addChild(std::move(quarter));
    panel.addChild(std::move(half));

    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().h, 25.f, 6);
    EXPECT_NEAR(panel.children()[1]->rect().h, 50.f, 6);
    EXPECT_NEAR(panel.children()[0]->rect().bottom(), panel.children()[1]->rect().top(), 6);
}

TEST_F(LayoutEngineTest, ResolvesNestedColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kNestedColumnLayout[] = "panel { flow: column; } #child { height: 50%; flow: column; } "
                                           "#grandchild { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedColumnLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<Panel>();
    child->setId("child");
    auto grandchild = std::make_unique<Label>("nested");
    grandchild->setId("grandchild");
    child->addChild(std::move(grandchild));
    panel.addChild(std::move(child));

    layoutTree(panel, styleSheet, text);
    const Widget& nestedPanel = *panel.children().front();
    EXPECT_NEAR(nestedPanel.rect().h, 50.f, 6);
    EXPECT_NEAR(nestedPanel.children().front()->rect().h, 25.f, 6);
}

TEST_F(LayoutEngineTest, RespectsColumnMinimumHeightsDuringShrink) {
    StyleSheet styleSheet;
    constexpr char kColumnMinimumLayout[] = "panel { flow: column; } "
                                            "label { height: 30px; min-height: 25px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnMinimumLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 25.f);
}

TEST_F(LayoutEngineTest, ReusesHeightConstrainedColumnCache) {
    StyleSheet styleSheet;
    constexpr char kFlexLayout[] = "panel { flow: column; } "
                                   "label { flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<Label>("first"));
    panel.addChild(std::make_unique<Label>("second"));

    const LayoutStatistics first = layoutTree(panel, styleSheet, text);
    const LayoutStatistics clean = layoutTree(panel, styleSheet, text);
    EXPECT_GT(first.constrainedRemeasures, std::size_t(0));
    EXPECT_EQ(clean.measuredNodes, std::size_t(0));
    EXPECT_EQ(clean.arrangedNodes, std::size_t(0));

    panel.setRect({0.f, 0.f, 100.f, 140.f});
    const LayoutStatistics resized = layoutTree(panel, styleSheet, text);
    EXPECT_GT(resized.constrainedRemeasures, std::size_t(0));
}

TEST_F(LayoutEngineTest, SeparatesLayoutCachesByStylesheetIdentity) {
    StyleSheet narrow;
    StyleSheet wide;
    constexpr char kNarrowLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kWideLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(narrow.loadRadia(kNarrowLabelLayout).ok());
    ASSERT_TRUE(wide.loadRadia(kWideLabelLayout).ok());

    Label label("identity");
    layoutTree(label, narrow, text);
    layoutTree(label, wide, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineTest, SeparatesLayoutCachesByTextMetricsIdentity) {
    class WidthMetrics final : public TextMetrics {
    public:
        explicit WidthMetrics(float width) : mWidth(width) {}
        Vec2 measureText(const std::string&, const Style&) const override { return {mWidth, 12.f}; }

    private:
        float mWidth;
    } narrow(12.f), wide(48.f);

    StyleSheet styleSheet;
    Label label("identity");
    layoutTree(label, styleSheet, narrow);
    layoutTree(label, styleSheet, wide);
    EXPECT_EQ(label.desiredSize().x, 48.f);
}

TEST_F(LayoutEngineTest, OrdersChildrenBeforeRowLayout) {
    StyleSheet styleSheet;
    constexpr char kOrderedFlow[] = "panel { flow: row; } #late { order: 2; width: 10px; height: 10px; } "
                                    "#early { order: -1; width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOrderedFlow).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto late = std::make_unique<Label>("late");
    late->setId("late");
    auto early = std::make_unique<Label>("early");
    early->setId("early");
    panel.addChild(std::move(late));
    panel.addChild(std::move(early));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[0]->rect().x, 10.f);
}

TEST_F(LayoutEngineTest, InvalidatesLayoutCacheWhenStylesheetIsAssigned) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kReplacementLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    Label label("assigned");
    layoutTree(label, styleSheet, text);

    StyleSheet replacement;
    ASSERT_TRUE(replacement.loadRadia(kReplacementLabelLayout).ok());
    styleSheet = replacement;
    layoutTree(label, styleSheet, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineTest, RemeasuresFreeFlowPercentageTextAfterResize) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowTextLayout[] = "panel { flow: free; } "
                                           "text { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowTextLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<Text>("alpha beta"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);

    panel.setRect({0.f, 0.f, 200.f, 100.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 10.f);
}

TEST_F(LayoutEngineTest, IncludesFreeFlowOffsetsInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowOffsetLayout[] = "panel { flow: free; } "
                                             "label { width: 20px; height: 10px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowOffsetLayout).ok());
    Panel panel;
    panel.addChild(std::make_unique<Label>("positioned"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 25.f);
    EXPECT_EQ(panel.desiredSize().y, 17.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitFreeFlowGeometryInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowLayout[] = "panel { flow: free; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowLayout).ok());
    Panel panel;
    auto child = std::make_unique<Panel>();
    child->setRect({0.f, 0.f, 40.f, 30.f});
    panel.addChild(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 40.f);
    EXPECT_EQ(panel.desiredSize().y, 30.f);
}

TEST_F(LayoutEngineTest, InvalidatesIntrinsicSizeAfterExplicitGeometryChange) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowLayout[] = "panel { flow: free; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowLayout).ok());
    Panel panel;
    auto child = std::make_unique<Panel>();
    child->setRect({0.f, 0.f, 20.f, 10.f});
    Widget* childPtr = child.get();
    panel.addChild(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);

    childPtr->setRect({0.f, 0.f, 60.f, 10.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 60.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitFreeFlowPositionInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowLayout[] = "panel { flow: free; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowLayout).ok());
    Panel panel;
    auto child = std::make_unique<Panel>();
    child->setRect({10.f, 12.f, 20.f, 8.f});
    panel.addChild(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
}

TEST_F(LayoutEngineTest, IncludesWrappedPercentageChildInFreeFlowIntrinsicHeight) {
    StyleSheet styleSheet;
    constexpr char kWrappedTextLayout[] = "panel { flow: free; width: 100px; } "
                                          "text { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrappedTextLayout).ok());
    Panel panel;
    panel.addChild(std::make_unique<Text>("alpha beta"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, IncludesFreeFlowPercentageOffsetsInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowPercentageOffsetLayout[] = "panel { flow: free; } "
                                                       "label { width: 20px; height: 10px; left: 50%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowPercentageOffsetLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<Label>("positioned"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 70.f);
    EXPECT_NEAR(panel.desiredSize().y, 30.f, 6);
}

TEST_F(LayoutEngineTest, ResolvesFreeFlowPercentageDimensionsAgainstExplicitParent) {
    StyleSheet styleSheet;
    constexpr char kFreeFlowPercentageLayout[] = "panel { flow: free; width: 50%; height: 50%; } "
                                                 "label { width: 50%; height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFreeFlowPercentageLayout).ok());
    Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 80.f});
    panel.addChild(std::make_unique<Label>("sized"));

    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children().front()->rect().w, 50.f, 6);
    EXPECT_NEAR(panel.children().front()->rect().h, 40.f, 6);
}

TEST_F(LayoutEngineTest, ResolvesNestedPercentageFlexBasis) {
    StyleSheet styleSheet;
    constexpr char kNestedFlexBasis[] = "#outer { flow: row; width: 100px; height: 20px; } "
                                        "#inner { flow: row; width: 50%; } label { flex-basis: 50%; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedFlexBasis).ok());
    Panel outer;
    outer.setRect({0.f, 0.f, 100.f, 20.f});
    outer.setId("outer");
    auto inner = std::make_unique<Panel>();
    inner->setId("inner");
    Widget* innerPtr = inner.get();
    inner->addChild(std::make_unique<Label>("basis"));
    Widget* label = inner->children().front().get();
    outer.addChild(std::move(inner));

    layoutTree(outer, styleSheet, text);
    EXPECT_NEAR(innerPtr->rect().w, 50.f, 6);
    EXPECT_NEAR(label->rect().w, 25.f, 6);
}
} // namespace
