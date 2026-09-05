/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include "css/stylesheet.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "resource/elementdefinition.h"
#include "style/stylepass.h"
#include "text/metrics.h"

namespace {
using radia::ui::BoxSizing;
using radia::ui::ComputedStyle;
using radia::ui::defaultStylesheetSource;
using radia::ui::DisplayMode;
using radia::ui::Element;
using radia::ui::FixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLIconElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::LayoutDirection;
using radia::ui::LayoutEngine;
using radia::ui::Rect;
using radia::ui::ScrollbarMode;
using radia::ui::ScrollLayoutOptions;
using radia::ui::StyleLayer;
using radia::ui::StyleOrigin;
using radia::ui::StylePass;
using radia::ui::StyleSheet;
using radia::ui::StyleSheetLoadResult;
using radia::ui::Text;
using radia::ui::TextMetrics;
using radia::ui::Vec2;
using radia::ui::Visibility;
using radia::ui::detail::appendText;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using radia::ui::detail::NodeAccess;
using radia::ui::detail::nodes;
using ::testing::Message;
using ::testing::Test;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    StylePass styles(stylesheet, FixedTextMetrics{});
    return styles.style(element);
}
} // namespace

namespace {
std::unique_ptr<Element> makeParagraph(std::string text) {
    auto paragraph = makeElement<Element>("p");
    paragraph->textContent(std::move(text));
    return paragraph;
}

HTMLIconElement& appendIcon(HTMLButtonElement& button, std::string name) {
    auto icon = makeElement<HTMLIconElement>(std::move(name));
    HTMLIconElement* result = icon.get();
    button.append(std::move(icon));
    return *result;
}

void appendButtonText(HTMLButtonElement& button, std::string text) {
    appendText(button, std::move(text));
}

class LayoutEngineTest : public Test {
protected:
    FixedTextMetrics text;
};

TEST_F(LayoutEngineTest, MeasuresButtonWithIconAndLabel) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { position: relative; left: 10px; top: 10px; padding: 7px; gap: 6px; display: flex; flex-direction: row; "
        "font-size: 13px; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = makeElement<HTMLButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    LayoutEngine::layout(root, styleSheet, text);
    ASSERT_EQ(root.children().size(), 1U);
    const Element& result = *root.children().front();
    EXPECT_EQ(result.rect().w, 300.f);
    EXPECT_EQ(result.rect().h, 32.f);
    const auto runtimeChildren = nodes(result);
    ASSERT_EQ(runtimeChildren.size(), 2U);
    ASSERT_NE(runtimeChildren.begin()->asElement(), nullptr);
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

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 20.f});
    auto leadingWhitespace = std::make_unique<Text>("\n        ");
    Text* leadingWhitespacePtr = leadingWhitespace.get();
    button.append(std::move(leadingWhitespace));
    HTMLIconElement& icon = appendIcon(button, "search");
    auto betweenWhitespace = std::make_unique<Text>("\n        ");
    Text* betweenWhitespacePtr = betweenWhitespace.get();
    button.append(std::move(betweenWhitespace));
    auto label = std::make_unique<Text>("Press");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    LayoutEngine::layout(button, styleSheet, text);

    EXPECT_EQ(leadingWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(betweenWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(icon.rect().x, 0.f);
    EXPECT_EQ(labelPtr->rect().x - icon.rect().right(), 6.f);
}

TEST_F(LayoutEngineTest, PreservesWhitespaceBetweenAdjacentInlineRuns) {
    StyleSheet styleSheet;
    constexpr char kInlineLayout[] = "p { display: block; } i, s { display: inline; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInlineLayout).ok());

    auto paragraph = makeElementValue<Element>("p");
    paragraph.setRect({0.f, 0.f, 200.f, 20.f});
    auto italic = makeElement<Element>("i");
    Element* italicPtr = italic.get();
    italic->append(std::make_unique<Text>("Italic I run"));
    auto separator = std::make_unique<Text>(" ");
    Text* separatorPtr = separator.get();
    auto strike = makeElement<Element>("s");
    Element* strikePtr = strike.get();
    strike->append(std::make_unique<Text>("Strike S run"));
    paragraph.append(std::move(italic));
    paragraph.append(std::move(separator));
    paragraph.append(std::move(strike));

    LayoutEngine::layout(paragraph, styleSheet, text);

    EXPECT_GT(separatorPtr->rect().w, 0.f);
    EXPECT_FLOAT_EQ(separatorPtr->rect().x, italicPtr->rect().right());
    EXPECT_FLOAT_EQ(strikePtr->rect().x, separatorPtr->rect().right());
}

TEST_F(LayoutEngineTest, CollapsesFormattingWhitespaceBetweenButtonContent) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { display: inline-block; width: 100px; height: 40px; padding: 0; font-size: 10px; line-height: 10px; } button > icon { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 40.f});
    HTMLIconElement& icon = appendIcon(button, "search");
    auto separator = std::make_unique<Text>("\n            ");
    Text* separatorPtr = separator.get();
    button.append(std::move(separator));
    auto label = std::make_unique<Text>("Press");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    LayoutEngine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(separatorPtr->rect().w, 6.f);
    EXPECT_FLOAT_EQ(separatorPtr->rect().h, 10.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().left() - icon.rect().right(), 6.f);
}

