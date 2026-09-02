/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstdlib>
#include <gtest/gtest.h>
#include <iostream>
#include <string>
#include <string_view>
#include <type_traits>
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/fragment.h"
#include "dom/text.h"
#include "html/button.h"
#include "html/element.h"
#include "html/elementnames.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "llerrorcontrol.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/host.h"
#include "text/metrics.h"

namespace {
using radia::ui::ConstElementList;
using radia::ui::Dimension;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::Event;
using radia::ui::FixedTextMetrics;
using radia::ui::fixedTextMetrics;
using radia::ui::Fragment;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLElement;
using radia::ui::HTMLIconElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::HTMLTag;
using radia::ui::insetRect;
using radia::ui::IntrinsicSizeConstraints;
using radia::ui::isVoidHTMLTag;
using radia::ui::kClickEvent;
using radia::ui::LayoutDirection;
using radia::ui::Length;
using radia::ui::Node;
using radia::ui::NodePtr;
using radia::ui::NodeType;
using radia::ui::Overflow;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PaintContext;
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
using radia::ui::detail::appendText;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;
using radia::ui::detail::nodes;

static_assert(!std::is_constructible_v<Element, std::string_view>);
static_assert(std::is_base_of_v<Element, HTMLElement>);
static_assert(std::is_base_of_v<HTMLElement, HTMLButtonElement>);
static_assert(!std::is_constructible_v<HTMLElement, std::string_view>);
static_assert(!std::is_constructible_v<HTMLButtonElement>);
static_assert(!std::is_constructible_v<HTMLButtonElement, std::string_view>);

[[noreturn]] void reportFatalDiagnostic(const std::string& message) {
    std::cerr << message << std::endl;
    std::abort();
}
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
    auto root = makeElementValue<HTMLPanelElement>();
    auto button = makeElement<HTMLButtonElement>();
    ElementRef<HTMLButtonElement> reference(button.get());
    root.append(std::move(button));
    ASSERT_NE(reference.get(), nullptr);
    root.replaceChildren();
    EXPECT_EQ(reference.get(), nullptr);
}

TEST(EventTest, TargetBecomesNullWhenDispatchDestroysIt) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto button = makeElement<HTMLButtonElement>();
    HTMLButtonElement* target = button.get();
    bool targetCleared = false;
    bool currentTargetCleared = false;
    bool targetRemainsLiveWhileRetained = false;
    NodePtr removed;
    button->addEventListener(kClickEvent, [&](Event& event) {
        removed = event.target()->remove();
        ASSERT_NE(removed, nullptr);
        targetRemainsLiveWhileRetained = event.target() == target;
        removed.reset();
        targetCleared = event.target() == nullptr;
        currentTargetCleared = event.currentTarget() == nullptr;
    });
    root.append(std::move(button));

    target->activate();

    EXPECT_TRUE(targetCleared);
    EXPECT_TRUE(currentTargetCleared);
    EXPECT_TRUE(targetRemainsLiveWhileRetained);
    EXPECT_TRUE(root.children().empty());
}

TEST(EventTest, CheckedPayloadAccessorRejectsOtherPayloads) {
    auto target = makeElementValue<HTMLButtonElement>();
    Event event(kClickEvent, target);

    EXPECT_FALSE(event.checked());
}

TEST(ElementTest, DisabledButtonsDoNotActivate) {
    auto button = makeElementValue<HTMLButtonElement>();
    int activations = 0;
    button.setOnActivate([&](Element&) { ++activations; });
    button.activate();
    button.disabled(true).activate();
    EXPECT_EQ(activations, 1);
}

