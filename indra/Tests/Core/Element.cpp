/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/Document.h>
#include <Core/ElementInternal.h>
#include <Core/EventHandlerCall.h>
#include <Core/Fragment.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLElement.h>
#include <Core/HTMLInputElement.h>
#include <Core/HTMLLabelElement.h>
#include <Core/HTMLName.h>
#include <Core/HTMLPanelElement.h>
#include <Core/LayoutGeometry.h>
#include <Core/RecordingPaintContext.h>
#include <Core/SkinCompiler.h>
#include <Core/StylePass.h>
#include <Core/Surface.h>
#include <Core/System.h>
#include <Core/Text.h>
#include <Core/TextLayout.h>
#include <Core/TextMeasurer.h>
#include <cstdlib>
#include <gtest/gtest.h>
#include <iostream>
#include <memory>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>
#include "llerrorcontrol.h"

namespace {
using Core::ConstElementList;
using Core::Document;
using Core::Element;
using Core::ElementRef;
using Core::ElementVisit;
using Core::Event;
using Core::findHTMLTag;
using Core::FixedTextMeasurer;
using Core::fixedTextMeasurer;
using Core::Fragment;
using Core::HTMLButtonElement;
using Core::HTMLElement;
using Core::HTMLInputElement;
using Core::HTMLLabelElement;
using Core::HTMLPanelElement;
using Core::HTMLTag;
using Core::isVoidHTMLTag;
using Core::kClickEvent;
using Core::Node;
using Core::NodePtr;
using Core::NodeType;
using Core::PaintCommand;
using Core::PaintCommandKind;
using Core::PaintContext;
using Core::PointerButton;
using Core::PointerEvent;
using Core::RecordingPaintContext;
using Core::ResourceSnapshot;
using Core::SkinCompiler;
using Core::SkinGenerationPrepareResult;
using Core::Surface;
using Core::System;
using Core::Text;
using Core::TextMeasurer;
using Core::CSS::PseudoClass;
using Core::CSS::StyleSheet;
using Core::detail::appendText;
using Core::detail::ElementInternalAccess;
using Core::detail::makeElement;
using Core::detail::makeElementValue;
using Core::detail::nodes;
using Core::Layout::Direction;
using Core::Layout::insetRect;
using Core::Layout::IntrinsicSizeConstraints;
using Core::Layout::paddingPixels;
using Core::Layout::TextLayout;
using Core::Layout::TextPaintStyle;
using Core::Layout::Vec2;
using Core::Style::ComputedStyle;
using Core::Style::Dimension;
using Core::Style::Length;
using Core::Style::LetterSpacing;
using Core::Style::Overflow;
using Core::Style::Pass;
using Core::Style::TextAlign;
using Core::Style::TextOverflow;
using Core::Style::TextWrapMode;
using Core::Style::TextWrapStyle;
using Core::Style::Visibility;
using Core::Style::WordSpacing;

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
void resolveTextTestColors(ComputedStyle& style) {
    using enum Core::CSS::KeywordName;
    Core::Style::resolveStyleColors(style, Core::Style::systemColorValue(KeywordCanvasText, style.usedColorScheme));
}

class TextLayoutTestElement final : public Element {
public:
    explicit TextLayoutTestElement(std::string text = {})
        : Element("p")
        , mLayout(std::move(text)) {}

    const std::string& text() const { return mLayout.plainText(); }

    Vec2 intrinsicSize(const StyleSheet& styleSheet, const ComputedStyle& style, const TextMeasurer& textMetrics,
        const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const override {
        return mLayout.measure(textMetrics, style, styleSheet, *this, constraints.width);
    }

    void paint(PaintContext& context, const ComputedStyle& style, float scale) const override {
        ComputedStyle paintStyle = style;
        resolveTextTestColors(paintStyle);
        context.paintBox(rect(), paintStyle);
        mLayout.paint(context, insetRect(rect(), paddingPixels(paintStyle)), paintStyle, styleSheet(), *this);
    }

private:
    TextLayout mLayout;
};

class CountingTextMeasurer final : public TextMeasurer {
public:
    Vec2 measureText(const std::string& text, const ComputedStyle& style) const override {
        ++mMeasureCalls;
        return mMetrics.measureText(text, style);
    }

    float usedLetterSpacing(const ComputedStyle& style) const override { return mMetrics.usedLetterSpacing(style); }
    std::uint64_t generation() const noexcept override { return mMetrics.generation(); }
    std::size_t measureCalls() const { return mMeasureCalls; }

private:
    FixedTextMeasurer mMetrics;
    mutable std::size_t mMeasureCalls = 0;
};

class EventDispatchProbe final : public Element {
public:
    EventDispatchProbe()
        : Element("probe") {}

    void dispatch(Event& event) { dispatchEvent(event); }
};
} // namespace

namespace {
class MutationCallbackProbe final : public Element {
public:
    MutationCallbackProbe(std::vector<std::string>& events, std::string name)
        : Element("probe")
        , mEvents(events)
        , mName(std::move(name)) {}

protected:
    void onTreeAttached() override { record("tree-attached"); }
    void onTreeDetached() override { record("tree-detached"); }
    void onChildAdded(Element&) override { record("child-added"); }
    void onChildRemoved(Element&) override { record("child-removed"); }
    void onDescendantAdded(Element&) override { record("descendant-added"); }
    void onDescendantRemoved(Element&) override { record("descendant-removed"); }

private:
    void record(std::string_view callback) { mEvents.push_back(mName + "." + std::string(callback)); }

    std::vector<std::string>& mEvents;
    std::string mName;
};

class DestroyOnChildrenCleared final : public Element {
public:
    explicit DestroyOnChildrenCleared(std::unique_ptr<Element>* owner)
        : Element("destroying")
        , mOwner(owner) {}

protected:
    void onChildrenCleared() override { mOwner->reset(); }

private:
    std::unique_ptr<Element>* mOwner;
};

class DestroyRootOnChildRemoved final : public Element {
public:
    DestroyRootOnChildRemoved(Surface& surface, Element& root)
        : Element("destroying")
        , mSurface(&surface)
        , mRoot(&root) {}

protected:
    void onChildRemoved(Element&) override { mSurface->unmount(*mRoot); }

private:
    Surface* mSurface;
    Element* mRoot;
};

class DestroySurfaceOnChildWillBeRemoved final : public Element {
public:
    explicit DestroySurfaceOnChildWillBeRemoved(std::unique_ptr<Surface>* owner)
        : Element("destroying")
        , mOwner(owner) {}

protected:
    void onChildWillBeRemoved(Element&) override { mOwner->reset(); }

private:
    std::unique_ptr<Surface>* mOwner;
};

class ObserveMountStateAtDestruction final : public Element {
public:
    explicit ObserveMountStateAtDestruction(bool* wasMounted)
        : Element("probe")
        , mWasMounted(wasMounted) {}
    ~ObserveMountStateAtDestruction() override { *mWasMounted = ElementInternalAccess::isMounted(*this); }

private:
    bool* mWasMounted;
};
} // namespace

namespace {
std::string paintedText(const RecordingPaintContext& recording) {
    std::string text;
    for (const PaintCommand& command : recording.commands())
        if (command.kind == PaintCommandKind::Text)
            text += command.text;
    return text;
}
} // namespace

TEST(Element, ExpiresReferencesOnUnmount) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto button = makeElement<HTMLButtonElement>();
    ElementRef<HTMLButtonElement> reference(button.get());
    root.append(std::move(button));
    ASSERT_NE(reference.get(), nullptr);
    root.replaceChildren();
    EXPECT_EQ(reference.get(), nullptr);
}

