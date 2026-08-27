/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include "../floater_test_helpers.h"
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "elements/elementtext.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "layout/engine.h"
#include "style/stylesheet.h"
#include "text/metrics.h"

namespace {
using radia::ui::arrangeTree;
using radia::ui::ButtonElement;
using radia::ui::DisplayMode;
using radia::ui::Element;
using radia::ui::FixedTextMetrics;
using radia::ui::FloaterElement;
using radia::ui::IconElement;
using radia::ui::InputElement;
using radia::ui::LabelElement;
using radia::ui::LayoutDirection;
using radia::ui::LayoutStatistics;
using radia::ui::layoutTree;
using radia::ui::measureElement;
using radia::ui::measureTree;
using radia::ui::PanelElement;
using radia::ui::Rect;
using radia::ui::resolveElementStyle;
using radia::ui::ScrollbarMode;
using radia::ui::ScrollLayoutOptions;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::StyleSheetLoadResult;
using radia::ui::Text;
using radia::ui::TextMetrics;
using radia::ui::Vec2;
using radia::ui::Visibility;
using ::testing::Message;
} // namespace

namespace {
std::unique_ptr<Element> makeParagraph(std::string text) {
    auto paragraph = std::make_unique<Element>("p");
    paragraph->textContent(std::move(text));
    return paragraph;
}

IconElement& appendIcon(ButtonElement& button, std::string name) {
    auto icon = std::make_unique<IconElement>(std::move(name));
    IconElement* result = icon.get();
    button.append(std::move(icon));
    return *result;
}

void appendButtonText(ButtonElement& button, std::string text) {
    radia::ui::detail::appendText(button, std::move(text));
}

class LayoutEngineTest : public ::testing::Test {
protected:
    FixedTextMetrics text;
};

TEST_F(LayoutEngineTest, MeasuresButtonWithIconAndLabel) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { position: relative; left: 10px; top: 10px; padding: 7px; gap: 6px; display: flex; flex-direction: row; "
        "font-size: 13px; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    PanelElement root;
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = std::make_unique<ButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    layoutTree(root, styleSheet, text);
    const Element& result = *root.children().front();
    EXPECT_EQ(result.rect().w, 300.f);
    EXPECT_EQ(result.rect().h, 32.f);
    const auto runtimeChildren = radia::ui::detail::nodes(result);
    ASSERT_EQ(runtimeChildren.size(), 2U);
    EXPECT_EQ(runtimeChildren.begin()->asElement()->elementName(), "icon");
    auto textChild = runtimeChildren.begin();
    ++textChild;
    ASSERT_NE(textChild->asText(), nullptr);
    EXPECT_EQ(textChild->asText()->rect().x - runtimeChildren.begin()->asElement()->rect().right(), 6.f);
}

TEST_F(LayoutEngineTest, IgnoresWhitespaceOnlyTextBetweenFlexItems) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { width: 100px; height: 20px; padding: 0; gap: 6px; display: flex; flex-direction: row; justify-content: start; } "
        "button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    ButtonElement button;
    button.setRect({0.f, 0.f, 100.f, 20.f});
    auto leadingWhitespace = std::make_unique<Text>("\n        ");
    Text* leadingWhitespacePtr = leadingWhitespace.get();
    button.append(std::move(leadingWhitespace));
    IconElement& icon = appendIcon(button, "search");
    auto betweenWhitespace = std::make_unique<Text>("\n        ");
    Text* betweenWhitespacePtr = betweenWhitespace.get();
    button.append(std::move(betweenWhitespace));
    auto label = std::make_unique<Text>("Press");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    layoutTree(button, styleSheet, text);

    EXPECT_EQ(leadingWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(betweenWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(icon.rect().x, 0.f);
    EXPECT_EQ(labelPtr->rect().x - icon.rect().right(), 6.f);
}

TEST_F(LayoutEngineTest, PreservesWhitespaceBetweenAdjacentInlineRuns) {
    StyleSheet styleSheet;
    constexpr char kInlineLayout[] = "p { display: block; } i, s { display: inline; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInlineLayout).ok());

    Element paragraph("p");
    paragraph.setRect({0.f, 0.f, 200.f, 20.f});
    auto italic = std::make_unique<Element>("i");
    Element* italicPtr = italic.get();
    italic->append(std::make_unique<Text>("Italic I run"));
    auto separator = std::make_unique<Text>(" ");
    Text* separatorPtr = separator.get();
    auto strike = std::make_unique<Element>("s");
    Element* strikePtr = strike.get();
    strike->append(std::make_unique<Text>("Strike S run"));
    paragraph.append(std::move(italic));
    paragraph.append(std::move(separator));
    paragraph.append(std::move(strike));

    layoutTree(paragraph, styleSheet, text);

    EXPECT_GT(separatorPtr->rect().w, 0.f);
    EXPECT_FLOAT_EQ(separatorPtr->rect().x, italicPtr->rect().right());
    EXPECT_FLOAT_EQ(strikePtr->rect().x, separatorPtr->rect().right());
}

TEST_F(LayoutEngineTest, IgnoresFormattingWhitespaceBeforeNormalFlowChildren) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } p { display: block; height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 60.f});
    auto leadingWhitespace = std::make_unique<Text>("\n        ");
    Text* leadingWhitespacePtr = leadingWhitespace.get();
    panel.append(std::move(leadingWhitespace));
    auto paragraph = makeParagraph("status");
    Element* paragraphPtr = paragraph.get();
    panel.append(std::move(paragraph));

    layoutTree(panel, styleSheet, text);

    EXPECT_EQ(leadingWhitespacePtr->rect().h, 0.f);
    EXPECT_EQ(paragraphPtr->rect().top(), 60.f);
}