TEST(ElementTest, ChildInsertionMaintainsOrderAndOwnership) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto first = makeElement<HTMLButtonElement>();
    first->setId("first");
    root.append(std::move(first));
    auto leading = makeElement<HTMLButtonElement>();
    leading->setId("leading");
    root.prepend(std::move(leading));
    ASSERT_EQ(root.children().size(), 2U);
    EXPECT_EQ(root.children()[0]->id(), "leading");
    EXPECT_EQ(root.children()[1]->id(), "first");
    EXPECT_EQ(root.children()[0]->parentElement(), &root);
    root.replaceChildren();
    EXPECT_TRUE(root.children().empty());
}

TEST(ElementTest, StoresGenericAttributesInAuthorOrder) {
    auto element = makeElementValue<Element>("p");

    element.setAttribute("data-state", "ready");
    element.setAttribute("aria-label", "A & B");
    element.setAttribute("data-state", "updated");

    ASSERT_EQ(element.attributes().size(), 2U);
    EXPECT_EQ(element.attributes()[0].name, "data-state");
    ASSERT_TRUE(element.attributes()[0].value.has_value());
    EXPECT_EQ(*element.attributes()[0].value, "updated");
    EXPECT_EQ(element.attributes()[1].name, "aria-label");
    ASSERT_NE(element.attribute("aria-label"), nullptr);
    EXPECT_TRUE(element.hasAttribute("data-state"));

    element.removeAttribute("data-state");

    ASSERT_EQ(element.attributes().size(), 1U);
    EXPECT_FALSE(element.hasAttribute("data-state"));
    EXPECT_EQ(element.attributes()[0].name, "aria-label");
}

TEST(ElementTest, UsesModernChildMutationMethods) {
    auto root = makeElementValue<HTMLPanelElement>();
    Node* firstNode = root.append(makeElement<HTMLLabelElement>("first"));
    ASSERT_NE(firstNode, nullptr);
    Element* first = firstNode->asElement();
    ASSERT_NE(first, nullptr);
    Node* lastNode = root.append(makeElement<HTMLLabelElement>("last"));
    ASSERT_NE(lastNode, nullptr);
    Element* last = lastNode->asElement();
    ASSERT_NE(last, nullptr);

    Node* before = first->before(makeElement<HTMLLabelElement>("before"));
    Node* after = last->after(makeElement<HTMLLabelElement>("after"));
    ASSERT_NE(before, nullptr);
    ASSERT_NE(after, nullptr);
    ASSERT_EQ(root.children().size(), 4U);
    EXPECT_EQ(root.children()[0], before->asElement());
    EXPECT_EQ(root.children()[3], after->asElement());

    NodePtr removed = before->remove();
    ASSERT_EQ(removed.get(), before);
    EXPECT_EQ(before->parentNode(), nullptr);

    auto replacementOwner = makeElement<HTMLLabelElement>("replacement");
    Element* replacement = replacementOwner.get();
    NodePtr replaced = last->replaceWith(std::move(replacementOwner));
    ASSERT_EQ(replaced.get(), last);
    EXPECT_EQ(last->parentNode(), nullptr);
    ASSERT_EQ(root.children().size(), 3U);
    EXPECT_EQ(root.children()[1], replacement);

    Node* text = root.append(std::make_unique<Text>("text"));
    ASSERT_NE(text, nullptr);
    ASSERT_EQ(text->nodeType(), NodeType::Text);
    EXPECT_EQ(text->parentElement(), &root);
    NodePtr detachedText = text->remove();
    EXPECT_EQ(detachedText.get(), text);
}

