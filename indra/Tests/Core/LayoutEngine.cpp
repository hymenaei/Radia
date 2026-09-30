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
#include <Core/LayoutEngine.h>
#include <Core/LayoutGeometry.h>
#include <Core/RecordingPaintContext.h>
#include <Core/ResourceElementDefinition.h>
#include <Core/StylePass.h>
#include <Core/StyleSheet.h>
#include <Core/Text.h>
#include <Core/TextMeasurer.h>
#include <gtest/gtest.h>
#include <variant>
#include "ComputedStyleProperties.h"
#include "FloaterTestHelpers.h"

namespace {
using Core::Style::BoxSizing;
using Core::Style::ComputedStyle;

using Core::Element;
using Core::FixedTextMeasurer;
using Core::HTMLButtonElement;
using Core::HTMLFloaterElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::PaintCommandKind;
using Core::RecordingPaintContext;
using Core::ScrollbarMode;
using Core::Text;
using Core::TextMeasurer;
using Core::CSS::StyleLayer;
using Core::CSS::StyleOrigin;
using Core::CSS::StyleSheet;
using Core::CSS::StyleSheetLoadResult;
using Core::detail::appendText;
using Core::detail::makeElement;
using Core::detail::makeElementValue;
using Core::detail::NodeAccess;
using Core::detail::nodes;
using Core::Layout::Direction;
using Core::Layout::Engine;
using Core::Layout::paddingPixels;
using Core::Layout::Rect;
using Core::Layout::ScrollLayoutOptions;
using Core::Layout::Vec2;
using Core::Style::Display;
using Core::Style::LineHeight;
using Core::Style::Pass;
using Core::Style::Visibility;
using ::testing::Message;
using ::testing::Test;

ComputedStyle computedStyle(const StyleSheet& stylesheet, const Element& element) {
    Pass styles(stylesheet, FixedTextMeasurer {});
    return styles.style(element);
}
} // namespace

namespace {
std::unique_ptr<Element> makeParagraph(std::string text) {
    auto paragraph = makeElement<Element>("p");
    paragraph->textContent(std::move(text));
    return paragraph;
}

Element& appendIcon(HTMLButtonElement& button, std::string name) {
    auto icon = makeElement<Element>("i");
    Element* result = icon.get();
    result->classList().add("i-" + name);
    button.append(std::move(icon));
    return *result;
}

void appendButtonText(HTMLButtonElement& button, std::string text) { appendText(button, std::move(text)); }

class LayoutEngineFixture : public Test {
protected:
    FixedTextMeasurer text;
};

TEST_F(LayoutEngineFixture, UsesUnitlessLineHeightAsMultiplier) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("p { font-size: 20px; line-height: 1.5; }").ok());

    const ComputedStyle style = stylesheet.resolve("p", "", {}, {});
    ASSERT_TRUE(std::holds_alternative<LineHeight::Number>(style.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Number>(style.lineHeight().mValue).value, 1.5f);
    EXPECT_FLOAT_EQ(text.measureText("line", style).y, 30.f);
}

TEST_F(LayoutEngineFixture, ResolvesPercentageLineHeightAgainstComputedFontSize) {
    StyleSheet stylesheet;
    ASSERT_TRUE(
        stylesheet.loadRadia("panel { font-size: 20px; line-height: 150%; } label { font-size: 10px; } label.local { line-height: 120%; }")
            .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto inherited = makeElement<HTMLLabelElement>();
    HTMLLabelElement* inheritedPointer = inherited.get();
    panel.append(std::move(inherited));
    auto local = makeElement<HTMLLabelElement>();
    local->classList().add("local");
    HTMLLabelElement* localPointer = local.get();
    panel.append(std::move(local));

    Pass styles(stylesheet, text);
    const ComputedStyle& panelStyle = styles.style(panel);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(panelStyle.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(panelStyle.lineHeight().mValue).pixels, 30.f);
    const ComputedStyle& inheritedStyle = styles.style(*inheritedPointer);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(inheritedStyle.lineHeight().mValue));
    EXPECT_EQ(std::get<LineHeight::Length>(inheritedStyle.lineHeight().mValue).pixels, 30.f);
    const ComputedStyle& localStyle = styles.style(*localPointer);
    ASSERT_TRUE(std::holds_alternative<LineHeight::Length>(localStyle.lineHeight().mValue));
    EXPECT_NEAR(std::get<LineHeight::Length>(localStyle.lineHeight().mValue).pixels, 12.f, 1.0e-4f);
}

TEST_F(LayoutEngineFixture, TranslatePercentages) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia(
                "panel { display: block; position: relative; } label { position: absolute; left: 0; top: 0; width: 20px; height: 10px; "
                "translate: 50% 100%; }")
            .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPointer = label.get();
    panel.append(std::move(label));

    Engine::layout(panel, stylesheet, text);

    EXPECT_FLOAT_EQ(labelPointer->rect().x, 10.f);
    EXPECT_FLOAT_EQ(labelPointer->rect().y, 80.f);
}

TEST_F(LayoutEngineFixture, MeasuresButtonContent) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { position: relative; left: 10px; top: 10px; padding: 7px; gap: 6px; display: flex; flex-direction: row; "
        "font-size: 13px; line-height: 18px; } button > i { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = makeElement<HTMLButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    Engine::layout(root, styleSheet, text);
    ASSERT_EQ(root.children().size(), 1U);
    const Element& result = *root.children().front();
    EXPECT_EQ(result.rect().w, 300.f);
    EXPECT_EQ(result.rect().h, 32.f);
    const auto runtimeChildren = nodes(result);
    ASSERT_EQ(runtimeChildren.size(), 2U);
    ASSERT_NE(runtimeChildren.begin()->asElement(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asElement()->elementName(), "i");
    auto textChild = runtimeChildren.begin();
    ++textChild;
    ASSERT_NE(textChild->asText(), nullptr);
    EXPECT_EQ(textChild->asText()->rect().x - runtimeChildren.begin()->asElement()->rect().right(), 6.f);
}

TEST_F(LayoutEngineFixture, IgnoresFlexWhitespace) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { width: 100px; height: 20px; padding: 0; gap: 6px; display: flex; flex-direction: row; justify-content: start; } "
        "button > i { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 20.f});
    auto leadingWhitespace = std::make_unique<Text>("\n        ");
    Text* leadingWhitespacePtr = leadingWhitespace.get();
    button.append(std::move(leadingWhitespace));
    Element& icon = appendIcon(button, "search");
    auto betweenWhitespace = std::make_unique<Text>("\n        ");
    Text* betweenWhitespacePtr = betweenWhitespace.get();
    button.append(std::move(betweenWhitespace));
    auto label = std::make_unique<Text>("Press");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    Engine::layout(button, styleSheet, text);

    EXPECT_EQ(leadingWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(betweenWhitespacePtr->rect().w, 0.f);
    EXPECT_EQ(icon.rect().x, 0.f);
    EXPECT_EQ(labelPtr->rect().x - icon.rect().right(), 6.f);
}

TEST_F(LayoutEngineFixture, PreservesInlineWhitespace) {
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

    Engine::layout(paragraph, styleSheet, text);

    EXPECT_GT(separatorPtr->rect().w, 0.f);
    EXPECT_FLOAT_EQ(separatorPtr->rect().x, italicPtr->rect().right());
    EXPECT_FLOAT_EQ(strikePtr->rect().x, separatorPtr->rect().right());
}

TEST_F(LayoutEngineFixture, CollapsesButtonWhitespace) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { display: inline-block; width: 100px; height: 40px; padding: 0; font-size: 10px; "
                                     "line-height: 10px; } button > i { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 40.f});
    Element& icon = appendIcon(button, "search");
    auto separator = std::make_unique<Text>("\n            ");
    Text* separatorPtr = separator.get();
    button.append(std::move(separator));
    auto label = std::make_unique<Text>("Press");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    Engine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(separatorPtr->rect().w, 6.f);
    EXPECT_FLOAT_EQ(separatorPtr->rect().h, 10.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().left() - icon.rect().right(), 6.f);
}

TEST_F(LayoutEngineFixture, IgnoresFlowWhitespace) {
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

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(leadingWhitespacePtr->rect().h, 0.f);
    EXPECT_EQ(paragraphPtr->rect().top(), 60.f);
}

TEST_F(LayoutEngineFixture, StartsFloaterBodyAtFirstElement) {
    StyleSheet styleSheet;
    constexpr char kFloaterLayout[] =
        "floater { width: 100px; height: 100px; display: flex; flex-direction: column; } "
        "floater > head { height: 20px; } "
        "floater > body { display: flex; flex-direction: column; flex-grow: 1; margin: 0; padding: 0; gap: 0; } "
        "p { height: 18px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterLayout).ok());

    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(std::make_unique<Text>("\n        "));
    auto status = makeElement<Element>("p");
    Element* statusPtr = status.get();
    status->setId("status");
    status->textContent("Ready");
    floater.body()->append(std::move(status));

    Engine::layout(floater, styleSheet, text);

    EXPECT_EQ(statusPtr->rect().top(), floater.body()->rect().top());
}

TEST_F(LayoutEngineFixture, LaysOutNormalFlowChildren) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; } .block { display: "
                                     "block; width: 50px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 60.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->classList().add("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->classList().add("inline");
    panel.append(std::move(second));
    auto block = makeElement<HTMLLabelElement>("block");
    block->classList().add("block");
    panel.append(std::move(block));
    auto after = makeElement<HTMLLabelElement>("after");
    after->classList().add("inline");
    panel.append(std::move(after));

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 30.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 60.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 50.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 40.f);
}