TEST_F(LayoutEngineTest, FloaterBodyStartsAtFirstElementAfterFormattingWhitespace) {
    StyleSheet styleSheet;
    constexpr char kFloaterLayout[] = "floater { width: 100px; height: 100px; display: flex; flex-direction: column; } "
                                      "floater > head { height: 20px; } "
                                      "floater > body { display: flex; flex-direction: column; flex-grow: 1; margin: 0; padding: 0; gap: 0; } "
                                      "p { height: 18px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterLayout).ok());

    FloaterElement floater;
    radia::ui::test::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(std::make_unique<Text>("\n        "));
    auto status = std::make_unique<Element>("p");
    Element* statusPtr = status.get();
    status->setId("status");
    status->textContent("Ready");
    floater.body()->append(std::move(status));

    layoutTree(floater, styleSheet, text);

    EXPECT_EQ(statusPtr->rect().top(), floater.body()->rect().top());
}

TEST_F(LayoutEngineTest, LaysOutInlineSiblingsAndBlockChildrenInNormalFlow) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] =
        "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; } .block { display: block; width: 50px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 60.f});
    auto first = std::make_unique<LabelElement>("first");
    first->addClass("inline");
    panel.append(std::move(first));
    auto second = std::make_unique<LabelElement>("second");
    second->addClass("inline");
    panel.append(std::move(second));
    auto block = std::make_unique<LabelElement>("block");
    block->addClass("block");
    panel.append(std::move(block));
    auto after = std::make_unique<LabelElement>("after");
    after->addClass("inline");
    panel.append(std::move(after));

    layoutTree(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 30.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 60.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 50.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 40.f);
}

TEST_F(LayoutEngineTest, WrapsInlineSiblingsAtTheContainingBlockWidth) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 50.f, 40.f});
    auto first = std::make_unique<LabelElement>("first");
    first->addClass("inline");
    panel.append(std::move(first));
    auto second = std::make_unique<LabelElement>("second");
    second->addClass("inline");
    panel.append(std::move(second));

    layoutTree(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 30.f);
}

TEST_F(LayoutEngineTest, CentersButtonContentWithinExplicitWidth) {
    StyleSheet styleSheet;
    constexpr char kCenteredButtonLayout[] = "button { width: 128px; height: 32px; padding: 7px; gap: 6px; display: flex; flex-direction: row; "
                                             "justify-content: center; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredButtonLayout).ok());
    PanelElement root;
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = std::make_unique<ButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    layoutTree(root, styleSheet, text);
    const Element& result = *root.children().front();
    const auto runtimeChildren = radia::ui::detail::nodes(result);
    ASSERT_EQ(runtimeChildren.size(), 2U);
    auto first = runtimeChildren.begin();
    auto second = first;
    ++second;
    ASSERT_NE(first->asElement(), nullptr);
    ASSERT_NE(second->asText(), nullptr);
    const float contentWidth = second->asText()->rect().right() - first->asElement()->rect().left();
    EXPECT_EQ(first->asElement()->rect().x - result.rect().x, (result.rect().w - contentWidth) * 0.5f);
}

TEST_F(LayoutEngineTest, LaysOutColumnChildrenWithPaddingAndGap) {
    StyleSheet styleSheet;
    constexpr char kColumnLayout[] = "panel { padding: 10px; display: flex; flex-direction: column; gap: 5px; } "
                                     "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnLayout).ok());
    PanelElement root;
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(std::make_unique<LabelElement>("one"));
    root.append(std::make_unique<LabelElement>("two"));
    layoutTree(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().w, 80.f);
    EXPECT_EQ(root.children()[0]->rect().bottom() - root.children()[1]->rect().top(), 5.f);
}

TEST_F(LayoutEngineTest, KeepsPaddingInTheClientBoxAndOutOfChildContent) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: hidden; padding: 10px 20px 30px 40px; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 40.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 80.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 30.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 90.f);
}

TEST_F(LayoutEngineTest, KeepsPaddingAfterOverflowingContent) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: hidden; padding: 10px 20px 30px 40px; } "
                               "#content { display: block; width: 100px; height: 100px; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setId("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 160.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 140.f);
}

TEST_F(LayoutEngineTest, KeepsBorderOutsidePaddingScrollport) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: hidden; border: 2px #ffffff; padding: 4px; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 96.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 96.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 6.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 94.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 6.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 94.f);
}

TEST_F(LayoutEngineTest, PositionsNormalChildByRightAndBottom) {
    StyleSheet styleSheet;
    constexpr char kRightBottomLayout[] = "panel { position: relative; width: 40px; height: 30px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRightBottomLayout).ok());
    PanelElement root;
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(std::make_unique<PanelElement>());
    layoutTree(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().right(), 35.f);
    EXPECT_EQ(root.children()[0]->rect().bottom(), 77.f);
}

TEST_F(LayoutEngineTest, LaysOutFloaterHeadAndBodyWithinPadding) {
    StyleSheet styleSheet;
    constexpr char kFloaterLayout[] =
        "floater { padding: 10px; display: flex; flex-direction: column; } floater > head { height: 30px; } "
        "floater > body { flex-grow: 1; } floater > head > title { position: relative; left: 5px; top: 5px; height: 15px; } "
        "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterLayout).ok());
    FloaterElement floater;
    radia::ui::test::appendFloaterStructure(floater);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(std::make_unique<LabelElement>("content"));
    layoutTree(floater, styleSheet, text);
    EXPECT_EQ(floater.head()->rect().top(), 90.f);
    EXPECT_EQ(floater.body()->rect().top(), 60.f);
}

TEST_F(LayoutEngineTest, DisplayNoneFloaterHeadDoesNotReserveSpace) {
    StyleSheet styleSheet;
    constexpr char kCollapsedFloaterLayout[] = "floater { display: flex; flex-direction: column; } floater > head { display: none; height: 30px; } "
                                               "floater > body { flex-grow: 1; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCollapsedFloaterLayout).ok());
    FloaterElement floater;
    radia::ui::test::appendFloaterStructure(floater);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(std::make_unique<LabelElement>("content"));
    layoutTree(floater, styleSheet, text);
    EXPECT_EQ(floater.body()->rect().top(), 100.f);
}

