/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <type_traits>
#include "elements/button.h"
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/host.h"
#include "text/metrics.h"

namespace {
using radia::ui::ButtonElement;
using radia::ui::ConstElementList;
using radia::ui::Dimension;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::FixedTextMetrics;
using radia::ui::fixedTextMetrics;
using radia::ui::IconElement;
using radia::ui::InputElement;
using radia::ui::insetRect;
using radia::ui::IntrinsicSizeConstraints;
using radia::ui::LabelElement;
using radia::ui::LayoutDirection;
using radia::ui::Length;
using radia::ui::Node;
using radia::ui::NodePtr;
using radia::ui::NodeType;
using radia::ui::Overflow;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PaintContext;
using radia::ui::PanelElement;
using radia::ui::PointerButton;
using radia::ui::RecordingPaintContext;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::Text;
using radia::ui::TextAlign;
using radia::ui::TextLayout;
using radia::ui::TextMetrics;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::Vec2;
using radia::ui::Visibility;
} // namespace

namespace {
class TextLayoutTestElement final : public Element {
public:
    explicit TextLayoutTestElement(std::string text = {}) : Element("p"), mLayout(std::move(text)) {}

    const std::string& text() const { return mLayout.plainText(); }

    Vec2 intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                       const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const override {
        return mLayout.measure(textMetrics, style, styleSheet, *this, constraints.width);
    }

    void paint(PaintContext& context, const Style& style, float scale) const override {
        context.paintBox(rect(), style);
        mLayout.paint(context, insetRect(rect(), style.padding), style, styleSheet(), *this);
    }

private:
    TextLayout mLayout;
};

} // namespace

namespace {
std::string paintedText(const RecordingPaintContext& recording) {
    std::string text;
    for (const PaintCommand& command : recording.commands())
        if (command.kind == PaintCommandKind::Text) text += command.textOrIconName;
    return text;
}
} // namespace

TEST(ElementTest, ElementReferencesExpireAfterUnmount) {
    PanelElement root;
    auto button = std::make_unique<ButtonElement>();
    ElementRef<ButtonElement> reference(button.get());
    root.append(std::move(button));
    ASSERT_TRUE(reference);
    root.replaceChildren();
    EXPECT_FALSE(reference);
}

TEST(ElementTest, DisabledButtonsDoNotActivate) {
    ButtonElement button;
    int activations = 0;
    button.setOnActivate([&](Element&) { ++activations; });
    button.activate();
    button.disabled(true).activate();
    EXPECT_EQ(activations, 1);
    EXPECT_TRUE(button.focusable());
}

TEST(ElementTest, ChildInsertionMaintainsOrderAndOwnership) {
    PanelElement root;
    auto first = std::make_unique<ButtonElement>();
    first->setId("first");
    root.append(std::move(first));
    auto leading = std::make_unique<ButtonElement>();
    leading->setId("leading");
    root.prepend(std::move(leading));
    EXPECT_EQ(root.children().front()->id(), "leading");
    EXPECT_EQ(root.children().front()->parentElement(), &root);
    root.replaceChildren();
    EXPECT_TRUE(root.children().empty());
}

TEST(ElementTest, UsesModernChildMutationMethods) {
    PanelElement root;
    Element* first = root.append(std::make_unique<LabelElement>("first"))->asElement();
    Element* last = root.append(std::make_unique<LabelElement>("last"))->asElement();

    Node* before = first->before(std::make_unique<LabelElement>("before"));
    Node* after = last->after(std::make_unique<LabelElement>("after"));
    ASSERT_EQ(root.children().size(), 4U);
    EXPECT_EQ(root.children()[0], before->asElement());
    EXPECT_EQ(root.children()[3], after->asElement());

    NodePtr removed = before->remove();
    ASSERT_EQ(removed.get(), before);
    EXPECT_EQ(before->parentNode(), nullptr);

    auto replacementOwner = std::make_unique<LabelElement>("replacement");
    Element* replacement = replacementOwner.get();
    NodePtr replaced = last->replaceWith(std::move(replacementOwner));
    ASSERT_EQ(replaced.get(), last);
    EXPECT_EQ(last->parentNode(), nullptr);
    ASSERT_EQ(root.children().size(), 3U);
    EXPECT_EQ(root.children()[1], replacement);

    Node* text = root.append(std::make_unique<Text>("text"));
    ASSERT_EQ(text->nodeType(), NodeType::Text);
    EXPECT_EQ(text->parentElement(), &root);
    NodePtr detachedText = text->remove();
    EXPECT_EQ(detachedText.get(), text);
}