TEST_F(LayoutEngineFixture, AlignsInlineContent) {
    StyleSheet styleSheet;
    constexpr char kAlignedLayout[] =
        "panel { display: block; text-align: center; } .inline { display: inline; width: 20px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kAlignedLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->classList().add("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->classList().add("inline");
    panel.append(std::move(second));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.children()[0]->rect().left(), 30.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().left(), 50.f);
}

TEST_F(LayoutEngineFixture, WrapsInlineSiblings) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } .inline { display: inline; width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 50.f, 40.f});
    auto first = makeElement<HTMLLabelElement>("first");
    first->classList().add("inline");
    panel.append(std::move(first));
    auto second = makeElement<HTMLLabelElement>("second");
    second->classList().add("inline");
    panel.append(std::move(second));

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 30.f);
}

TEST_F(LayoutEngineFixture, CentersContentInWidth) {
    StyleSheet styleSheet;
    constexpr char kCenteredButtonLayout[] =
        "button { width: 128px; height: 32px; padding: 7px; gap: 6px; display: flex; flex-direction: row; "
        "justify-content: center; line-height: 18px; } button > i { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredButtonLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = makeElement<HTMLButtonElement>();
    appendIcon(*button, "search");
    appendButtonText(*button, "Apply");
    root.append(std::move(button));
    Engine::layout(root, styleSheet, text);
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

TEST_F(LayoutEngineFixture, UsesNormalButtonLayout) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] =
        "button { appearance: auto; display: inline-block; width: 128px; height: 32px; padding: 7px; text-align: "
        "center; line-height: 18px; } button > i { size: 14px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 128.f, 32.f});
    Element& icon = appendIcon(button, "search");
    auto label = std::make_unique<Text>("\n        Apply\n    ");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    const ComputedStyle computed = computedStyle(styleSheet, button);
    ASSERT_EQ(computed.appearance(), Core::Style::Appearance::Auto);
    ASSERT_EQ(computed.display(), Display::InlineBlock);

    Engine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().h, 18.f);
    EXPECT_EQ(labelPtr->data(), "\n        Apply\n    ");
    EXPECT_EQ(computedStyle(styleSheet, button).display(), Display::InlineBlock);
    const Core::Layout::RectEdges<float> borderInsets = Core::Layout::borderWidths(computed);
    const float contentLeft = button.rect().left() + borderInsets.left + paddingPixels(computed).left;
    const float contentWidth = button.rect().w - borderInsets.horizontal() - paddingPixels(computed).horizontal();
    const float contentWidthUsed = labelPtr->rect().right() - icon.rect().left();
    EXPECT_FLOAT_EQ(icon.rect().left(), contentLeft + (contentWidth - contentWidthUsed) * 0.5f);
}

TEST_F(LayoutEngineFixture, CentersButtonContent) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { display: inline-block; width: 100px; height: 40px; padding: 0; line-height: 10px; }";
    const std::vector<StyleLayer> layers {
        {StyleOrigin::UserAgent, {"ua.css", std::string(Core::CSS::userAgentStyleSheet())}},
        {StyleOrigin::Skin, {"skin.css", kButtonLayout}},
    };
    ASSERT_TRUE(styleSheet.loadRadiaLayers(layers).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 100.f, 40.f});
    auto label = std::make_unique<Text>("Apply");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    Engine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().bottom(), 15.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().top(), 25.f);
}

TEST_F(LayoutEngineFixture, LeavesUnstyledButtonUncentered) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { display: inline-block; width: 100px; height: 40px; padding: 0; line-height: 10px; } "
                                     "button.unstyled { appearance: none; }";
    const std::vector<StyleLayer> layers {
        {StyleOrigin::UserAgent, {"ua.css", std::string(Core::CSS::userAgentStyleSheet())}},
        {StyleOrigin::Skin, {"skin.css", kButtonLayout}},
    };
    ASSERT_TRUE(styleSheet.loadRadiaLayers(layers).ok());

    auto button = makeElementValue<HTMLButtonElement>();
    button.classList().add("unstyled");
    button.setRect({0.f, 0.f, 100.f, 40.f});
    auto label = std::make_unique<Text>("Apply");
    Text* labelPtr = label.get();
    button.append(std::move(label));

    const ComputedStyle computed = computedStyle(styleSheet, button);
    ASSERT_EQ(computed.appearance(), Core::Style::Appearance::NoneValue);

    Engine::layout(button, styleSheet, text);

    EXPECT_FLOAT_EQ(labelPtr->rect().bottom(), 29.f);
    EXPECT_FLOAT_EQ(labelPtr->rect().top(), 39.f);
}

TEST_F(LayoutEngineFixture, LaysOutPaddedColumn) {
    StyleSheet styleSheet;
    constexpr char kColumnLayout[] = "panel { padding: 10px; display: flex; flex-direction: column; gap: 5px; } "
                                     "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(makeElement<HTMLLabelElement>("one"));
    root.append(makeElement<HTMLLabelElement>("two"));
    Engine::layout(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().w, 80.f);
    EXPECT_EQ(root.children()[0]->rect().bottom() - root.children()[1]->rect().top(), 5.f);
}

TEST_F(LayoutEngineFixture, AppliesBoxSizing) {
    StyleSheet styleSheet;
    constexpr char kBoxSizingLayout[] = "panel { display: block; } label { display: block; width: 100px; height: 20px; padding: 10px; "
                                        "border: 2px solid #000000; } label.border { box-sizing: border-box; }";
    ASSERT_TRUE(styleSheet.loadRadia(kBoxSizingLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 400.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("content-box"));
    auto borderBox = makeElement<HTMLLabelElement>("border-box");
    borderBox->classList().add("border");
    panel.append(std::move(borderBox));

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(computedStyle(styleSheet, *panel.children()[0]).boxSizing(), BoxSizing::ContentBox);
    EXPECT_EQ(computedStyle(styleSheet, *panel.children()[1]).boxSizing(), BoxSizing::BorderBox);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().w, 124.f);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().h, 44.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().w, 100.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().h, 20.f);
}

TEST_F(LayoutEngineFixture, KeepsPaddingOutOfContent) {
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

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 40.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 80.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 30.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 90.f);
}

TEST_F(LayoutEngineFixture, KeepsPaddingOnOverflow) {
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

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 160.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 140.f);
}

TEST_F(LayoutEngineFixture, KeepsBorderOutsideScrollport) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel#viewport { display: block; overflow: hidden; border: 2px solid #ffffff; padding: 4px; } "
                       "#content { display: block; width: 100%; height: 100%; }")
            .ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setId("content");
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 96.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 96.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 6.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 94.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 6.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 94.f);
}

TEST_F(LayoutEngineFixture, PositionsChildByEdges) {
    StyleSheet styleSheet;
    constexpr char kRightBottomLayout[] = "panel { position: relative; width: 40px; height: 30px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRightBottomLayout).ok());
    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.append(makeElement<HTMLPanelElement>());
    Engine::layout(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().right(), 35.f);
    EXPECT_EQ(root.children()[0]->rect().bottom(), 77.f);
}

TEST_F(LayoutEngineFixture, RemovesAbsoluteAndFixedChildrenFromFlow) {
    StyleSheet styleSheet;
    constexpr char kPositionedLayout[] =
        "panel { position: relative; width: 100px; height: 100px; } label { display: block; width: 10px; height: 10px; } "
        "#absolute { position: absolute; left: 10px; top: 5px; width: 20px; height: 15px; } "
        "#fixed { position: fixed; right: 7px; bottom: 9px; width: 12px; height: 11px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPositionedLayout).ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    auto normal = makeElement<HTMLLabelElement>("normal");
    auto absolute = makeElement<HTMLLabelElement>("absolute");
    absolute->setId("absolute");
    auto fixed = makeElement<HTMLLabelElement>("fixed");
    fixed->setId("fixed");
    root.append(std::move(normal));
    root.append(std::move(absolute));
    root.append(std::move(fixed));
    root.append(makeElement<HTMLLabelElement>());

    Engine::layout(root, styleSheet, text);
    EXPECT_EQ(root.children()[0]->rect().top(), 100.f);
    EXPECT_EQ(root.children()[3]->rect().top(), 90.f);
    EXPECT_EQ(root.children()[1]->rect().left(), 10.f);
    EXPECT_EQ(root.children()[1]->rect().top(), 95.f);
    EXPECT_EQ(root.children()[2]->rect().right(), 93.f);
    EXPECT_EQ(root.children()[2]->rect().bottom(), 9.f);
}

TEST_F(LayoutEngineFixture, UsesNearestPositionedContainingBlock) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: block; } panel.positioned { position: relative; width: 100px; height: 60px; left: 20px; } "
                       "label { width: 10px; height: 10px; } label.absolute { position: absolute; left: 10px; top: 5px; }")
            .ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 200.f, 100.f});
    auto positioned = makeElement<HTMLPanelElement>();
    positioned->classList().add("positioned");
    Element* positionedPtr = positioned.get();
    auto absolute = makeElement<HTMLLabelElement>();
    absolute->classList().add("absolute");
    Element* absolutePtr = absolute.get();
    positioned->append(std::move(absolute));
    root.append(std::move(positioned));

    Engine::layout(root, styleSheet, text);

    EXPECT_EQ(positionedPtr->rect().left(), 20.f);
    EXPECT_EQ(absolutePtr->rect().left(), 30.f);
    EXPECT_EQ(absolutePtr->rect().top(), positionedPtr->rect().top() - 5.f);
}