TEST_F(LayoutEngineTest, DistributesAutoMarginAlongRow) {
    StyleSheet styleSheet;
    constexpr char kAutoMarginLayout[] = "panel { display: flex; flex-direction: row; } label { width: 10px; height: 10px; } "
                                         "#first { margin: 0px auto 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kAutoMarginLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto first = std::make_unique<LabelElement>("first");
    first->setId("first");
    panel.append(std::move(first));
    panel.append(std::make_unique<LabelElement>("second"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[1]->rect().x, 90.f);
}

TEST_F(LayoutEngineTest, CentersColumnChildWithAutoMargins) {
    StyleSheet styleSheet;
    constexpr char kCenteredColumnLayout[] = "panel { display: flex; flex-direction: column; } "
                                             "label { width: 20px; height: 10px; margin: 0px auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredColumnLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(std::make_unique<LabelElement>("center"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 40.f);
}

TEST_F(LayoutEngineTest, CentersIconInsideOversizedButtonPadding) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { size: 24px; padding: 20px; display: flex; flex-direction: row; justify-content: center; } "
                                     "button > icon { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    ButtonElement button;
    button.setRect({0.f, 0.f, 24.f, 24.f});
    IconElement& icon = appendIcon(button, "search");
    layoutTree(button, styleSheet, text);
    EXPECT_EQ(icon.rect().x, 4.f);
    EXPECT_EQ(icon.rect().y, 4.f);
}

TEST_F(LayoutEngineTest, AlignsColumnContentToEnd) {
    StyleSheet styleSheet;
    constexpr char kColumnContentLayout[] = "panel { display: flex; flex-direction: column; justify-content: end; } "
                                            "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnContentLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(std::make_unique<LabelElement>("bottom"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, DistributesRemainingWidthAcrossFlexChildren) {
    StyleSheet styleSheet;
    constexpr char kFlexDistributionLayout[] = "panel { display: flex; flex-direction: row; } "
                                               "label { width: 10px; height: 10px; flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexDistributionLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().w, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineTest, MeasuresAndArrangesRowChildren) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; gap: 3px; padding: 2px; } "
                                  "label { width: 10px; height: 8px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    PanelElement panel;
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));

    measureTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 27.f);
    EXPECT_EQ(panel.desiredSize().y, 12.f);

    panel.setRect({0.f, 0.f, 40.f, 20.f});
    arrangeTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().right(), 25.f);
}

TEST_F(LayoutEngineTest, PreservesExplicitGeometryInNormalLayout) {
    StyleSheet styleSheet;
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto button = std::make_unique<ButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    panel.append(std::move(button));

    layoutTree(panel, styleSheet, text);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.x, 10.f);
    EXPECT_EQ(rect.y, 10.f);
    EXPECT_EQ(rect.w, 20.f);
    EXPECT_EQ(rect.h, 20.f);
}

TEST_F(LayoutEngineTest, PreservesExplicitHeightWhenNormalWidthIsPercentage) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } label { display: block; } label#sized { width: 50%; height: auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto label = std::make_unique<LabelElement>("sized");
    label->setId("sized");
    label->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(label));

    layoutTree(panel, styleSheet, text);

    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.w, 50.f);
    EXPECT_EQ(rect.h, 30.f);
}

TEST_F(LayoutEngineTest, AppliesSwitchIntrinsicAndPartLayout) {
    StyleSheet styleSheet;
    constexpr char kIntrinsicSwitchLayout[] = "input { display: flex; flex-direction: column; justify-content: center; }";
    const StyleSheetLoadResult intrinsic = styleSheet.loadRadia(kIntrinsicSwitchLayout, "switch.css");
    ASSERT_TRUE(intrinsic.ok());
    constexpr char kSwitch[] = "input[switch] { width: 64px; height: 32px; padding: 3px 5px; display: flex; flex-direction: row; } "
                               "input[switch]::track { width: 100%; min-width: 0; align-self: stretch; } "
                               "input[switch]::thumb { order: -1; border-radius: 7px; } "
                               "input[switch]:checked::thumb { order: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitch).ok());
    InputElement control;
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 64.f, 32.f});

    layoutTree(control, styleSheet, text);
    EXPECT_EQ(resolveElementStyle(styleSheet, control).display, radia::ui::DisplayMode::Flex);
    ASSERT_NE(control.track(), nullptr);
    EXPECT_EQ(control.track()->rect().bottom(), 23.f);
    EXPECT_EQ(control.track()->rect().h, 26.f);
    EXPECT_EQ(control.thumb()->rect().left(), 15.f);
    EXPECT_EQ(control.thumb()->rect().h, 26.f);
    EXPECT_EQ(control.thumb()->rect().w, 26.f);
    EXPECT_EQ(control.track()->rect().left(), control.thumb()->rect().right());
    EXPECT_EQ(resolveElementStyle(styleSheet, *control.thumb()).borderRadius.topLeft.horizontal.pixels, 7.f);

    const float uncheckedThumbLeft = control.thumb()->rect().left();
    control.checked(true);
    layoutTree(control, styleSheet, text);
    EXPECT_EQ(control.thumb()->rect().left(), control.track()->rect().right());
    EXPECT_GT(control.thumb()->rect().left(), uncheckedThumbLeft);

    control.replaceChildren();
    ASSERT_TRUE(control.track());
    ASSERT_TRUE(control.thumb());
    EXPECT_EQ(control.track()->part(), "track");
    EXPECT_EQ(control.track()->parentElement(), &control);
    EXPECT_EQ(control.thumb()->part(), "thumb");
    EXPECT_EQ(control.thumb()->parentElement(), &control);
}