TEST(ElementTest, ExposesDomShapedNodeTraversal) {
    PanelElement root;
    root.textContent("before");
    auto childOwner = std::make_unique<LabelElement>("child");
    Element* child = root.append(std::move(childOwner))->asElement();

    ASSERT_EQ(root.nodeType(), NodeType::Element);
    ASSERT_EQ(root.childNodes().size(), 2U);
    ASSERT_EQ(root.children().size(), 1U);

    Node* text = root.firstChild();
    ASSERT_NE(text, nullptr);
    ASSERT_EQ(text->nodeType(), NodeType::Text);
    ASSERT_NE(text->asText(), nullptr);
    EXPECT_EQ(text->asText()->getData(), "before");
    EXPECT_EQ(text->parentNode(), &root);
    EXPECT_EQ(text->parentElement(), &root);
    EXPECT_EQ(text->nextSibling(), child);

    EXPECT_EQ(root.children().front(), child);
    EXPECT_EQ(child->nodeType(), NodeType::Element);
    EXPECT_EQ(child->parentNode(), &root);
    EXPECT_EQ(child->nextSibling(), nullptr);
}

TEST(ElementTest, ConstChildrenExposeConstBorrowedElements) {
    PanelElement root;
    auto childOwner = std::make_unique<LabelElement>("child");
    const Element* expected = childOwner.get();
    root.append(std::move(childOwner));

    const Element& constRoot = root;
    static_assert(std::is_same_v<decltype(constRoot.children()), ConstElementList>);
    const ConstElementList children = constRoot.children();

    ASSERT_EQ(children.size(), 1U);
    EXPECT_EQ(children.front(), expected);
}

TEST(DocumentTest, OwnsDocumentElementAndTransfersDetachedChildren) {
    auto documentElementOwner = std::make_unique<PanelElement>();
    documentElementOwner->setId("root");
    Element* documentElement = documentElementOwner.get();
    Document document(std::move(documentElementOwner));

    EXPECT_EQ(document.nodeType(), NodeType::Document);
    EXPECT_EQ(document.parentNode(), nullptr);
    EXPECT_EQ(document.firstChild(), documentElement);
    ASSERT_EQ(document.childNodes().size(), 1U);
    EXPECT_EQ(document.childNodes().front(), documentElement);
    ASSERT_EQ(document.documentElement(), documentElement);
    EXPECT_EQ(documentElement->nodeType(), NodeType::Element);
    EXPECT_EQ(documentElement->parentNode(), static_cast<radia::ui::Node*>(&document));
    EXPECT_EQ(documentElement->parentElement(), nullptr);
    ASSERT_EQ(document.getElementById("root"), documentElement);
    auto buttonOwner = document.createElement("button");
    buttonOwner->setId("button");
    Element* button = documentElement->append(std::move(buttonOwner))->asElement();

    EXPECT_EQ(document.getElementById("button"), button);
    EXPECT_EQ(button->parentElement(), documentElement);

    NodePtr detached = button->remove();
    EXPECT_EQ(detached.get(), button);
    EXPECT_EQ(button->parentElement(), nullptr);
    EXPECT_EQ(document.getElementById("button"), nullptr);

    EXPECT_EQ(documentElement->append(std::move(detached)), button);
    EXPECT_EQ(document.getElementById("button"), button);
}

TEST(DocumentTest, ReparentsDetachedElementsWithinDocument) {
    auto documentElementOwner = std::make_unique<PanelElement>();
    Document document(std::move(documentElementOwner));
    Element* documentElement = document.documentElement();

    auto firstPanelOwner = document.createElement("panel");
    Element* firstPanel = documentElement->append(std::move(firstPanelOwner))->asElement();
    auto secondPanelOwner = document.createElement("panel");
    Element* secondPanel = documentElement->append(std::move(secondPanelOwner))->asElement();

    auto buttonOwner = document.createElement("button");
    Element* button = firstPanel->append(std::move(buttonOwner))->asElement();
    NodePtr detached = button->remove();

    ASSERT_EQ(detached.get(), button);
    EXPECT_EQ(button->parentElement(), nullptr);
    EXPECT_EQ(button->elementName(), "button");

    EXPECT_EQ(secondPanel->append(std::move(detached)), button);
    EXPECT_EQ(button->parentElement(), secondPanel);
    EXPECT_EQ(secondPanel->children().front(), button);
}

