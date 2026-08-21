/**
 * @file field_test.cpp
 * @brief Tests Field composition, labels, hints, errors, and value controls.
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
#include <functional>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <string>
#include <utility>
#include "../layout/test_layout_helpers.h"
#include "binding/binder.h"
#include "layout/document.h"
#include "layout/engine.h"
#include "layout/resourcecompiler.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/fieldset.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace {
using radia::ui::Binder;
using radia::ui::Binding;
using radia::ui::ChangeEvent;
using radia::ui::CompositePartContract;
using radia::ui::EventCall;
using radia::ui::Field;
using radia::ui::Fieldset;
using radia::ui::FixedTextMetrics;
using radia::ui::InlineContentKind;
using radia::ui::Label;
using radia::ui::LayoutBuildContext;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutDirection;
using radia::ui::LayoutResourceCompiler;
using radia::ui::layoutTree;
using radia::ui::LocalizationCatalog;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::Panel;
using radia::ui::PreparedBindingResult;
using radia::ui::resolveWidgetStyle;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::StyleSheetLoadResult;
using radia::ui::Switch;
using radia::ui::Text;
using radia::ui::Visibility;
using radia::ui::Widget;
using radia::ui::WidgetContract;
using radia::ui::WidgetEvent;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetRef;
using radia::ui::detail::findWidgetInScope;
using radia::ui::detail::instantiateCompositePart;
using radia::ui::detail::instantiateCompositeParts;
using radia::ui::detail::makeEventRegistration;
using radia::ui::detail::WidgetCompilerAccess;
using radia::ui::test::LayoutCompilerTestHelper;
using ::testing::Message;

void bindChangeEvent(Binder& binder, std::string name, std::function<void(const ChangeEvent&)> callback) {
    binder.event(makeEventRegistration(
        std::move(name), WidgetEventKind::Change,
        [callback = std::move(callback)](const WidgetEvent& event, const EventCall&) { callback(static_cast<const ChangeEvent&>(event)); },
        [](const EventCall& call, WidgetEventKind) { return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch"; }));
}

class FieldTest : public ::testing::Test {
protected:
    template<typename WidgetT> WidgetRef<WidgetT> requireWidget(Widget& root, const std::string& id) const {
        return WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(findWidgetInScope(root, id)));
    }

    LayoutCompilerTestHelper factory;
    std::map<std::string, std::string>& resources = factory.resources;
};
} // namespace

TEST_F(FieldTest, BuildsFieldWithLabelValueAndSupportContent) {
    constexpr char kFieldLayout[] = "<field id=\"exampleField\"><label for=\"fieldSwitch\">label.example</label>"
                                    "<switch id=\"fieldSwitch\" checked=\"true\" "
                                    "onChange=\"switchChanged()\"/><br/>"
                                    "<hint>Helpful <b>detail</b></hint><br/><error>Fallback error</error></field>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kFieldLayout, "field.xml");
    ASSERT_TRUE(result.ok());

    const Field* field = result.rootAs<Field>();
    ASSERT_NE(field, nullptr);
    ASSERT_NE(field->label(), nullptr);
    ASSERT_NE(field->valueControl(), nullptr);
    ASSERT_NE(field->hint(), nullptr);
    ASSERT_NE(field->error(), nullptr);

    EXPECT_EQ(field->hint()->parent(), field);
    EXPECT_TRUE(field->hint()->part().empty());
    EXPECT_EQ(field->error()->parent(), field);
    EXPECT_TRUE(field->error()->part().empty());
    EXPECT_EQ(field->hint()->text(), "Helpful detail");
    EXPECT_EQ(field->error()->text(), "Fallback error");
    EXPECT_EQ(field->error()->visibility(), Visibility::Visible);
    EXPECT_FALSE(field->error()->isDisplayed(Style{}));
    EXPECT_TRUE(field->hint()->flowBreakBefore());
    EXPECT_TRUE(field->error()->flowBreakBefore());
}

TEST_F(FieldTest, RejectsInvalidFieldStructureAndSupportContent) {
    struct InvalidFieldCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidFieldCase cases[] = {
        {"interactive hint",
         "<field><label for=\"fieldSwitch\">Label</label><switch id=\"fieldSwitch\"/>"
         "<hint><link href=\"https://example.com\">link</link></hint></field>",
         "layout.inline.unsupported"},
        {"standalone hint", "<hint>orphan</hint>", "layout.element.scoped"},
        {"standalone error", "<error>orphan</error>", "layout.element.scoped"},
        {"duplicate hint",
         "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/>"
         "<hint>one</hint><hint>two</hint></field>",
         "layout.field.hint_duplicate"},
        {"missing label and value control", "<field/>", "layout.field.label_required"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid field case: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    {
        SCOPED_TRACE("unsupported field child");
        constexpr char kUnsupportedChildLayout[] = "<field><label for=\"button\">Label</label>"
                                                   "<button id=\"button\"/></field>";
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(kUnsupportedChildLayout, "field-child.xml");
        ASSERT_FALSE(result.ok());
        EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(),
                                [](const auto& diagnostic) { return diagnostic.code == "layout.field.child_unsupported"; }));
    }

    {
        SCOPED_TRACE("mismatched field label target");
        constexpr char kMismatchedTargetLayout[] = "<panel><switch id=\"other\"/>"
                                                   "<field><label for=\"other\">Label</label><switch id=\"direct\"/></field></panel>";
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(kMismatchedTargetLayout, "field-target.xml");
        ASSERT_FALSE(result.ok());
        EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(),
                                [](const auto& diagnostic) { return diagnostic.code == "layout.field.label_target_mismatch"; }));
    }
}

TEST_F(FieldTest, RejectsInvalidFlowBreakPlacementAndAttributes) {
    struct InvalidBreakCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidBreakCase cases[] = {
        {"leading break", "<panel><br/><button/></panel>", "layout.flow_break.leading"},
        {"trailing break", "<panel><button/><br/></panel>", "layout.flow_break.trailing"},
        {"consecutive breaks", "<panel><button/><br/><br/><button/></panel>", "layout.flow_break.consecutive"},
        {"attributed break", "<panel><button/><br class=\"invalid\"/><button/></panel>", "layout.attribute.unknown"},
        {"attributed hint",
         "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/>"
         "<hint visibility=\"hidden\">Hint</hint></field>",
         "layout.attribute.unknown"},
        {"composite chrome before break", "<floater title=\"title\"><header/><br/><p>content</p></floater>", "layout.flow_break.leading"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid flow-break case: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(FieldTest, PreservesInlineContentStructureAcrossTextAndLabels) {
    constexpr char kInlineLayout[] = "<panel><p id=\"copy\">before <b>bold<i>both</i></b><br/>"
                                     "<i>after</i></p><p id=\"title\">Title</p></panel>";
    constexpr char kLabelInlineLayout[] = "<panel><label id=\"label\" for=\"target\">name <b>important</b>"
                                          "<br/><i>detail</i></label><switch id=\"target\"/></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kInlineLayout, "inline.xml");
    ASSERT_TRUE(result.ok());

    const WidgetRef<Text> text = requireWidget<Text>(*result.root, "copy");
    const WidgetRef<Text> title = requireWidget<Text>(*result.root, "title");
    ASSERT_TRUE(text);
    ASSERT_TRUE(title);
    EXPECT_TRUE(text->children().empty());
    ASSERT_EQ(text->content().nodes().size(), 5U);
    EXPECT_EQ(text->content().nodes()[0].kind(), InlineContentKind::Text);
    EXPECT_EQ(text->content().nodes()[1].value(), " ");
    EXPECT_EQ(text->content().nodes()[2].children().size(), 2U);
    EXPECT_EQ(text->content().nodes()[3].kind(), InlineContentKind::Br);

    const LayoutBuildResult labelResult = factory.buildWidgetTreeFromString(kLabelInlineLayout, "label-inline.xml");
    ASSERT_TRUE(labelResult.ok());
    const WidgetRef<Label> label = requireWidget<Label>(*labelResult.root, "label");
    ASSERT_TRUE(label);
    EXPECT_TRUE(label->children().empty());
    EXPECT_EQ(label->text(), "name important\ndetail");
}

TEST_F(FieldTest, LocalizesAndDecoratesInlineContent) {
    LocalizationCatalog localization;
    constexpr char kInlineLocalization[] = "defaultLocale: en\n"
                                           "locales: {en: {name: English, strings: "
                                           "{inlineExample: \"First <b>Second</b>\"}}}\n";
    ASSERT_TRUE(localization.loadYaml(kInlineLocalization).ok());
    const LayoutBuildContext context(localization, "en");
    constexpr char kLocalizedTextLayout[] = "<p>inlineExample</p>";
    constexpr char kDecorationLayout[] = "<p><s>outdated</s> "
                                         "<kbd shortcut=\"toggle-fly\"/></p>";
    constexpr char kUnsupportedHeadingLayout[] = "<heading>Title</heading>";
    const LayoutBuildResult localizedResult =
        LayoutResourceCompiler().buildWidgetTreeFromString(kLocalizedTextLayout, "localized-inline.xml", &context);
    ASSERT_TRUE(localizedResult.ok());

    const Text* localized = localizedResult.rootAs<Text>();
    ASSERT_NE(localized, nullptr);
    ASSERT_EQ(localized->content().nodes().size(), 2U);
    EXPECT_EQ(localized->content().nodes()[0].value(), "First ");
    EXPECT_EQ(localized->content().nodes()[1].children()[0].value(), "Second");

    const LayoutBuildResult decoration = factory.buildWidgetTreeFromString(kDecorationLayout, "decoration.xml");
    ASSERT_TRUE(decoration.ok());
    const Text* decorated = decoration.rootAs<Text>();
    ASSERT_NE(decorated, nullptr);
    ASSERT_GE(decorated->content().nodes().size(), 3U);
    EXPECT_EQ(decorated->content().nodes()[0].kind(), InlineContentKind::S);
    EXPECT_EQ(decorated->content().nodes()[2].shortcutId(), "toggle-fly");
    EXPECT_FALSE(factory.buildWidgetTreeFromString(kUnsupportedHeadingLayout, "heading.xml").ok());
}

TEST_F(FieldTest, RejectsUnsupportedInlineContent) {
    struct InvalidInlineCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidInlineCase cases[] = {
        {"missing shortcut", "<p><kbd/></p>", "layout.inline.kbd.shortcut_required"},
        {"invalid shortcut", "<p><kbd shortcut=\"toggle_fly\"/></p>", "layout.inline.kbd.shortcut_invalid"},
        {"widget child", "<p><label>not-inline</label></p>", "layout.inline.element_unknown"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "unsupported inline case: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(FieldTest, ActivatesLabelTargetOnlyWhenInteractive) {
    constexpr char kLabelTargetLayout[] = "<panel><label id=\"toggleLabel\" for=\"toggle\">Enable</label>"
                                          "<switch id=\"toggle\" onChange=\"toggleChanged()\"/></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kLabelTargetLayout, "label-target.xml");
    ASSERT_TRUE(result.ok());

    const WidgetRef<Label> label = requireWidget<Label>(*result.root, "toggleLabel");
    const WidgetRef<Switch> target = requireWidget<Switch>(*result.root, "toggle");
    ASSERT_TRUE(label);
    ASSERT_TRUE(target);
    EXPECT_TRUE(label->defaultPointerEvents());

    int changes = 0;
    Binder binder(*result.root);
    bindChangeEvent(binder, "toggleChanged", [&](const ChangeEvent& event) {
        EXPECT_TRUE(event.checked);
        ++changes;
    });
    PreparedBindingResult prepared = binder.prepare();
    ASSERT_TRUE(prepared.ok());
    const Binding binding = prepared.binding.commit();
    ASSERT_TRUE(binding);

    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    target->setDisabled(true);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    target->setDisabled(false);
    result.root->setDisabled(true);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);

    result.root->setDisabled(false);
    result.root->setVisibility(Visibility::Hidden);
    label->activate();
    EXPECT_TRUE(target->checked());
    EXPECT_EQ(changes, 1);
}

TEST_F(FieldTest, RejectsInvalidLabelRelationships) {
    struct InvalidLabelCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    constexpr char kNestedTargetLayout[] = "<panel>"
                                           "<switch id=\"nestedTarget\"/></panel>";
    resources["nested-target.xml"] = kNestedTargetLayout;
    const InvalidLabelCase cases[] = {
        {"missing for", "<panel><label>Missing relationship</label><switch id=\"toggle\"/></panel>", "layout.label.for_required"},
        {"invalid target id",
         "<panel><label for=\"Bad_Target\">Invalid relationship</label>"
         "<switch id=\"toggle\"/></panel>",
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
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(FieldTest, ResolvesLabelTargetsInsideIncludedResources) {
    constexpr char kNestedValidLayout[] = "<panel><label for=\"nestedSwitch\">Nested</label>"
                                          "<switch id=\"nestedSwitch\"/></panel>";
    constexpr char kNestedLabelLayout[] = "<panel><panel filename=\"nested-valid.xml\"/></panel>";
    resources["nested-valid.xml"] = kNestedValidLayout;
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kNestedLabelLayout, "label-nested.xml");
    EXPECT_TRUE(result.ok());
}

TEST_F(FieldTest, LaysOutAndPaintsFieldsetAroundItsLegend) {
    constexpr char kFieldsetLayout[] = "<fieldset id=\"settings\"><legend>Settings <b>demo</b></legend>"
                                       "<field><label for=\"first\">First</label><switch id=\"first\"/></field>"
                                       "<field><switch id=\"second\"/><label for=\"second\">Second</label>"
                                       "<br/><hint>Second hint</hint></field></fieldset>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kFieldsetLayout, "fieldset.xml");
    ASSERT_TRUE(result.ok());

    Fieldset* fieldset = result.rootAs<Fieldset>();
    ASSERT_NE(fieldset, nullptr);
    ASSERT_NE(fieldset->legend(), nullptr);
    ASSERT_EQ(fieldset->children().size(), 3U);
    EXPECT_EQ(fieldset->legend()->text(), "Settings demo");
    EXPECT_EQ(fieldset->legend()->elementName(), "legend");
    EXPECT_TRUE(fieldset->legend()->part().empty());

    Field* controlFirst = dynamic_cast<Field*>(fieldset->children()[2].get());
    ASSERT_NE(controlFirst, nullptr);
    ASSERT_NE(controlFirst->hint(), nullptr);

    StyleSheet stylesheet;
    constexpr char kFieldsetStyles[] = "fieldset { width: 120px; height: 90px; padding: 3px 6px; border: 1px #ffffff24; gap: 10px; "
                                       "& > legend { font-weight: bold; font-size: 10px; line-height: 10px; } } "
                                       "field { width: 100px; height: 30px; gap: 2px; } switch { size: 10px; } "
                                       "label, field > hint { width: 20px; height: 10px; }";
    ASSERT_TRUE(stylesheet.loadRadia(kFieldsetStyles).ok());

    FixedTextMetrics text;
    fieldset->setRect({0.f, 0.f, 120.f, 90.f});
    layoutTree(*fieldset, stylesheet, text, LayoutDirection::LeftToRight);
    const Style legendStyle = resolveWidgetStyle(stylesheet, *fieldset->legend());
    EXPECT_EQ(legendStyle.fontWeight, 700);
    EXPECT_FLOAT_EQ(legendStyle.fontSize, 10.f);
    EXPECT_FLOAT_EQ(fieldset->legend()->rect().w, fieldset->legend()->desiredSize().x);
    EXPECT_FLOAT_EQ(fieldset->legend()->rect().left(), 6.f);
    EXPECT_FLOAT_EQ(fieldset->legend()->rect().top(), fieldset->rect().top());

    const Widget& firstField = *fieldset->children()[1];
    const Widget& secondField = *fieldset->children()[2];
    EXPECT_FLOAT_EQ(firstField.rect().bottom() - secondField.rect().top(), 10.f);

    radia::ui::RecordingPaintContext recording(text);
    const Style fieldsetStyle = resolveWidgetStyle(stylesheet, *fieldset);
    fieldset->paint(recording, fieldsetStyle, 1.f);
    const PaintCommand* fieldsetBox = recording.last(PaintCommandKind::Box);
    ASSERT_NE(fieldsetBox, nullptr);
    ASSERT_TRUE(fieldsetBox->topBorderGap.has_value());
    EXPECT_FLOAT_EQ(fieldsetBox->rect.top() - fieldsetStyle.borderWidth.top - firstField.rect().top(), 3.f);
    EXPECT_FLOAT_EQ(fieldsetBox->rect.top(), fieldset->legend()->rect().y + fieldset->legend()->rect().h * .5f);
    EXPECT_LT(fieldsetBox->topBorderGap->left, fieldset->legend()->rect().left());
    EXPECT_GT(fieldsetBox->topBorderGap->right, fieldset->legend()->rect().right());

    layoutTree(*fieldset, stylesheet, text, LayoutDirection::RightToLeft);
    EXPECT_FLOAT_EQ(fieldset->legend()->rect().right(), fieldset->rect().right() - 6.f);

    controlFirst->setRect({0.f, 0.f, 100.f, 30.f});
    layoutTree(*controlFirst, stylesheet, text, LayoutDirection::LeftToRight);
    EXPECT_FLOAT_EQ(controlFirst->hint()->rect().left(), controlFirst->label()->rect().left());
    layoutTree(*controlFirst, stylesheet, text, LayoutDirection::RightToLeft);
    EXPECT_FLOAT_EQ(controlFirst->hint()->rect().right(), controlFirst->label()->rect().right());
}

TEST_F(FieldTest, RejectsInvalidFieldsetStructureAndRemovedPartSelectors) {
    struct InvalidFieldsetCase {
        const char* name;
        const char* source;
        const char* diagnostic;
    };

    const InvalidFieldsetCase cases[] = {
        {"missing legend",
         "<fieldset><field><label for=\"toggle\">Label</label>"
         "<switch id=\"toggle\"/></field></fieldset>",
         "layout.fieldset.legend_required"},
        {"missing field", "<fieldset><legend>Empty</legend></fieldset>", "layout.fieldset.field_required"},
        {"duplicate legend",
         "<fieldset><legend>One</legend><legend>Two</legend>"
         "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>",
         "layout.fieldset.legend_duplicate"},
        {"unsupported child", "<fieldset><legend>Settings</legend><panel/></fieldset>", "layout.fieldset.child_unsupported"},
        {"unsupported flow break",
         "<fieldset><legend>Settings</legend><br/>"
         "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>",
         "layout.fieldset.flow_break_unsupported"},
        {"standalone legend", "<legend>Orphan</legend>", "layout.element.scoped"},
    };

    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid fieldset case: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.source, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    constexpr char kRemovedLegendSelector[] = "fieldset::legend { font-size: 10px; }";
    constexpr char kRemovedHintSelector[] = "field::hint { font-size: 10px; }";
    StyleSheet legendPartStylesheet;
    const StyleSheetLoadResult legendPart = legendPartStylesheet.loadRadia(kRemovedLegendSelector);
    ASSERT_FALSE(legendPart.ok());
    ASSERT_FALSE(legendPart.errors.empty());
    EXPECT_EQ(legendPart.errors.front().code, "stylesheet.selector.part_unknown");

    StyleSheet fieldPartStylesheet;
    const StyleSheetLoadResult fieldPart = fieldPartStylesheet.loadRadia(kRemovedHintSelector);
    ASSERT_FALSE(fieldPart.ok());
    ASSERT_FALSE(fieldPart.errors.empty());
    EXPECT_EQ(fieldPart.errors.front().code, "stylesheet.selector.part_unknown");
}

TEST_F(FieldTest, OrdersFieldsAroundTheLegendDuringLayout) {
    constexpr char kReorderedFieldsetLayout[] = "<fieldset><field class=\"late\"><label for=\"late\">Late</label>"
                                                "<switch id=\"late\"/></field><legend>Settings</legend>"
                                                "<field class=\"early\"><label for=\"early\">Early</label>"
                                                "<switch id=\"early\"/></field></fieldset>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kReorderedFieldsetLayout, "reordered-fieldset.xml");
    ASSERT_TRUE(result.ok());

    Fieldset* fieldset = result.rootAs<Fieldset>();
    ASSERT_NE(fieldset, nullptr);
    Field* late = dynamic_cast<Field*>(fieldset->children()[0].get());
    Field* early = dynamic_cast<Field*>(fieldset->children()[2].get());
    ASSERT_NE(late, nullptr);
    ASSERT_NE(early, nullptr);

    StyleSheet stylesheet;
    constexpr char kFieldsetStyles[] = "fieldset { width: 120px; height: 90px; padding: 3px 6px; border: 1px #ffffff24; "
                                       "gap: 10px; & > legend { order: 100; font-size: 10px; line-height: 10px; } } "
                                       "field { width: 100px; height: 20px; } .early { order: -1; } .late { order: 2; }";
    ASSERT_TRUE(stylesheet.loadRadia(kFieldsetStyles).ok());

    FixedTextMetrics text;
    fieldset->setRect({0.f, 0.f, 120.f, 90.f});
    layoutTree(*fieldset, stylesheet, text, LayoutDirection::LeftToRight);
    EXPECT_FLOAT_EQ(fieldset->legend()->rect().top(), fieldset->rect().top());

    const float contentTop = fieldset->legend()->rect().y + fieldset->legend()->rect().h * .5f - 1.f - 3.f;
    EXPECT_FLOAT_EQ(early->rect().top(), contentTop);
    EXPECT_FLOAT_EQ(early->rect().bottom() - late->rect().top(), 10.f);
}

TEST_F(FieldTest, ReusesExistingCompositePartsAndInstantiatesThemIdempotently) {
    Panel owner;
    auto existingParent = std::make_unique<Panel>();
    Panel* parent = existingParent.get();
    auto existingChild = std::make_unique<Panel>();
    Panel* child = existingChild.get();
    WidgetCompilerAccess::setStyleIdentity(*parent, owner.styleElement(), "parent");
    WidgetCompilerAccess::setStyleIdentity(*child, owner.styleElement(), "parent::child");
    parent->addChild(std::move(existingChild));
    owner.addChild(std::move(existingParent));

    WidgetContract contract;
    CompositePartContract nested;
    nested.path = "parent::child";
    nested.parentPath = "parent";
    nested.create = [] { return std::make_unique<Panel>(); };
    CompositePartContract root;
    root.path = "parent";
    root.create = [] { return std::make_unique<Panel>(); };
    contract.compositeParts = {nested, root};

    instantiateCompositeParts(owner, contract);
    ASSERT_EQ(owner.children().size(), 1U);
    ASSERT_EQ(parent->children().size(), 1U);
    EXPECT_EQ(owner.children().front().get(), parent);
    EXPECT_EQ(parent->children().front().get(), child);

    instantiateCompositePart(owner, contract, "parent::child");
    EXPECT_EQ(parent->children().size(), 1U);
}