TEST(ElementTest, ExposesDomShapedNodeTraversal) {
    auto root = makeElementValue<HTMLPanelElement>();
    EXPECT_EQ(root.lastChild(), nullptr);
    root.textContent("before");
    auto childOwner = makeElement<HTMLLabelElement>("child");
    Node* childNode = root.append(std::move(childOwner));
    ASSERT_NE(childNode, nullptr);
    Element* child = childNode->asElement();
    ASSERT_NE(child, nullptr);

    ASSERT_EQ(root.nodeType(), NodeType::Element);
    ASSERT_EQ(root.childNodes().size(), 2U);
    ASSERT_EQ(root.children().size(), 1U);

    Node* text = root.firstChild();
    ASSERT_NE(text, nullptr);
    ASSERT_EQ(text->nodeType(), NodeType::Text);
    ASSERT_NE(text->asText(), nullptr);
    EXPECT_EQ(text->asText()->data(), "before");
    EXPECT_EQ(text->parentNode(), &root);
    EXPECT_EQ(text->parentElement(), &root);
    EXPECT_EQ(text->lastChild(), nullptr);
    EXPECT_EQ(text->previousSibling(), nullptr);
    EXPECT_EQ(text->nextSibling(), child);

    EXPECT_EQ(root.children().front(), child);
    EXPECT_EQ(child->nodeType(), NodeType::Element);
    EXPECT_EQ(child->parentNode(), &root);
    EXPECT_EQ(root.lastChild(), child);
    EXPECT_EQ(child->previousSibling(), text);
    EXPECT_EQ(child->nextSibling(), nullptr);

    const auto& constRoot = root;
    EXPECT_EQ(constRoot.lastChild(), child);
    EXPECT_EQ(constRoot.lastChild()->previousSibling(), text);
}

TEST(NodeTest, DetachedMutationMethodsAreNoOps) {
    auto node = makeElement<HTMLLabelElement>("detached");

    EXPECT_EQ(node->remove(), nullptr);
    EXPECT_EQ(node->before(makeElement<HTMLLabelElement>("before")), nullptr);
    EXPECT_EQ(node->after(makeElement<HTMLLabelElement>("after")), nullptr);
    EXPECT_EQ(node->replaceWith(makeElement<HTMLLabelElement>("replacement")), nullptr);
}

TEST(FragmentTest, ConsumesChildrenInOrderAndUpdatesParents) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto fragment = std::make_unique<Fragment>();
    auto first = makeElement<HTMLLabelElement>("first");
    Node* firstPtr = first.get();
    fragment->append(std::move(first));
    fragment->append(std::make_unique<Text>("middle"));
    Node* last = fragment->append(makeElement<HTMLLabelElement>("last"));

    EXPECT_EQ(fragment->nodeType(), NodeType::Fragment);
    EXPECT_EQ(fragment->firstChild(), firstPtr);
    EXPECT_EQ(fragment->lastChild(), last);
    EXPECT_EQ(firstPtr->previousSibling(), nullptr);
    ASSERT_NE(last->previousSibling(), nullptr);
    ASSERT_NE(last->previousSibling()->asText(), nullptr);
    EXPECT_EQ(last->previousSibling()->asText()->data(), "middle");
    root.append(std::move(fragment));

    ASSERT_EQ(root.childNodes().size(), 3U);
    EXPECT_EQ(root.childNodes()[0], firstPtr);
    EXPECT_EQ(root.childNodes()[0]->parentNode(), &root);
    EXPECT_EQ(root.childNodes()[1]->asText()->data(), "middle");
    EXPECT_EQ(root.childNodes()[2]->asElement()->textContent(), "last");
}

TEST(FragmentTest, ReplacesWithFragmentWithoutReversingChildren) {
    auto root = makeElementValue<HTMLPanelElement>();
    Node* old = root.append(makeElement<HTMLLabelElement>("old"));
    root.append(makeElement<HTMLLabelElement>("tail"));
    auto replacement = std::make_unique<Fragment>();
    replacement->append(makeElement<HTMLLabelElement>("first"));
    replacement->append(makeElement<HTMLLabelElement>("second"));

    NodePtr removed = old->replaceWith(std::move(replacement));

    ASSERT_EQ(removed.get(), old);
    ASSERT_EQ(root.children().size(), 3U);
    EXPECT_EQ(root.children()[0]->textContent(), "first");
    EXPECT_EQ(root.children()[1]->textContent(), "second");
    EXPECT_EQ(root.children()[2]->textContent(), "tail");
}