TEST(Event, ClearsTargetOnDestruction) {
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

TEST(Event, RejectsWrongPayload) {
    auto target = makeElementValue<HTMLButtonElement>();
    Event event(kClickEvent, target);

    EXPECT_FALSE(event.checked());
}

TEST(Event, RejectsInvalidPayloadForKnownType) {
    auto target = std::make_unique<EventDispatchProbe>();
    int invocations = 0;
    target->addEventListener(kClickEvent, [&](Event&) {
        ++invocations;
    });

    Event event(kClickEvent, *target, PointerEvent {});
    target->dispatch(event);

    EXPECT_EQ(invocations, 0);
}

TEST(Element, DisabledButtonsDoNotActivate) {
    auto button = makeElementValue<HTMLButtonElement>();
    int activations = 0;
    button.setOnActivate([&](Element&) {
        ++activations;
    });
    button.activate();
    button.disabled(true).activate();
    EXPECT_EQ(activations, 1);
}

TEST(Element, PreservesChildOrder) {
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

TEST(Element, PreservesAttributeOrder) {
    auto element = makeElementValue<Element>("p");

    element.setAttribute("DATA-State", "ready");
    element.setAttribute("aria-LABEL", "A & B");
    element.setAttribute("data-STATE", "updated");

    ASSERT_EQ(element.attributes().size(), 2U);
    EXPECT_EQ(element.attributes()[0].name, "data-state");
    ASSERT_TRUE(element.attributes()[0].value.has_value());
    EXPECT_EQ(*element.attributes()[0].value, "updated");
    EXPECT_EQ(element.attributes()[1].name, "aria-label");
    ASSERT_NE(element.attribute("ARIA-Label"), nullptr);
    EXPECT_TRUE(element.hasAttribute("DATA-state"));

    element.removeAttribute("");
    EXPECT_EQ(element.attributes().size(), 2U);

    element.removeAttribute("data-state");

    ASSERT_EQ(element.attributes().size(), 1U);
    EXPECT_FALSE(element.hasAttribute("data-state"));
    EXPECT_EQ(element.attributes()[0].name, "aria-label");
}

TEST(Element, ManagesClassTokensInFirstSeenOrder) {
    auto element = makeElementValue<Element>("p");

    element.setAttribute("CLASS", "second first second");
    ASSERT_NE(element.attribute("class"), nullptr);
    ASSERT_TRUE(element.attribute("class")->value.has_value());
    EXPECT_EQ(*element.attribute("class")->value, "second first");
    EXPECT_TRUE(element.classList().contains("second"));
    EXPECT_TRUE(element.classList().contains("first"));

    element.classList().add("third").add("first");
    EXPECT_EQ(*element.attribute("class")->value, "second first third");
    element.classList().remove("first");
    EXPECT_EQ(*element.attribute("class")->value, "second third");
    EXPECT_FALSE(element.classList().toggle("second"));
    EXPECT_TRUE(element.classList().toggle("last"));
    EXPECT_EQ(*element.attribute("class")->value, "third last");
    EXPECT_TRUE(element.classList().replace("last", "final"));
    EXPECT_EQ(*element.attribute("class")->value, "third final");
    EXPECT_FALSE(element.classList().replace("missing", "new"));
    EXPECT_TRUE(element.classList().replace("final", "third"));
    EXPECT_EQ(*element.attribute("class")->value, "third");
    EXPECT_FALSE(element.classList().replace("third", "bad token"));
    EXPECT_EQ(*element.attribute("class")->value, "third");
}

TEST(Element, UsesModernChildMutationMethods) {
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
    const ElementVisit replacedObservation(*last);
    NodePtr replaced = last->replaceWith(std::move(replacementOwner));
    ASSERT_EQ(replaced.get(), last);
    EXPECT_EQ(last->parentNode(), nullptr);
    EXPECT_FALSE(replacedObservation.topologyValid());
    ASSERT_EQ(root.children().size(), 3U);
    EXPECT_EQ(root.children()[1], replacement);

    Node* text = root.append(std::make_unique<Text>("text"));
    ASSERT_NE(text, nullptr);
    ASSERT_EQ(text->nodeType(), NodeType::Text);
    EXPECT_EQ(text->parentElement(), &root);
    NodePtr detachedText = text->remove();
    EXPECT_EQ(detachedText.get(), text);
}

TEST(Element, ExposesDomShapedNodeTraversal) {
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

TEST(Node, DetachedMutationMethodsAreNoOps) {
    auto node = makeElement<HTMLLabelElement>("detached");

    EXPECT_EQ(node->remove(), nullptr);
    EXPECT_EQ(node->before(makeElement<HTMLLabelElement>("before")), nullptr);
    EXPECT_EQ(node->after(makeElement<HTMLLabelElement>("after")), nullptr);
    EXPECT_EQ(node->replaceWith(makeElement<HTMLLabelElement>("replacement")), nullptr);
}

TEST(ElementTreeDeathTest, RejectsDocumentParentSiblingMutation) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto node = makeElement<HTMLLabelElement>("before");
            document.documentElement()->before(std::move(node));
        },
        "parent->asDocument");
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto fragment = document.createFragment();
            document.documentElement()->before(std::move(fragment));
        },
        "parent->asDocument");
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto node = makeElement<HTMLLabelElement>("after");
            document.documentElement()->after(std::move(node));
        },
        "parent->asDocument");
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto fragment = document.createFragment();
            document.documentElement()->after(std::move(fragment));
        },
        "parent->asDocument");
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto node = makeElement<HTMLLabelElement>("replacement");
            document.documentElement()->replaceWith(std::move(node));
        },
        "parent->asDocument");
    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto fragment = document.createFragment();
            document.documentElement()->replaceWith(std::move(fragment));
        },
        "parent->asDocument");
}

TEST(Fragment, PreservesChildOrder) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto fragment = std::make_unique<Fragment>();
    auto first = makeElement<HTMLLabelElement>("first");
    Node* firstPtr = first.get();
    fragment->append(std::move(first));
    Node* middle = fragment->append(std::make_unique<Text>("middle"));
    Node* last = fragment->append(makeElement<HTMLLabelElement>("last"));

    EXPECT_EQ(fragment->nodeType(), NodeType::Fragment);
    EXPECT_EQ(fragment->firstChild(), firstPtr);
    EXPECT_EQ(fragment->lastChild(), last);
    EXPECT_EQ(firstPtr->previousSibling(), nullptr);
    EXPECT_EQ(firstPtr->nextSibling(), middle);
    EXPECT_EQ(middle->previousSibling(), firstPtr);
    EXPECT_EQ(middle->nextSibling(), last);
    ASSERT_NE(last->previousSibling(), nullptr);
    ASSERT_NE(last->previousSibling()->asText(), nullptr);
    EXPECT_EQ(last->previousSibling()->asText()->data(), "middle");
    EXPECT_EQ(last->nextSibling(), nullptr);
    root.append(std::move(fragment));

    ASSERT_EQ(root.childNodes().size(), 3U);
    EXPECT_EQ(root.childNodes()[0], firstPtr);
    EXPECT_EQ(root.childNodes()[0]->parentNode(), &root);
    EXPECT_EQ(root.childNodes()[1]->asText()->data(), "middle");
    EXPECT_EQ(root.childNodes()[2]->asElement()->textContent(), "last");
}

