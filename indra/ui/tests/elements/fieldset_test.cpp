/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <functional>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <string>
#include <utility>
#include "../layout/test_layout_helpers.h"
#include "binding/binder.h"
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "layout/document.h"
#include "layout/engine.h"
#include "layout/resourcecompiler.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::CompositePartDefinition;
using radia::ui::Element;
using radia::ui::ElementDefinition;
using radia::ui::ElementRef;
using radia::ui::Event;
using radia::ui::EventCall;
using radia::ui::FixedTextMetrics;
using radia::ui::InputElement;
using radia::ui::LabelElement;
using radia::ui::LayoutBuildContext;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutDirection;
using radia::ui::LayoutResourceCompiler;
using radia::ui::layoutTree;
using radia::ui::LocalizationCatalog;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PanelElement;
using radia::ui::PreparedBindingResult;
using radia::ui::resolveElementStyle;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Visibility;
using radia::ui::detail::ElementCompilerAccess;
using radia::ui::detail::findElementInScope;
using radia::ui::detail::instantiateCompositePart;
using radia::ui::detail::instantiateCompositeParts;
using radia::ui::detail::makeEventRegistration;
using radia::ui::test::LayoutCompilerTestHelper;
using ::testing::Message;

void bindChangeEvent(Binder& binder, std::string name, std::function<void(const Event&)> callback) {
    binder.event(makeEventRegistration(
        std::move(name), [callback = std::move(callback)](Event& event, const EventCall&) { callback(event); },
        [](const EventCall& call) { return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch"; }));
}

class FieldsetTest : public ::testing::Test {
protected:
    template<typename ElementT> ElementRef<ElementT> requireElement(Element& root, const std::string& id) const {
        return ElementRef<ElementT>(dynamic_cast<ElementT*>(findElementInScope(root, id)));
    }

    LayoutCompilerTestHelper factory;
    std::map<std::string, std::string>& resources = factory.resources;
};
} // namespace

TEST_F(FieldsetTest, PreservesInlineElementStructureAcrossTextAndLabels) {
    constexpr char kInlineLayout[] = "<panel><p id=\"copy\">before <b>bold<i>both</i></b><br/>"
                                     "<i>after</i></p><p id=\"title\">Title</p></panel>";
    constexpr char kLabelInlineLayout[] = "<panel><label id=\"label\" for=\"target\">name <b>important</b>"
                                          "<br/><i>detail</i></label><input type=\"checkbox\" switch=\"true\" id=\"target\"/></panel>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kInlineLayout, "inline.xml");
    ASSERT_TRUE(result.ok());

    const ElementRef<Element> text = requireElement<Element>(*result.document->documentElement(), "copy");
    const ElementRef<Element> title = requireElement<Element>(*result.document->documentElement(), "title");
    ASSERT_TRUE(text);
    ASSERT_TRUE(title);
    EXPECT_EQ(text->textContent(), "before boldboth\nafter");
    ASSERT_EQ(text->children().size(), 3U);
    EXPECT_EQ(text->children()[0]->elementName(), "b");
    EXPECT_EQ(text->children()[0]->textContent(), "boldboth");
    EXPECT_EQ(text->children()[1]->elementName(), "br");
    EXPECT_EQ(text->children()[2]->elementName(), "i");
    EXPECT_EQ(text->children()[2]->textContent(), "after");
    EXPECT_TRUE(title->children().empty());
    EXPECT_EQ(title->textContent(), "Title");

    const LayoutBuildResult labelResult = factory.buildElementTreeFromString(kLabelInlineLayout, "label-inline.xml");
    ASSERT_TRUE(labelResult.ok());
    const ElementRef<LabelElement> label = requireElement<LabelElement>(*labelResult.document->documentElement(), "label");
    ASSERT_TRUE(label);
    ASSERT_EQ(label->children().size(), 3U);
    EXPECT_EQ(label->children()[0]->elementName(), "b");
    EXPECT_EQ(label->children()[1]->elementName(), "br");
    EXPECT_EQ(label->children()[2]->elementName(), "i");
    EXPECT_EQ(label->textContent(), "name important\ndetail");
}

TEST_F(FieldsetTest, LocalizesAndDecoratesInlineElements) {
    LocalizationCatalog localization;
    constexpr char kInlineLocalization[] = "defaultLocale: en\n"
                                           "locales: {en: {strings: "
                                           "{inlineExample: \"First <b>Second</b>\"}}}\n";
    ASSERT_FALSE(localization.loadYaml(kInlineLocalization).hasErrors());
    const LayoutBuildContext context(localization, "en");
    constexpr char kLocalizedTextLayout[] = "<p>{{inlineExample}}</p>";
    constexpr char kDecorationLayout[] = "<p><s>outdated</s> "
                                         "<kbd shortcut=\"toggle-fly\"/></p>";
    constexpr char kUnsupportedHeadingLayout[] = "<heading>Title</heading>";
    const LayoutBuildResult localizedResult =
        LayoutResourceCompiler().buildElementTreeFromString(kLocalizedTextLayout, "localized-inline.xml", &context);
    ASSERT_TRUE(localizedResult.ok());

    const Element* localized = localizedResult.rootAs<Element>();
    ASSERT_NE(localized, nullptr);
    EXPECT_EQ(localized->textContent(), "First Second");
    ASSERT_EQ(localized->children().size(), 1U);
    EXPECT_EQ(localized->children()[0]->elementName(), "b");
    EXPECT_EQ(localized->children()[0]->textContent(), "Second");

    const LayoutBuildResult decoration = factory.buildElementTreeFromString(kDecorationLayout, "decoration.xml");
    ASSERT_TRUE(decoration.ok());
    const Element* decorated = decoration.rootAs<Element>();
    ASSERT_NE(decorated, nullptr);
    ASSERT_EQ(decorated->children().size(), 2U);
    EXPECT_EQ(decorated->children()[0]->elementName(), "s");
    EXPECT_EQ(decorated->children()[0]->textContent(), "outdated");
    EXPECT_EQ(decorated->children()[1]->elementName(), "kbd");
    EXPECT_EQ(decorated->children()[1]->textContent(), "");
    EXPECT_FALSE(factory.buildElementTreeFromString(kUnsupportedHeadingLayout, "heading.xml").ok());
}

TEST_F(FieldsetTest, BuildsAllLocalizedSemanticInlineElements) {
    LocalizationCatalog localization;
    constexpr char kSemanticLocalization[] = "defaultLocale: en\n"
                                             "locales: {en: {strings: {semantic: "
                                             "'<abbr>a</abbr><b>b</b><cite>c</cite><code>d</code><dfn>e</dfn>"
                                             "<del>f</del><em>g</em><i>h</i><ins>i</ins><mark>j</mark>"
                                             "<q>k</q><s>l</s><small>m</small><strong>n</strong><u>o</u>'}}}\n";
    ASSERT_FALSE(localization.loadYaml(kSemanticLocalization).hasErrors());
    const LayoutBuildContext context(localization, "en");
    const LayoutBuildResult result = LayoutResourceCompiler().buildElementTreeFromString("<p>{{semantic}}</p>", "semantic-inline.xml", &context);
    ASSERT_TRUE(result.ok());

    const Element* paragraph = result.rootAs<Element>();
    ASSERT_NE(paragraph, nullptr);
    const std::vector<const char*> expected = {"abbr", "b", "cite", "code", "dfn", "del", "em", "i", "ins", "mark", "q", "s", "small", "strong", "u"};
    ASSERT_EQ(paragraph->children().size(), expected.size());
    for (std::size_t index = 0; index < expected.size(); ++index) EXPECT_EQ(paragraph->children()[index]->elementName(), expected[index]);
}

TEST_F(FieldsetTest, PreservesLocalizedBreaksBetweenInlineRuns) {
    LocalizationCatalog localization;
    constexpr char kInlineLocalization[] = "defaultLocale: en\n"
                                           "locales: {en: {strings: "
                                           "{inlineExample: 'First <b>Second</b><br/><i>Third</i><br/>Fourth <kbd shortcut=\"toggle-fly\"/>'}}}\n";
    ASSERT_FALSE(localization.loadYaml(kInlineLocalization).hasErrors());

    EXPECT_EQ(localization.resolveMarkup("en", radia::ui::LocalizedText("inlineExample")),
              "First <b>Second</b><br/><i>Third</i><br/>Fourth <kbd shortcut=\"toggle-fly\"/>");

    const LayoutBuildContext context(localization, "en");
    const LayoutBuildResult result =
        LayoutResourceCompiler().buildElementTreeFromString("<p>{{inlineExample}}</p>", "localized-breaks.xml", &context);
    ASSERT_TRUE(result.ok());

    Element* paragraph = result.document->documentElement();
    ASSERT_NE(paragraph, nullptr);
    paragraph->setRect({0.f, 0.f, 300.f, 100.f});
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("p, b, i, kbd { display: inline; font-size: 10px; line-height: 10px; }").ok());
    layoutTree(*paragraph, stylesheet, FixedTextMetrics());

    std::vector<std::string> nodeNames;
    const radia::ui::Node* postBreakText = nullptr;
    bool sawBreak = false;
    for (radia::ui::Node& node : radia::ui::detail::nodes(*paragraph)) {
        if (const Element* element = node.asElement()) {
            nodeNames.push_back(element->elementName());
            if (element->elementName() == "br") sawBreak = true;
        } else {
            nodeNames.push_back("#text");
            if (sawBreak && !postBreakText) postBreakText = &node;
        }
    }
    const std::vector<std::string> expectedNodeNames{"#text", "b", "br", "i", "br", "#text", "kbd"};
    EXPECT_EQ(nodeNames, expectedNodeNames);
    ASSERT_NE(postBreakText, nullptr);

    const Element* bold = nullptr;
    const Element* italic = nullptr;
    const Element* shortcut = nullptr;
    for (const auto& child : paragraph->children()) {
        if (child->elementName() == "b") bold = child;
        if (child->elementName() == "i") italic = child;
        if (child->elementName() == "kbd") shortcut = child;
    }
    ASSERT_NE(bold, nullptr);
    ASSERT_NE(italic, nullptr);
    ASSERT_NE(shortcut, nullptr);
    const auto* postBreak = postBreakText->asText();
    ASSERT_NE(postBreak, nullptr);
    EXPECT_TRUE(italic->flowBreakBefore());
    EXPECT_TRUE(radia::ui::detail::NodeAccess::flowBreakBefore(*postBreakText));
    EXPECT_LT(italic->rect().y, bold->rect().y);
    EXPECT_LT(postBreak->rect().y, italic->rect().y);
}

TEST_F(FieldsetTest, RejectsUnsupportedInlineElements) {
    struct InvalidInlineCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidInlineCase cases[] = {
        {"missing shortcut", "<p><kbd/></p>", "layout.kbd.shortcut_required"},
        {"invalid shortcut", "<p><kbd shortcut=\"toggle.fly\"/></p>", "layout.kbd.shortcut_invalid"},
        {"inline attribute", "<fieldset><legend><b emphasis=\"true\">bad</b></legend></fieldset>", "layout.inline.attribute_unknown"},
        {"inline children", "<fieldset><legend><kbd shortcut=\"toggle-fly\">bad</kbd></legend></fieldset>", "layout.inline.children_unsupported"},
        {"element child", "<p><label>not-inline</label></p>", "layout.label.for_required"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "unsupported inline case: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic) << result.errors.front().message;
    }
}

TEST_F(FieldsetTest, ActivatesLabelTargetOnlyWhenInteractive) {
    constexpr char kLabelTargetLayout[] = "<panel><label id=\"toggleLabel\" for=\"toggle\">Enable</label>"
                                          "<input type=\"checkbox\" switch=\"true\" id=\"toggle\" onChange=\"toggleChanged()\"/></panel>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kLabelTargetLayout, "label-target.xml");
    ASSERT_TRUE(result.ok());

    const ElementRef<LabelElement> label = requireElement<LabelElement>(*result.document->documentElement(), "toggleLabel");
    const ElementRef<InputElement> target = requireElement<InputElement>(*result.document->documentElement(), "toggle");
    ASSERT_TRUE(label);
    ASSERT_TRUE(target);
    EXPECT_TRUE(label->defaultPointerEvents());

    int changes = 0;
    Binder binder(*result.document->documentElement());
    bindChangeEvent(binder, "toggleChanged", [&](const Event& event) {
        EXPECT_TRUE(event.checked());
        ++changes;
    });
    PreparedBindingResult prepared = binder.prepare();
    ASSERT_TRUE(prepared.ok());
    const Binding binding = prepared.binding.commit();
    ASSERT_TRUE(binding);

    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    target->disabled(true);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    target->disabled(false);
    result.document->documentElement()->disabled(true);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    result.document->documentElement()->disabled(false);
    result.document->documentElement()->setVisibility(Visibility::Hidden);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);
}

TEST_F(FieldsetTest, RejectsInvalidLabelRelationships) {
    struct InvalidLabelCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    constexpr char kNestedTargetLayout[] = "<panel>"
                                           "<input type=\"checkbox\" switch=\"true\" id=\"nestedTarget\"/></panel>";
    resources["nested-target.xml"] = kNestedTargetLayout;
    const InvalidLabelCase cases[] = {
        {"missing for", "<panel><label>Missing relationship</label><input type=\"checkbox\" switch=\"true\" id=\"toggle\"/></panel>",
         "layout.label.for_required"},
        {"invalid target id",
         "<panel><label for=\"Bad_Target!\">Invalid relationship</label>"
         "<input type=\"checkbox\" switch=\"true\" id=\"toggle\"/></panel>",
         "layout.label.for_invalid"},
        {"missing target", "<panel><label for=\"missing\">Missing target</label></panel>", "layout.label.target_missing"},
        {"non-labelable target",
         "<panel><label for=\"copy\">Wrong target</label>"
         "<p id=\"copy\">Copy</p></panel>",
         "layout.label.target_not_labelable"},
        {"cross-scope target",
         "<panel><label for=\"nestedTarget\">Cross scope</label>"
         "<panel filename=\"nested-target.xml\"/></panel>",
         "layout.label.target_missing"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid label relationship: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(FieldsetTest, ResolvesLabelTargetsInsideIncludedResources) {
    constexpr char kNestedValidLayout[] = "<panel><label for=\"nestedSwitch\">Nested</label>"
                                          "<input type=\"checkbox\" switch=\"true\" id=\"nestedSwitch\"/></panel>";
    constexpr char kNestedLabelLayout[] = "<panel><panel filename=\"nested-valid.xml\"/></panel>";
    resources["nested-valid.xml"] = kNestedValidLayout;
    const LayoutBuildResult result = factory.buildElementTreeFromString(kNestedLabelLayout, "label-nested.xml");
    EXPECT_TRUE(result.ok());
}

TEST_F(FieldsetTest, AcceptsGenericFieldsetChildrenAndScopesLegend) {
    constexpr char kFieldsetLayout[] = "<fieldset id=\"settings\"><legend id=\"settingsLegend\" class=\"heading\">Settings <b>demo</b></legend>"
                                       "<div class=\"row\"><label for=\"toggle\">Toggle</label>"
                                       "<input type=\"checkbox\" switch=\"true\" id=\"toggle\"/><div class=\"hint\">Helpful</div>"
                                       "<div class=\"error\">Fallback</div></div>"
                                       "<div class=\"row\">Second row</div></fieldset>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kFieldsetLayout, "fieldset.xml");
    ASSERT_TRUE(result.ok());

    Element* fieldset = result.rootAs<Element>();
    ASSERT_NE(fieldset, nullptr);
    ASSERT_EQ(fieldset->children().size(), 3U);
    EXPECT_EQ(fieldset->children()[0]->elementName(), "legend");
    EXPECT_EQ(fieldset->children()[0]->id(), "settingsLegend");
    EXPECT_TRUE(fieldset->children()[0]->classes().contains("heading"));
    EXPECT_EQ(fieldset->children()[0]->textContent(), "Settings demo");
    EXPECT_EQ(fieldset->children()[1]->elementName(), "div");
    EXPECT_TRUE(fieldset->children()[1]->classes().contains("row"));
    ASSERT_EQ(fieldset->children()[1]->children().size(), 4U);
    EXPECT_EQ(fieldset->children()[1]->children()[2]->classes().contains("hint"), true);
    EXPECT_EQ(fieldset->children()[1]->children()[3]->classes().contains("error"), true);
    EXPECT_TRUE(fieldset->children()[2]->classes().contains("row"));

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
                    .loadRadia("fieldset { display: flex; flex-direction: column; gap: 10px; padding: 3px 6px; border: 1px #ffffff; } "
                               "fieldset > legend { height: 10px; } div.row { height: 20px; }")
                    .ok());
    fieldset->setRect({0.f, 0.f, 120.f, 90.f});
    layoutTree(*fieldset, stylesheet, FixedTextMetrics{});
    EXPECT_FLOAT_EQ(fieldset->children()[0]->rect().left(), 7.f);
    EXPECT_LT(fieldset->children()[0]->rect().right(), fieldset->rect().right());
    EXPECT_LT(fieldset->children()[0]->rect().w, fieldset->rect().w - 12.f);
    EXPECT_FLOAT_EQ((fieldset->children()[0]->rect().top() + fieldset->children()[0]->rect().bottom()) * 0.5f, fieldset->rect().top() - 0.5f);
    EXPECT_FLOAT_EQ(fieldset->children()[0]->rect().bottom() - fieldset->children()[1]->rect().top(), 1.f);
    EXPECT_FLOAT_EQ(fieldset->children()[1]->rect().bottom() - fieldset->children()[2]->rect().top(), 10.f);

    radia::ui::RecordingPaintContext recording(FixedTextMetrics{});
    fieldset->paint(recording, resolveElementStyle(stylesheet, *fieldset), 1.f);
    const PaintCommand* fieldsetBox = recording.last(PaintCommandKind::Box);
    ASSERT_NE(fieldsetBox, nullptr);
    ASSERT_TRUE(fieldsetBox->topBorderGap.has_value());
    EXPECT_FLOAT_EQ(fieldsetBox->topBorderGap->left, 7.f);
    EXPECT_LT(fieldsetBox->topBorderGap->right, fieldset->rect().right());
}

TEST_F(FieldsetTest, EnforcesLegendScopeAndUniqueness) {
    struct InvalidLayoutCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidLayoutCase cases[] = {
        {"standalone legend", "<legend>Orphan</legend>", "layout.element.scoped"},
        {"nested legend", "<fieldset><div><legend>Nested</legend></div></fieldset>", "layout.element.scoped"},
        {"duplicate legend", "<fieldset><legend>One</legend><legend>Two</legend></fieldset>", "layout.fieldset.legend_duplicate"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid generic layout: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    EXPECT_TRUE(factory.buildElementTreeFromString("<fieldset><panel/><div/></fieldset>", "generic-fieldset.xml").ok());
    EXPECT_TRUE(factory.buildElementTreeFromString("<div class=\"hint\">Hint</div>", "hint-class.xml").ok());
    EXPECT_TRUE(factory.buildElementTreeFromString("<div class=\"error\">Error</div>", "error-class.xml").ok());

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("div.hint { text-color: #ffffffff; } div.error { text-color: #ff0000ff; }").ok());
}

TEST_F(FieldsetTest, PreservesFieldsetChildOrder) {
    const LayoutBuildResult result = factory.buildElementTreeFromString(
        "<fieldset><div class=\"late\"/><legend>Settings</legend><div class=\"early\"/></fieldset>", "fieldset-order.xml");
    ASSERT_TRUE(result.ok());
    const Element* fieldset = result.rootAs<Element>();
    ASSERT_NE(fieldset, nullptr);
    ASSERT_EQ(fieldset->children().size(), 3U);
    EXPECT_TRUE(fieldset->children()[0]->classes().contains("late"));
    EXPECT_EQ(fieldset->children()[1]->elementName(), "legend");
    EXPECT_TRUE(fieldset->children()[2]->classes().contains("early"));
}

TEST_F(FieldsetTest, ReusesExistingCompositePartsAndInstantiatesThemIdempotently) {
    PanelElement owner;
    auto existingParent = std::make_unique<PanelElement>();
    PanelElement* parent = existingParent.get();
    auto existingChild = std::make_unique<PanelElement>();
    PanelElement* child = existingChild.get();
    ElementCompilerAccess::setStyleIdentity(*parent, owner.styleElement(), "parent");
    ElementCompilerAccess::setStyleIdentity(*child, owner.styleElement(), "parent::child");
    parent->append(std::move(existingChild));
    owner.append(std::move(existingParent));

    ElementDefinition definition;
    CompositePartDefinition nested;
    nested.path = "parent::child";
    nested.parentPath = "parent";
    nested.create = [] { return std::make_unique<PanelElement>(); };
    CompositePartDefinition root;
    root.path = "parent";
    root.create = [] { return std::make_unique<PanelElement>(); };
    definition.compositeParts = {nested, root};

    instantiateCompositeParts(owner, definition);
    ASSERT_EQ(owner.children().size(), 1U);
    ASSERT_EQ(parent->children().size(), 1U);
    EXPECT_EQ(owner.children().front(), parent);
    EXPECT_EQ(parent->children().front(), child);

    instantiateCompositePart(owner, definition, "parent::child");
    EXPECT_EQ(parent->children().size(), 1U);
}