TEST(FragmentTest, ParsesAndSerializesBoundedHTML) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<p id='123:bad.id' class='primary.bad @token'>Hello &amp; <br>world</p><input type=checkbox>");

    EXPECT_EQ(root.textContent(), "Hello & \nworld");
    EXPECT_EQ(root.innerHTML(), "<p id=\"123:bad.id\" class=\"primary.bad @token\">Hello &amp; <br>world</p><input type=\"checkbox\">");
    ASSERT_EQ(root.children().size(), 2U);
    EXPECT_EQ(root.children()[0]->id(), "123:bad.id");
    EXPECT_TRUE(root.children()[0]->classes().contains("primary.bad"));
    EXPECT_TRUE(root.children()[0]->classes().contains("@token"));
    EXPECT_EQ(root.children()[1]->elementName(), "input");
}

TEST(FragmentTest, KeepsSlashInUnquotedAttributeValues) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<input name=mode/>");

    EXPECT_EQ(root.innerHTML(), "<input type=\"text\" name=\"mode/\">");
}

TEST(FragmentTest, AppliesInputTypeBeforeStateAttributes) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<input checked type=checkbox>");

    ASSERT_EQ(root.children().size(), 1U);
    const auto* input = dynamic_cast<const HTMLInputElement*>(root.children().front());
    ASSERT_NE(input, nullptr);
    EXPECT_TRUE(input->checked());
}

TEST(FragmentTest, TreatsBooleanAttributesAsPresenceFlags) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<input type=checkbox checked=false switch=0>");
    ASSERT_EQ(root.children().size(), 1U);
    const auto* input = dynamic_cast<const HTMLInputElement*>(root.children().front());
    ASSERT_NE(input, nullptr);
    EXPECT_TRUE(input->checked());
    EXPECT_TRUE(input->switchMode());

    root.innerHTML("<input type=checkbox checked=yes>");
    ASSERT_EQ(root.children().size(), 1U);
    const auto* valueInput = dynamic_cast<const HTMLInputElement*>(root.children().front());
    ASSERT_NE(valueInput, nullptr);
    EXPECT_TRUE(valueInput->checked());

    root.innerHTML("<button disabled=false>Save</button>");
    ASSERT_EQ(root.children().size(), 1U);
    const auto* button = dynamic_cast<const HTMLButtonElement*>(root.children().front());
    ASSERT_NE(button, nullptr);
    EXPECT_TRUE(button->disabled());
}

TEST(FragmentTest, SerializesViewerOwnedPartsOnlyThroughTheirAuthoringElement) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto input = makeElement<HTMLInputElement>();
    input->type("checkbox").switchMode(true);
    root.append(std::move(input));

    EXPECT_EQ(root.innerHTML(), "<input type=\"checkbox\" switch>");
}

TEST(FragmentTest, SerializesGenericAttributesFromElementStorage) {
    auto root = makeElementValue<Element>("section");
    auto child = makeElement<Element>("p");
    child->setAttribute("data-state", "ready");
    root.append(std::move(child));

    EXPECT_EQ(root.innerHTML(), "<p data-state=\"ready\"></p>");
}

TEST(FragmentTest, TreatsMalformedHTMLAsLiteralText) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<p>unclosed");

    EXPECT_EQ(root.textContent(), "<p>unclosed");
    EXPECT_EQ(root.innerHTML(), "&lt;p&gt;unclosed");

    root.innerHTML("<p/>");

    EXPECT_EQ(root.textContent(), "<p/>");
    EXPECT_EQ(root.innerHTML(), "&lt;p/&gt;");
}

TEST(HTMLNamesTest, KeepsVoidnessInTheHTMLVocabulary) {
    EXPECT_TRUE(isVoidHTMLTag(HTMLTag::Br));
    EXPECT_TRUE(isVoidHTMLTag(HTMLTag::Input));
    EXPECT_FALSE(isVoidHTMLTag(HTMLTag::Div));
}