TEST_F(LayoutEngineTest, OverlaysInlineGridSwitchPartsAndAppliesTranslate) {
    StyleSheet styleSheet;
    constexpr char kGridSwitch[] =
        "input[switch] { appearance: none; display: inline-grid; position: relative; width: 44px; height: 20px; padding: 0px; } "
        "input[switch]::track { grid-area: 1 / 1; width: 100%; } "
        "input[switch]::thumb { grid-area: 1 / 1; width: 24px; height: 24px; margin: -2px -1px; } "
        "input[switch]:checked::thumb { translate: 22px 0; } "
        "input[switch]:dir(rtl):checked::thumb { translate: -22px 0; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridSwitch).ok());

    InputElement control;
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 44.f, 20.f});

    layoutTree(control, styleSheet, text);
    EXPECT_EQ(resolveElementStyle(styleSheet, control).display, DisplayMode::InlineGrid);
    EXPECT_EQ(control.track()->rect().x, 10.f);
    EXPECT_EQ(control.track()->rect().y, 20.f);
    EXPECT_EQ(control.track()->rect().w, 44.f);
    EXPECT_EQ(control.track()->rect().h, 20.f);
    EXPECT_EQ(control.thumb()->rect().x, 9.f);
    EXPECT_EQ(control.thumb()->rect().y, 18.f);
    EXPECT_EQ(control.thumb()->rect().w, 24.f);
    EXPECT_EQ(control.thumb()->rect().h, 24.f);

    control.checked(true);
    layoutTree(control, styleSheet, text);
    EXPECT_EQ(control.thumb()->rect().x, 31.f);
    EXPECT_EQ(control.thumb()->rect().y, 18.f);

    InputElement rtlControl;
    rtlControl.type("checkbox").switchMode(true).checked(true);
    rtlControl.setRect({10.f, 20.f, 44.f, 20.f});
    layoutTree(rtlControl, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(rtlControl.thumb()->rect().x, -13.f);
    EXPECT_EQ(rtlControl.thumb()->rect().y, 18.f);
}

TEST_F(LayoutEngineTest, PlacesGridAreasInImplicitTracks) {
    StyleSheet styleSheet;
    constexpr char kGridLayout[] = "panel { display: grid; } label { width: 10px; height: 10px; justify-self: start; } "
                                   "#top-right { grid-area: 1 / 2; } #bottom-left { grid-area: 2 / 1; } #bottom-right { grid-area: 2 / 2; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridLayout).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(std::make_unique<LabelElement>("top-left"));
    auto topRight = std::make_unique<LabelElement>("top-right");
    topRight->setId("top-right");
    panel.append(std::move(topRight));
    auto bottomLeft = std::make_unique<LabelElement>("bottom-left");
    bottomLeft->setId("bottom-left");
    panel.append(std::move(bottomLeft));
    auto bottomRight = std::make_unique<LabelElement>("bottom-right");
    bottomRight->setId("bottom-right");
    panel.append(std::move(bottomRight));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 50.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 20.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 50.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 20.f);

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 40.f);
}

TEST_F(LayoutEngineTest, UsesInjectedTextMetricsForMeasurement) {
    class ExactTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string&, const Style&) const override { return {47.f, 19.f}; }
    } exact;

    StyleSheet styleSheet;
    LabelElement label("adapter-owned");
    measureTree(label, styleSheet, exact);
    EXPECT_EQ(label.desiredSize().x, 47.f);
    EXPECT_EQ(label.desiredSize().y, 19.f);
}

TEST_F(LayoutEngineTest, AppliesRightToLeftRowDirection) {
    StyleSheet styleSheet;
    constexpr char kDirection[] =
        "panel { display: flex; flex-direction: row; justify-content: start; gap: 5px; } label { width: 10px; height: 10px; } "
        "#physical { margin: 0px 7px 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDirection).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>("first"));
    auto physical = std::make_unique<LabelElement>("second");
    physical->setId("physical");
    panel.append(std::move(physical));

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 78.f);
}

TEST_F(LayoutEngineTest, PreservesNegativeNormalOffsets) {
    StyleSheet styleSheet;
    constexpr char kNegativeOffsetLayout[] = "label { position: relative; width: 10px; height: 10px; left: -8px; bottom: -3px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNegativeOffsetLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(std::make_unique<LabelElement>("offset"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), -8.f);
    EXPECT_EQ(panel.children()[0]->rect().bottom(), 87.f);
}

TEST_F(LayoutEngineTest, ResolvesPercentageGeometryAgainstContainingBlock) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "label { position: relative; width: 50%; height: 25%; left: 10%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 200.f, 100.f});
    panel.append(std::make_unique<LabelElement>("percentage"));

    layoutTree(panel, styleSheet, text);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_NEAR(rect.w, 100.f, 6);
    EXPECT_NEAR(rect.h, 25.f, 6);
    EXPECT_NEAR(rect.left(), 20.f, 6);
    EXPECT_NEAR(rect.top(), 80.f, 6);
}