TEST_F(LayoutEngineTest, IgnoresFormattingWhitespaceBeforeNormalFlowChildren) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } p { display: block; height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 60.f});
    auto leadingWhitespace = std::make_unique<Text>("\n        ");
    Text* leadingWhitespacePtr = leadingWhitespace.get();
    panel.append(std::move(leadingWhitespace));
    auto paragraph = makeParagraph("status");
    Element* paragraphPtr = paragraph.get();
    panel.append(std::move(paragraph));

    LayoutEngine::layout(panel, styleSheet, text);

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

    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(std::make_unique<Text>("\n        "));
    auto status = makeElement<Element>("p");
    Element* statusPtr = status.get();
    status->setId("status");
    status->textContent("Ready");
    floater.body()->append(std::move(status));

    LayoutEngine::layout(floater, styleSheet, text);

    EXPECT_EQ(statusPtr->rect().top(), floater.body()->rect().top());
}

TEST_F(LayoutEngineTest, LaysOutInlineSiblingsAndBlockChildrenInNormalFlow) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] =
        "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; } .block { display: block; width: 50px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 60.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->addClass("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->addClass("inline");
    panel.append(std::move(second));
    auto block = makeElement<HTMLLabelElement>("block");
    block->addClass("block");
    panel.append(std::move(block));
    auto after = makeElement<HTMLLabelElement>("after");
    after->addClass("inline");
    panel.append(std::move(after));

    LayoutEngine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 30.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 60.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 50.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 40.f);
}

TEST_F(LayoutEngineTest, AlignsNormalFlowInlineContentWithTextAlign) {
    StyleSheet styleSheet;
    constexpr char kAlignedLayout[] = "panel { display: block; text-align: center; } .inline { display: inline; width: 20px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kAlignedLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->addClass("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->addClass("inline");
    panel.append(std::move(second));

    LayoutEngine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.children()[0]->rect().left(), 30.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().left(), 50.f);
}

TEST_F(LayoutEngineTest, WrapsInlineSiblingsAtTheContainingBlockWidth) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 50.f, 40.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->addClass("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->addClass("inline");
    panel.append(std::move(second));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = makeElement<HTMLButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    LayoutEngine::layout(root, styleSheet, text);
    ASSERT_EQ(root.children().size(), 1U);
    const Element& result = *root.children().front();
    const auto runtimeChildren = nodes(result);
    ASSERT_EQ(runtimeChildren.size(), 2U);
    auto first = runtimeChildren.begin();
    auto second = first;
    ++second;
    ASSERT_NE(first->asElement(), nullptr);
    ASSERT_NE(second->asText(), nullptr);
    const float contentWidth = second->asText()->rect().right() - first->asElement()->rect().left();
    EXPECT_EQ(first->asElement()->rect().x - result.rect().x, (result.rect().w - contentWidth) * 0.5f);
}

TEST_F(LayoutEngineTest, NativeButtonUsesNormalContentLayoutWithoutChangingDisplay) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { display: inline-block; width: 128px; height: 32px; padding: 7px; text-align: center; line-height: 18px; } button > icon { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 128.f, 32.f});
    HTMLIconElement& icon = appendIcon(button, "search");
    auto label = std::make_unique<Text>("\n        Apply\n    ");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    const ComputedStyle computed = computedStyle(styleSheet, button);
    ASSERT_EQ(computed.appearance, radia::ui::AppearanceMode::Auto);
    ASSERT_EQ(computed.display, DisplayMode::InlineBlock);

    LayoutEngine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().h, 18.f);
    EXPECT_EQ(labelPtr->data(), "\n        Apply\n    ");
    EXPECT_EQ(computedStyle(styleSheet, button).display, DisplayMode::InlineBlock);
    const float contentLeft = button.rect().left() + computed.borderWidth.left + computed.padding.left;
    const float contentWidth = button.rect().w - computed.borderWidth.horizontal() - computed.padding.horizontal();
    const float contentWidthUsed = labelPtr->rect().right() - icon.rect().left();
    EXPECT_FLOAT_EQ(icon.rect().left(), contentLeft + (contentWidth - contentWidthUsed) * 0.5f);
}

TEST_F(LayoutEngineTest, CentersButtonBlockContentFromDefaultAlignment) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { display: inline-block; width: 100px; height: 40px; padding: 0; line-height: 10px; }";
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Default, {"defaults.css", std::string(defaultStylesheetSource())}},
        {StyleOrigin::Skin, {"skin.css", kButtonLayout}},
    };
    ASSERT_TRUE(styleSheet.loadRadiaLayers(layers).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 40.f});
    auto label = std::make_unique<Text>("Apply");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    LayoutEngine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().bottom(), 15.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().top(), 25.f);
}

TEST_F(LayoutEngineTest, DoesNotCenterUnstyledButtonBlockContent) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { display: inline-block; width: 100px; height: 40px; padding: 0; line-height: 10px; } button.unstyled { appearance: none; }";
    const std::vector<StyleLayer> layers{
        {StyleOrigin::Default, {"defaults.css", std::string(defaultStylesheetSource())}},
        {StyleOrigin::Skin, {"skin.css", kButtonLayout}},
    };
    ASSERT_TRUE(styleSheet.loadRadiaLayers(layers).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.addClass("unstyled");
    button.setRect({0.f, 0.f, 100.f, 40.f});
    auto label = std::make_unique<Text>("Apply");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    const ComputedStyle computed = computedStyle(styleSheet, button);
    ASSERT_EQ(computed.appearance, radia::ui::AppearanceMode::Unstyled);

    LayoutEngine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().bottom(), 29.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().top(), 39.f);
}

TEST_F(LayoutEngineTest, LaysOutColumnChildrenWithPaddingAndGap) {
    StyleSheet styleSheet;
    constexpr char kColumnLayout[] = "panel { padding: 10px; display: flex; flex-direction: column; gap: 5px; } "
                                     "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(makeElement<HTMLLabelElement>("one"));
    root.append(makeElement<HTMLLabelElement>("two"));
    LayoutEngine::layout(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().w, 80.f);
    EXPECT_EQ(root.children()[0]->rect().bottom() - root.children()[1]->rect().top(), 5.f);
}