TEST(ElementTreeDeathTest, RejectsNullChild) {
    PanelElement root;
    EXPECT_DEATH(root.append(NodePtr()), ".*");
    EXPECT_TRUE(root.children().empty());
}

TEST(ElementTreeDeathTest, RejectsCrossDocumentChild) {
    auto firstRootOwner = std::make_unique<PanelElement>();
    Document first(std::move(firstRootOwner));
    auto secondRootOwner = std::make_unique<PanelElement>();
    Document second(std::move(secondRootOwner));
    auto childOwner = first.createElement("button");

    EXPECT_DEATH(second.documentElement()->append(std::move(childOwner)), ".*");
    EXPECT_TRUE(second.documentElement()->children().empty());
}

TEST(ElementTreeDeathTest, RejectsMutationOfDocumentElement) {
    auto documentElementOwner = std::make_unique<PanelElement>();
    Document document(std::move(documentElementOwner));

    EXPECT_DEATH(document.documentElement()->remove(), ".*");
    EXPECT_NE(document.documentElement(), nullptr);
}

TEST(ElementTreeDeathTest, RejectsUnknownRuntimeElement) {
    auto documentElementOwner = std::make_unique<PanelElement>();
    Document document(std::move(documentElementOwner));

    EXPECT_DEATH(document.createElement("not-a-radia-element"), ".*");
}

TEST(ElementTest, NormalizesAdjacentTextNodes) {
    Element root("p");
    radia::ui::detail::appendText(root, "before");
    radia::ui::detail::appendText(root, "after");

    const auto runtimeChildren = radia::ui::detail::nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->getData(), "beforeafter");
}

TEST(ElementTest, StoresLiteralTextContent) {
    Element root("p");
    root.textContent("beforeafter");

    const auto runtimeChildren = radia::ui::detail::nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->getData(), "beforeafter");
}

TEST(ElementTest, PreservesWhitespaceOnlyTextNodes) {
    Element root("p");
    root.textContent(" \t\n");

    const auto runtimeChildren = radia::ui::detail::nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->getData(), " \t\n");
}

TEST(ElementTest, StoresLiteralText) {
    Element root("p");
    root.textContent("Hello world!");

    EXPECT_EQ(root.textContent(), "Hello world!");
}

TEST(ElementTest, TextDataMutationUpdatesOwnerTextContent) {
    Element root("p");
    Text* text = root.append(std::make_unique<Text>("before"))->asText();
    ASSERT_NE(text, nullptr);

    text->setData("after");

    EXPECT_EQ(text->getData(), "after");
    EXPECT_EQ(root.textContent(), "after");
}

TEST(ElementPaintTest, RecordsElementOwnPrimitives) {
    RecordingPaintContext recording;
    Style style;

    LabelElement label("hello");
    label.setRect({1.f, 2.f, 30.f, 10.f});
    label.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 0U);

    IconElement icon("search");
    icon.setRect({4.f, 5.f, 16.f, 16.f});
    icon.paint(recording, style, 2.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 2U);
    EXPECT_EQ(recording.count(PaintCommandKind::Icon), 1U);
    const PaintCommand* iconCommand = recording.last(PaintCommandKind::Icon);
    ASSERT_NE(iconCommand, nullptr);
    EXPECT_EQ(iconCommand->textOrIconName, "search");
    EXPECT_EQ(iconCommand->scale, 2.f);
}