TEST_F(LayoutEngineFixture, KeepsFixedChildrenAtViewportWhenScrolled) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: block; width: 200px; height: 100px; } .scroll { width: 100px; height: 50px; overflow-y: scroll; } "
                       ".spacer { height: 200px; } .fixed { position: fixed; left: 20px; top: 10px; width: 10px; height: 10px; }")
            .ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 200.f, 100.f});
    auto scroll = makeElement<HTMLPanelElement>();
    scroll->classList().add("scroll");
    Element* scrollPtr = scroll.get();
    scroll->append(makeElement<HTMLPanelElement>());
    scroll->children().front()->classList().add("spacer");
    auto fixed = makeElement<HTMLLabelElement>();
    fixed->classList().add("fixed");
    Element* fixedPtr = fixed.get();
    scroll->append(std::move(fixed));
    root.append(std::move(scroll));

    Engine::layout(root, styleSheet, text);
    ASSERT_GT(scrollPtr->scrollMetrics().maxScrollTop, 20.f);
    scrollPtr->scrollTo(0.f, 20.f);
    Engine::layout(root, styleSheet, text);

    EXPECT_FLOAT_EQ(fixedPtr->rect().left(), 20.f);
    EXPECT_FLOAT_EQ(fixedPtr->rect().top(), 90.f);
}

TEST_F(LayoutEngineFixture, ExcludesFixedChildrenFromScrollableOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: block; width: 100px; height: 50px; overflow: auto; } "
                       ".fixed { position: fixed; left: 20px; top: 500px; width: 10px; height: 10px; }")
            .ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 50.f});
    root.append(makeElement<HTMLLabelElement>());
    root.children().front()->classList().add("fixed");

    Engine::layout(root, styleSheet, text);

    EXPECT_FLOAT_EQ(root.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(root.scrollHeight(), 50.f);
    EXPECT_FLOAT_EQ(root.scrollMetrics().maxScrollTop, 0.f);
}

TEST_F(LayoutEngineFixture, AppliesIntrinsicDimensionKeywords) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: flex; flex-direction: column; align-items: flex-start; width: 20px; height: 100px; } "
                       "label { display: block; font-size: 10px; line-height: 10px; text-wrap: wrap; } "
                       "#min { width: min-content; } #max { width: max-content; } #fit { width: fit-content; }")
            .ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 20.f, 100.f});
    auto min = makeElement<HTMLLabelElement>();
    min->setId("min").textContent("alpha beta");
    Element* minPtr = min.get();
    auto max = makeElement<HTMLLabelElement>();
    max->setId("max").textContent("alpha beta");
    Element* maxPtr = max.get();
    auto fit = makeElement<HTMLLabelElement>();
    fit->setId("fit").textContent("alpha beta");
    Element* fitPtr = fit.get();
    root.append(std::move(min));
    root.append(std::move(max));
    root.append(std::move(fit));

    Engine::layout(root, styleSheet, text);

    EXPECT_FLOAT_EQ(minPtr->rect().w, 29.f);
    EXPECT_FLOAT_EQ(maxPtr->rect().w, 58.f);
    EXPECT_FLOAT_EQ(fitPtr->rect().w, 29.f);

    StyleSheet flexBasisSheet;
    ASSERT_TRUE(flexBasisSheet
            .loadRadia("panel { display: flex; width: 100px; height: 20px; } #basis { display: block; flex-basis: min-content; "
                       "font-size: 10px; line-height: 10px; text-wrap: wrap; }")
            .ok());
    auto flexRoot = makeElementValue<HTMLPanelElement>();
    flexRoot.setRect({0.f, 0.f, 100.f, 20.f});
    auto basis = makeElement<HTMLLabelElement>();
    basis->setId("basis").textContent("alpha beta");
    Element* basisPtr = basis.get();
    flexRoot.append(std::move(basis));
    Engine::layout(flexRoot, flexBasisSheet, text);
    EXPECT_FLOAT_EQ(basisPtr->rect().w, 29.f);
}

TEST_F(LayoutEngineFixture, ClampsStickyChildrenToScrollport) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: block; width: 100px; height: 100px; } .scroll { width: 100px; height: 50px; overflow-y: scroll; } "
                       ".sticky { position: sticky; top: 0px; width: 100px; height: 10px; } .spacer { height: 200px; }")
            .ok());

    auto root = makeElementValue<HTMLPanelElement>();
    root.setRect({0.f, 0.f, 100.f, 100.f});
    auto scroll = makeElement<HTMLPanelElement>();
    scroll->classList().add("scroll");
    Element* scrollPtr = scroll.get();
    auto sticky = makeElement<HTMLLabelElement>();
    sticky->classList().add("sticky");
    Element* stickyPtr = sticky.get();
    scroll->append(std::move(sticky));
    auto spacer = makeElement<HTMLPanelElement>();
    spacer->classList().add("spacer");
    scroll->append(std::move(spacer));
    root.append(std::move(scroll));

    Engine::layout(root, styleSheet, text);
    scrollPtr->scrollTo(0.f, 20.f);
    Engine::layout(root, styleSheet, text);

    EXPECT_FLOAT_EQ(stickyPtr->rect().top(), 80.f);
}

TEST_F(LayoutEngineFixture, LaysOutFloaterParts) {
    StyleSheet styleSheet;
    constexpr char kFloaterLayout[] =
        "floater { padding: 10px; display: flex; flex-direction: column; } floater > head { height: 30px; } "
        "floater > body { flex-grow: 1; } floater > head > title { position: relative; left: 5px; top: 5px; height: 15px; } "
        "label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterLayout).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater);
    ASSERT_NE(floater.head(), nullptr);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(makeElement<HTMLLabelElement>("content"));
    Engine::layout(floater, styleSheet, text);
    EXPECT_EQ(floater.head()->rect().top(), 90.f);
    EXPECT_EQ(floater.body()->rect().top(), 60.f);
}

TEST_F(LayoutEngineFixture, HidesFloaterHeadWithoutSpace) {
    StyleSheet styleSheet;
    constexpr char kCollapsedFloaterLayout[] =
        "floater { display: flex; flex-direction: column; } floater > head { display: none; height: 30px; } "
        "floater > body { flex-grow: 1; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCollapsedFloaterLayout).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.setRect({0.f, 0.f, 100.f, 100.f});
    floater.body()->append(makeElement<HTMLLabelElement>("content"));
    Engine::layout(floater, styleSheet, text);
    EXPECT_EQ(floater.body()->rect().top(), 100.f);
}

TEST_F(LayoutEngineFixture, DistributesAutoMarginAlongRow) {
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
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[1]->rect().x, 90.f);
}

TEST_F(LayoutEngineFixture, GrowsFlexItemsBeforeResolvingAutoMargins) {
    StyleSheet styleSheet;
    constexpr char kFlexLayout[] = "panel { display: flex; flex-direction: row; } "
                                   "label { width: 10px; height: 10px; } #growing { flex-grow: 1; } #pushed { margin-left: auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto growing = makeElement<HTMLLabelElement>();
    growing->setId("growing");
    auto pushed = makeElement<HTMLLabelElement>();
    pushed->setId("pushed");
    panel.append(std::move(growing));
    panel.append(std::move(pushed));

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().w, 90.f);
    EXPECT_EQ(panel.children()[1]->rect().x, 90.f);
}

TEST_F(LayoutEngineFixture, CentersColumnChildWithAutoMargins) {
    StyleSheet styleSheet;
    constexpr char kCenteredColumnLayout[] = "panel { display: flex; flex-direction: column; } "
                                             "label { width: 20px; height: 10px; margin: 0px auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kCenteredColumnLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("center"));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().x, 40.f);
}

TEST_F(LayoutEngineFixture, CentersIconInButton) {
    StyleSheet styleSheet;
    constexpr char kButtonLayout[] = "button { size: 24px; padding: 4px; display: flex; flex-direction: row; justify-content: center; } "
                                     "button > i { size: 16px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kButtonLayout).ok());
    auto button = makeElementValue<HTMLButtonElement>();
    button.setRect({0.f, 0.f, 24.f, 24.f});
    Element& icon = appendIcon(button, "search");
    Engine::layout(button, styleSheet, text);
    EXPECT_EQ(icon.rect().x, 4.f);
    EXPECT_EQ(icon.rect().y, 4.f);
}

TEST_F(LayoutEngineFixture, AlignsColumnContentToEnd) {
    StyleSheet styleSheet;
    constexpr char kColumnContentLayout[] = "panel { display: flex; flex-direction: column; justify-content: end; } "
                                            "label { height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnContentLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("bottom"));
    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.children().front()->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineFixture, DistributesFlexWidth) {
    StyleSheet styleSheet;
    constexpr char kFlexDistributionLayout[] = "panel { display: flex; flex-direction: row; } "
                                               "label { width: 10px; height: 10px; flex: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexDistributionLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().w, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineFixture, ArrangesRowChildren) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] = "panel { display: flex; flex-direction: row; gap: 3px; padding: 2px; } "
                                  "label { width: 10px; height: 8px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    Engine::measure(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 27.f);
    EXPECT_EQ(panel.desiredSize().y, 12.f);

    panel.setRect({0.f, 0.f, 40.f, 20.f});
    Engine::arrange(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().right(), 25.f);
}

TEST_F(LayoutEngineFixture, PreservesExplicitGeometry) {
    StyleSheet styleSheet;
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto button = makeElement<HTMLButtonElement>();
    button->setRect({10.f, 10.f, 20.f, 20.f});
    panel.append(std::move(button));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.x, 10.f);
    EXPECT_EQ(rect.y, 10.f);
    EXPECT_EQ(rect.w, 20.f);
    EXPECT_EQ(rect.h, 20.f);
}