TEST_F(LayoutEngineTest, AppliesContentBoxAndBorderBoxToExplicitDimensions) {
    StyleSheet styleSheet;
    constexpr char kBoxSizingLayout[] = "panel { display: block; } label { display: block; width: 100px; height: 20px; padding: 10px; "
                                        "border: 2px solid #000000; } label.border { box-sizing: border-box; }";
    ASSERT_TRUE(styleSheet.loadRadia(kBoxSizingLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 400.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("content-box"));
    auto borderBox = makeElement<HTMLLabelElement>("border-box");
    borderBox->addClass("border");
    panel.append(std::move(borderBox));

    LayoutEngine::layout(panel, styleSheet, text);

    EXPECT_EQ(computedStyle(styleSheet, *panel.children()[0]).boxSizing, BoxSizing::ContentBox);
    EXPECT_EQ(computedStyle(styleSheet, *panel.children()[1]).boxSizing, BoxSizing::BorderBox);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().w, 124.f);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().h, 44.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().w, 100.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, KeepsPaddingInTheClientBoxAndOutOfChildContent) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: hidden; padding: 10px 20px 30px 40px; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(makeElement<HTMLPanelElement>());
    LayoutEngine::layout(root, styleSheet, text);
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
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater);
    ASSERT_NE(floater.head(), nullptr);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(makeElement<HTMLLabelElement>("content"));
    LayoutEngine::layout(floater, styleSheet, text);
    EXPECT_EQ(floater.head()->rect().top(), 90.f);
    EXPECT_EQ(floater.body()->rect().top(), 60.f);
}

TEST_F(LayoutEngineTest, DisplayNoneFloaterHeadDoesNotReserveSpace) {
    StyleSheet styleSheet;
    constexpr char kCollapsedFloaterLayout[] = "floater { display: flex; flex-direction: column; } floater > head { display: none; height: 30px; } "
                                               "floater > body { flex-grow: 1; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCollapsedFloaterLayout).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(makeElement<HTMLLabelElement>("content"));
    LayoutEngine::layout(floater, styleSheet, text);
    EXPECT_EQ(floater.body()->rect().top(), 100.f);
}

TEST_F(LayoutEngineTest, DistributesAutoMarginAlongRow) {
    StyleSheet styleSheet;
    constexpr char kAutoMarginLayout[] = "panel { display: flex; flex-direction: row; } label { width: 10px; height: 10px; } "
                                         "#first { margin: 0px auto 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kAutoMarginLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->setId("first");
    panel.append(std::move(first));
    panel.append(makeElement<HTMLLabelElement>("second"));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[1]->rect().x, 90.f);
}

TEST_F(LayoutEngineTest, CentersColumnChildWithAutoMargins) {
    StyleSheet styleSheet;
    constexpr char kCenteredColumnLayout[] = "panel { display: flex; flex-direction: column; } "
                                             "label { width: 20px; height: 10px; margin: 0px auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredColumnLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("center"));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 40.f);
}

TEST_F(LayoutEngineTest, CentersIconInsideOversizedButtonPadding) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { size: 24px; padding: 20px; display: flex; flex-direction: row; justify-content: center; } "
                                     "button > icon { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 24.f, 24.f});
    HTMLIconElement& icon = appendIcon(button, "search");
    LayoutEngine::layout(button, styleSheet, text);
    EXPECT_EQ(icon.rect().x, 4.f);
    EXPECT_EQ(icon.rect().y, 4.f);
}

TEST_F(LayoutEngineTest, AlignsColumnContentToEnd) {
    StyleSheet styleSheet;
    constexpr char kColumnContentLayout[] = "panel { display: flex; flex-direction: column; justify-content: end; } "
                                            "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnContentLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("bottom"));
    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.children().front()->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, DistributesRemainingWidthAcrossFlexChildren) {
    StyleSheet styleSheet;
    constexpr char kFlexDistributionLayout[] = "panel { display: flex; flex-direction: row; } "
                                               "label { width: 10px; height: 10px; flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexDistributionLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().w, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineTest, MeasuresAndArrangesRowChildren) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; gap: 3px; padding: 2px; } "
                                  "label { width: 10px; height: 8px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    LayoutEngine::measure(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 27.f);
    EXPECT_EQ(panel.desiredSize().y, 12.f);

    panel.setRect({0.f, 0.f, 40.f, 20.f});
    LayoutEngine::arrange(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().right(), 25.f);
}

TEST_F(LayoutEngineTest, PreservesExplicitGeometryInNormalLayout) {
    StyleSheet styleSheet;
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    panel.append(std::move(button));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
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

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto label = makeElement<HTMLLabelElement>("sized");
    label->setId("sized");
    label->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(label));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);

    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.w, 50.f);
    EXPECT_EQ(rect.h, 30.f);
}