TEST(Fragment, PreservesChildOrderOnReplace) {
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

TEST(Fragment, OrdersMutationCallbacks) {
    std::vector<std::string> events;
    auto outer = std::make_unique<MutationCallbackProbe>(events, "outer");
    auto root = std::make_unique<MutationCallbackProbe>(events, "root");
    MutationCallbackProbe* rootPtr = root.get();
    outer->append(std::move(root));
    events.clear();

    auto child = std::make_unique<MutationCallbackProbe>(events, "child");
    MutationCallbackProbe* childPtr = child.get();
    rootPtr->append(std::move(child));

    ASSERT_EQ(events.size(), 3U);
    EXPECT_EQ(events[0], "child.tree-attached");
    EXPECT_EQ(events[1], "root.child-added");
    EXPECT_EQ(events[2], "outer.descendant-added");

    events.clear();
    NodePtr removed = childPtr->remove();
    ASSERT_EQ(removed.get(), childPtr);
    ASSERT_EQ(events.size(), 3U);
    EXPECT_EQ(events[0], "root.child-removed");
    EXPECT_EQ(events[1], "child.tree-detached");
    EXPECT_EQ(events[2], "outer.descendant-removed");
}

TEST(Fragment, AllowsParentDestructionDuringClear) {
    std::unique_ptr<Element> owner;
    owner = std::make_unique<DestroyOnChildrenCleared>(&owner);
    Element* parent = owner.get();
    parent->append(makeElement<HTMLLabelElement>("old"));

    parent->replaceChildren();

    EXPECT_FALSE(owner);
}

TEST(Fragment, DetachesChildrenBeforeParent) {
    Surface surface;
    auto root = makeElement<HTMLPanelElement>();
    HTMLPanelElement* rootPtr = root.get();
    auto parent = std::make_unique<DestroyRootOnChildRemoved>(surface, *rootPtr);
    DestroyRootOnChildRemoved* parentPtr = parent.get();
    parent->append(makeElement<HTMLLabelElement>("first"));

    bool remainingChildWasMounted = false;
    parent->append(makeElement<ObserveMountStateAtDestruction>(&remainingChildWasMounted));
    root->append(std::move(parent));
    surface.mount(std::move(root));

    parentPtr->replaceChildren();

    EXPECT_FALSE(remainingChildWasMounted);
}

TEST(Fragment, DetachesChildrenBeforeSurface) {
    auto surface = std::make_unique<Surface>();
    auto root = makeElement<HTMLPanelElement>();
    auto parent = std::make_unique<DestroySurfaceOnChildWillBeRemoved>(&surface);
    DestroySurfaceOnChildWillBeRemoved* parentPtr = parent.get();
    parent->append(makeElement<HTMLLabelElement>("first"));

    bool remainingChildWasMounted = false;
    parent->append(makeElement<ObserveMountStateAtDestruction>(&remainingChildWasMounted));
    root->append(std::move(parent));
    surface->mount(std::move(root));

    parentPtr->replaceChildren();

    EXPECT_FALSE(surface);
    EXPECT_FALSE(remainingChildWasMounted);
}

TEST(Fragment, RoundTripsBoundedHTML) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<p id='123:bad.id' class='primary.bad @token'>Hello &amp; <br>world</p><input type=checkbox>");

    EXPECT_EQ(root.textContent(), "Hello & world");
    EXPECT_EQ(root.innerHTML(), "<p id=\"123:bad.id\" class=\"primary.bad @token\">Hello &amp; <br>world</p><input type=\"checkbox\">");
    ASSERT_EQ(root.children().size(), 2U);
    EXPECT_EQ(root.children()[0]->id(), "123:bad.id");
    EXPECT_TRUE(root.children()[0]->classList().contains("primary.bad"));
    EXPECT_TRUE(root.children()[0]->classList().contains("@token"));
    EXPECT_EQ(root.children()[1]->elementName(), "input");
}

TEST(Fragment, DefersFlowBreakUntilNextLayoutChild) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<span>before</span><br> \n <span>after</span>");

    const auto children = root.children();
    ASSERT_EQ(children.size(), 3U);
    EXPECT_FALSE(children[0]->flowBreakBefore());
    EXPECT_EQ(children[1]->elementName(), "br");
    EXPECT_TRUE(children[2]->flowBreakBefore());
}

TEST(Fragment, RejectsScopedElementsWithoutOwner) {
    auto root = makeElementValue<HTMLPanelElement>();
    constexpr char kInvalidHTML[] = "<legend>Orphan</legend>";

    root.innerHTML(kInvalidHTML);

    ASSERT_EQ(root.childNodes().size(), 1U);
    EXPECT_EQ(root.textContent(), kInvalidHTML);
}

TEST(Fragment, ParsesLegendWithinFieldset) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<fieldset><legend id='title'>Settings <b>basic</b></legend><button>Save</button></fieldset>");

    ASSERT_EQ(root.children().size(), 1U);
    const Element* fieldset = root.children().front();
    ASSERT_EQ(fieldset->children().size(), 2U);
    EXPECT_EQ(fieldset->children()[0]->elementName(), "legend");
    EXPECT_EQ(fieldset->children()[0]->id(), "title");
    EXPECT_EQ(fieldset->children()[0]->textContent(), "Settings basic");
    EXPECT_EQ(fieldset->children()[1]->elementName(), "button");

    auto fieldsetContext = makeElementValue<Element>("fieldset");
    fieldsetContext.innerHTML("<legend>Title <strong>text</strong></legend>");
    ASSERT_EQ(fieldsetContext.children().size(), 1U);
    EXPECT_EQ(fieldsetContext.children().front()->elementName(), "legend");
    EXPECT_EQ(fieldsetContext.children().front()->textContent(), "Title text");

    constexpr char kDuplicateLegends[] = "<legend>One</legend><legend>Two</legend>";
    fieldsetContext.innerHTML(kDuplicateLegends);
    EXPECT_EQ(fieldsetContext.textContent(), kDuplicateLegends);

    constexpr char kInvalidHTML[] = "<legend><div>Invalid</div></legend>";
    fieldsetContext.innerHTML(kInvalidHTML);
    EXPECT_EQ(fieldsetContext.textContent(), kInvalidHTML);

    auto legendContext = makeElementValue<Element>("legend");
    legendContext.innerHTML("<strong>Allowed</strong>");
    ASSERT_EQ(legendContext.children().size(), 1U);
    EXPECT_EQ(legendContext.children().front()->elementName(), "strong");

    constexpr char kInvalidLegendContent[] = "<div>Invalid</div>";
    legendContext.innerHTML(kInvalidLegendContent);
    EXPECT_EQ(legendContext.textContent(), kInvalidLegendContent);
}