TEST(ElementTest, ConstChildrenExposeConstBorrowedElements) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto childOwner = makeElement<HTMLLabelElement>("child");
    const Element* expected = childOwner.get();
    root.append(std::move(childOwner));

    const Element& constRoot = root;
    static_assert(std::is_same_v<decltype(constRoot.children()), ConstElementList>);
    const ConstElementList children = constRoot.children();

    ASSERT_EQ(children.size(), 1U);
    EXPECT_EQ(children.front(), expected);
}

TEST(DocumentTest, OwnsDocumentElementAndTransfersDetachedChildren) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    documentElementOwner->setId("root");
    Element* documentElement = documentElementOwner.get();
    Document document(std::move(documentElementOwner));

    EXPECT_EQ(document.nodeType(), NodeType::Document);
    EXPECT_EQ(document.parentNode(), nullptr);
    EXPECT_EQ(document.firstChild(), documentElement);
    EXPECT_EQ(document.lastChild(), documentElement);
    ASSERT_EQ(document.childNodes().size(), 1U);
    EXPECT_EQ(document.childNodes().front(), documentElement);
    ASSERT_EQ(document.documentElement(), documentElement);
    EXPECT_EQ(documentElement->nodeType(), NodeType::Element);
    EXPECT_EQ(documentElement->parentNode(), static_cast<radia::ui::Node*>(&document));
    EXPECT_EQ(documentElement->parentElement(), nullptr);
    ASSERT_EQ(document.getElementById("root"), documentElement);
    auto buttonOwner = document.createElement("button");
    ASSERT_NE(buttonOwner.get(), nullptr);
    buttonOwner->setId("button");
    Node* buttonNode = documentElement->append(std::move(buttonOwner));
    ASSERT_NE(buttonNode, nullptr);
    Element* button = buttonNode->asElement();
    ASSERT_NE(button, nullptr);

    EXPECT_EQ(document.getElementById("button"), button);
    EXPECT_EQ(button->parentElement(), documentElement);

    NodePtr detached = button->remove();
    ASSERT_EQ(detached.get(), button);
    EXPECT_EQ(button->parentElement(), nullptr);
    EXPECT_EQ(document.getElementById("button"), nullptr);

    ASSERT_EQ(documentElement->append(std::move(detached)), button);
    EXPECT_EQ(document.getElementById("button"), button);
}

TEST(DocumentTest, CreatesBaseAndConcreteHTMLElementsThroughFactory) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    auto base = document.createElement("DiV");
    ASSERT_NE(base, nullptr);
    EXPECT_NE(dynamic_cast<HTMLElement*>(base.get()), nullptr);
    EXPECT_EQ(dynamic_cast<HTMLButtonElement*>(base.get()), nullptr);
    EXPECT_EQ(base->elementName(), "div");

    auto button = document.createElement("BuTtOn");
    ASSERT_NE(button, nullptr);
    EXPECT_NE(dynamic_cast<HTMLButtonElement*>(button.get()), nullptr);
    EXPECT_EQ(button->elementName(), "button");
}