TEST_F(LayoutEngineFixture, PreservesExplicitHeight) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } label { display: block; } label#sized { width: 50%; height: auto; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto label = makeElement<HTMLLabelElement>("sized");
    label->setId("sized");
    label->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(label));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);

    const Rect& rect = panel.children().front()->rect();
    EXPECT_EQ(rect.w, 50.f);
    EXPECT_EQ(rect.h, 30.f);
}

TEST_F(LayoutEngineFixture, LaysOutSwitchPseudos) {
    StyleSheet styleSheet;
    constexpr char kIntrinsicSwitchLayout[] = "input { display: flex; flex-direction: column; justify-content: center; }";
    const StyleSheetLoadResult intrinsic = styleSheet.loadRadia(kIntrinsicSwitchLayout, "switch.css");
    ASSERT_TRUE(intrinsic.ok());
    constexpr char kSwitch[] =
        "input[switch] { appearance: base; width: 64px; height: 32px; padding: 3px 5px; display: flex; flex-direction: row; } "
        "input[switch]::slider-track { width: 100%; min-width: 0; align-self: stretch; } "
        "input[switch]::slider-fill { display: block; width: 0%; height: 100%; } "
        "input[switch]:checked::slider-fill { width: 100%; } "
        "input[switch]::slider-thumb { order: -1; width: 26px; height: 26px; border-radius: 7px; } "
        "input[switch]:checked::slider-thumb { order: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitch).ok());
    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 64.f, 32.f});

    Engine::layout(control, styleSheet, text);
    EXPECT_EQ(computedStyle(styleSheet, control).display(), Core::Style::Display::Flex);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_EQ(control.sliderTrack()->rect().bottom(), 23.f);
    EXPECT_EQ(control.sliderTrack()->rect().h, 26.f);
    EXPECT_EQ(control.sliderThumb()->rect().left(), 15.f);
    EXPECT_EQ(control.sliderThumb()->rect().h, 26.f);
    EXPECT_EQ(control.sliderThumb()->rect().w, 26.f);
    EXPECT_EQ(control.sliderTrack()->rect().left(), control.sliderThumb()->rect().right());
    EXPECT_EQ(control.sliderThumb()->style().borderRadius().topLeft.horizontal.pixels, 7.f);
    EXPECT_EQ(control.sliderFill()->rect().left(), control.sliderTrack()->rect().left());
    EXPECT_EQ(control.sliderFill()->rect().right(), control.sliderFill()->rect().left());
    EXPECT_EQ(control.sliderFill()->rect().bottom(), control.sliderTrack()->rect().bottom());
    EXPECT_EQ(control.sliderFill()->rect().top(), control.sliderTrack()->rect().top());

    const float uncheckedThumbLeft = control.sliderThumb()->rect().left();
    control.checked(true);
    Engine::layout(control, styleSheet, text);
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

TEST_F(LayoutEngineFixture, LaysOutUnstyledCheckmark) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("input[type=checkbox] { appearance: none; display: flex; width: 20px; height: 20px; } "
                       "input[type=checkbox]::checkmark { width: 10px; height: 10px; }")
            .ok());

    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").checked(true);
    control.setRect({10.f, 20.f, 20.f, 20.f});

    Engine::layout(control, styleSheet, text);

    ASSERT_NE(control.checkmark(), nullptr);
    EXPECT_EQ(control.checkmark()->rect().w, 10.f);
    EXPECT_EQ(control.checkmark()->rect().h, 10.f);
}

TEST_F(LayoutEngineFixture, PositionsSwitchPseudos) {
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

    Engine::layout(control, styleSheet, text);
    EXPECT_EQ(computedStyle(styleSheet, control).display(), Display::InlineGrid);
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
    Engine::layout(control, styleSheet, text);
    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    EXPECT_EQ(control.sliderThumb()->rect().x, 31.f);
    EXPECT_EQ(control.sliderThumb()->rect().y, 18.f);

    auto rtlControl = makeElementValue<HTMLInputElement>();
    rtlControl.type("checkbox").switchMode(true).checked(true);
    rtlControl.setAttribute("dir", "rtl");
    rtlControl.setRect({10.f, 20.f, 44.f, 20.f});
    Engine::layout(rtlControl, styleSheet, text, Direction::RightToLeft);
    ASSERT_NE(rtlControl.sliderTrack(), nullptr);
    ASSERT_NE(rtlControl.sliderThumb(), nullptr);
    EXPECT_EQ(rtlControl.sliderThumb()->rect().x, -13.f);
    EXPECT_EQ(rtlControl.sliderThumb()->rect().y, 18.f);
}

TEST_F(LayoutEngineFixture, PositionsSkinSwitch) {
    StyleSheet styleSheet;
    constexpr char kSwitch[] =
        "input[switch] { appearance: base; display: inline-grid; position: relative; size: 20px 44px; } "
        "input[switch]::slider-track { display: grid; grid-area: 1 / 1; width: 100%; background-color: #98989d; } "
        "input[switch]::slider-fill { visibility: hidden; } "
        "input[switch]::slider-thumb { grid-area: 1 / 1; background-color: #ffffff; size: auto 26px; margin: 2px; justify-self: start; } "
        "input[switch]:checked::slider-thumb { translate: 14px 0; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitch).ok());

    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    control.setRect({10.f, 20.f, 44.f, 20.f});
    Engine::layout(control, styleSheet, text);

    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderThumb(), nullptr);
    ASSERT_TRUE(control.sliderThumb()->style().gridArea.has_value());
    EXPECT_EQ(control.sliderThumb()->style().gridArea->row, 1);
    EXPECT_EQ(control.sliderThumb()->style().gridArea->column, 1);
    EXPECT_FLOAT_EQ(control.sliderTrack()->rect().w, 44.f);
    EXPECT_FLOAT_EQ(control.sliderTrack()->rect().h, 20.f);
    EXPECT_GT(control.sliderThumb()->rect().w, 0.f);
    EXPECT_GT(control.sliderThumb()->rect().h, 0.f);
    EXPECT_FLOAT_EQ(control.sliderThumb()->rect().left(), control.sliderTrack()->rect().left() + 2.f);

    control.checked(true);
    Engine::layout(control, styleSheet, text);
    EXPECT_FLOAT_EQ(control.sliderThumb()->rect().left(), control.sliderTrack()->rect().left() + 16.f);

    RecordingPaintContext recording;
    control.paint(recording, computedStyle(styleSheet, control), 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), std::size_t {3});
}

TEST_F(LayoutEngineFixture, PositionsOutOfFlowPseudoChildren) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("input[switch] { appearance: base; display: block; position: relative; width: 80px; height: 40px; padding: 0; } "
                       "input[switch]::slider-track { display: block; position: relative; width: 40px; height: 20px; } "
                       "input[switch]::slider-fill { position: absolute; left: 3px; top: 4px; width: 10px; height: 6px; }")
            .ok());

    auto control = makeElementValue<HTMLInputElement>();
    control.type("checkbox").switchMode(true);
    control.setRect({0.f, 0.f, 80.f, 40.f});

    Engine::layout(control, styleSheet, text);

    ASSERT_NE(control.sliderTrack(), nullptr);
    ASSERT_NE(control.sliderFill(), nullptr);
    EXPECT_FLOAT_EQ(control.sliderFill()->rect().left(), control.sliderTrack()->rect().left() + 3.f);
    EXPECT_FLOAT_EQ(control.sliderFill()->rect().top(), control.sliderTrack()->rect().top() - 4.f);
    EXPECT_FLOAT_EQ(control.sliderFill()->rect().w, 10.f);
    EXPECT_FLOAT_EQ(control.sliderFill()->rect().h, 6.f);
}

TEST_F(LayoutEngineFixture, PlacesGridAreasInImplicitTracks) {
    StyleSheet styleSheet;
    constexpr char kGridLayout[] =
        "panel { display: grid; gap: 10px; } label { width: 10px; height: 10px; justify-self: start; } "
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

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 55.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 15.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 55.f);
    EXPECT_EQ(panel.children()[3]->rect().top(), 15.f);

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 35.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 90.f);
    EXPECT_EQ(panel.children()[3]->rect().left(), 35.f);
}

TEST_F(LayoutEngineFixture, UsesInjectedMetrics) {
    class ExactTextMeasurer final : public TextMeasurer {
    public:
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return {47.f, 19.f}; }
        std::uint64_t generation() const noexcept override { return 1; }
    } exact;

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("adapter-owned");
    Engine::measure(label, styleSheet, exact);
    EXPECT_EQ(label.desiredSize().x, 47.f);
    EXPECT_EQ(label.desiredSize().y, 19.f);
}

TEST_F(LayoutEngineFixture, AppliesRightToLeftRowDirection) {
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

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[1]->rect().right(), 78.f);
}