TEST(Fragment, InnerHTMLReplacesExistingChildren) {
    auto root = makeElementValue<HTMLPanelElement>();
    Node* oldChild = root.append(makeElement<HTMLLabelElement>("old"));
    ASSERT_NE(oldChild, nullptr);
    const ElementRef<Element> oldChildRef(oldChild->asElement());
    root.append(makeElement<HTMLLabelElement>("tail"));

    root.innerHTML("<button id='new'>new</button>");

    EXPECT_FALSE(oldChildRef);
    ASSERT_EQ(root.children().size(), 1U);
    EXPECT_EQ(root.children().front()->elementName(), "button");
    EXPECT_EQ(root.children().front()->textContent(), "new");
}

TEST(Fragment, KeepsSlashInUnquotedValue) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<input name=mode/>");

    EXPECT_EQ(root.innerHTML(), "<input type=\"text\" name=\"mode/\">");
}

TEST(Fragment, AppliesInputTypeFirst) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<input checked type=checkbox>");

    ASSERT_EQ(root.children().size(), 1U);
    const auto* input = dynamic_cast<const HTMLInputElement*>(root.children().front());
    ASSERT_NE(input, nullptr);
    EXPECT_TRUE(input->checked());
}

TEST(Fragment, TreatsBooleanAttributesAsPresence) {
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

TEST(Fragment, AppliesAuthoredEventAttributes) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<button onClick='activate()'>Save</button>");

    ASSERT_EQ(root.children().size(), 1U);
    const auto* button = dynamic_cast<const HTMLButtonElement*>(root.children().front());
    ASSERT_NE(button, nullptr);
    ASSERT_NE(Core::eventHandlerCall(*button, kClickEvent), nullptr);
    EXPECT_EQ(Core::eventHandlerCall(*button, kClickEvent)->name(), "activate");
}

TEST(Fragment, RejectsIncompatibleElementAttributes) {
    auto root = makeElementValue<HTMLPanelElement>();
    constexpr char kInvalidHTML[] = "<input type=text checked>";

    root.innerHTML(kInvalidHTML);

    EXPECT_EQ(root.childNodes().size(), 1U);
    EXPECT_EQ(root.textContent(), kInvalidHTML);
}

TEST(Fragment, SerializesOwnedParts) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto input = makeElement<HTMLInputElement>();
    input->type("checkbox").switchMode(true);
    root.append(std::move(input));

    EXPECT_EQ(root.innerHTML(), "<input type=\"checkbox\" switch>");
}

TEST(Fragment, SerializesStoredAttributes) {
    auto root = makeElementValue<Element>("section");
    auto child = makeElement<Element>("p");
    child->setAttribute("data-state", "ready");
    root.append(std::move(child));

    EXPECT_EQ(root.innerHTML(), "<p data-state=\"ready\"></p>");
}

TEST(Fragment, TreatsMalformedHTMLAsLiteralText) {
    auto root = makeElementValue<HTMLPanelElement>();

    root.innerHTML("<p>unclosed");

    EXPECT_EQ(root.textContent(), "<p>unclosed");
    EXPECT_EQ(root.innerHTML(), "&lt;p&gt;unclosed");

    root.innerHTML("<p/>");

    EXPECT_EQ(root.textContent(), "<p/>");
    EXPECT_EQ(root.innerHTML(), "&lt;p/&gt;");

    root.innerHTML("<input checked=\"true\"type=checkbox>");

    EXPECT_EQ(root.textContent(), "<input checked=\"true\"type=checkbox>");
    EXPECT_EQ(root.innerHTML(), "&lt;input checked=\"true\"type=checkbox&gt;");

    constexpr const char* kInvalidFlowBreaks[] = {
        "<br>leading",
        " \n<br>leading",
        "before<br><br>after",
        "before<br>",
        "<span><br>leading</span>",
        "<span>before<br><br>after</span>",
        "<span>before<br></span>",
    };
    for (const char* html : kInvalidFlowBreaks) {
        SCOPED_TRACE(html);
        root.innerHTML(html);

        ASSERT_EQ(root.childNodes().size(), 1U);
        ASSERT_NE(root.childNodes().front()->asText(), nullptr);
        EXPECT_EQ(root.textContent(), html);
    }
}

TEST(Element, RejectsAmbiguousLabelTarget) {
    auto root = makeElementValue<HTMLPanelElement>();
    root.innerHTML("<input id=target><label for=target>Target</label>");

    ASSERT_EQ(root.children().size(), 2U);
    auto* label = dynamic_cast<HTMLLabelElement*>(root.children()[1]);
    ASSERT_NE(label, nullptr);
    EXPECT_TRUE(label->defaultPointerEvents());

    auto duplicate = makeElement<HTMLInputElement>();
    HTMLInputElement* duplicatePtr = duplicate.get();
    duplicate->setId("target");
    root.append(std::move(duplicate));
    EXPECT_FALSE(label->defaultPointerEvents());

    NodePtr detached = duplicatePtr->remove();
    ASSERT_NE(detached, nullptr);
    EXPECT_TRUE(label->defaultPointerEvents());
}

TEST(HTMLNames, KeepsVoidnessInTheHTMLVocabulary) {
    EXPECT_TRUE(isVoidHTMLTag(HTMLTag::Br));
    EXPECT_TRUE(isVoidHTMLTag(HTMLTag::Input));
    EXPECT_FALSE(isVoidHTMLTag(HTMLTag::Div));
}

TEST(HTMLNames, UsesIForCSSBackedIcons) {
    EXPECT_EQ(findHTMLTag("i"), HTMLTag::I);
    EXPECT_EQ(findHTMLTag("icon"), HTMLTag::Unknown);
}