TEST(DocumentTest, ReparentsDetachedElementsWithinDocument) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));
    Element* documentElement = document.documentElement();
    ASSERT_NE(documentElement, nullptr);

    auto firstPanelOwner = document.createElement("panel");
    ASSERT_NE(firstPanelOwner.get(), nullptr);
    Node* firstPanelNode = documentElement->append(std::move(firstPanelOwner));
    ASSERT_NE(firstPanelNode, nullptr);
    Element* firstPanel = firstPanelNode->asElement();
    ASSERT_NE(firstPanel, nullptr);
    auto secondPanelOwner = document.createElement("panel");
    ASSERT_NE(secondPanelOwner.get(), nullptr);
    Node* secondPanelNode = documentElement->append(std::move(secondPanelOwner));
    ASSERT_NE(secondPanelNode, nullptr);
    Element* secondPanel = secondPanelNode->asElement();
    ASSERT_NE(secondPanel, nullptr);

    auto buttonOwner = document.createElement("button");
    ASSERT_NE(buttonOwner.get(), nullptr);
    Node* buttonNode = firstPanel->append(std::move(buttonOwner));
    ASSERT_NE(buttonNode, nullptr);
    Element* button = buttonNode->asElement();
    ASSERT_NE(button, nullptr);
    NodePtr detached = button->remove();

    ASSERT_EQ(detached.get(), button);
    EXPECT_EQ(button->parentElement(), nullptr);
    EXPECT_EQ(button->elementName(), "button");

    ASSERT_EQ(secondPanel->append(std::move(detached)), button);
    EXPECT_EQ(button->parentElement(), secondPanel);
    ASSERT_EQ(secondPanel->children().size(), 1U);
    EXPECT_EQ(secondPanel->children().front(), button);
}

TEST(ElementTreeDeathTest, RejectsNullChild) {
    auto root = makeElementValue<HTMLPanelElement>();
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            root.append(NodePtr());
        },
        "ASSERT \\(child\\)");
    EXPECT_TRUE(root.children().empty());
}

TEST(DocumentTest, AdoptsCrossDocumentChildOnInsertion) {
    auto firstRootOwner = makeElement<HTMLPanelElement>();
    Document first(std::move(firstRootOwner));
    auto secondRootOwner = makeElement<HTMLPanelElement>();
    Document second(std::move(secondRootOwner));
    auto childOwner = first.createElement("button");
    childOwner->setId("adopted");
    Element* child = childOwner.get();

    ASSERT_EQ(second.documentElement()->append(std::move(childOwner)), child);
    EXPECT_EQ(child->parentElement(), second.documentElement());
    EXPECT_EQ(first.getElementById("adopted"), nullptr);
    EXPECT_EQ(second.getElementById("adopted"), child);
}

TEST(DocumentTest, RemovesDocumentElementThroughNodeMutation) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    NodePtr detached = document.documentElement()->remove();
    ASSERT_NE(detached, nullptr);
    EXPECT_EQ(document.documentElement(), nullptr);
    EXPECT_EQ(detached->parentNode(), nullptr);
    EXPECT_EQ(radia::ui::detail::NodeAccess::documentIdentity(*detached), radia::ui::detail::NodeAccess::documentIdentity(document));
}

TEST(ElementTreeDeathTest, RejectsUnknownRuntimeElement) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            document.createElement("not-a-radia-element");
        },
        "Unknown UI Element type");
}

TEST(ElementTest, NormalizesAdjacentTextNodes) {
    auto root = makeElementValue<Element>("p");
    appendText(root, "before");
    appendText(root, "after");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), "beforeafter");
}

TEST(ElementTest, StoresLiteralTextContent) {
    auto root = makeElementValue<Element>("p");
    root.textContent("beforeafter");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), "beforeafter");
}

TEST(ElementTest, PreservesWhitespaceOnlyTextNodes) {
    auto root = makeElementValue<Element>("p");
    root.textContent(" \t\n");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), " \t\n");
}

TEST(ElementTest, TextDataMutationUpdatesOwnerTextContent) {
    auto root = makeElementValue<Element>("p");
    Node* textNode = root.append(std::make_unique<Text>("before"));
    ASSERT_NE(textNode, nullptr);
    Text* text = textNode->asText();
    ASSERT_NE(text, nullptr);

    text->setData("after");

    EXPECT_EQ(text->data(), "after");
    EXPECT_EQ(root.textContent(), "after");
}