TEST_F(LayoutEngineFixture, AppliesReverseFlexStartAndLogicalStart) {
    StyleSheet styleSheet;
    constexpr char kDirection[] =
        "panel { display: flex; flex-direction: row-reverse; gap: 2px; } panel.flex { justify-content: flex-start; } "
        "panel.logical { justify-content: start; } panel.column { flex-direction: column-reverse; justify-content: flex-start; } "
        "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kDirection).ok());

    auto flex = makeElementValue<HTMLPanelElement>();
    flex.setRect({0.f, 0.f, 100.f, 20.f});
    flex.classList().add("flex");
    flex.append(makeElement<HTMLLabelElement>("first"));
    flex.append(makeElement<HTMLLabelElement>("second"));
    Engine::layout(flex, styleSheet, text);
    EXPECT_EQ(flex.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(flex.children()[1]->rect().right(), 88.f);

    auto logical = makeElementValue<HTMLPanelElement>();
    logical.setRect({0.f, 0.f, 100.f, 20.f});
    logical.classList().add("logical");
    logical.append(makeElement<HTMLLabelElement>("first"));
    logical.append(makeElement<HTMLLabelElement>("second"));
    Engine::layout(logical, styleSheet, text);
    EXPECT_EQ(logical.children()[0]->rect().left(), 12.f);
    EXPECT_EQ(logical.children()[1]->rect().left(), 0.f);

    Engine::layout(flex, styleSheet, text, Direction::RightToLeft);
    EXPECT_EQ(flex.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(flex.children()[1]->rect().left(), 12.f);

    auto column = makeElementValue<HTMLPanelElement>();
    column.setRect({0.f, 0.f, 20.f, 100.f});
    column.classList().add("column");
    column.append(makeElement<HTMLLabelElement>("first"));
    column.append(makeElement<HTMLLabelElement>("second"));
    Engine::layout(column, styleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().bottom(), 0.f);
    EXPECT_EQ(column.children()[1]->rect().bottom(), 12.f);
}

TEST_F(LayoutEngineFixture, AppliesWrapReverseCrossStart) {
    StyleSheet styleSheet;
    constexpr char kWrapLayout[] =
        "panel { display: flex; flex-direction: row; flex-wrap: wrap-reverse; align-content: flex-start; gap: 2px; } label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 22.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    panel.append(makeElement<HTMLLabelElement>("third"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 12.f);
}

TEST_F(LayoutEngineFixture, PreservesNegativeNormalOffsets) {
    StyleSheet styleSheet;
    constexpr char kNegativeOffsetLayout[] = "label { position: relative; width: 10px; height: 10px; left: -8px; bottom: -3px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNegativeOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("offset"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), -8.f);
    EXPECT_EQ(panel.children()[0]->rect().bottom(), 87.f);
}

TEST_F(LayoutEngineFixture, ResolvesPercentageGeometry) {
    StyleSheet styleSheet;
    constexpr char kPercentageGeometryLayout[] = "label { position: relative; width: 50%; height: 25%; left: 10%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kPercentageGeometryLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 200.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("percentage"));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    const Rect& rect = panel.children().front()->rect();
    EXPECT_FLOAT_EQ(rect.w, 100.f);
    EXPECT_FLOAT_EQ(rect.h, 25.f);
    EXPECT_FLOAT_EQ(rect.left(), 20.f);
    EXPECT_FLOAT_EQ(rect.top(), 80.f);
}

TEST_F(LayoutEngineFixture, AppliesFlexGap) {
    StyleSheet styleSheet;
    constexpr char kRowGapLayout[] = "panel { display: flex; flex-direction: row; gap: 10px; } "
                                     "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowGapLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    panel.append(makeElement<HTMLLabelElement>("third"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().left(), 20.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 50.f);
    EXPECT_EQ(panel.desiredSize().x, 50.f);

    constexpr char kColumnGapLayout[] = "panel { display: flex; flex-direction: column; gap: 10px; } "
                                        "label { width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnGapLayout).ok());
    panel.setRect({0.f, 0.f, 20.f, 100.f});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().top(), 80.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 50.f);
}

TEST_F(LayoutEngineFixture, MeasuresFixedHeightFloater) {
    StyleSheet styleSheet;
    constexpr char kFloater[] = "floater { size: auto 100px; display: flex; flex-direction: column; } floater > head { height: 30px; } "
                                "floater > body { display: flex; flex-direction: column; gap: 5px; } label { height: 20px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloater).ok());
    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater);
    ASSERT_NE(floater.body(), nullptr);
    floater.body()->append(makeElement<HTMLLabelElement>("first"));
    floater.body()->append(makeElement<HTMLLabelElement>("second"));

    const Vec2 measured = Engine::measure(floater, styleSheet, text);
    EXPECT_EQ(measured.x, 100.f);
    EXPECT_EQ(measured.y, 75.f);
}

TEST_F(LayoutEngineFixture, AppliesCrossAxisAlignment) {
    StyleSheet styleSheet;
    constexpr char kRowAlignment[] =
        "panel { display: flex; flex-direction: row; align-items: start; } label { width: 10px; height: 10px; } "
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

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 15.f);
    EXPECT_EQ(panel.children()[2]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[3]->rect().h, 40.f);

    constexpr char kColumnAlignment[] =
        "panel { display: flex; flex-direction: column; align-items: start; } label { width: 10px; height: 10px; } "
        "label#center { align-self: center; } label#end { align-self: end; } "
        "label#stretch { width: auto; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnAlignment).ok());
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 45.f);
    EXPECT_EQ(panel.children()[2]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[3]->rect().w, 100.f);

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().right(), 100.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineFixture, AppliesSafeCrossAxisAlignment) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia(".safe { display: flex; align-items: safe center; } .unsafe { display: flex; align-items: unsafe center; } label { "
                       "size: 20px "
                       "10px; }")
            .ok());

    auto safe = makeElementValue<HTMLPanelElement>();
    safe.classList().add("safe");
    safe.setRect({0.f, 0.f, 20.f, 10.f});
    safe.append(makeElement<HTMLLabelElement>());
    EXPECT_EQ(computedStyle(styleSheet, safe).alignItems().overflow, Core::Style::OverflowAlignment::Safe);
    Engine::layout(safe, styleSheet, text);

    auto unsafe = makeElementValue<HTMLPanelElement>();
    unsafe.classList().add("unsafe");
    unsafe.setRect({0.f, 0.f, 20.f, 10.f});
    unsafe.append(makeElement<HTMLLabelElement>());
    EXPECT_EQ(computedStyle(styleSheet, unsafe).alignItems().overflow, Core::Style::OverflowAlignment::Unsafe);
    Engine::layout(unsafe, styleSheet, text);

    EXPECT_EQ(safe.children().front()->rect().top(), safe.rect().top());
    EXPECT_GT(unsafe.children().front()->rect().top(), unsafe.rect().top());
}

TEST_F(LayoutEngineFixture, StretchesNormalFlexItems) {
    StyleSheet rowStyles;
    constexpr char kRowNormal[] = "panel { display: flex; flex-direction: row; align-items: normal; } "
                                  "label { width: 10px; height: auto; }";
    ASSERT_TRUE(rowStyles.loadRadia(kRowNormal).ok());
    auto row = makeElementValue<HTMLPanelElement>();
    row.setRect({0.f, 0.f, 100.f, 40.f});
    row.append(makeElement<HTMLLabelElement>());

    Engine::layout(row, rowStyles, text);
    ASSERT_EQ(row.children().size(), 1U);
    EXPECT_EQ(row.children().front()->rect().top(), 40.f);
    EXPECT_EQ(row.children().front()->rect().h, 40.f);

    StyleSheet columnStyles;
    constexpr char kColumnNormal[] = "panel { display: flex; flex-direction: column; align-items: normal; } "
                                     "label { width: auto; height: 10px; }";
    ASSERT_TRUE(columnStyles.loadRadia(kColumnNormal).ok());
    auto column = makeElementValue<HTMLPanelElement>();
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.append(makeElement<HTMLLabelElement>());

    Engine::layout(column, columnStyles, text);
    ASSERT_EQ(column.children().size(), 1U);
    EXPECT_EQ(column.children().front()->rect().left(), 0.f);
    EXPECT_EQ(column.children().front()->rect().w, 100.f);
}

TEST_F(LayoutEngineFixture, AppliesGridJustification) {
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

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().left(), 0.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 80.f);

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);
    EXPECT_EQ(panel.children()[0]->rect().left(), 80.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 40.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);
}

TEST_F(LayoutEngineFixture, SeparatesVisibilityFromDisplay) {
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
    displayNone->classList().add("none");
    panel.append(std::move(displayNone));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 10.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
    EXPECT_EQ(Engine::measure(*panel.children()[2], styleSheet, text).x, 10.f);
    EXPECT_EQ(panel.children()[3]->desiredSize().x, 0.f);

    panel.children()[1]->setVisibility(Visibility::Collapse);
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);

    panel.children()[2]->setVisibility(Visibility::Visible);
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 20.f);
}