TEST(Element, ExposesConstChildren) {
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

TEST(Document, AdoptsDetachedChildren) {
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
    EXPECT_EQ(documentElement->ownerDocument(), &document);
    EXPECT_EQ(documentElement->parentNode(), static_cast<Core::Node*>(&document));
    EXPECT_EQ(documentElement->parentElement(), nullptr);
    ASSERT_EQ(document.getElementById("root"), documentElement);
    auto buttonOwner = document.createElement("button");
    ASSERT_NE(buttonOwner.get(), nullptr);
    buttonOwner->setId("button");
    Node* buttonNode = documentElement->append(std::move(buttonOwner));
    ASSERT_NE(buttonNode, nullptr);
    Element* button = buttonNode->asElement();
    ASSERT_NE(button, nullptr);

    EXPECT_EQ(button->ownerDocument(), &document);
    EXPECT_EQ(document.getElementById("button"), button);
    EXPECT_EQ(button->parentElement(), documentElement);

    NodePtr detached = button->remove();
    ASSERT_EQ(detached.get(), button);
    EXPECT_EQ(button->parentElement(), nullptr);
    EXPECT_EQ(document.getElementById("button"), nullptr);

    ASSERT_EQ(documentElement->append(std::move(detached)), button);
    EXPECT_EQ(document.getElementById("button"), button);
}

TEST(Document, CreatesElements) {
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

TEST(Document, ReparentsDetachedElements) {
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

TEST(Document, AdoptsCrossDocumentChild) {
    auto firstRootOwner = makeElement<HTMLPanelElement>();
    Document first(std::move(firstRootOwner));
    auto secondRootOwner = makeElement<HTMLPanelElement>();
    Document second(std::move(secondRootOwner));
    auto childOwner = first.createElement("button");
    childOwner->setId("adopted");
    Element* child = childOwner.get();

    NodePtr childNode = second.adoptNode(std::move(childOwner));
    ASSERT_EQ(second.documentElement()->append(std::move(childNode)), child);
    EXPECT_EQ(child->parentElement(), second.documentElement());
    EXPECT_EQ(first.getElementById("adopted"), nullptr);
    EXPECT_EQ(second.getElementById("adopted"), child);
}

TEST(ElementTreeDeathTest, RejectsCrossDocumentInsertionWithoutAdoption) {
    auto firstRootOwner = makeElement<HTMLPanelElement>();
    Document first(std::move(firstRootOwner));
    auto secondRootOwner = makeElement<HTMLPanelElement>();
    Document second(std::move(secondRootOwner));

    EXPECT_DEATH(
        {
            LLError::setFatalFunction(reportFatalDiagnostic);
            auto child = first.createElement("button");
            second.documentElement()->append(std::move(child));
        },
        ".*");
}

TEST(Document, AdoptsDetachedSubtrees) {
    auto firstRootOwner = makeElement<HTMLPanelElement>();
    Document first(std::move(firstRootOwner));
    auto secondRootOwner = makeElement<HTMLPanelElement>();
    Document second(std::move(secondRootOwner));

    auto subtree = first.createElement("panel");
    auto descendant = first.createElement("button");
    descendant->setId("nested-adopted");
    Element* subtreeElement = subtree.get();
    Element* descendantElement = descendant.get();
    subtree->append(std::move(descendant));

    NodePtr subtreeNode = std::move(subtree);
    NodePtr adopted = second.adoptNode(std::move(subtreeNode));

    EXPECT_EQ(subtreeElement->ownerDocument(), &second);
    EXPECT_EQ(descendantElement->ownerDocument(), &second);
    ASSERT_EQ(second.documentElement()->append(std::move(adopted)), subtreeElement);
    EXPECT_EQ(second.getElementById("nested-adopted"), descendantElement);
}

TEST(Document, FindsFirstDuplicateId) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));
    Element* documentElement = document.documentElement();
    ASSERT_NE(documentElement, nullptr);

    auto firstOwner = document.createElement("button");
    auto secondOwner = document.createElement("button");
    firstOwner->setId("duplicate");
    secondOwner->setId("duplicate");
    Element* first = firstOwner.get();
    Element* second = secondOwner.get();
    documentElement->append(std::move(firstOwner));
    documentElement->append(std::move(secondOwner));

    EXPECT_EQ(document.getElementById("duplicate"), first);
    NodePtr detached = first->remove();
    ASSERT_EQ(detached.get(), first);
    EXPECT_EQ(document.getElementById("duplicate"), second);
}

TEST(Document, PreservesDuplicateIdLookup) {
    auto targetRootOwner = makeElement<HTMLPanelElement>();
    Document target(std::move(targetRootOwner));
    Element* targetRoot = target.documentElement();
    ASSERT_NE(targetRoot, nullptr);

    auto firstOwner = target.createElement("button");
    auto secondOwner = target.createElement("button");
    auto thirdOwner = target.createElement("button");
    firstOwner->setId("duplicate");
    secondOwner->setId("duplicate");
    thirdOwner->setId("duplicate");
    Element* first = firstOwner.get();
    Element* second = secondOwner.get();
    Element* third = thirdOwner.get();
    targetRoot->append(std::move(firstOwner));
    targetRoot->append(std::move(secondOwner));
    targetRoot->append(std::move(thirdOwner));
    EXPECT_EQ(target.getElementById("duplicate"), first);

    NodePtr detachedFirst = first->remove();
    ASSERT_EQ(detachedFirst.get(), first);
    EXPECT_EQ(target.getElementById("duplicate"), second);
    ASSERT_EQ(targetRoot->append(std::move(detachedFirst)), first);
    EXPECT_EQ(target.getElementById("duplicate"), second);

    auto replacementOwner = target.createElement("button");
    replacementOwner->setId("duplicate");
    Element* replacement = replacementOwner.get();
    NodePtr detachedSecond = second->replaceWith(std::move(replacementOwner));
    ASSERT_EQ(detachedSecond.get(), second);
    EXPECT_EQ(target.getElementById("duplicate"), replacement);

    auto sourceRootOwner = makeElement<HTMLPanelElement>();
    Document source(std::move(sourceRootOwner));
    auto adoptedOwner = source.createElement("button");
    adoptedOwner->setId("duplicate");
    Element* adopted = adoptedOwner.get();
    source.documentElement()->append(std::move(adoptedOwner));
    NodePtr adoptedNode = adopted->remove();
    adoptedNode = target.adoptNode(std::move(adoptedNode));
    ASSERT_EQ(targetRoot->prepend(std::move(adoptedNode)), adopted);
    EXPECT_EQ(source.getElementById("duplicate"), nullptr);
    EXPECT_EQ(target.getElementById("duplicate"), adopted);
    EXPECT_NE(target.getElementById("duplicate"), third);
}

TEST(Document, RemovesDocumentElement) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    NodePtr detached = document.documentElement()->remove();
    ASSERT_NE(detached, nullptr);
    EXPECT_EQ(document.documentElement(), nullptr);
    EXPECT_EQ(detached->parentNode(), nullptr);
}

TEST(Document, ReturnsNullForUnknownRuntimeElement) {
    auto documentElementOwner = makeElement<HTMLPanelElement>();
    Document document(std::move(documentElementOwner));

    EXPECT_EQ(document.createElement("not-a-radia-element"), nullptr);
}

TEST(Element, NormalizesAdjacentTextNodes) {
    auto root = makeElementValue<Element>("p");
    appendText(root, "before");
    appendText(root, "after");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), "beforeafter");
}

TEST(ElementVisit, SeparatesMountObservations) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto childOwner = makeElement<HTMLPanelElement>();
    Element* child = childOwner.get();
    root.append(std::move(childOwner));

    const ElementVisit initial(*child);
    EXPECT_TRUE(initial.objectAlive());
    EXPECT_TRUE(initial.mountValid());
    EXPECT_TRUE(initial.topologyValid());
    EXPECT_TRUE(initial.styleValid());
    EXPECT_TRUE(initial.layoutValid());

    child->setId("changed-id");
    EXPECT_TRUE(initial.objectAlive());
    EXPECT_TRUE(initial.mountValid());
    EXPECT_TRUE(initial.topologyValid());
    EXPECT_FALSE(initial.layoutValid());
    EXPECT_FALSE(initial.styleValid());

    const ElementVisit beforeLayoutChange(*child);
    child->setRect({1.f, 2.f, 3.f, 4.f});
    EXPECT_TRUE(beforeLayoutChange.objectAlive());
    EXPECT_TRUE(beforeLayoutChange.mountValid());
    EXPECT_TRUE(beforeLayoutChange.topologyValid());
    EXPECT_TRUE(beforeLayoutChange.styleValid());
    EXPECT_FALSE(beforeLayoutChange.layoutValid());

    auto other = makeElementValue<HTMLPanelElement>();
    NodePtr detached = child->remove();
    ASSERT_EQ(detached.get(), child);
    other.append(std::move(detached));
    EXPECT_TRUE(initial.objectAlive());
    EXPECT_TRUE(initial.mountValid());
    EXPECT_FALSE(initial.topologyValid());
    EXPECT_FALSE(initial.layoutValid());

    ElementVisit dead;
    {
        auto temporary = makeElementValue<HTMLPanelElement>();
        dead = ElementVisit(temporary);
    }
    EXPECT_FALSE(dead.objectAlive());
}