TEST_F(LayoutEngineTest, DistributesAutomaticRowAndColumnGaps) {
    StyleSheet styleSheet;
    constexpr char kRowGapLayout[] = "panel { display: flex; flex-direction: row; gap: auto; } "
                                     "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowGapLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));
    panel.append(std::make_unique<LabelElement>("third"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    constexpr char kColumnGapLayout[] = "panel { display: flex; flex-direction: column; gap: auto; } "
                                        "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnGapLayout).ok());
    panel.setRect({0.f, 0.f, 20.f, 100.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().top(), 55.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, MeasuresFloaterWithFixedHeightAndAutomaticWidth) {
    StyleSheet styleSheet;
    constexpr char kFloater[] = "floater { size: auto 100px; display: flex; flex-direction: column; } floater > head { height: 30px; } "
                                "floater > body { display: flex; flex-direction: column; gap: 5px; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloater).ok());
    FloaterElement floater;
    radia::ui::test::appendFloaterStructure(floater);
    floater.body()->append(std::make_unique<LabelElement>("first"));
    floater.body()->append(std::make_unique<LabelElement>("second"));

    const Vec2 measured = measureElement(floater, styleSheet, text);
    EXPECT_EQ(measured.x, 100.f);
    EXPECT_EQ(measured.y, 75.f);
}

TEST_F(LayoutEngineTest, AppliesCrossAxisAlignmentAndDirection) {
    StyleSheet styleSheet;
    constexpr char kRowAlignment[] = "panel { display: flex; flex-direction: row; align-items: start; } label { width: 10px; height: 10px; } "
                                     "label#center { align-self: center; } label#end { align-self: end; } "
                                     "label#stretch { height: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowAlignment).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end", "stretch"}) {
        auto label = std::make_unique<LabelElement>(id);
        label->setId(id);
        panel.append(std::move(label));
    }

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 15.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[3]->rect().h, 40.f);

    constexpr char kColumnAlignment[] = "panel { display: flex; flex-direction: column; align-items: start; } label { width: 10px; height: 10px; } "
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

TEST_F(LayoutEngineTest, AppliesGridJustifySelfInBothDirections) {
    StyleSheet styleSheet;
    constexpr char kGridAlignment[] = "panel { display: grid; } label { width: 20px; height: 10px; } "
                                      "label#start { justify-self: start; } label#center { justify-self: center; } "
                                      "label#end { justify-self: end; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridAlignment).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end"}) {
        auto label = std::make_unique<LabelElement>(id);
        label->setId(id);
        panel.append(std::move(label));
    }

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 80.f);

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 80.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineTest, DistinguishesVisibilityFromDisplayLayout) {
    StyleSheet styleSheet;
    constexpr char kVisibilityLayout[] = "panel { display: flex; flex-direction: row; } "
                                         "label { width: 10px; height: 10px; } label.none { display: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibilityLayout).ok());
    PanelElement panel;
    panel.append(std::make_unique<LabelElement>("visible"));
    auto hidden = std::make_unique<LabelElement>("hidden");
    hidden->setVisibility(Visibility::Hidden);
    panel.append(std::move(hidden));
    auto collapsed = std::make_unique<LabelElement>("collapsed");
    collapsed->setVisibility(Visibility::Collapse);
    panel.append(std::move(collapsed));
    auto displayNone = std::make_unique<LabelElement>("display-none");
    displayNone->addClass("none");
    panel.append(std::move(displayNone));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 10.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
    EXPECT_EQ(measureElement(*panel.children()[2], styleSheet, text).x, 10.f);
    EXPECT_EQ(panel.children()[3]->desiredSize().x, 0.f);

    panel.children()[1]->setVisibility(Visibility::Collapse);
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    panel.children()[2]->setVisibility(Visibility::Visible);
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
}

TEST_F(LayoutEngineTest, AppliesContainerVerticalAlignmentToContent) {
    StyleSheet styleSheet;
    constexpr char kVerticalAlignment[] = "panel { size: 40px 100px; display: flex; flex-direction: row; } label { size: 10px; } "
                                          "panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } "
                                          "panel.column { display: flex; flex-direction: column; vertical-align: bottom; } "
                                          "panel.free-bottom { display: block; vertical-align: bottom; } "
                                          "button { size: 40px 100px; } button > * { size: 10px; } "
                                          "button.top { vertical-align: top; }";
    ASSERT_TRUE(styleSheet
                    .loadRadiaLayers({
                        radia::ui::StyleLayer{radia::ui::StyleOrigin::Default,
                                              radia::ui::ResourceLayer{std::string(radia::ui::kDefaultStylesheetResourceId),
                                                                       std::string(radia::ui::defaultStylesheetSource())}},
                        radia::ui::StyleLayer{radia::ui::StyleOrigin::Skin, radia::ui::ResourceLayer{"test.css", kVerticalAlignment}},
                    })
                    .ok());

    auto addLabel = [](Element& container) { container.append(std::make_unique<LabelElement>("child")); };

    PanelElement top;
    top.setRect({0.f, 0.f, 100.f, 40.f});
    addLabel(top);
    layoutTree(top, styleSheet, text);
    EXPECT_EQ(top.children()[0]->rect().top(), 40.f);

    PanelElement middle;
    middle.setRect({0.f, 0.f, 100.f, 40.f});
    middle.addClass("middle");
    addLabel(middle);
    layoutTree(middle, styleSheet, text);
    EXPECT_EQ(middle.children()[0]->rect().bottom(), 15.f);

    PanelElement bottom;
    bottom.setRect({0.f, 0.f, 100.f, 40.f});
    bottom.addClass("bottom");
    addLabel(bottom);
    layoutTree(bottom, styleSheet, text);
    EXPECT_EQ(bottom.children()[0]->rect().bottom(), 0.f);

    PanelElement column;
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.addClass("column");
    addLabel(column);
    addLabel(column);
    layoutTree(column, styleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().top(), 20.f);
    EXPECT_EQ(column.children()[1]->rect().bottom(), 0.f);

    PanelElement freeBottom;
    freeBottom.setRect({0.f, 0.f, 100.f, 40.f});
    freeBottom.addClass("free-bottom");
    addLabel(freeBottom);
    layoutTree(freeBottom, styleSheet, text);
    EXPECT_EQ(freeBottom.children()[0]->rect().bottom(), 30.f);

    ButtonElement button;
    button.setRect({0.f, 0.f, 100.f, 40.f});
    auto buttonContent = std::make_unique<Element>("span");
    buttonContent->textContent("button");
    Element* buttonContentPtr = buttonContent.get();
    button.append(std::move(buttonContent));
    layoutTree(button, styleSheet, text);
    EXPECT_EQ(buttonContentPtr->rect().bottom(), 15.f);
    EXPECT_EQ(buttonContentPtr->rect().left(), 45.f);

    ButtonElement topButton;
    topButton.setRect({0.f, 0.f, 100.f, 40.f});
    topButton.addClass("top");
    auto topButtonContent = std::make_unique<Element>("span");
    topButtonContent->textContent("button");
    Element* topButtonContentPtr = topButtonContent.get();
    topButton.append(std::move(topButtonContent));
    layoutTree(topButton, styleSheet, text);
    EXPECT_EQ(topButtonContentPtr->rect().top(), 40.f);
}