TEST(ElementPaintTest, RecordsElementOwnPrimitives) {
    RecordingPaintContext recording;
    Style style;

    auto label = makeElementValue<HTMLLabelElement>("hello");
    label.setRect({1.f, 2.f, 30.f, 10.f});
    label.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 0U);

    auto icon = makeElementValue<HTMLIconElement>("search");
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
    ASSERT_TRUE(system.publish(prepared.generation));
    ASSERT_TRUE(system.setLocale("ar"));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto label = makeElement<HTMLLabelElement>("hello");
    label->setRect({0.f, 20.f, 30.f, 10.f});
    panel->append(std::move(label));
    auto icon = makeElement<HTMLIconElement>("search");
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
    EXPECT_EQ(textCommand->style.color.a, .25f);
    EXPECT_EQ(textCommand->style.textAlign, TextAlign::Left);
    EXPECT_EQ(textCommand->rect.x, -8.f);
    EXPECT_EQ(recording.commands().front().kind, PaintCommandKind::BeginFrame);
    EXPECT_EQ(recording.commands().back().kind, PaintCommandKind::EndFrame);
    EXPECT_LT(textCommand, iconCommand);
    EXPECT_FALSE(surface->needsPaint());
}

TEST(ElementPaintTest, PaintsMixedTextAndElementsInSourceOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("p { font-size: 10px; line-height: 10px; } b { font-weight: bold; }").ok());

    Surface surface(stylesheet);
    surface.setViewport(200.f, 40.f);
    auto paragraph = makeElement<Element>("p");
    paragraph->setRect({0.f, 0.f, 200.f, 20.f});
    appendText(*paragraph, "before ");
    auto bold = makeElement<Element>("b");
    appendText(*bold, "bold");
    paragraph->append(std::move(bold));
    appendText(*paragraph, " after");
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
    ASSERT_EQ(style.textWrap, TextWrap::Wrap);

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
    constexpr char kTextOverflowStyle[] = "panel { display: block; } "
                                          "p { width: 20px; height: 10px; font-size: 10px; line-height: 10px; text-wrap: nowrap; overflow: hidden; } "
                                          "p.end { text-overflow: ellipsis; } "
                                          "p.center { text-overflow: ellipsis-center; }";
    ASSERT_TRUE(stylesheet.loadRadia(kTextOverflowStyle).ok());

    Surface surface(stylesheet);
    surface.setViewport(20.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 20.f, 20.f});

    auto end = makeElement<Element>("p");
    end->addClass("end");
    end->textContent("abcdef");
    panel->append(std::move(end));

    auto center = makeElement<Element>("p");
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

TEST(SwitchElementTest, PointerActivationUpdatesStateAndThumb) {
    StyleSheet styleSheet;
    constexpr char kSwitchLayout[] =
        "panel { display: flex; flex-direction: row; } "
        "input[switch] { appearance: base; display: flex; flex-direction: row; width: 40px; height: 20px; background-color: #000000ff; } "
        "input[switch]::slider-track { width: 100%; min-width: 0; align-self: stretch; } "
        "input[switch]::slider-thumb { order: -1; width: 20px; height: 20px; } "
        "input[switch]:checked::slider-thumb { order: 1; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitchLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = makeElement<HTMLInputElement>();
    HTMLInputElement* target = control.get();
    control->type("checkbox").switchMode(true);
    panel->append(std::move(control));
    surface.mount(std::move(panel));

    surface.updateLayout();
    ASSERT_NE(target->sliderTrack(), nullptr);
    ASSERT_NE(target->sliderThumb(), nullptr);
    const float uncheckedLeft = target->sliderThumb()->rect().left();
    surface.pointerDown({{5.f, 10.f}, PointerButton::Left});
    surface.pointerUp({{5.f, 10.f}, PointerButton::Left});
    surface.updateLayout();

    EXPECT_TRUE(target->checked());
    ASSERT_NE(target->sliderThumb(), nullptr);
    EXPECT_GT(target->sliderThumb()->rect().left(), uncheckedLeft);
}