TEST_F(LayoutEngineFixture, IgnoresVerticalAlignOutsideInlineFlow) {
    StyleSheet styleSheet;
    constexpr char kVerticalAlignment[] = "panel { size: 40px 100px; display: flex; flex-direction: row; } label { size: 10px; } "
                                          "panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } "
                                          "panel.column { display: flex; flex-direction: column; vertical-align: bottom; } "
                                          "panel.free-bottom { display: block; vertical-align: bottom; }";
    ASSERT_TRUE(styleSheet.loadRadia(kVerticalAlignment).ok());

    auto addLabel = [](Element& container) {
        container.append(makeElement<HTMLLabelElement>("child"));
    };

    auto top = makeElementValue<HTMLPanelElement>();
    top.setRect({0.f, 0.f, 100.f, 40.f});
    addLabel(top);
    Engine::layout(top, styleSheet, text);
    EXPECT_EQ(top.children()[0]->rect().top(), 40.f);

    auto middle = makeElementValue<HTMLPanelElement>();
    middle.setRect({0.f, 0.f, 100.f, 40.f});
    middle.classList().add("middle");
    addLabel(middle);
    Engine::layout(middle, styleSheet, text);
    EXPECT_EQ(middle.children()[0]->rect().top(), 40.f);

    auto bottom = makeElementValue<HTMLPanelElement>();
    bottom.setRect({0.f, 0.f, 100.f, 40.f});
    bottom.classList().add("bottom");
    addLabel(bottom);
    Engine::layout(bottom, styleSheet, text);
    EXPECT_EQ(bottom.children()[0]->rect().bottom(), 30.f);

    auto column = makeElementValue<HTMLPanelElement>();
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.classList().add("column");
    addLabel(column);
    addLabel(column);
    Engine::layout(column, styleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(column.children()[1]->rect().bottom(), 20.f);

    auto freeBottom = makeElementValue<HTMLPanelElement>();
    freeBottom.setRect({0.f, 0.f, 100.f, 40.f});
    freeBottom.classList().add("free-bottom");
    addLabel(freeBottom);
    Engine::layout(freeBottom, styleSheet, text);
    EXPECT_EQ(freeBottom.children()[0]->rect().bottom(), 30.f);
}

TEST_F(LayoutEngineFixture, WrapsFlexItemsByAvailableMainSize) {
    StyleSheet styleSheet;
    constexpr char kWrapLayout[] = "panel { display: flex; flex-direction: row; flex-wrap: wrap; align-content: start; gap: 2px; } "
                                   "panel.end { align-content: end; } "
                                   "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    auto first = makeElement<HTMLLabelElement>("first");
    auto second = makeElement<HTMLLabelElement>("second");
    auto third = makeElement<HTMLLabelElement>("third");
    NodeAccess::setFlowBreakBefore(*second, true);
    panel.append(std::move(first));
    panel.append(std::move(second));
    panel.append(std::move(third));

    Engine::measure(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 34.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);

    panel.setRect({0.f, 0.f, 22.f, 40.f});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 12.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 28.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 0.f);

    panel.classList().add("end");
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 22.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 22.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 10.f);
}

TEST_F(LayoutEngineFixture, CarriesFlowBreakPastHiddenInlineChildren) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: block; width: 100px; height: 30px; } "
                       "span { display: inline; font-size: 10px; line-height: 10px; } "
                       ".hidden { display: none; }")
            .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 30.f});
    panel.innerHTML("<span>before</span><br><span class=hidden></span><span>after</span>");
    ASSERT_EQ(panel.children().size(), 4U);

    Engine::layout(panel, styleSheet, text);

    EXPECT_LT(panel.children()[3]->rect().top(), panel.children()[0]->rect().top());
}

TEST_F(LayoutEngineFixture, DoesNotTurnFlowBreaksIntoFlexLines) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel { display: flex; flex-wrap: wrap; align-content: start; width: 100px; height: 30px; } "
                                     "span { display: block; width: 10px; height: 10px; } .hidden { display: none; }")
                    .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 30.f});
    panel.innerHTML("<span>before</span><br><span class=hidden></span><span>after</span>");
    ASSERT_EQ(panel.children().size(), 4U);

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[3]->rect().top(), panel.children()[0]->rect().top());
}

TEST_F(LayoutEngineFixture, WrapsColumnFlexItems) {
    StyleSheet styleSheet;
    constexpr char kWrapLayout[] = "panel { display: flex; flex-direction: column; flex-wrap: wrap; align-content: start; gap: 2px; } "
                                   "label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 40.f, 22.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));
    panel.append(makeElement<HTMLLabelElement>("third"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().top(), 22.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
    EXPECT_EQ(panel.children()[2]->rect().top(), 22.f);
    EXPECT_EQ(panel.children()[2]->rect().left(), 12.f);
}

TEST_F(LayoutEngineFixture, BalancesFlexWrapLinesWithoutChangingDirection) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel { display: flex; flex-direction: row; flex-wrap: balance; align-content: start; } label { width: 20px; "
                       "height: 10px; }")
            .ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 60.f, 40.f});
    for (int index = 0; index < 4; ++index)
        panel.append(makeElement<HTMLLabelElement>());

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().top(), panel.children()[1]->rect().top());
    EXPECT_EQ(panel.children()[2]->rect().top(), panel.children()[3]->rect().top());
    EXPECT_NE(panel.children()[0]->rect().top(), panel.children()[2]->rect().top());
}

TEST_F(LayoutEngineFixture, AppliesAlignContentToSingleWrappedLine) {
    StyleSheet styleSheet;
    constexpr char kRowLayout[] =
        "panel { display: flex; flex-direction: row; flex-wrap: wrap; align-content: center; } label { size: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowLayout).ok());

    auto row = makeElementValue<HTMLPanelElement>();
    row.setRect({0.f, 0.f, 100.f, 40.f});
    row.append(makeElement<HTMLLabelElement>());
    Engine::layout(row, styleSheet, text);
    EXPECT_EQ(row.children()[0]->rect().top(), 25.f);

    StyleSheet columnStyleSheet;
    constexpr char kColumnLayout[] =
        "panel { display: flex; flex-direction: column; flex-wrap: wrap; align-content: center; } label { size: 10px; }";
    ASSERT_TRUE(columnStyleSheet.loadRadia(kColumnLayout).ok());

    auto column = makeElementValue<HTMLPanelElement>();
    column.setRect({0.f, 0.f, 40.f, 100.f});
    column.append(makeElement<HTMLLabelElement>());
    Engine::layout(column, columnStyleSheet, text);
    EXPECT_EQ(column.children()[0]->rect().left(), 15.f);
}

TEST_F(LayoutEngineFixture, PreservesSignedFreeSpaceForOverflowAlignment) {
    StyleSheet styleSheet;
    constexpr char kMainAxisLayout[] =
        "panel { display: flex; flex-direction: row; justify-content: center; } label { width: 40px; height: 10px; flex-shrink: 0; }";
    ASSERT_TRUE(styleSheet.loadRadia(kMainAxisLayout).ok());

    auto centered = makeElementValue<HTMLPanelElement>();
    centered.setRect({0.f, 0.f, 20.f, 20.f});
    centered.append(makeElement<HTMLLabelElement>());
    Engine::layout(centered, styleSheet, text);
    EXPECT_EQ(centered.children()[0]->rect().left(), -10.f);

    StyleSheet crossAxisStyleSheet;
    constexpr char kCrossAxisLayout[] =
        "panel { display: flex; flex-direction: row; flex-wrap: wrap; align-content: center; } label { width: 10px; height: 10px; }";
    ASSERT_TRUE(crossAxisStyleSheet.loadRadia(kCrossAxisLayout).ok());

    auto wrapped = makeElementValue<HTMLPanelElement>();
    wrapped.setRect({0.f, 0.f, 10.f, 10.f});
    wrapped.append(makeElement<HTMLLabelElement>());
    wrapped.append(makeElement<HTMLLabelElement>());
    Engine::layout(wrapped, crossAxisStyleSheet, text);
    EXPECT_EQ(wrapped.children()[0]->rect().top(), 15.f);
    EXPECT_EQ(wrapped.children()[1]->rect().top(), 5.f);
}

TEST_F(LayoutEngineFixture, FreezesMaxSizeBeforeFlexShrink) {
    StyleSheet styleSheet;
    constexpr char kFlexLayout[] =
        "panel { display: flex; flex-direction: row; } label { flex: 0 1 100px; height: 10px; } label.capped { max-width: 60px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 120.f, 20.f});
    auto capped = makeElement<HTMLLabelElement>();
    capped->classList().add("capped");
    panel.append(std::move(capped));
    panel.append(makeElement<HTMLLabelElement>());

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->rect().w, 60.f);
    EXPECT_EQ(panel.children()[1]->rect().w, 60.f);
    EXPECT_EQ(panel.children()[1]->rect().left(), 60.f);
}

TEST_F(LayoutEngineFixture, DistributesFlexContentAndCapsGrowth) {
    StyleSheet styleSheet;
    constexpr char kFlexLayout[] =
        "panel { display: flex; flex-direction: row; justify-content: space-evenly; } panel.growth { justify-content: start; } "
        "label { width: 10px; height: 10px; } label.growing { flex: 1; max-width: 30px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexLayout).ok());

    auto distributed = makeElementValue<HTMLPanelElement>();
    distributed.setRect({0.f, 0.f, 40.f, 20.f});
    distributed.append(makeElement<HTMLLabelElement>());
    distributed.append(makeElement<HTMLLabelElement>());
    distributed.append(makeElement<HTMLLabelElement>());
    Engine::layout(distributed, styleSheet, text);
    EXPECT_FLOAT_EQ(distributed.children()[0]->rect().left(), 2.5f);
    EXPECT_FLOAT_EQ(distributed.children()[1]->rect().left(), 15.f);
    EXPECT_FLOAT_EQ(distributed.children()[2]->rect().left(), 27.5f);

    auto capped = makeElementValue<HTMLPanelElement>();
    capped.setRect({0.f, 0.f, 100.f, 20.f});
    capped.classList().add("growth");
    for (int index = 0; index != 2; ++index) {
        auto child = makeElement<HTMLLabelElement>();
        child->classList().add("growing");
        capped.append(std::move(child));
    }
    Engine::layout(capped, styleSheet, text);
    EXPECT_EQ(capped.children()[0]->rect().w, 30.f);
    EXPECT_EQ(capped.children()[1]->rect().w, 30.f);
    EXPECT_EQ(capped.children()[1]->rect().left(), 30.f);
}