TEST_F(LayoutEngineTest, WrapsChildrenAcrossFlowBreaks) {
    StyleSheet styleSheet;
    constexpr char kFlowBreakLayout[] = "panel { display: flex; flex-direction: row; gap: 2px; } "
                                        "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlowBreakLayout).ok());

    PanelElement panel;
    auto first = std::make_unique<LabelElement>("first");
    auto second = std::make_unique<LabelElement>("second");
    auto third = std::make_unique<LabelElement>("third");
    radia::ui::detail::ElementCompilerAccess::setFlowBreakBefore(*second, true);
    panel.append(std::move(first));
    panel.append(std::move(second));
    panel.append(std::move(third));

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
    constexpr char kFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } label { height: 10px; flex: 0 1 80px; } "
                                        "label.second { flex-basis: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexBasisLayout).ok());

    PanelElement intrinsic;
    intrinsic.append(std::make_unique<LabelElement>());
    auto intrinsicSecond = std::make_unique<LabelElement>();
    intrinsicSecond->addClass("second");
    intrinsic.append(std::move(intrinsicSecond));
    measureTree(intrinsic, styleSheet, text);
    EXPECT_EQ(intrinsic.desiredSize().x, 120.f);

    StyleSheet percentageTheme;
    constexpr char kPercentageFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } "
                                                  "label { width: 30px; flex-basis: 50%; }";
    ASSERT_TRUE(percentageTheme.loadRadia(kPercentageFlexBasisLayout).ok());
    PanelElement indefinite;
    indefinite.append(std::make_unique<LabelElement>());
    measureTree(indefinite, percentageTheme, text);
    EXPECT_EQ(indefinite.desiredSize().x, 30.f);

    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>());
    auto second = std::make_unique<LabelElement>();
    second->addClass("second");
    panel.append(std::move(second));
    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().w, 200.f / 3.f, 5);
    EXPECT_NEAR(panel.children()[1]->rect().w, 100.f / 3.f, 5);
    EXPECT_NEAR(panel.children()[1]->rect().right(), 100.f, 5);
}

TEST_F(LayoutEngineTest, CentersOversizedFloaterHeadChildren) {
    StyleSheet styleSheet;
    constexpr char kFloaterHead[] =
        "floater > head { height: 48px; display: flex; flex-direction: row; padding: 12px; } "
        "floater > head > title { height: 24px; display: flex; flex-direction: row; align-items: center; flex-grow: 1; line-height: 18px; } "
        "floater > head > title > icon { size: 28px; } "
        "floater > head > close { size: 24px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterHead).ok());

    FloaterElement floater;
    radia::ui::test::appendFloaterStructure(floater, true);
    auto icon = std::make_unique<IconElement>("search");
    floater.head()->children().front()->append(std::move(icon));
    floater.head()->setRect({0.f, 0.f, 200.f, 48.f});
    layoutTree(*floater.head(), styleSheet, text);

    const float headCenter = floater.head()->rect().y + floater.head()->rect().h * .5f;
    for (const auto& child : floater.head()->children()) {
        if (!child->isVisible(resolveElementStyle(styleSheet, *child))) continue;
        const float childCenter = child->rect().y + child->rect().h * .5f;
        SCOPED_TRACE(Message() << "head child: " << child->elementName());
        EXPECT_FLOAT_EQ(childCenter, headCenter);
    }
}

TEST_F(LayoutEngineTest, ReflowsWrappedTextInColumnLayout) {
    StyleSheet styleSheet;
    constexpr char kWrapping[] = "panel { display: flex; flex-direction: column; } "
                                 "p { font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapping).ok());

    PanelElement panel;
    panel.setRect({0.f, 0.f, 30.f, 100.f});
    panel.append(makeParagraph("alpha beta"));
    panel.append(makeParagraph("after"));

    layoutTree(panel, styleSheet, text);
    const Element& wrapped = *panel.children()[0];
    const Element& following = *panel.children()[1];
    EXPECT_EQ(wrapped.rect().h, 20.f);
    EXPECT_EQ(following.rect().top(), wrapped.rect().bottom());
}

TEST_F(LayoutEngineTest, RemeasuresWrappedTextAfterRowFlexShrink) {
    StyleSheet styleSheet;
    constexpr char kRowWrapping[] = "panel { width: 45px; display: flex; flex-direction: row; align-items: start; } "
                                    "p { min-width: 0px; flex: 1; font-size: 10px; line-height: 10px; text-wrap: wrap; } "
                                    "label { width: 10px; flex-shrink: 0; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowWrapping).ok());

    PanelElement panel;
    panel.append(makeParagraph("alpha beta"));
    panel.append(std::make_unique<LabelElement>("x"));

    const Vec2 measured = measureElement(panel, styleSheet, text);
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
    constexpr char kColumnBasis[] = "panel { width: 30px; display: flex; flex-direction: column; } "
                                    "p { flex-basis: 40px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(columnTheme.loadRadia(kColumnBasis).ok());
    PanelElement column;
    column.append(makeParagraph("alpha beta"));
    EXPECT_EQ(measureElement(column, columnTheme, text).y, 40.f);

    StyleSheet rowTheme;
    constexpr char kRowMinimum[] = "panel { width: 80px; display: flex; flex-direction: row; } p { flex: 0 1 100px; font-size: 10px; "
                                   "line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; }";
    ASSERT_TRUE(rowTheme.loadRadia(kRowMinimum).ok());
    PanelElement row;
    row.append(makeParagraph("alpha beta"));
    row.append(std::make_unique<LabelElement>("x"));
    row.setRect({0.f, 0.f, 80.f, 20.f});
    layoutTree(row, rowTheme, text);
    EXPECT_EQ(row.children()[0]->rect().w, 70.f);
}
TEST_F(LayoutEngineTest, ReusesCleanLayoutCache) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; } "
                                  "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));

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
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; } "
                                  "label { size: 10px; } label:dir(rtl) { width: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));
    layoutTree(panel, styleSheet, text);

    panel.setRect({0.f, 0.f, 140.f, 20.f});
    const LayoutStatistics resized = layoutTree(panel, styleSheet, text);
    EXPECT_GT(resized.measuredNodes, std::size_t(0));
    EXPECT_GT(resized.arrangedNodes, std::size_t(0));

    const LayoutStatistics directionChanged = layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_GT(directionChanged.measuredNodes, std::size_t(0));
    EXPECT_GT(directionChanged.arrangedNodes, std::size_t(0));
    EXPECT_EQ(panel.children()[0]->rect().w, 20.f);
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
    LabelElement label("generation");
    const LayoutStatistics first = layoutTree(label, styleSheet, metrics);
    metrics.advance();
    const LayoutStatistics second = layoutTree(label, styleSheet, metrics);
    EXPECT_GT(first.measuredNodes, std::size_t(0));
    EXPECT_GT(second.measuredNodes, std::size_t(0));
}