TEST(ElementPaintTest, PaintsCompiledResourcesWithLocaleAndEffects) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {strings: {}}, "
                                     "ar: {strings: {}}}\n";
    constexpr char kStyles[] = "panel { opacity: .5; effect: background-blur(3px), layer-blur(to right, 0px 0%, 4px 100%); } "
                               "label { opacity: .5; text-align: start; } icon { size: 16px; }";
    constexpr char kSearchIconSvg[] = "<svg viewBox=\"0 0 24 24\">"
                                      "<path d=\"M2 2 L22 22\"/></svg>";
    resources.add("localization.yaml", kLocalization);
    resources.add("skin.css", kStyles);
    resources.add("resources/icons/search.svg", kSearchIconSvg);
    SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(resources));
    ASSERT_TRUE(prepared.ok());
    system.publish(prepared.generation);
    ASSERT_TRUE(system.setLocale("ar"));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(100.f, 100.f);
    auto panel = std::make_unique<PanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto label = std::make_unique<LabelElement>("hello");
    label->setRect({0.f, 20.f, 30.f, 10.f});
    panel->append(std::move(label));
    auto icon = std::make_unique<IconElement>("search");
    icon->setRect({0.f, 0.f, 16.f, 16.f});
    panel->append(std::move(icon));
    surface->mount(std::move(panel));

    RecordingPaintContext recording;
    surface->paint(recording, 2.f);
    EXPECT_EQ(recording.count(PaintCommandKind::BeginFrame), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::EndFrame), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::BeginEffects), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::EndEffects), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Icon), 1U);
    const PaintCommand* textCommand = recording.last(PaintCommandKind::Text);
    const PaintCommand* iconCommand = recording.last(PaintCommandKind::Icon);
    const PaintCommand* effectCommand = recording.last(PaintCommandKind::BeginEffects);
    ASSERT_NE(textCommand, nullptr);
    ASSERT_NE(iconCommand, nullptr);
    ASSERT_NE(effectCommand, nullptr);
    EXPECT_EQ(effectCommand->scale, 2.f);
    EXPECT_EQ(effectCommand->style.effects.size(), 2U);
    EXPECT_EQ(iconCommand->textOrIconName, "search");
    EXPECT_EQ(textCommand->style.textColor.a, .25f);
    EXPECT_EQ(static_cast<int>(textCommand->style.textAlign), static_cast<int>(TextAlign::Left));
    EXPECT_EQ(textCommand->rect.x, -8.f);
    EXPECT_EQ(static_cast<int>(recording.commands().front().kind), static_cast<int>(PaintCommandKind::BeginFrame));
    EXPECT_EQ(static_cast<int>(recording.commands().back().kind), static_cast<int>(PaintCommandKind::EndFrame));
    EXPECT_LT(textCommand, iconCommand);
    EXPECT_FALSE(surface->needsPaint());
}

TEST(ElementPaintTest, PaintsMixedTextAndElementsInSourceOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("p { font-size: 10px; line-height: 10px; } b { font-weight: bold; }").ok());

    Surface surface(stylesheet);
    surface.setViewport(200.f, 40.f);
    auto paragraph = std::make_unique<Element>("p");
    paragraph->setRect({0.f, 0.f, 200.f, 20.f});
    radia::ui::detail::appendText(*paragraph, "before ");
    auto bold = std::make_unique<Element>("b");
    radia::ui::detail::appendText(*bold, "bold");
    paragraph->append(std::move(bold));
    radia::ui::detail::appendText(*paragraph, " after");
    surface.mount(std::move(paragraph));

    RecordingPaintContext recording;
    surface.paint(recording);

    std::vector<std::string> painted;
    for (const PaintCommand& command : recording.commands())
        if (command.kind == PaintCommandKind::Text) painted.push_back(command.textOrIconName);
    const std::vector<std::string> expectedPainted{"before ", "bold", " after"};
    EXPECT_EQ(painted, expectedPainted);
}

TEST(TextLayoutTest, PreservesFittingShapedLines) {
    class ShapedRunMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string& value, const Style&) const override {
            if (value == "Radia UI Demo") return {50.f, 10.f};
            if (value == "Radia UI") return {28.f, 10.f};
            if (value == " ") return {6.f, 10.f};
            if (value == "Demo") return {20.f, 10.f};
            return {0.f, 10.f};
        }
    } shapedMetrics;

    TextLayoutTestElement naturallySized("Radia UI Demo");
    naturallySized.setRect({0.f, 0.f, 50.f, 10.f});
    RecordingPaintContext recording(shapedMetrics);
    naturallySized.paint(recording, Style{}, 1.f);

    ASSERT_EQ(recording.count(PaintCommandKind::Text), 1U);
    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].textOrIconName, "Radia UI Demo");
}