TEST_F(LayoutEngineFixture, AppliesFlexShrink) {
    StyleSheet styleSheet;
    constexpr char kFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } label { height: 10px; flex: 0 1 80px; } "
                                        "label.second { flex-basis: 40px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFlexBasisLayout).ok());

    auto intrinsic = makeElementValue<HTMLPanelElement>();
    intrinsic.append(makeElement<HTMLLabelElement>());
    auto intrinsicSecond = makeElement<HTMLLabelElement>();
    intrinsicSecond->classList().add("second");
    intrinsic.append(std::move(intrinsicSecond));
    Engine::measure(intrinsic, styleSheet, text);
    EXPECT_EQ(intrinsic.desiredSize().x, 120.f);

    StyleSheet percentageTheme;
    constexpr char kPercentageFlexBasisLayout[] = "panel { display: flex; flex-direction: row; } "
                                                  "label { width: 30px; flex-basis: 50%; }";
    ASSERT_TRUE(percentageTheme.loadRadia(kPercentageFlexBasisLayout).ok());
    auto indefinite = makeElementValue<HTMLPanelElement>();
    indefinite.append(makeElement<HTMLLabelElement>());
    Engine::measure(indefinite, percentageTheme, text);
    EXPECT_EQ(indefinite.desiredSize().x, 30.f);

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.append(makeElement<HTMLLabelElement>());
    auto second = makeElement<HTMLLabelElement>();
    second->classList().add("second");
    panel.append(std::move(second));
    Engine::layout(panel, styleSheet, text);
    EXPECT_NEAR(panel.children()[0]->rect().w, 200.f / 3.f, 0.001f);
    EXPECT_NEAR(panel.children()[1]->rect().w, 100.f / 3.f, 0.001f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().right(), 100.f);
}

TEST_F(LayoutEngineFixture, CentersFloaterHeadChildren) {
    StyleSheet styleSheet;
    constexpr char kFloaterHead[] = "floater > head { height: 48px; display: flex; flex-direction: row; padding: 12px; } "
                                    "floater > head > title { height: 24px; display: flex; flex-direction: row; align-items: center; "
                                    "flex-grow: 1; line-height: 18px; } "
                                    "floater > head > title > i { size: 28px; } "
                                    "floater > head > close { size: 24px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterHead).ok());

    auto floater = makeElementValue<HTMLFloaterElement>();
    CoreTests::appendFloaterStructure(floater, true);
    Element* head = floater.head();
    ASSERT_NE(head, nullptr);
    ASSERT_FALSE(head->children().empty());
    auto icon = makeElement<Element>("i");
    icon->classList().add("i-search");
    head->children().front()->append(std::move(icon));
    head->setRect({0.f, 0.f, 200.f, 48.f});
    Engine::layout(*head, styleSheet, text);

    const float headCenter = head->rect().y + head->rect().h * .5f;
    for (const auto& child : head->children()) {
        if (!child->isVisible(computedStyle(styleSheet, *child)))
            continue;
        const float childCenter = child->rect().y + child->rect().h * .5f;
        SCOPED_TRACE(Message() << "head child: " << child->elementName());
        EXPECT_FLOAT_EQ(childCenter, headCenter);
    }
}

TEST_F(LayoutEngineFixture, ReflowsWrappedTextInColumnLayout) {
    StyleSheet styleSheet;
    constexpr char kWrapping[] = "panel { display: flex; flex-direction: column; } "
                                 "p { font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrapping).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 30.f, 100.f});
    panel.append(makeParagraph("alpha beta"));
    panel.append(makeParagraph("after"));

    Engine::layout(panel, styleSheet, text);
    const Element& wrapped = *panel.children()[0];
    const Element& following = *panel.children()[1];
    EXPECT_EQ(wrapped.rect().h, 20.f);
    EXPECT_EQ(following.rect().top(), wrapped.rect().bottom());
}

TEST_F(LayoutEngineFixture, RemeasuresAfterFlexShrink) {
    StyleSheet styleSheet;
    constexpr char kRowWrapping[] = "panel { width: 45px; display: flex; flex-direction: row; align-items: start; } "
                                    "p { min-width: 0px; flex: 1; font-size: 10px; line-height: 10px; text-wrap: wrap; } "
                                    "label { width: 10px; flex-shrink: 0; align-self: stretch; }";
    ASSERT_TRUE(styleSheet.loadRadia(kRowWrapping).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeParagraph("alpha beta"));
    panel.append(makeElement<HTMLLabelElement>("x"));

    const Vec2 measured = Engine::measure(panel, styleSheet, text);
    EXPECT_EQ(measured.y, 20.f);

    panel.setRect({0.f, 0.f, measured.x, measured.y});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 20.f);
    EXPECT_EQ(panel.children()[1]->rect().w, 10.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 20.f);
}

TEST_F(LayoutEngineFixture, ReappliesFlexBasisAfterTextReflow) {
    StyleSheet columnTheme;
    constexpr char kColumnBasis[] = "panel { width: 30px; display: flex; flex-direction: column; } "
                                    "p { flex-basis: 40px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(columnTheme.loadRadia(kColumnBasis).ok());
    auto column = makeElementValue<HTMLPanelElement>();
    column.append(makeParagraph("alpha beta"));
    EXPECT_EQ(Engine::measure(column, columnTheme, text).y, 40.f);

    StyleSheet rowTheme;
    constexpr char kRowMinimum[] = "panel { width: 80px; display: flex; flex-direction: row; } p { flex: 0 1 100px; font-size: 10px; "
                                   "line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; }";
    ASSERT_TRUE(rowTheme.loadRadia(kRowMinimum).ok());
    auto row = makeElementValue<HTMLPanelElement>();
    row.append(makeParagraph("alpha beta"));
    row.append(makeElement<HTMLLabelElement>("x"));
    row.setRect({0.f, 0.f, 80.f, 20.f});
    Engine::layout(row, rowTheme, text);
    EXPECT_EQ(row.children()[0]->rect().w, 70.f);
}
TEST_F(LayoutEngineFixture, RemeasuresAfterMetricsChange) {
    class GenerationTextMeasurer final : public TextMeasurer {
    public:
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return mSize; }
        std::uint64_t generation() const noexcept override { return mGeneration; }
        void advance() {
            ++mGeneration;
            mSize = {64.f, 18.f};
        }

    private:
        std::uint64_t mGeneration = 1;
        Vec2 mSize {32.f, 12.f};
    } metrics;

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("generation");
    Engine::measure(label, styleSheet, metrics);
    EXPECT_FLOAT_EQ(label.desiredSize().x, 32.f);
    EXPECT_FLOAT_EQ(label.desiredSize().y, 12.f);

    metrics.advance();
    Engine::measure(label, styleSheet, metrics);
    EXPECT_FLOAT_EQ(label.desiredSize().x, 64.f);
    EXPECT_FLOAT_EQ(label.desiredSize().y, 18.f);
}

TEST_F(LayoutEngineFixture, ReallocatesFlexChildren) {
    StyleSheet styleSheet;
    constexpr char kColumnFlexLayout[] = "panel { display: flex; flex-direction: column; } "
                                         "label { height: 10px; flex-grow: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnFlexLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_EQ(panel.children()[1]->rect().bottom(), 0.f);
}

TEST_F(LayoutEngineFixture, ResolvesSiblingColumnPercentages) {
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

    Engine::layout(panel, styleSheet, text);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_FLOAT_EQ(panel.children()[1]->rect().h, 50.f);
    EXPECT_FLOAT_EQ(panel.children()[0]->rect().bottom(), panel.children()[1]->rect().top());
}

TEST_F(LayoutEngineFixture, ResolvesNestedColumnPercentages) {
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

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    const Element& nestedPanel = *panel.children().front();
    ASSERT_EQ(nestedPanel.children().size(), 1U);
    EXPECT_FLOAT_EQ(nestedPanel.rect().h, 50.f);
    EXPECT_FLOAT_EQ(nestedPanel.children().front()->rect().h, 25.f);
}

TEST_F(LayoutEngineFixture, PreservesColumnMinimums) {
    StyleSheet styleSheet;
    constexpr char kColumnMinimumLayout[] = "panel { display: flex; flex-direction: column; } "
                                            "label { height: 30px; min-height: 25px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kColumnMinimumLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.append(makeElement<HTMLLabelElement>("first"));
    panel.append(makeElement<HTMLLabelElement>("second"));

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[0]->rect().h, 25.f);
    EXPECT_EQ(panel.children()[1]->rect().h, 25.f);
}

TEST_F(LayoutEngineFixture, UsesStylesheetSpecificLayout) {
    StyleSheet narrow;
    StyleSheet wide;
    constexpr char kNarrowLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kWideLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(narrow.loadRadia(kNarrowLabelLayout).ok());
    ASSERT_TRUE(wide.loadRadia(kWideLabelLayout).ok());

    auto label = makeElementValue<HTMLLabelElement>("identity");
    Engine::layout(label, narrow, text);
    Engine::layout(label, wide, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineFixture, MatchesCopiedStylesheetSnapshot) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("label { width: 10px; height: 10px; }").ok());
    StyleSheet copy = styleSheet;
    Pass pass(styleSheet, text);

    EXPECT_TRUE(pass.matches(copy, text));
}

TEST_F(LayoutEngineFixture, UsesMetricsSpecificLayout) {
    class WidthMetrics final : public TextMeasurer {
    public:
        explicit WidthMetrics(float width)
            : mWidth(width) {}
        Vec2 measureText(const std::string&, const ComputedStyle&) const override { return {mWidth, 12.f}; }
        std::uint64_t generation() const noexcept override { return 1; }

    private:
        float mWidth;
    } narrow(12.f), wide(48.f);

    StyleSheet styleSheet;
    auto label = makeElementValue<HTMLLabelElement>("identity");
    Engine::layout(label, styleSheet, narrow);
    Engine::layout(label, styleSheet, wide);
    EXPECT_EQ(label.desiredSize().x, 48.f);
}

TEST_F(LayoutEngineFixture, OrdersChildrenBeforeRowLayout) {
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

    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children()[1]->rect().x, 0.f);
    EXPECT_EQ(panel.children()[0]->rect().x, 10.f);
}