TEST_F(LayoutEngineTest, ReallocatesColumnFlexChildrenUnderHeightConstraint) {
    StyleSheet styleSheet;
    constexpr char kColumnFlexLayout[] = "panel { display: flex; flex-direction: column; } "
                                         "label { height: 10px; flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnFlexLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));

    const LayoutStatistics stats = layoutTree(panel, styleSheet, text);
    EXPECT_GE(stats.constrainedRemeasures, std::size_t(2));
    EXPECT_EQ(panel.children()[0]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, ResolvesSiblingColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kSiblingColumnLayout[] = "panel { display: flex; flex-direction: column; } "
                                            "#quarter { height: 25%; } #half { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSiblingColumnLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto quarter = std::make_unique<LabelElement>("quarter");
    quarter->setId("quarter");
    auto half = std::make_unique<LabelElement>("half");
    half->setId("half");
    panel.append(std::move(quarter));
    panel.append(std::move(half));

    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().h, 25.f, 6);
    EXPECT_NEAR(panel.children()[1]->rect().h, 50.f, 6);
    EXPECT_NEAR(panel.children()[0]->rect().bottom(), panel.children()[1]->rect().top(), 6);
}

TEST_F(LayoutEngineTest, ResolvesNestedColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kNestedColumnLayout[] =
        "panel { display: flex; flex-direction: column; } #child { height: 50%; display: flex; flex-direction: column; } "
        "#grandchild { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedColumnLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<PanelElement>();
    child->setId("child");
    auto grandchild = std::make_unique<LabelElement>("nested");
    grandchild->setId("grandchild");
    child->append(std::move(grandchild));
    panel.append(std::move(child));

    layoutTree(panel, styleSheet, text);
    const Element& nestedPanel = *panel.children().front();
    EXPECT_NEAR(nestedPanel.rect().h, 50.f, 6);
    EXPECT_NEAR(nestedPanel.children().front()->rect().h, 25.f, 6);
}

TEST_F(LayoutEngineTest, RespectsColumnMinimumHeightsDuringShrink) {
    StyleSheet styleSheet;
    constexpr char kColumnMinimumLayout[] = "panel { display: flex; flex-direction: column; } "
                                            "label { height: 30px; min-height: 25px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnMinimumLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 25.f);
}

TEST_F(LayoutEngineTest, ReusesHeightConstrainedColumnCache) {
    StyleSheet styleSheet;
    constexpr char kFlexLayout[] = "panel { display: flex; flex-direction: column; } "
                                   "label { flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(std::make_unique<LabelElement>("first"));
    panel.append(std::make_unique<LabelElement>("second"));

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

    LabelElement label("identity");
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
    LabelElement label("identity");
    layoutTree(label, styleSheet, narrow);
    layoutTree(label, styleSheet, wide);
    EXPECT_EQ(label.desiredSize().x, 48.f);
}

TEST_F(LayoutEngineTest, OrdersChildrenBeforeRowLayout) {
    StyleSheet styleSheet;
    constexpr char kOrderedFlow[] = "panel { display: flex; flex-direction: row; } #late { order: 2; width: 10px; height: 10px; } "
                                    "#early { order: -1; width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOrderedFlow).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto late = std::make_unique<LabelElement>("late");
    late->setId("late");
    auto early = std::make_unique<LabelElement>("early");
    early->setId("early");
    panel.append(std::move(late));
    panel.append(std::move(early));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[0]->rect().x, 10.f);
}

TEST_F(LayoutEngineTest, InvalidatesLayoutCacheWhenStylesheetIsAssigned) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kReplacementLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    LabelElement label("assigned");
    layoutTree(label, styleSheet, text);

    StyleSheet replacement;
    ASSERT_TRUE(replacement.loadRadia(kReplacementLabelLayout).ok());
    styleSheet = replacement;
    layoutTree(label, styleSheet, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineTest, RemeasuresNormalPercentageTextAfterResize) {
    StyleSheet styleSheet;
    constexpr char kNormalTextLayout[] = "panel { display: block; } "
                                         "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalTextLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeParagraph("alpha beta"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);

    panel.setRect({0.f, 0.f, 200.f, 100.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 10.f);
}

TEST_F(LayoutEngineTest, RelativeOffsetsDoNotChangeIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalOffsetLayout[] = "panel { display: block; } "
                                           "label { position: relative; width: 20px; height: 10px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalOffsetLayout).ok());
    PanelElement panel;
    panel.append(std::make_unique<LabelElement>("positioned"));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitNormalGeometryInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    PanelElement panel;
    auto child = std::make_unique<PanelElement>();
    child->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 40.f);
    EXPECT_EQ(panel.desiredSize().y, 30.f);
}