TEST(ElementVisit, KeepsLayoutObservationCurrent) {
    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("panel:hover { color: #ffffff; }").ok());
    EXPECT_FALSE(styleSheet.pseudoClassAffectsLayout(PseudoClass::Hover));

    Surface surface(styleSheet);
    surface.setViewport(100.f, 100.f);
    auto panelOwner = makeElement<HTMLPanelElement>();
    Element* panel = panelOwner.get();
    panelOwner->setRect({0.f, 0.f, 100.f, 100.f}).setPointerEvents(true);
    surface.mount(std::move(panelOwner));
    surface.updateLayout();

    const ElementVisit observation(*panel);
    surface.pointerMove({{5.f, 5.f}});
    ASSERT_TRUE(panel->hovered());
    EXPECT_TRUE(observation.objectAlive());
    EXPECT_TRUE(observation.mountValid());
    EXPECT_TRUE(observation.topologyValid());
    EXPECT_TRUE(observation.layoutValid());
    EXPECT_FALSE(observation.styleValid());
}

TEST(ElementVisit, RejectsSiblingOrderChanges) {
    auto root = makeElementValue<HTMLPanelElement>();
    auto firstOwner = makeElement<HTMLPanelElement>();
    auto secondOwner = makeElement<HTMLPanelElement>();
    Element* first = firstOwner.get();
    Element* second = secondOwner.get();
    root.append(std::move(firstOwner));
    root.append(std::move(secondOwner));

    const ElementVisit observation(*second);
    NodePtr detached = first->remove();
    ASSERT_EQ(detached.get(), first);
    ASSERT_EQ(root.append(std::move(detached)), first);

    EXPECT_TRUE(observation.objectAlive());
    EXPECT_FALSE(observation.topologyValid());
}

TEST(ElementVisit, RejectsRemountAsNewMount) {
    Surface surface;
    auto rootOwner = makeElement<HTMLPanelElement>();
    Element* root = rootOwner.get();
    surface.mount(std::move(rootOwner));

    const ElementVisit observation(*root);
    std::unique_ptr<Element> detached = surface.unmount(*root);
    ASSERT_EQ(detached.get(), root);
    Element& remounted = surface.mount(std::move(detached));
    ASSERT_EQ(&remounted, root);

    EXPECT_TRUE(observation.objectAlive());
    EXPECT_TRUE(observation.topologyValid());
    EXPECT_FALSE(observation.mountValid());
}

TEST(Element, StoresLiteralTextContent) {
    auto root = makeElementValue<Element>("p");
    root.textContent("beforeafter");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), "beforeafter");
}

TEST(Element, EmptyTextContentInstallsEmptyTextNode) {
    auto root = makeElementValue<Element>("p");
    root.textContent("content");
    root.textContent("");

    ASSERT_EQ(root.childNodes().size(), 1U);
    ASSERT_NE(root.childNodes().front()->asText(), nullptr);
    EXPECT_TRUE(root.childNodes().front()->asText()->data().empty());
}

TEST(Element, PreservesWhitespaceOnlyTextNodes) {
    auto root = makeElementValue<Element>("p");
    root.textContent(" \t\n");

    const auto runtimeChildren = nodes(root);
    ASSERT_EQ(runtimeChildren.size(), 1U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), " \t\n");
}

TEST(Element, UpdatesOwnerText) {
    auto root = makeElementValue<Element>("p");
    Node* textNode = root.append(std::make_unique<Text>("before"));
    ASSERT_NE(textNode, nullptr);
    Text* text = textNode->asText();
    ASSERT_NE(text, nullptr);

    text->setData("after");

    EXPECT_EQ(text->data(), "after");
    EXPECT_EQ(root.textContent(), "after");
}

TEST(ElementPaint, RecordsElementOwnPrimitives) {
    RecordingPaintContext recording;
    ComputedStyle style;

    auto label = makeElementValue<HTMLLabelElement>("hello");
    label.setRect({1.f, 2.f, 30.f, 10.f});
    label.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 0U);

    auto icon = makeElementValue<Element>("i");
    icon.classList().add("i-search");
    icon.setRect({4.f, 5.f, 16.f, 16.f});
    icon.paint(recording, style, 2.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 2U);
}

TEST(ElementPaint, DoesNotPaintBorderWhenStyleIsOmitted) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("label { border: 3px #123456; }").ok());

    auto label = makeElementValue<HTMLLabelElement>("hello");
    label.setRect({0.f, 0.f, 40.f, 12.f});
    Pass styles(stylesheet, fixedTextMeasurer());
    const ComputedStyle style = styles.style(label);

    EXPECT_EQ(style.borderWidth().top.pixels, 3.f);
    EXPECT_EQ(style.borderStyle().top, Core::Style::BorderStyle::NoneValue);
    RecordingPaintContext recording;
    label.paint(recording, style, 1.f);
    const PaintCommand* box = recording.last(PaintCommandKind::Box);
    ASSERT_NE(box, nullptr);
    EXPECT_EQ(box->style.borderStyle().top, Core::Style::BorderStyle::NoneValue);
}

TEST(ElementPaint, PaintsLocalizedResources) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {strings: {}}, "
                                     "ar: {strings: {}}}\n";
    constexpr char kStyles[] = "panel { opacity: .5; backdrop-filter: blur(3px); filter: blur(4px); } "
                               "label { opacity: .5; text-align: start; } i { size: 16px; }";
    constexpr char kSearchIconSvg[] = "<svg viewBox=\"0 0 24 24\">"
                                      "<path d=\"M2 2 L22 22\"/></svg>";
    resources.add("localization.yaml", kLocalization);
    resources.add("skin.css", kStyles);
    resources.add("resources/icons/search.svg", kSearchIconSvg);
    SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(resources));
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));
    ASSERT_TRUE(system.setLocale("ar"));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMeasurer());
    surface->setViewport(100.f, 100.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto label = makeElement<HTMLLabelElement>("hello");
    label->setRect({0.f, 20.f, 30.f, 10.f});
    panel->append(std::move(label));
    auto icon = makeElement<Element>("i");
    icon->classList().add("i-search");
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
    const PaintCommand* textCommand = recording.last(PaintCommandKind::Text);
    const PaintCommand* iconBox = recording.last(PaintCommandKind::Box);
    const PaintCommand* effectCommand = recording.last(PaintCommandKind::BeginEffects);
    ASSERT_NE(textCommand, nullptr);
    ASSERT_NE(iconBox, nullptr);
    ASSERT_NE(effectCommand, nullptr);
    EXPECT_EQ(effectCommand->scale, 2.f);
    EXPECT_EQ(effectCommand->style.backdropFilter().operations.size(), 1U);
    EXPECT_EQ(effectCommand->style.filter.operations.size(), 1U);
    EXPECT_EQ(textCommand->style.color().resolvedColor().a, .25f);
    EXPECT_EQ(textCommand->style.textAlign(), TextAlign::Left);
    EXPECT_EQ(textCommand->rect.x, -8.f);
    EXPECT_EQ(recording.commands().front().kind, PaintCommandKind::BeginFrame);
    EXPECT_EQ(recording.commands().back().kind, PaintCommandKind::EndFrame);
    EXPECT_LT(textCommand, iconBox);
    EXPECT_FALSE(surface->needsPaint());
}