TEST_F(LayoutEngineTest, AppliesSwitchIntrinsicAndPseudoElementLayout) {
    StyleSheet styleSheet;
    constexpr char kIntrinsicSwitchLayout[] = "input { display: flex; flex-direction: column; justify-content: center; }";
    const StyleSheetLoadResult intrinsic = styleSheet.loadRadia(kIntrinsicSwitchLayout, "switch.css");
    ASSERT_TRUE(intrinsic.ok());
    constexpr char kSwitch[] = "input[switch] { appearance: base; width: 64px; height: 32px; padding: 3px 5px; display: flex; flex-direction: row; } "
                               "input[switch]::slider-track { width: 100%; min-width: 0; align-self: stretch; } "
                               "input[switch]::slider-fill { display: block; width: 0%; height: 100%; } "
                               "input[switch]:checked::slider-fill { width: 100%; } "
                               "input[switch]::slider-thumb { order: -1; width: 26px; height: 26px; border-radius: 7px; } "
                               "input[switch]:checked::slider-thumb { order: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitch).ok());
    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 64.f, 32.f});

    LayoutEngine::layout(control, styleSheet, text);
    EXPECT_EQ(computedStyle(styleSheet, control).display, radia::ui::DisplayMode::Flex);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_EQ(control.sliderTrack()->rect().bottom(), 23.f);
    EXPECT_EQ(control.sliderTrack()->rect().h, 26.f);
    EXPECT_EQ(control.sliderThumb()->rect().left(), 15.f);
    EXPECT_EQ(control.sliderThumb()->rect().h, 26.f);
    EXPECT_EQ(control.sliderThumb()->rect().w, 26.f);
    EXPECT_EQ(control.sliderTrack()->rect().left(), control.sliderThumb()->rect().right());
    EXPECT_EQ(control.sliderThumb()->style().borderRadius.topLeft.horizontal.pixels, 7.f);
    EXPECT_EQ(control.sliderFill()->rect().left(), control.sliderTrack()->rect().left());
    EXPECT_EQ(control.sliderFill()->rect().right(), control.sliderFill()->rect().left());
    EXPECT_EQ(control.sliderFill()->rect().bottom(), control.sliderTrack()->rect().bottom());
    EXPECT_EQ(control.sliderFill()->rect().top(), control.sliderTrack()->rect().top());

    const float uncheckedThumbLeft = control.sliderThumb()->rect().left();
    control.checked(true);
    LayoutEngine::layout(control, styleSheet, text);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    ASSERT_NE(control.sliderFill(), nullptr);
    EXPECT_EQ(control.sliderThumb()->rect().left(), control.sliderTrack()->rect().right());
    EXPECT_GT(control.sliderThumb()->rect().left(), uncheckedThumbLeft);
    EXPECT_EQ(control.sliderFill()->rect().left(), control.sliderTrack()->rect().left());
    EXPECT_EQ(control.sliderFill()->rect().right(), control.sliderTrack()->rect().right());

    control.replaceChildren();
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderFill(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_TRUE(control.children().empty());
    EXPECT_TRUE(control.childNodes().empty());
    EXPECT_EQ(control.sliderTrack()->name(), "slider-track");
    EXPECT_EQ(control.sliderFill()->name(), "slider-fill");
    EXPECT_EQ(control.sliderThumb()->name(), "slider-thumb");
}

TEST_F(LayoutEngineTest, OverlaysInlineGridSwitchPseudosAndAppliesTranslate) {
    StyleSheet styleSheet;
    constexpr char kGridSwitch[] =
        "input[switch] { appearance: base; display: inline-grid; position: relative; width: 44px; height: 20px; padding: 0px; } "
        "input[switch]::slider-track { grid-area: 1 / 1; width: 100%; } "
        "input[switch]::slider-thumb { grid-area: 1 / 1; width: 24px; height: 24px; margin: -2px -1px; } "
        "input[switch]:checked::slider-thumb { translate: 22px 0; } "
        "input[switch]:dir(rtl):checked::slider-thumb { translate: -22px 0; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridSwitch).ok());

    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 44.f, 20.f});

    LayoutEngine::layout(control, styleSheet, text);
    EXPECT_EQ(computedStyle(styleSheet, control).display, DisplayMode::InlineGrid);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_EQ(control.sliderTrack()->rect().x, 10.f);
    EXPECT_EQ(control.sliderTrack()->rect().y, 20.f);
    EXPECT_EQ(control.sliderTrack()->rect().w, 44.f);
    EXPECT_EQ(control.sliderTrack()->rect().h, 20.f);
    EXPECT_EQ(control.sliderThumb()->rect().x, 9.f);
    EXPECT_EQ(control.sliderThumb()->rect().y, 18.f);
    EXPECT_EQ(control.sliderThumb()->rect().w, 24.f);
    EXPECT_EQ(control.sliderThumb()->rect().h, 24.f);

    control.checked(true);
    LayoutEngine::layout(control, styleSheet, text);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_EQ(control.sliderThumb()->rect().x, 31.f);
    EXPECT_EQ(control.sliderThumb()->rect().y, 18.f);

    auto rtlControl = makeElementValue<HTMLInputElement>();
    rtlControl.type("checkbox").switchMode(true).checked(true);
    rtlControl.setRect({10.f, 20.f, 44.f, 20.f});
    LayoutEngine::layout(rtlControl, styleSheet, text, LayoutDirection::RightToLeft);
    ASSERT_NE(rtlControl.sliderTrack(), nullptr);
    ASSERT_NE(rtlControl.sliderThumb(), nullptr);
    EXPECT_EQ(rtlControl.sliderThumb()->rect().x, -13.f);
    EXPECT_EQ(rtlControl.sliderThumb()->rect().y, 18.f);
}

TEST_F(LayoutEngineTest, PlacesGridAreasInImplicitTracks) {
    StyleSheet styleSheet;
    constexpr char kGridLayout[] = "panel { display: grid; } label { width: 10px; height: 10px; justify-self: start; } "
                                   "#top-right { grid-area: 1 / 2; } #bottom-left { grid-area: 2 / 1; } #bottom-right { grid-area: 2 / 2; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("top-left"));
    auto topRight = makeElement<HTMLLabelElement>("top-right");
    topRight->setId("top-right");
    panel.append(std::move(topRight));
    auto bottomLeft = makeElement<HTMLLabelElement>("bottom-left");
    bottomLeft->setId("bottom-left");
    panel.append(std::move(bottomLeft));
    auto bottomRight = makeElement<HTMLLabelElement>("bottom-right");
    bottomRight->setId("bottom-right");
    panel.append(std::move(bottomRight));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 50.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 20.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 50.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 20.f);

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 40.f);
}