TEST(TextLayoutTest, EllipsizesAccordingToTextDirection) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::EllipsisCenter;
    style.overflowX = Overflow::Hidden;

    TextLayoutTestElement inventory("abcdefghij");
    inventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext centered(metrics);
    inventory.paint(centered, style, 1.f);
    EXPECT_EQ(paintedText(centered), "abc\xE2\x80\xA6hij");

    style.textOverflow = TextOverflow::Ellipsis;
    RecordingPaintContext ended(metrics);
    inventory.paint(ended, style, 1.f);
    EXPECT_EQ(paintedText(ended), "abcdef\xE2\x80\xA6");

    style.direction = LayoutDirection::RightToLeft;
    TextLayoutTestElement rtlInventory("ابتثجحخدذر");
    rtlInventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext rtlEnded(metrics);
    rtlInventory.paint(rtlEnded, style, 1.f);
    ASSERT_GE(rtlEnded.commands().size(), 3U);
    EXPECT_EQ(rtlEnded.commands()[1].textOrIconName, "\xE2\x80\xA6");
    EXPECT_EQ(rtlEnded.commands()[2].textOrIconName, "ابتثجح");
}

TEST(TextLayoutTest, PreservesGraphemeBoundariesWhenEllipsizing) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::Ellipsis;
    style.overflowX = Overflow::Hidden;

    TextLayoutTestElement combining("a\u0301bcdef");
    combining.setRect({0.f, 0.f, 20.f, 10.f});
    RecordingPaintContext combiningRecording(metrics);
    combining.paint(combiningRecording, style, 1.f);
    EXPECT_EQ(paintedText(combiningRecording), "a\u0301b\u2026");

    TextLayoutTestElement family("\U0001F468\u200D\U0001F469\u200D\U0001F467ABC");
    family.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext familyRecording(metrics);
    family.paint(familyRecording, style, 1.f);
    EXPECT_EQ(paintedText(familyRecording), "\U0001F468\u200D\U0001F469\u200D\U0001F467A\u2026");
}

TEST(TextLayoutTest, ClipsOverflowWithoutRewritingText) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::Clip;
    style.overflowX = Overflow::Hidden;

    TextLayoutTestElement clipped("abc");
    clipped.setRect({0.f, 0.f, 2.f, 10.f});
    RecordingPaintContext recording(metrics);
    clipped.paint(recording, style, 1.f);

    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].textOrIconName, "abc");
}

TEST(TextLayoutTest, WrapsAtWordAndUnicodeBoundaries) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textOverflow = TextOverflow::Clip;
    EXPECT_EQ(static_cast<int>(style.textWrap), static_cast<int>(TextWrap::Wrap));

    TextLayoutTestElement wrapped("alpha beta");
    wrapped.setRect({0.f, 0.f, 30.f, 20.f});
    RecordingPaintContext wrapping(metrics);
    wrapped.paint(wrapping, style, 1.f);
    ASSERT_EQ(wrapping.count(PaintCommandKind::Text), 2U);
    ASSERT_GE(wrapping.commands().size(), 3U);
    EXPECT_EQ(wrapping.commands()[1].textOrIconName, "alpha");
    EXPECT_EQ(wrapping.commands()[2].textOrIconName, "beta");

    TextLayoutTestElement zeroWidthWrapped("alpha beta");
    zeroWidthWrapped.setRect({0.f, 0.f, 0.f, 20.f});
    RecordingPaintContext zeroWidthWrapping(metrics);
    zeroWidthWrapped.paint(zeroWidthWrapping, style, 1.f);
    EXPECT_EQ(zeroWidthWrapping.count(PaintCommandKind::Text), 2U);

    TextLayoutTestElement multiword("alpha beta gamma");
    multiword.setRect({0.f, 0.f, 50.f, 20.f});
    RecordingPaintContext shapedWrapping(metrics);
    multiword.paint(shapedWrapping, style, 1.f);
    ASSERT_EQ(shapedWrapping.count(PaintCommandKind::Text), 2U);
    ASSERT_GE(shapedWrapping.commands().size(), 3U);
    EXPECT_EQ(shapedWrapping.commands()[1].textOrIconName, "alpha beta");
    EXPECT_EQ(shapedWrapping.commands()[2].textOrIconName, "gamma");

    TextLayoutTestElement cjk("你好世界");
    cjk.setRect({0.f, 0.f, 10.f, 20.f});
    RecordingPaintContext cjkWrapping(metrics);
    cjk.paint(cjkWrapping, style, 1.f);
    EXPECT_EQ(cjkWrapping.count(PaintCommandKind::Text), 2U);

    style.textOverflow = TextOverflow::Clip;
    style.width = Dimension::fromPixels(30.f);
    EXPECT_EQ(wrapped.intrinsicSize(StyleSheet(), style, metrics).y, 20.f);
}