TEST_F(LayoutEngineTest, InvalidatesIntrinsicSizeAfterExplicitGeometryChange) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    PanelElement panel;
    auto child = std::make_unique<PanelElement>();
    child->setRect({0.f, 0.f, 20.f, 10.f});
    Element* childPtr = child.get();
    panel.append(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);

    childPtr->setRect({0.f, 0.f, 60.f, 10.f});
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 60.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitNormalPositionInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    PanelElement panel;
    auto child = std::make_unique<PanelElement>();
    child->setRect({10.f, 12.f, 20.f, 8.f});
    panel.append(std::move(child));
    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
}

TEST_F(LayoutEngineTest, IncludesWrappedPercentageChildInNormalIntrinsicHeight) {
    StyleSheet styleSheet;
    constexpr char kWrappedTextLayout[] = "panel { display: block; width: 100px; } "
                                          "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrappedTextLayout).ok());
    PanelElement panel;
    panel.append(makeParagraph("alpha beta"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, RelativePercentageOffsetsDoNotChangeIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalPercentageOffsetLayout[] = "panel { display: block; } "
                                                     "label { position: relative; width: 20px; height: 10px; left: 50%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalPercentageOffsetLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(std::make_unique<LabelElement>("positioned"));

    layoutTree(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);
    EXPECT_EQ(panel.children().front()->rect().left(), 50.f);
    EXPECT_EQ(panel.children().front()->rect().top(), 80.f);
}

TEST_F(LayoutEngineTest, ResolvesNormalPercentageDimensionsAgainstExplicitParent) {
    StyleSheet styleSheet;
    constexpr char kNormalPercentageLayout[] = "panel { display: block; width: 50%; height: 50%; } "
                                               "label { width: 50%; height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalPercentageLayout).ok());
    PanelElement panel;
    panel.setRect({0.f, 0.f, 100.f, 80.f});
    panel.append(std::make_unique<LabelElement>("sized"));

    layoutTree(panel, styleSheet, text);
    EXPECT_NEAR(panel.children().front()->rect().w, 50.f, 6);
    EXPECT_NEAR(panel.children().front()->rect().h, 40.f, 6);
}

TEST_F(LayoutEngineTest, ResolvesNestedPercentageFlexBasis) {
    StyleSheet styleSheet;
    constexpr char kNestedFlexBasis[] = "#outer { display: flex; flex-direction: row; width: 100px; height: 20px; } "
                                        "#inner { display: flex; flex-direction: row; width: 50%; } label { flex-basis: 50%; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedFlexBasis).ok());
    PanelElement outer;
    outer.setRect({0.f, 0.f, 100.f, 20.f});
    outer.setId("outer");
    auto inner = std::make_unique<PanelElement>();
    inner->setId("inner");
    Element* innerPtr = inner.get();
    inner->append(std::make_unique<LabelElement>("basis"));
    Element* label = inner->children().front();
    outer.append(std::move(inner));

    layoutTree(outer, styleSheet, text);
    EXPECT_NEAR(innerPtr->rect().w, 50.f, 6);
    EXPECT_NEAR(label->rect().w, 25.f, 6);
}

TEST_F(LayoutEngineTest, ComputesScrollMetricsAndClampsProgrammaticPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 95.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 155.f);

    panel.scrollTo(500.f, -10.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 95.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
    panel.scrollBy(-20.f, 50.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 75.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 50.f);
}

TEST_F(LayoutEngineTest, ClassicScrollbarsReflowTheOppositeAxis) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 100.f, 120.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 120.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 15.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 35.f);
}

TEST_F(LayoutEngineTest, HiddenOverflowHasAProgrammaticRangeWithoutScrollbarSpace) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: hidden; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
    panel.scrollTo(500.f, 500.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 80.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 140.f);
}

TEST_F(LayoutEngineTest, OverlayScrollbarsDoNotReduceClientSize) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Overlay;

    layoutTree(panel, styleSheet, text, LayoutDirection::LeftToRight, options);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineTest, ForcedScrollbarsReserveClassicSpaceWithoutOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(
        styleSheet.loadRadia("panel#viewport { display: block; overflow: scroll; } #content { display: block; width: 40px; height: 40px; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 0.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineTest, VisibleOverflowDoesNotCreateAProgrammaticRange) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: visible; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 0.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 0.f);
    panel.scrollTo(50.f, 50.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 0.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineTest, RecomputingOverflowClampsStaleScrollPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: hidden; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    Element* contentPtr = content.get();
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);
    panel.scrollTo(500.f, 500.f);
    contentPtr->setRect({0.f, 0.f, 40.f, 40.f});
    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 0.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 0.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 0.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineTest, UsesNormalizedScrollLeftInRightToLeftLayout) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 40.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text, LayoutDirection::RightToLeft);
    panel.scrollTo(20.f, 0.f);

    EXPECT_FLOAT_EQ(panel.scrollLeft(), 20.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineTest, AuthoredScrollbarModeOverridesSurfacePolicy) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-mode: overlay; "
                               "scrollbar-gutter: stable both-edges; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Classic;

    layoutTree(panel, styleSheet, text, LayoutDirection::LeftToRight, options);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineTest, NoneScrollbarWidthPreservesRangeWithoutScrollbarSpace) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-width: none; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineTest, ThinScrollbarWidthUsesReducedClassicThickness) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: scroll; scrollbar-width: thin; }").ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 92.5f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 92.5f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 87.5f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 147.5f);
}

TEST_F(LayoutEngineTest, StableScrollbarGutterReservesClassicSpaceWithoutOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-gutter: stable; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 85.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 100.f);
}

TEST_F(LayoutEngineTest, StableBothEdgesMirrorsTheInlineGutter) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-gutter: stable both-edges; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    PanelElement panel;
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = std::make_unique<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    layoutTree(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 15.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 85.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 100.f);
}
} // namespace