TEST_F(LayoutEngineTest, UsesInjectedTextMetricsForMeasurement) {
    class ExactTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return {47.f, 19.f}; }
        std::uint64_t generation() const noexcept override { return 1; }
    } exact;

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("adapter-owned");
    LayoutEngine::measure(label, styleSheet, exact);
    EXPECT_EQ(label.desiredSize().x, 47.f);
    EXPECT_EQ(label.desiredSize().y, 19.f);
}

TEST_F(LayoutEngineTest, AppliesRightToLeftRowDirection) {
    StyleSheet styleSheet;
    constexpr char kDirection[] =
        "panel { display: flex; flex-direction: row; justify-content: start; gap: 5px; } label { width: 10px; height: 10px; } "
        "#physical { margin: 0px 7px 0px 0px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDirection).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    auto physical = makeElement<HTMLLabelElement>("second");
    physical->setId("physical");
    panel.append(std::move(physical));

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 78.f);
}

TEST_F(LayoutEngineTest, PreservesNegativeNormalOffsets) {
    StyleSheet styleSheet;
    constexpr char kNegativeOffsetLayout[] = "label { position: relative; width: 10px; height: 10px; left: -8px; bottom: -3px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNegativeOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("offset"));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), -8.f);
    EXPECT_EQ(panel.children()[0]->rect().bottom(), 87.f);
}

TEST_F(LayoutEngineTest, ResolvesPercentageGeometryAgainstContainingBlock) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "label { position: relative; width: 50%; height: 25%; left: 10%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 200.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("percentage"));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_FLOAT_EQ(rect.w, 100.f);
    EXPECT_FLOAT_EQ(rect.h, 25.f);
    EXPECT_FLOAT_EQ(rect.left(), 20.f);
    EXPECT_FLOAT_EQ(rect.top(), 80.f);
}

TEST_F(LayoutEngineTest, DistributesAutomaticRowAndColumnGaps) {
    StyleSheet styleSheet;
    constexpr char kRowGapLayout[] = "panel { display: flex; flex-direction: row; gap: auto; } "
                                     "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowGapLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    panel.append(makeElement<HTMLLabelElement>("third"));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    constexpr char kColumnGapLayout[] = "panel { display: flex; flex-direction: column; gap: auto; } "
                                        "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnGapLayout).ok());
    panel.setRect({0.f, 0.f, 20.f, 100.f});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().top(), 55.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, MeasuresFloaterWithFixedHeightAndAutomaticWidth) {
    StyleSheet styleSheet;
    constexpr char kFloater[] = "floater { size: auto 100px; display: flex; flex-direction: column; } floater > head { height: 30px; } "
                                "floater > body { display: flex; flex-direction: column; gap: 5px; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloater).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.body()->append(makeElement<HTMLLabelElement>("first"));
    floater.body()->append(makeElement<HTMLLabelElement>("second"));

    const Vec2 measured = LayoutEngine::measure(floater, styleSheet, text);
    EXPECT_EQ(measured.x, 100.f);
    EXPECT_EQ(measured.y, 75.f);
}

TEST_F(LayoutEngineTest, AppliesCrossAxisAlignmentAndDirection) {
    StyleSheet styleSheet;
    constexpr char kRowAlignment[] = "panel { display: flex; flex-direction: row; align-items: start; } label { width: 10px; height: 10px; } "
                                     "label#center { align-self: center; } label#end { align-self: end; } "
                                     "label#stretch { height: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowAlignment).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end", "stretch"}) {
        auto label = makeElement<HTMLLabelElement>(id);
        label->setId(id);
        panel.append(std::move(label));
    }

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 15.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[3]->rect().h, 40.f);

    constexpr char kColumnAlignment[] = "panel { display: flex; flex-direction: column; align-items: start; } label { width: 10px; height: 10px; } "
                                        "label#center { align-self: center; } label#end { align-self: end; } "
                                        "label#stretch { width: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnAlignment).ok());
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[3]->rect().w, 100.f);

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineTest, AppliesGridJustifySelfInBothDirections) {
    StyleSheet styleSheet;
    constexpr char kGridAlignment[] = "panel { display: grid; } label { width: 20px; height: 10px; } "
                                      "label#start { justify-self: start; } label#center { justify-self: center; } "
                                      "label#end { justify-self: end; }";
    ASSERT_TRUE(styleSheet.loadRadia(kGridAlignment).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end"}) {
        auto label = makeElement<HTMLLabelElement>(id);
        label->setId(id);
        panel.append(std::move(label));
    }

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 80.f);

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 80.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineTest, DistinguishesVisibilityFromDisplayLayout) {
    StyleSheet styleSheet;
    constexpr char kVisibilityLayout[] = "panel { display: flex; flex-direction: row; } "
                                         "label { width: 10px; height: 10px; } label.none { display: none; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVisibilityLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeElement<HTMLLabelElement>("visible"));
    auto hidden = makeElement<HTMLLabelElement>("hidden");
    hidden->setVisibility(Visibility::Hidden);
    panel.append(std::move(hidden));
    auto collapsed = makeElement<HTMLLabelElement>("collapsed");
    collapsed->setVisibility(Visibility::Collapse);
    panel.append(std::move(collapsed));
    auto displayNone = makeElement<HTMLLabelElement>("display-none");
    displayNone->addClass("none");
    panel.append(std::move(displayNone));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 10.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
    EXPECT_EQ(LayoutEngine::measure(*panel.children()[2], styleSheet, text).x, 10.f);
    EXPECT_EQ(panel.children()[3]->desiredSize().x, 0.f);

    panel.children()[1]->setVisibility(Visibility::Collapse);
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    panel.children()[2]->setVisibility(Visibility::Visible);
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
}