TEST(TextLayoutTest, UsesShapedWidthsForCenterEllipsis) {
    class VariableTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string& value, const Style&) const override {
            if (value.empty()) return {0.f, 10.f};
            if (value == "W") return {20.f, 10.f};
            if (value == "\xE2\x80\xA6") return {5.f, 10.f};
            return {
                static_cast<float>(value.size()) * 5.f,
                10.f,
            };
        }
    } variableMetrics;

    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::EllipsisCenter;
    style.overflowX = Overflow::Hidden;
    TextLayoutTestElement asymmetric("Wabc");
    asymmetric.setRect({0.f, 0.f, 10.f, 10.f});
    RecordingPaintContext recording(variableMetrics);
    asymmetric.paint(recording, style, 1.f);

    EXPECT_EQ(paintedText(recording), "\u2026c");
}

TEST(TextLayoutTest, AppliesOverflowToMountedTextNodes) {
    StyleSheet stylesheet;
    constexpr char kTextOverflowStyle[] =
        "panel { display: block; } "
        "p { width: 20px; height: 10px; font-size: 10px; line-height: 10px; text-wrap: nowrap; overflow: hidden; } "
        "p.end { text-overflow: ellipsis; } "
        "p.center { text-overflow: ellipsis-center; }";
    ASSERT_TRUE(stylesheet.loadRadia(kTextOverflowStyle).ok());

    Surface surface(stylesheet);
    surface.setViewport(20.f, 20.f);
    auto panel = std::make_unique<PanelElement>();
    panel->setRect({0.f, 0.f, 20.f, 20.f});

    auto end = std::make_unique<Element>("p");
    end->addClass("end");
    end->textContent("abcdef");
    panel->append(std::move(end));

    auto center = std::make_unique<Element>("p");
    center->addClass("center");
    center->textContent("abcdef");
    panel->append(std::move(center));

    surface.mount(std::move(panel));
    RecordingPaintContext recording;
    surface.paint(recording);

    EXPECT_EQ(paintedText(recording), "ab\u2026a\u2026f");
}

TEST(TextLayoutTest, AppliesLetterAndWordSpacingToMeasuredText) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;

    style.letterSpacing = Length{2.f};
    EXPECT_EQ(metrics.measureText("abc", style).x, 19.f);
    style.letterSpacing = Length{0.f, .5f};
    EXPECT_EQ(metrics.measureText("abc", style).x, 20.f);

    style.letterSpacing = {};
    style.wordSpacing = Length{3.f};
    EXPECT_EQ(metrics.measureText("a b", style).x, 18.f);
    style.wordSpacing = Length{0.f, .5f};
    EXPECT_EQ(metrics.measureText("a b", style).x, 20.f);
    style.wordSpacing = Length{3.f};
    EXPECT_EQ(metrics.measureText("a\u2003b", style).x, 18.f);
}

TEST(ElementTest, UpdatesDisabledAndVisibilityState) {
    ButtonElement button;

    button.disabled(true);
    EXPECT_TRUE(button.disabled());
    button.disabled(false);
    EXPECT_FALSE(button.disabled());

    button.setHidden(true);
    EXPECT_EQ(button.visibility(), Visibility::Hidden);
    button.setHidden(false);
    EXPECT_EQ(button.visibility(), Visibility::Visible);
}

TEST(SwitchElementTest, PointerActivationUpdatesStateAndThumb) {
    StyleSheet styleSheet;
    constexpr char kSwitchLayout[] = "panel { display: flex; flex-direction: row; } "
                                     "input[switch] { display: flex; flex-direction: row; width: 40px; height: 20px; background-color: #000000ff; } "
                                     "input[switch]::track { width: 100%; min-width: 0; align-self: stretch; } "
                                     "input[switch]::thumb { order: -1; } "
                                     "input[switch]:checked::thumb { order: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitchLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = std::make_unique<PanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = std::make_unique<InputElement>();
    InputElement* target = control.get();
    control->type("checkbox").switchMode(true);
    panel->append(std::move(control));
    surface.mount(std::move(panel));

    surface.updateLayout();
    ASSERT_NE(target->track(), nullptr);
    const float uncheckedLeft = target->thumb()->rect().left();
    surface.pointerDown({{5.f, 10.f}, PointerButton::Left});
    surface.pointerUp({{5.f, 10.f}, PointerButton::Left});
    surface.updateLayout();

    EXPECT_TRUE(target->checked());
    EXPECT_GT(target->thumb()->rect().left(), uncheckedLeft);
}