TEST(ElementPaint, PaintsSourceOrder) {
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
        if (command.kind == PaintCommandKind::Text)
            painted.push_back(command.text);
    const std::vector<std::string> expectedPainted {"before ", "bold", " after"};
    EXPECT_EQ(painted, expectedPainted);
}

TEST(TextLayout, PreservesFittingShapedLines) {
    class ShapedRunMetrics final : public TextMeasurer {
    public:
        Vec2 measureText(const std::string& value, const ComputedStyle&) const override {
            if (value == "Radia UI Demo")
                return {50.f, 10.f};
            if (value == "Radia UI")
                return {28.f, 10.f};
            if (value == " ")
                return {6.f, 10.f};
            if (value == "Demo")
                return {20.f, 10.f};
            return {0.f, 10.f};
        }
        std::uint64_t generation() const noexcept override { return 1; }
    } shapedMetrics;

    TextLayoutTestElement naturallySized("Radia UI Demo");
    naturallySized.setRect({0.f, 0.f, 50.f, 10.f});
    RecordingPaintContext recording(shapedMetrics);
    naturallySized.paint(recording, ComputedStyle {}, 1.f);

    ASSERT_EQ(recording.count(PaintCommandKind::Text), 1U);
    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].text, "Radia UI Demo");
}

TEST(TextLayout, EllipsizesAccordingToTextDirection) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    style.setTextOverflow(TextOverflow::EllipsisCenter);
    style.setOverflowX(Overflow::Hidden);

    TextLayoutTestElement inventory("abcdefghij");
    inventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext centered(metrics);
    inventory.paint(centered, style, 1.f);
    EXPECT_EQ(paintedText(centered), "abc\xE2\x80\xA6hij");

    style.setTextOverflow(TextOverflow::Ellipsis);
    RecordingPaintContext ended(metrics);
    inventory.paint(ended, style, 1.f);
    EXPECT_EQ(paintedText(ended), "abcdef\xE2\x80\xA6");

    style.direction = Direction::RightToLeft;
    TextLayoutTestElement rtlInventory("ابتثجحخدذر");
    rtlInventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext rtlEnded(metrics);
    rtlInventory.paint(rtlEnded, style, 1.f);
    ASSERT_GE(rtlEnded.commands().size(), 3U);
    EXPECT_EQ(rtlEnded.commands()[1].text, "\xE2\x80\xA6");
    EXPECT_EQ(rtlEnded.commands()[2].text, "ابتثجح");
}

TEST(TextLayout, PreservesGraphemes) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    style.setTextOverflow(TextOverflow::Ellipsis);
    style.setOverflowX(Overflow::Hidden);

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

TEST(TextLayout, ClipsOverflowWithoutRewritingText) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    style.setTextOverflow(TextOverflow::Clip);
    style.setOverflowX(Overflow::Hidden);

    TextLayoutTestElement clipped("abc");
    clipped.setRect({0.f, 0.f, 2.f, 10.f});
    RecordingPaintContext recording(metrics);
    clipped.paint(recording, style, 1.f);

    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].text, "abc");
}

TEST(TextLayout, WrapsAtTextBoundaries) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextOverflow(TextOverflow::Clip);
    ASSERT_EQ(style.textWrapMode(), TextWrapMode::Wrap);

    TextLayoutTestElement wrapped("alpha beta");
    wrapped.setRect({0.f, 0.f, 30.f, 20.f});
    RecordingPaintContext wrapping(metrics);
    wrapped.paint(wrapping, style, 1.f);
    ASSERT_EQ(wrapping.count(PaintCommandKind::Text), 2U);
    ASSERT_GE(wrapping.commands().size(), 3U);
    EXPECT_EQ(wrapping.commands()[1].text, "alpha");
    EXPECT_EQ(wrapping.commands()[2].text, "beta");

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
    EXPECT_EQ(shapedWrapping.commands()[1].text, "alpha beta");
    EXPECT_EQ(shapedWrapping.commands()[2].text, "gamma");

    TextLayoutTestElement cjk("你好世界");
    cjk.setRect({0.f, 0.f, 10.f, 20.f});
    RecordingPaintContext cjkWrapping(metrics);
    cjk.paint(cjkWrapping, style, 1.f);
    EXPECT_EQ(cjkWrapping.count(PaintCommandKind::Text), 2U);

    style.setTextOverflow(TextOverflow::Clip);
    style.setWidth(Dimension::fromPixels(30.f));
    EXPECT_EQ(wrapped.intrinsicSize(StyleSheet(), style, metrics).y, 20.f);
}

TEST(TextLayout, AppliesBalanceAndPrettyWrapStyles) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextOverflow(TextOverflow::Clip);

    TextLayoutTestElement autoWrapped("alpha beta gamma delta epsilon");
    autoWrapped.setRect({0.f, 0.f, 50.f, 100.f});
    RecordingPaintContext autoRecording(metrics);
    autoWrapped.paint(autoRecording, style, 1.f);

    style.setTextWrapStyle(TextWrapStyle::Balance);
    TextLayoutTestElement balanced("alpha beta gamma delta epsilon");
    balanced.setRect({0.f, 0.f, 50.f, 100.f});
    RecordingPaintContext balanceRecording(metrics);
    balanced.paint(balanceRecording, style, 1.f);

    style.setTextWrapStyle(TextWrapStyle::Pretty);
    TextLayoutTestElement pretty("alpha beta gamma delta epsilon");
    pretty.setRect({0.f, 0.f, 50.f, 100.f});
    RecordingPaintContext prettyRecording(metrics);
    pretty.paint(prettyRecording, style, 1.f);

    EXPECT_GT(autoRecording.count(PaintCommandKind::Text), 1U);
    EXPECT_EQ(balanceRecording.count(PaintCommandKind::Text), autoRecording.count(PaintCommandKind::Text));
    EXPECT_EQ(prettyRecording.count(PaintCommandKind::Text), autoRecording.count(PaintCommandKind::Text));
    const auto textRuns = [](const RecordingPaintContext& recording) {
        std::vector<std::string> result;
        for (const PaintCommand& command : recording.commands())
            if (command.kind == PaintCommandKind::Text)
                result.push_back(command.text);
        return result;
    };
    EXPECT_NE(textRuns(balanceRecording), textRuns(autoRecording));
    EXPECT_NE(textRuns(prettyRecording), textRuns(autoRecording));
}