TEST_F(LayoutEngineTest, AppliesContainerVerticalAlignmentToContent) {
    StyleSheet styleSheet;
    constexpr char kVerticalAlignment[] = "panel { size: 40px 100px; display: flex; flex-direction: row; } label { size: 10px; } "
                                          "panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } "
                                          "panel.column { display: flex; flex-direction: column; vertical-align: bottom; } "
                                          "panel.free-bottom { display: block; vertical-align: bottom; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVerticalAlignment).ok());

    auto addLabel = [](Element& container) { container.append(makeElement<HTMLLabelElement>("child")); };

    auto top = makeElementValue<HTMLPanelElement>();
    top.setRect({0.f, 0.f, 100.f, 40.f});
    addLabel(top);
    LayoutEngine::layout(top, styleSheet, text);
    EXPECT_EQ(top.children()[0]->rect().top(), 40.f);

    auto middle = makeElementValue<HTMLPanelElement>();
    middle.setRect({0.f, 0.f, 100.f, 40.f});
    middle.addClass("middle");
    addLabel(middle);
    LayoutEngine::layout(middle, styleSheet, text);
    EXPECT_EQ(middle.children()[0]->rect().bottom(), 15.f);

    auto bottom = makeElementValue<HTMLPanelElement>();
    bottom.setRect({0.f, 0.f, 100.f, 40.f});
    bottom.addClass("bottom");
    addLabel(bottom);
    LayoutEngine::layout(bottom, styleSheet, text);
    EXPECT_EQ(bottom.children()[0]->rect().bottom(), 0.f);

    auto column = makeElementValue<HTMLPanelElement>();
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.addClass("column");
    addLabel(column);
    addLabel(column);
    LayoutEngine::layout(column, styleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().top(), 20.f);
    EXPECT_EQ(column.children()[1]->rect().bottom(), 0.f);

    auto freeBottom = makeElementValue<HTMLPanelElement>();
    freeBottom.setRect({0.f, 0.f, 100.f, 40.f});
    freeBottom.addClass("free-bottom");
    addLabel(freeBottom);
    LayoutEngine::layout(freeBottom, styleSheet, text);
    EXPECT_EQ(freeBottom.children()[0]->rect().bottom(), 30.f);
}

TEST_F(LayoutEngineTest, WrapsChildrenAcrossFlowBreaks) {
    StyleSheet styleSheet;
    constexpr char kFlowBreakLayout[] = "panel { display: flex; flex-direction: row; gap: 2px; } "
                                        "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlowBreakLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto first = makeElement<HTMLLabelElement>("first");
    auto second = makeElement<HTMLLabelElement>("second");
    auto third = makeElement<HTMLLabelElement>("third");
    NodeAccess::setFlowBreakBefore(*second, true);
    panel.append(std::move(first));
    panel.append(std::move(second));
    panel.append(std::move(third));

    LayoutEngine::measure(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 22.f);
    EXPECT_EQ(panel.desiredSize().y, 22.f);

    panel.setRect({0.f, 0.f, 40.f, 40.f});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 28.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 12.f);
}