TEST_F(LayoutEngineFixture, PreservesSourceOrderOutsideFlexAndGrid) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; } "
                                     "#late { display: block; order: 2; width: 10px; height: 10px; } "
                                     "#early { display: block; order: -1; width: 10px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());

    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    auto late = makeElement<HTMLLabelElement>();
    late->setId("late");
    auto early = makeElement<HTMLLabelElement>();
    early->setId("early");
    panel.append(std::move(late));
    panel.append(std::move(early));

    Engine::layout(panel, styleSheet, text);

    EXPECT_EQ(panel.children()[0]->id(), "late");
    EXPECT_EQ(panel.children()[1]->id(), "early");
    EXPECT_EQ(panel.children()[0]->rect().top(), 40.f);
    EXPECT_EQ(panel.children()[1]->rect().top(), 30.f);
}

TEST_F(LayoutEngineFixture, RemeasuresAfterStylesheetChange) {
    StyleSheet styleSheet;
    constexpr char kInitialLabelLayout[] = "label { width: 10px; height: 10px; }";
    constexpr char kReplacementLabelLayout[] = "label { width: 30px; height: 10px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kInitialLabelLayout).ok());
    auto label = makeElementValue<HTMLLabelElement>("assigned");
    Engine::layout(label, styleSheet, text);

    StyleSheet replacement;
    ASSERT_TRUE(replacement.loadRadia(kReplacementLabelLayout).ok());
    styleSheet = replacement;
    Engine::layout(label, styleSheet, text);
    EXPECT_EQ(label.desiredSize().x, 30.f);
}

TEST_F(LayoutEngineFixture, RemeasuresPercentageText) {
    StyleSheet styleSheet;
    constexpr char kNormalTextLayout[] = "panel { display: block; } "
                                         "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalTextLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeParagraph("alpha beta"));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);

    panel.setRect({0.f, 0.f, 200.f, 100.f});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.children().front()->rect().h, 10.f);
}

TEST_F(LayoutEngineFixture, PreservesIntrinsicSizeWithOffsets) {
    StyleSheet styleSheet;
    constexpr char kNormalOffsetLayout[] = "panel { display: block; } "
                                           "label { position: relative; width: 20px; height: 10px; right: 5px; bottom: 7px; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeElement<HTMLLabelElement>("positioned"));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);
}

TEST_F(LayoutEngineFixture, IncludesExplicitGeometry) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({0.f, 0.f, 40.f, 30.f});
    panel.append(std::move(child));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 40.f);
    EXPECT_EQ(panel.desiredSize().y, 30.f);
}

TEST_F(LayoutEngineFixture, InvalidatesIntrinsicSize) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({0.f, 0.f, 20.f, 10.f});
    Element* childPtr = child.get();
    panel.append(std::move(child));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 20.f);

    childPtr->setRect({0.f, 0.f, 60.f, 10.f});
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 60.f);
}

TEST_F(LayoutEngineFixture, IncludesExplicitPosition) {
    StyleSheet styleSheet;
    constexpr char kNormalLayout[] = "panel { display: block; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    auto child = makeElement<HTMLPanelElement>();
    child->setRect({10.f, 12.f, 20.f, 8.f});
    panel.append(std::move(child));
    Engine::layout(panel, styleSheet, text);
    EXPECT_EQ(panel.desiredSize().x, 30.f);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
}

TEST_F(LayoutEngineFixture, IncludesWrappedChildHeight) {
    StyleSheet styleSheet;
    constexpr char kWrappedTextLayout[] = "panel { display: block; width: 100px; } "
                                          "p { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ASSERT_TRUE(styleSheet.loadRadia(kWrappedTextLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.append(makeParagraph("alpha beta"));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.desiredSize().y, 20.f);
    EXPECT_EQ(panel.children().front()->rect().h, 20.f);
}

TEST_F(LayoutEngineFixture, IgnoresRelativeOffsets) {
    StyleSheet styleSheet;
    constexpr char kNormalPercentageOffsetLayout[] = "panel { display: block; } "
                                                     "label { position: relative; width: 20px; height: 10px; left: 50%; top: 20%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalPercentageOffsetLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.append(makeElement<HTMLLabelElement>("positioned"));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_EQ(panel.desiredSize().x, 20.f);
    EXPECT_EQ(panel.desiredSize().y, 10.f);
    EXPECT_EQ(panel.children().front()->rect().left(), 50.f);
    EXPECT_EQ(panel.children().front()->rect().top(), 80.f);
}

TEST_F(LayoutEngineFixture, ResolvesPercentageDimensions) {
    StyleSheet styleSheet;
    constexpr char kNormalPercentageLayout[] = "panel { display: block; width: 50%; height: 50%; } "
                                               "label { width: 50%; height: 50%; }";
    ASSERT_TRUE(styleSheet.loadRadia(kNormalPercentageLayout).ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setRect({0.f, 0.f, 100.f, 80.f});
    panel.append(makeElement<HTMLLabelElement>("sized"));

    Engine::layout(panel, styleSheet, text);
    ASSERT_EQ(panel.children().size(), 1U);
    EXPECT_FLOAT_EQ(panel.children().front()->rect().w, 50.f);
    EXPECT_FLOAT_EQ(panel.children().front()->rect().h, 40.f);
}

TEST_F(LayoutEngineFixture, ResolvesNestedPercentageFlexBasis) {
    StyleSheet styleSheet;
    constexpr char kNestedFlexBasis[] =
        "#outer { display: flex; flex-direction: row; width: 100px; height: 20px; } "
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

    Engine::layout(outer, styleSheet, text);
    EXPECT_FLOAT_EQ(innerPtr->rect().w, 50.f);
    EXPECT_FLOAT_EQ(label->rect().w, 25.f);
}

TEST_F(LayoutEngineFixture, ClampsProgrammaticScrollPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

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

TEST_F(LayoutEngineFixture, ReflowsOppositeAxis) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 100.f, 120.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 120.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 15.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 35.f);
}

TEST_F(LayoutEngineFixture, PreservesRangeForHiddenOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: hidden; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

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

TEST_F(LayoutEngineFixture, PreservesClientSizeWithOverlay) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Overlay;

    Engine::layout(panel, styleSheet, text, Direction::LeftToRight, options);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineFixture, ReservesSpaceForForcedScrollbars) {
    StyleSheet styleSheet;
    ASSERT_TRUE(
        styleSheet.loadRadia("panel#viewport { display: block; overflow: scroll; } #content { display: block; width: 40px; height: 40px; }")
            .ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 0.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineFixture, NoRangeForVisibleOverflow) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: visible; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

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

TEST_F(LayoutEngineFixture, NoRangeForClip) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: clip; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Pass styles(styleSheet, text);
    const ComputedStyle& style = styles.style(panel);
    EXPECT_EQ(style.overflowX(), Core::Style::Overflow::Clip);
    EXPECT_EQ(style.overflowY(), Core::Style::Overflow::Clip);
    Engine::layout(panel, styles);

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

TEST_F(LayoutEngineFixture, ClampsStaleScrollPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: hidden; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    Element* contentPtr = content.get();
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);
    panel.scrollTo(500.f, 500.f);
    contentPtr->setRect({0.f, 0.f, 40.f, 40.f});
    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.scrollWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 0.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 0.f);
    EXPECT_FLOAT_EQ(panel.scrollLeft(), 0.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineFixture, NormalizesRtlScrollPosition) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 40.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);
    panel.scrollTo(20.f, 0.f);

    EXPECT_FLOAT_EQ(panel.scrollLeft(), 20.f);
    EXPECT_FLOAT_EQ(panel.scrollTop(), 0.f);
}

TEST_F(LayoutEngineFixture, UsesStableScrollbarGutterWithRuntimeMode) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet
            .loadRadia("panel#viewport { display: block; overflow: auto; "
                       "scrollbar-gutter: stable both-edges; }")
            .ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Classic;

    Engine::layout(panel, styleSheet, text, Direction::LeftToRight, options);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 110.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 155.f);
}

TEST_F(LayoutEngineFixture, PreservesRangeWithNoScrollbar) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: auto; scrollbar-width: none; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 180.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 240.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 80.f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 140.f);
}

TEST_F(LayoutEngineFixture, UsesThinScrollbarWidth) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel#viewport { display: block; overflow: scroll; scrollbar-width: thin; }").ok());
    auto panel = makeElementValue<HTMLPanelElement>();
    panel.setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<Element>("content");
    content->setRect({0.f, 0.f, 180.f, 240.f});
    panel.append(std::move(content));

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 92.5f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 92.5f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollLeft, 87.5f);
    EXPECT_FLOAT_EQ(panel.scrollMetrics().maxScrollTop, 147.5f);
}

TEST_F(LayoutEngineFixture, ReservesStableScrollbarSpace) {
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

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 85.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 85.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 100.f);
}

TEST_F(LayoutEngineFixture, MirrorsInlineGutter) {
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

    Engine::layout(panel, styleSheet, text);

    EXPECT_FLOAT_EQ(panel.clientWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.clientHeight(), 100.f);
    EXPECT_FLOAT_EQ(panel.scrollWidth(), 70.f);
    EXPECT_FLOAT_EQ(panel.scrollHeight(), 100.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().left(), 15.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().right(), 85.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().bottom(), 0.f);
    EXPECT_FLOAT_EQ(contentPtr->rect().top(), 100.f);
}

TEST_F(LayoutEngineFixture, ReservesRtlInlineEdges) {
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

    Engine::layout(panel, styleSheet, text, Direction::RightToLeft);

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