TEST(TextLayout, UsesShapedWidthsForCenterEllipsis) {
    class VariableTextMeasurer final : public TextMeasurer {
    public:
        Vec2 measureText(const std::string& value, const ComputedStyle&) const override {
            if (value.empty())
                return {0.f, 10.f};
            if (value == "W")
                return {20.f, 10.f};
            if (value == "\xE2\x80\xA6")
                return {5.f, 10.f};
            return {
                static_cast<float>(value.size()) * 5.f,
                10.f,
            };
        }
        std::uint64_t generation() const noexcept override { return 1; }
    } variableMetrics;

    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    style.setTextOverflow(TextOverflow::EllipsisCenter);
    style.setOverflowX(Overflow::Hidden);
    TextLayoutTestElement asymmetric("Wabc");
    asymmetric.setRect({0.f, 0.f, 10.f, 10.f});
    RecordingPaintContext recording(variableMetrics);
    asymmetric.paint(recording, style, 1.f);

    EXPECT_EQ(paintedText(recording), "\u2026c");
}

TEST(TextLayout, AppliesOverflowToMountedTextNodes) {
    StyleSheet stylesheet;
    constexpr char kTextOverflowStyle[] =
        "panel { display: block; } "
        "p { width: 20px; height: 10px; font-size: 10px; line-height: 10px; text-wrap: nowrap; overflow: hidden; } "
        "p.end { text-overflow: ellipsis; } "
        "p.center { text-overflow: ellipsis-center; }";
    ASSERT_TRUE(stylesheet.loadRadia(kTextOverflowStyle).ok());

    Surface surface(stylesheet);
    surface.setViewport(20.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 20.f, 20.f});

    auto end = makeElement<Element>("p");
    end->classList().add("end");
    end->textContent("abcdef");
    panel->append(std::move(end));

    auto center = makeElement<Element>("p");
    center->classList().add("center");
    center->textContent("abcdef");
    panel->append(std::move(center));

    surface.mount(std::move(panel));
    RecordingPaintContext recording;
    surface.paint(recording);

    EXPECT_EQ(paintedText(recording), "ab\u2026a\u2026f");
}

TEST(TextLayout, PreparesPaintLayoutBeforePainting) {
    CountingTextMeasurer metrics;
    TextLayout layout("abcdef");
    auto owner = makeElement<Element>("p");
    StyleSheet stylesheet;
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    style.setTextOverflow(TextOverflow::Ellipsis);
    style.setOverflowX(Overflow::Hidden);
    resolveTextTestColors(style);

    layout.measure(metrics, style, stylesheet, *owner, 20.f);
    layout.preparePaint(metrics, style, stylesheet, *owner, 20.f);
    const std::size_t preparedMeasureCalls = metrics.measureCalls();

    RecordingPaintContext recording(metrics);
    const TextPaintStyle paintStyle {style.color().resolvedColor(), style.textDecoration(), style.textAlign()};
    layout.paintPrepared(recording, {0.f, 0.f, 20.f, 10.f}, style, paintStyle, &stylesheet, *owner);

    EXPECT_EQ(metrics.measureCalls(), preparedMeasureCalls);
}

TEST(TextLayout, RebuildsAfterContentChange) {
    FixedTextMeasurer metrics(1.f, 1.f);
    TextLayout layout("old");
    auto owner = makeElement<Element>("p");
    StyleSheet stylesheet;
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    resolveTextTestColors(style);

    layout.preparePaint(metrics, style, stylesheet, *owner, 100.f);
    layout.setText("new");

    RecordingPaintContext recording(metrics);
    const TextPaintStyle paintStyle {style.color().resolvedColor(), style.textDecoration(), style.textAlign()};
    layout.paintPrepared(recording, {0.f, 0.f, 100.f, 10.f}, style, paintStyle, &stylesheet, *owner);

    EXPECT_EQ(paintedText(recording), "new");
}

TEST(TextLayout, RebuildsForPaintMetrics) {
    FixedTextMeasurer narrowMetrics(.5f, .5f);
    FixedTextMeasurer wideMetrics(1.f, 1.f);
    TextLayout layout("abc");
    auto owner = makeElement<Element>("p");
    StyleSheet stylesheet;
    ComputedStyle style;
    style.setFontSize(10.f);
    style.setTextWrapMode(TextWrapMode::NoWrap);
    resolveTextTestColors(style);

    layout.preparePaint(narrowMetrics, style, stylesheet, *owner, 100.f);

    RecordingPaintContext recording(wideMetrics);
    const TextPaintStyle paintStyle {style.color().resolvedColor(), style.textDecoration(), style.textAlign()};
    layout.paintPrepared(recording, {0.f, 0.f, 100.f, 10.f}, style, paintStyle, &stylesheet, *owner);

    const PaintCommand* text = recording.last(PaintCommandKind::Text);
    ASSERT_NE(text, nullptr);
    EXPECT_FLOAT_EQ(text->rect.w, 30.f);
}

TEST(TextLayout, ProjectsPaintColor) {
    StyleSheet stylesheet;
    ASSERT_TRUE(
        stylesheet.loadRadia("panel { display: block; } p { width: 40px; height: 10px; color: #ff0000; } p.accent { color: #0000ff; }")
            .ok());

    Surface surface(stylesheet);
    surface.setViewport(40.f, 20.f);
    auto panel = makeElement<HTMLPanelElement>();
    panel->setRect({0.f, 0.f, 40.f, 20.f});
    auto paragraph = makeElement<Element>("p");
    Element* paragraphPtr = paragraph.get();
    paragraph->textContent("abcdef");
    panel->append(std::move(paragraph));
    surface.mount(std::move(panel));

    RecordingPaintContext first;
    surface.paint(first);
    paragraphPtr->classList().add("accent");
    RecordingPaintContext second;
    surface.paint(second);

    const PaintCommand* firstText = first.last(PaintCommandKind::Text);
    const PaintCommand* secondText = second.last(PaintCommandKind::Text);
    ASSERT_NE(firstText, nullptr);
    ASSERT_NE(secondText, nullptr);
    EXPECT_FLOAT_EQ(firstText->style.color().resolvedColor().r, 1.f);
    EXPECT_FLOAT_EQ(secondText->style.color().resolvedColor().b, 1.f);
    EXPECT_EQ(firstText->rect.x, secondText->rect.x);
    EXPECT_EQ(firstText->rect.y, secondText->rect.y);
    EXPECT_EQ(firstText->rect.w, secondText->rect.w);
    EXPECT_EQ(firstText->rect.h, secondText->rect.h);
}

TEST(TextLayout, AppliesTextSpacing) {
    const FixedTextMeasurer metrics(.5f, .5f);
    ComputedStyle style;
    style.setFontSize(10.f);

    style.setLetterSpacing(LetterSpacing {2.f});
    EXPECT_EQ(metrics.measureText("abc", style).x, 19.f);
    style.setLetterSpacing(LetterSpacing {0.f, .5f});
    EXPECT_EQ(metrics.measureText("abc", style).x, 25.f);

    style.setLetterSpacing({});
    style.setWordSpacing(WordSpacing {3.f});
    EXPECT_EQ(metrics.measureText("a b", style).x, 18.f);
    style.setWordSpacing(WordSpacing {0.f, .5f});
    EXPECT_EQ(metrics.measureText("a b", style).x, 20.f);
    style.setWordSpacing(WordSpacing {3.f});
    EXPECT_EQ(metrics.measureText("a\u2003b", style).x, 18.f);
}

TEST(SwitchElement, UpdatesSwitchThumb) {
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