TEST_F(LayoutEngineTest, AppliesFlexBasisAndScaledShrink) {
    StyleSheet styleSheet;
    constexpr char kFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } label { height: 10px; flex: 0 1 80px; } "
                                        "label.second { flex-basis: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexBasisLayout).ok());

    auto intrinsic = makeElementValue<HTMLPanelElement>();
    intrinsic.append(makeElement<HTMLLabelElement>());
    auto intrinsicSecond = makeElement<HTMLLabelElement>();
    intrinsicSecond->addClass("second");
    intrinsic.append(std::move(intrinsicSecond));
    LayoutEngine::measure(intrinsic, styleSheet, text);
    EXPECT_EQ(intrinsic.desiredSize().x, 120.f);

    StyleSheet percentageTheme;
    constexpr char kPercentageFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } "
                                                  "label { width: 30px; flex-basis: 50%; }";
    ASSERT_TRUE(percentageTheme.loadRadia(kPercentageFlexBasisLayout).ok());
    auto indefinite = makeElementValue<HTMLPanelElement>();
    indefinite.append(makeElement<HTMLLabelElement>());
    LayoutEngine::measure(indefinite, percentageTheme, text);
    EXPECT_EQ(indefinite.desiredSize().x, 30.f);

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>());
    auto second = makeElement<HTMLLabelElement>();
    second->addClass("second");
    panel.append(std::move(second));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().w, 200.f / 3.f, 0.001f);
    EXPECT_NEAR(panel.children()[1]->rect().w, 100.f / 3.f, 0.001f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineTest, CentersOversizedFloaterHeadChildren) {
    StyleSheet styleSheet;
    constexpr char kFloaterHead[] =
        "floater > head { height: 48px; display: flex; flex-direction: row; padding: 12px; } "
        "floater > head > title { height: 24px; display: flex; flex-direction: row; align-items: center; flex-grow: 1; line-height: 18px; } "
        "floater > head > title > icon { size: 28px; } "
        "floater > head > close { size: 24px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterHead).ok());

    auto floater = makeElementValue<HTMLFloaterElement>();
    radia::ui::test::appendFloaterStructure(floater, true);
    Element* head = floater.head();
    ASSERT_NE(head, nullptr);
    ASSERT_FALSE(head->children().empty());
    auto icon = makeElement<HTMLIconElement>("search");
    head->children().front()->append(std::move(icon));
    head->setRect({0.f, 0.f, 200.f, 48.f});
    LayoutEngine::layout(*head, styleSheet, text);

    const float headCenter = head->rect().y + head->rect().h * .5f;
    for (const auto& child : head->children()) {
        if (!child->isVisible(computedStyle(styleSheet, *child))) continue;
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

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 30.f, 100.f});
    panel.append(makeParagraph("alpha beta"));
    panel.append(makeParagraph("after"));

    LayoutEngine::layout(panel, styleSheet, text);
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

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeParagraph("alpha beta"));
    panel.append(makeElement<HTMLLabelElement>("x"));

    const Vec2 measured = LayoutEngine::measure(panel, styleSheet, text);
    EXPECT_EQ(measured.y, 20.f);

    panel.setRect({0.f, 0.f, measured.x, measured.y});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 20.f);
    EXPECT_EQ(panel.children()[1]->rect().w, 10.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, ReappliesFlexBasisAfterTextReflow) {
    StyleSheet columnTheme;
    constexpr char kColumnBasis[] = "panel { width: 30px; display: flex; flex-direction: column; } "
                                    "p { flex-basis: 40px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(columnTheme.loadRadia(kColumnBasis).ok());
    auto column = makeElementValue<HTMLPanelElement>();
    column.append(makeParagraph("alpha beta"));
    EXPECT_EQ(LayoutEngine::measure(column, columnTheme, text).y, 40.f);

    StyleSheet rowTheme;
    constexpr char kRowMinimum[] = "panel { width: 80px; display: flex; flex-direction: row; } p { flex: 0 1 100px; font-size: 10px; "
                                   "line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; }";
    ASSERT_TRUE(rowTheme.loadRadia(kRowMinimum).ok());
    auto row = makeElementValue<HTMLPanelElement>();
    row.append(makeParagraph("alpha beta"));
    row.append(makeElement<HTMLLabelElement>("x"));
    row.setRect({0.f, 0.f, 80.f, 20.f});
    LayoutEngine::layout(row, rowTheme, text);
    EXPECT_EQ(row.children()[0]->rect().w, 70.f);
}
TEST_F(LayoutEngineTest, InvalidatesTextMeasurementCacheWhenMetricsGenerationChanges) {
    class GenerationTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return mSize; }
        std::uint64_t generation() const noexcept override { return mGeneration; }
        void advance() {
            ++mGeneration;
            mSize = {64.f, 18.f};
        }

    private:
        std::uint64_t mGeneration = 1;
        Vec2 mSize{32.f, 12.f};
    } metrics;

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("generation");
    LayoutEngine::measure(label, styleSheet, metrics);
    EXPECT_FLOAT_EQ(label.desiredSize().x, 32.f);
    EXPECT_FLOAT_EQ(label.desiredSize().y, 12.f);

    metrics.advance();
    LayoutEngine::measure(label, styleSheet, metrics);
    EXPECT_FLOAT_EQ(label.desiredSize().x, 64.f);
    EXPECT_FLOAT_EQ(label.desiredSize().y, 18.f);
}

TEST_F(LayoutEngineTest, ReallocatesColumnFlexChildrenUnderHeightConstraint) {
    StyleSheet styleSheet;
    constexpr char kColumnFlexLayout[] = "panel { display: flex; flex-direction: column; } "
                                         "label { height: 10px; flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnFlexLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineTest, ResolvesSiblingColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kSiblingColumnLayout[] = "panel { display: flex; flex-direction: column; } "
                                            "#quarter { height: 25%; } #half { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSiblingColumnLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto quarter = makeElement<HTMLLabelElement>("quarter");
    quarter->setId("quarter");
    auto half = makeElement<HTMLLabelElement>("half");
    half->setId("half");
    panel.append(std::move(quarter));
    panel.append(std::move(half));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().bottom(), panel.children()[1]->rect().top());
}

TEST_F(LayoutEngineTest, ResolvesNestedColumnPercentages) {
    StyleSheet styleSheet;
    constexpr char kNestedColumnLayout[] =
        "panel { display: flex; flex-direction: column; } #child { height: 50%; display: flex; flex-direction: column; } "
        "#grandchild { height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedColumnLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto child = makeElement<HTMLPanelElement>();
    child->setId("child");
    auto grandchild = makeElement<HTMLLabelElement>("nested");
    grandchild->setId("grandchild");
    child->append(std::move(grandchild));
    panel.append(std::move(child));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    const Element& nestedPanel = *panel.children().front();
    ASSERT_EQ(nestedPanel.children().size(), 1U);
    EXPECT_FLOAT_EQ(nestedPanel.rect().h, 50.f);
    EXPECT_FLOAT_EQ(nestedPanel.children().front()->rect().h, 25.f);
}

TEST_F(LayoutEngineTest, RespectsColumnMinimumHeightsDuringShrink) {
    StyleSheet styleSheet;
    constexpr char kColumnMinimumLayout[] = "panel { display: flex; flex-direction: column; } "
                                            "label { height: 30px; min-height: 25px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnMinimumLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 25.f);
}

TEST_F(LayoutEngineTest, SeparatesLayoutCachesByStylesheetIdentity) {
    StyleSheet narrow;
    StyleSheet wide;
    constexpr char kNarrowLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kWideLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(narrow.loadRadia(kNarrowLabelLayout).ok());
    ASSERT_TRUE(wide.loadRadia(kWideLabelLayout).ok());

    auto label = makeElementValue<HTMLLabelElement>("identity");
    LayoutEngine::layout(label, narrow, text);
    LayoutEngine::layout(label, wide, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineTest, MatchesCopiedStylesheetSnapshot) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("label { width: 10px; height: 10px; }").ok());
    StyleSheet copy = styleSheet;
    StylePass pass(styleSheet, text);

    EXPECT_TRUE(pass.matches(copy, text));
}

TEST_F(LayoutEngineTest, SeparatesLayoutCachesByTextMetricsIdentity) {
    class WidthMetrics final : public TextMetrics {
    public:
        explicit WidthMetrics(float width) : mWidth(width) {}
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return {mWidth, 12.f}; }
        std::uint64_t generation() const noexcept override { return 1; }

    private:
        float mWidth;
    } narrow(12.f), wide(48.f);

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("identity");
    LayoutEngine::layout(label, styleSheet, narrow);
    LayoutEngine::layout(label, styleSheet, wide);
    EXPECT_EQ(label.desiredSize().x, 48.f);
}

TEST_F(LayoutEngineTest, OrdersChildrenBeforeRowLayout) {
    StyleSheet styleSheet;
    constexpr char kOrderedFlow[] = "panel { display: flex; flex-direction: row; } #late { order: 2; width: 10px; height: 10px; } "
                                    "#early { order: -1; width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kOrderedFlow).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto late = makeElement<HTMLLabelElement>("late");
    late->setId("late");
    auto early = makeElement<HTMLLabelElement>("early");
    early->setId("early");
    panel.append(std::move(late));
    panel.append(std::move(early));

    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[0]->rect().x, 10.f);
}

TEST_F(LayoutEngineTest, InvalidatesLayoutCacheWhenStylesheetIsAssigned) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kReplacementLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    auto label = makeElementValue<HTMLLabelElement>("assigned");
    LayoutEngine::layout(label, styleSheet, text);

    StyleSheet replacement;
    ASSERT_TRUE(replacement.loadRadia(kReplacementLabelLayout).ok());
    styleSheet = replacement;
    LayoutEngine::layout(label, styleSheet, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineTest, RemeasuresNormalPercentageTextAfterResize) {
    StyleSheet styleSheet;
    constexpr char kNormalTextLayout[] = "panel { display: block; } "
                                         "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalTextLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeParagraph("alpha beta"));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);

    panel.setRect({0.f, 0.f, 200.f, 100.f});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 10.f);
}

TEST_F(LayoutEngineTest, RelativeOffsetsDoNotChangeIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalOffsetLayout[] = "panel { display: block; } "
                                           "label { position: relative; width: 20px; height: 10px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeElement<HTMLLabelElement>("positioned"));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitNormalGeometryInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(child));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 40.f);
    EXPECT_EQ(panel.desiredSize().y, 30.f);
}

TEST_F(LayoutEngineTest, InvalidatesIntrinsicSizeAfterExplicitGeometryChange) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({0.f, 0.f, 20.f, 10.f});
    Element* childPtr = child.get();
    panel.append(std::move(child));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);

    childPtr->setRect({0.f, 0.f, 60.f, 10.f});
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 60.f);
}

TEST_F(LayoutEngineTest, IncludesExplicitNormalPositionInIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({10.f, 12.f, 20.f, 8.f});
    panel.append(std::move(child));
    LayoutEngine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
}

TEST_F(LayoutEngineTest, IncludesWrappedPercentageChildInNormalIntrinsicHeight) {
    StyleSheet styleSheet;
    constexpr char kWrappedTextLayout[] = "panel { display: block; width: 100px; } "
                                          "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrappedTextLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeParagraph("alpha beta"));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);
}

TEST_F(LayoutEngineTest, RelativePercentageOffsetsDoNotChangeIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalPercentageOffsetLayout[] = "panel { display: block; } "
                                                     "label { position: relative; width: 20px; height: 10px; left: 50%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalPercentageOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("positioned"));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 80.f});
    panel.append(makeElement<HTMLLabelElement>("sized"));

    LayoutEngine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_FLOAT_EQ(panel.children().front()->rect().w, 50.f);
    EXPECT_FLOAT_EQ(panel.children().front()->rect().h, 40.f);
}

TEST_F(LayoutEngineTest, ResolvesNestedPercentageFlexBasis) {
    StyleSheet styleSheet;
    constexpr char kNestedFlexBasis[] = "#outer { display: flex; flex-direction: row; width: 100px; height: 20px; } "
                                        "#inner { display: flex; flex-direction: row; width: 50%; } label { flex-basis: 50%; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNestedFlexBasis).ok());
    auto outer = makeElementValue<HTMLPanelElement>();
    outer.setRect({0.f, 0.f, 100.f, 20.f});
    outer.setId("outer");
    auto inner = makeElement<HTMLPanelElement>();
    inner->setId("inner");
    Element* innerPtr = inner.get();
    inner->append(makeElement<HTMLLabelElement>("basis"));
    ASSERT_EQ(inner->children().size(), 1U);
    Element* label = inner->children().front();
    outer.append(std::move(inner));

    LayoutEngine::layout(outer, styleSheet, text);
    EXPECT_FLOAT_EQ(innerPtr->rect().w, 50.f);
    EXPECT_FLOAT_EQ(label->rect().w, 25.f);
}

TEST_F(LayoutEngineTest, ComputesScrollMetricsAndClampsProgrammaticPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 100.f, 120.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Overlay;

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::LeftToRight, options);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);
    panel.scrollTo(500.f, 500.f);
    contentPtr->setRect({0.f, 0.f, 40.f, 40.f});
    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 40.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);
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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Classic;

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::LeftToRight, options);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineTest, NoneScrollbarWidthPreservesRangeWithoutScrollbarSpace) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-width: none; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

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
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 15.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 85.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 100.f);
}

TEST_F(LayoutEngineTest, StableBothEdgesReservesPhysicalInlineEdgesInRtl) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
                    .loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-gutter: stable both-edges; } "
                               "#content { display: block; width: 100%; height: 100%; }")
                    .ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    LayoutEngine::layout(panel, styleSheet, text, LayoutDirection::RightToLeft);

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
