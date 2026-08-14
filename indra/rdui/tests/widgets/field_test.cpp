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
#include <fstream>
#include <functional>
#include <map>
#include <sstream>
#include "../layout/fixture.h"
#include "../test/lltut.h"
#include "../test/test.h"
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
void bindChangeEvent(rdui::Binder& binder, std::string name, std::function<void(const rdui::ChangeEvent&)> callback) {
    binder.event(rdui::detail::makeEventRegistration(
        std::move(name), rdui::WidgetEventKind::Change,
        [callback = std::move(callback)](const rdui::WidgetEvent& event, const rdui::EventCall&) {
            callback(static_cast<const rdui::ChangeEvent&>(event));
        },
        [](const rdui::EventCall& call, rdui::WidgetEventKind) { return call.arguments().empty() ? nullptr : "binding.event.arity_mismatch"; }));
}
} // namespace

namespace tut {
struct fieldData {
    LayoutCompilerFixture factory;
    std::map<std::string, std::string>& resources = factory.resources;

    template<typename WidgetT> rdui::WidgetRef<WidgetT> requireWidget(rdui::Widget& root, const std::string& id) {
        return rdui::WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(rdui::detail::findWidgetInScope(root, id)));
    }
};
using fieldTest = test_group<fieldData>;
using fieldObject = fieldTest::object;
fieldTest fieldTestCase("field");

template<> template<> void fieldObject::test<1>() {
    const char* kFieldLayout =
        "<field id=\"exampleField\"><label for=\"fieldSwitch\">label.example</label><switch id=\"fieldSwitch\" checked=\"true\" onChange=\"switchChanged()\"/><br/><hint>Helpful <b>detail</b></hint><br/><error>Fallback error</error></field>";
    const rdui::LayoutBuildResult result = factory.buildWidgetTreeFromString(kFieldLayout, "field.xml");
    auto* field = result.rootAs<rdui::Field>();
    ensure("field markup builds", result.ok() && field);
    ensure("Field exposes its direct Label and Value Control", field->label() && field->valueControl());
    ensure("Field exposes scoped Hint and Error Text Hosts", field->hint() && field->error());
    ensure("authored Hint and Error remain ordinary direct children",
           field->hint()->parent() == field && field->hint()->part().empty() && field->error()->parent() == field && field->error()->part().empty());
    ensure_equals("Hint preserves shared Inline Content", field->hint()->text(), "Helpful detail");
    ensure_equals("authored Error is retained as fallback", field->error()->text(), "Fallback error");
    ensure_equals("Error starts collapsed while the Value Control is valid", static_cast<int>(field->error()->visibility()),
                  static_cast<int>(rdui::Visibility::Collapsed));
    ensure("Flow Break marks Hint for a new row", field->hint()->flowBreakBefore());
    ensure("Flow Break marks Error for a new row", field->error()->flowBreakBefore());

    const char* kLinkLayout =
        "<field><label for=\"fieldSwitch\">Label</label><switch id=\"fieldSwitch\"/><hint><link href=\"https://example.com\">link</link></hint></field>";
    const rdui::LayoutBuildResult linkResult = factory.buildWidgetTreeFromString(kLinkLayout, "link.xml");
    ensure("Hint rejects interactive Inline Content", !linkResult.ok());
    ensure_equals("unsupported Hint inline node diagnostic is stable", linkResult.errors.front().code, "layout.inline.unsupported");

    const rdui::LayoutBuildResult standaloneHint = factory.buildWidgetTreeFromString("<hint>orphan</hint>", "hint.xml");
    ensure("Hint is not a standalone Widget", !standaloneHint.ok());
    ensure_equals("standalone Hint reports its Field-scoped grammar", standaloneHint.errors.front().code, "layout.element.scoped");

    const rdui::LayoutBuildResult standaloneError = factory.buildWidgetTreeFromString("<error>orphan</error>", "error.xml");
    ensure("Error is not a standalone Widget", !standaloneError.ok());
    ensure_equals("standalone Error reports its Field-scoped grammar", standaloneError.errors.front().code, "layout.element.scoped");

    const char* kDuplicateHintLayout = "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/><hint>one</hint><hint>two</hint></field>";
    const rdui::LayoutBuildResult duplicateHint = factory.buildWidgetTreeFromString(kDuplicateHintLayout, "duplicate-hint.xml");
    ensure("Field rejects duplicate Hint content", !duplicateHint.ok());
    ensure_equals("duplicate Hint diagnostic is stable", duplicateHint.errors.front().code, "layout.field.hint_duplicate");

    const rdui::LayoutBuildResult missing = factory.buildWidgetTreeFromString("<field/>", "field-missing.xml");
    ensure("Field requires its direct Label and Value Control", !missing.ok());
    ensure_equals("missing Field Label diagnostic is stable", missing.errors.front().code, "layout.field.label_required");

    const rdui::LayoutBuildResult unsupported =
        factory.buildWidgetTreeFromString("<field><label for=\"button\">Label</label><button id=\"button\"/></field>", "field-child.xml");
    ensure("Field rejects a non-Value-Control child", !unsupported.ok());
    ensure("unsupported Field child diagnostic is reported",
           std::any_of(unsupported.errors.begin(), unsupported.errors.end(),
                       [](const rdui::Diagnostic& diagnostic) { return diagnostic.code == "layout.field.child_unsupported"; }));

    const char* kMismatchLayout = "<panel><switch id=\"other\"/><field><label for=\"other\">Label</label><switch id=\"direct\"/></field></panel>";
    const rdui::LayoutBuildResult mismatch = factory.buildWidgetTreeFromString(kMismatchLayout, "field-target.xml");
    ensure("Field Label must target its direct Value Control", !mismatch.ok());
    ensure("Field target mismatch diagnostic is reported",
           std::any_of(mismatch.errors.begin(), mismatch.errors.end(),
                       [](const rdui::Diagnostic& diagnostic) { return diagnostic.code == "layout.field.label_target_mismatch"; }));

    ensure("Switch rejects setting with authored checked value",
           !factory.buildWidgetTreeFromString("<switch setting=\"demo-enabled\" checked=\"true\"/>", "switch-sources.xml").ok());
    ensure("Switch rejects an empty setting name", !factory.buildWidgetTreeFromString("<switch setting=\"\"/>", "switch-setting.xml").ok());

    const rdui::LayoutBuildResult leadingBreak = factory.buildWidgetTreeFromString("<panel><br/><button/></panel>", "leading-break.xml");
    ensure("Flow Break rejects a leading directive", !leadingBreak.ok());
    ensure_equals("leading Flow Break diagnostic is stable", leadingBreak.errors.front().code, "layout.flow_break.leading");
    const rdui::LayoutBuildResult trailingBreak = factory.buildWidgetTreeFromString("<panel><button/><br/></panel>", "trailing-break.xml");
    ensure("Flow Break rejects a trailing directive", !trailingBreak.ok());
    ensure_equals("trailing Flow Break diagnostic is stable", trailingBreak.errors.front().code, "layout.flow_break.trailing");
    const rdui::LayoutBuildResult consecutiveBreak =
        factory.buildWidgetTreeFromString("<panel><button/><br/><br/><button/></panel>", "consecutive-break.xml");
    ensure("Flow Break rejects consecutive directives", !consecutiveBreak.ok());
    ensure_equals("consecutive Flow Break diagnostic is stable", consecutiveBreak.errors.front().code, "layout.flow_break.consecutive");
    const rdui::LayoutBuildResult attributedBreak =
        factory.buildWidgetTreeFromString("<panel><button/><br class=\"invalid\"/><button/></panel>", "attributed-break.xml");
    ensure("Flow Break rejects Widget attributes", !attributedBreak.ok());
    ensure_equals("Flow Break attribute diagnostic is stable", attributedBreak.errors.front().code, "layout.attribute.unknown");

    const char* kAttributedHintLayout =
        "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/><hint visibility=\"hidden\">Hint</hint></field>";
    const rdui::LayoutBuildResult attributedHint = factory.buildWidgetTreeFromString(kAttributedHintLayout, "attributed-hint.xml");
    ensure("Field-scoped Inline Content rejects Widget attributes", !attributedHint.ok());
    ensure_equals("Hint attribute diagnostic is stable", attributedHint.errors.front().code, "layout.attribute.unknown");

    const rdui::LayoutBuildResult chromeBreak =
        factory.buildWidgetTreeFromString("<floater title=\"title\"><header/><br/><text>content</text></floater>", "chrome-break.xml");
    ensure("non-layout composite slots do not satisfy Flow Break", !chromeBreak.ok());
    ensure_equals("chrome before Flow Break remains a leading directive", chromeBreak.errors.front().code, "layout.flow_break.leading");
}

template<> template<> void fieldObject::test<2>() {
    const char* kInlineLayout =
        "<panel><text id=\"copy\">before <b>bold<i>both</i></b><br/><i>after</i></text><text id=\"title\">Title</text></panel>";
    rdui::LayoutBuildResult result = factory.buildWidgetTreeFromString(kInlineLayout, "inline.xml");
    auto text = result.root ? requireWidget<rdui::Text>(*result.root, "copy") : rdui::WidgetRef<rdui::Text>();
    auto title = result.root ? requireWidget<rdui::Text>(*result.root, "title") : rdui::WidgetRef<rdui::Text>();
    ensure("multiple Text inline-content hosts build", result.ok() && text && title);
    ensure("inline formatting does not create child Widgets", text->children().empty());
    ensure_equals("mixed inline order is retained", text->content().nodes().size(), 5U);
    ensure_equals("plain content remains a text value", static_cast<int>(text->content().nodes()[0].kind()),
                  static_cast<int>(rdui::InlineContentKind::Text));
    ensure_equals("whitespace before a formatted span stays in the parent flow", text->content().nodes()[1].value(), std::string(" "));
    ensure_equals("nested semantic content is retained", text->content().nodes()[2].children().size(), 2U);
    ensure_equals("explicit line break remains an inline value", static_cast<int>(text->content().nodes()[3].kind()),
                  static_cast<int>(rdui::InlineContentKind::Br));
    const char* kLabelLayout =
        "<panel><label id=\"label\" for=\"target\">name <b>important</b><br/><i>detail</i></label><switch id=\"target\"/></panel>";
    rdui::LayoutBuildResult labelResult = factory.buildWidgetTreeFromString(kLabelLayout, "label-inline.xml");
    auto label = labelResult.root ? requireWidget<rdui::Label>(*labelResult.root, "label") : rdui::WidgetRef<rdui::Label>();
    ensure("Label reuses Inline Content without child Widgets", labelResult.ok() && label && label->children().empty());
    ensure_equals("Label exposes flattened semantic text", label->text(), std::string("name important\ndetail"));

    rdui::LocalizationCatalog localization;
    ensure("inline localization fixture loads",
           localization.loadYaml("defaultLocale: en\nlocales: {en: {name: English, strings: {inlineExample: \"First <b>Second</b>\"}}}\n").ok());
    const rdui::LayoutBuildContext context(localization, "en");
    rdui::LayoutBuildResult localizedResult =
        rdui::LayoutResourceCompiler().buildWidgetTreeFromString("<text>inlineExample</text>", "localized-inline.xml", &context);
    auto* localized = localizedResult.rootAs<rdui::Text>();
    ensure("localized inline content builds", localizedResult.ok() && localized);
    const auto& localizedNodes = localized->content().nodes();
    ensure_equals("localized inline structure is retained", localizedNodes.size(), 2U);
    ensure_equals("first localized inline run resolves", localizedNodes[0].value(), std::string("First "));
    ensure_equals("second localized inline run resolves", localizedNodes[1].children()[0].value(), std::string("Second"));

    ensure("removed Heading element rejects the Widget tree", !factory.buildWidgetTreeFromString("<heading>Title</heading>", "heading.xml").ok());

    const rdui::LayoutBuildResult decoration =
        factory.buildWidgetTreeFromString("<text><s>outdated</s> <kbd shortcut=\"toggle-fly\"/></text>", "decoration.xml");
    const auto* decorated = decoration.rootAs<rdui::Text>();
    ensure("strike and keybinding Inline Content compile", decoration.ok() && decorated);
    ensure_equals("strike remains a semantic inline node", static_cast<int>(decorated->content().nodes()[0].kind()),
                  static_cast<int>(rdui::InlineContentKind::S));
    ensure_equals("keybinding command id remains a shortcut id", decorated->content().nodes()[2].shortcutId(), std::string("toggle-fly"));

    const rdui::LayoutBuildResult strikeSpacing = factory.buildWidgetTreeFromString("<text>keep <s>remove</s></text>", "strike-spacing.xml");
    const auto* struck = strikeSpacing.rootAs<rdui::Text>();
    ensure("strike-spacing fixture compiles", strikeSpacing.ok() && struck);
    ensure_equals("space before S remains a plain sibling", struck->content().nodes()[1].value(), std::string(" "));
    ensure_equals("S begins with its authored text rather than the preceding space", struck->content().nodes()[2].children()[0].value(),
                  std::string("remove"));

    const rdui::LayoutBuildResult missingShortcut = factory.buildWidgetTreeFromString("<text><kbd/></text>", "missing-kbd-shortcut.xml");
    ensure("Kbd requires a shortcut", !missingShortcut.ok());
    ensure_equals("missing Kbd shortcut diagnostic is stable", missingShortcut.errors.front().code,
                  std::string("layout.inline.kbd.shortcut_required"));

    const rdui::LayoutBuildResult invalidShortcut =
        factory.buildWidgetTreeFromString("<text><kbd shortcut=\"toggle_fly\"/></text>", "invalid-kbd-shortcut.xml");
    ensure("Kbd requires a canonical command id", !invalidShortcut.ok());
    ensure_equals("invalid Kbd shortcut diagnostic is stable", invalidShortcut.errors.front().code,
                  std::string("layout.inline.kbd.shortcut_invalid"));

    const rdui::LayoutBuildResult widgetChild = factory.buildWidgetTreeFromString("<text><label>not-inline</label></text>", "widget-child.xml");
    ensure("Text rejects Widget children", !widgetChild.ok());
    ensure_equals("inline vocabulary diagnostic is stable", widgetChild.errors.front().code, std::string("layout.inline.element_unknown"));
}

template<> template<> void fieldObject::test<3>() {
    const char* kLabelTargetLayout =
        "<panel><label id=\"toggleLabel\" for=\"toggle\">Enable</label><switch id=\"toggle\" onChange=\"toggleChanged()\"/></panel>";
    rdui::LayoutBuildResult result = factory.buildWidgetTreeFromString(kLabelTargetLayout, "label-target.xml");
    auto label = result.root ? requireWidget<rdui::Label>(*result.root, "toggleLabel") : rdui::WidgetRef<rdui::Label>();
    auto target = result.root ? requireWidget<rdui::Switch>(*result.root, "toggle") : rdui::WidgetRef<rdui::Switch>();
    ensure("same-scope Label target resolves", result.ok() && label && target);
    ensure("resolved Label accepts pointer activation", label->defaultPointerEvents());

    int changes = 0;
    rdui::Binder binder(*result.root);
    bindChangeEvent(binder, "toggleChanged", [&](const rdui::ChangeEvent& event) {
        ensure("Label activation reports the completed Switch value", event.checked);
        ++changes;
    });
    rdui::PreparedBindingResult prepared = binder.prepare();
    const bool bindingPrepared = prepared.ok();
    rdui::Binding binding = bindingPrepared ? prepared.binding.commit() : rdui::Binding{};
    ensure("Label activation action binding commits", bindingPrepared && binding);
    label->activate();
    ensure("Label activation toggles its Switch target", target->checked());
    ensure_equals("Label activation emits the target semantic action", changes, 1);
    target->setDisabled(true);
    label->activate();
    ensure("disabled target ignores Label activation", target->checked());
    ensure_equals("disabled target emits no additional action", changes, 1);
    target->setDisabled(false);
    result.root->setDisabled(true);
    label->activate();
    ensure("disabled target ancestor blocks Label activation", target->checked());
    ensure_equals("disabled target ancestor emits no additional action", changes, 1);
    result.root->setDisabled(false);
    result.root->setVisibility(rdui::Visibility::Hidden);
    label->activate();
    ensure("hidden target ancestor blocks Label activation", target->checked());
    ensure_equals("hidden target ancestor emits no additional action", changes, 1);

    const rdui::LayoutBuildResult required =
        factory.buildWidgetTreeFromString("<panel><label>Missing relationship</label><switch id=\"toggle\"/></panel>", "label-required.xml");
    ensure("Label without for rejects the Widget tree", !required.ok());
    ensure_equals("required Label relationship diagnostic is stable", required.errors.front().code, std::string("layout.label.for_required"));

    const char* kInvalidLabelTargetLayout = "<panel><label for=\"Bad_Target\">Invalid relationship</label><switch id=\"toggle\"/></panel>";
    const rdui::LayoutBuildResult invalid = factory.buildWidgetTreeFromString(kInvalidLabelTargetLayout, "label-invalid.xml");
    ensure("invalid Label target id rejects the Widget tree", !invalid.ok());
    ensure_equals("invalid Label relationship diagnostic is stable", invalid.errors.front().code, std::string("layout.label.for_invalid"));

    const rdui::LayoutBuildResult missing =
        factory.buildWidgetTreeFromString("<panel><label for=\"missing\">Missing target</label></panel>", "label-missing.xml");
    ensure("missing same-scope Label target rejects the Widget tree", !missing.ok());
    ensure_equals("missing Label target diagnostic is stable", missing.errors.front().code, std::string("layout.label.target_missing"));

    const rdui::LayoutBuildResult wrongType =
        factory.buildWidgetTreeFromString("<panel><label for=\"copy\">Wrong target</label><text id=\"copy\">Copy</text></panel>", "label-type.xml");
    ensure("non-labelable Label target rejects the Widget tree", !wrongType.ok());
    ensure_equals("non-labelable target diagnostic is stable", wrongType.errors.front().code, std::string("layout.label.target_not_labelable"));

    resources["nested-target.xml"] = "<panel><switch id=\"nestedTarget\"/></panel>";
    const char* kCrossScopeLabelLayout = "<panel><label for=\"nestedTarget\">Cross scope</label><panel filename=\"nested-target.xml\"/></panel>";
    const rdui::LayoutBuildResult crossScope = factory.buildWidgetTreeFromString(kCrossScopeLabelLayout, "label-scope.xml");
    ensure("cross-scope Label target rejects the Widget tree", !crossScope.ok());
    ensure_equals("cross-scope target is unavailable in the Label scope", crossScope.errors.front().code, std::string("layout.label.target_missing"));

    resources["nested-valid.xml"] = "<panel><label for=\"nestedSwitch\">Nested</label><switch id=\"nestedSwitch\"/></panel>";
    const rdui::LayoutBuildResult nested =
        factory.buildWidgetTreeFromString("<panel><panel filename=\"nested-valid.xml\"/></panel>", "label-nested.xml");
    ensure("Label resolves a target inside its own included-resource scope", nested.ok());
}

template<> template<> void fieldObject::test<4>() {
    const char* kFieldsetLayout =
        "<fieldset id=\"settings\"><legend>Settings <b>demo</b></legend><field><label for=\"first\">First</label><switch id=\"first\"/></field><field><switch id=\"second\"/><label for=\"second\">Second</label><br/><hint>Second hint</hint></field></fieldset>";
    rdui::LayoutBuildResult result = factory.buildWidgetTreeFromString(kFieldsetLayout, "fieldset.xml");
    auto* fieldset = result.rootAs<rdui::Fieldset>();
    ensure("Fieldset with Legend and direct Fields compiles", result.ok() && fieldset);
    ensure("Fieldset exposes its scoped Legend", fieldset->legend());
    ensure_equals("Legend preserves Inline Content", fieldset->legend()->text(), "Settings demo");
    ensure_equals("Fieldset contains one direct Legend and two direct Fields", fieldset->children().size(), 3U);
    ensure("Legend is an ordinary direct child rather than a Part",
           fieldset->legend()->elementName() == "legend" && fieldset->legend()->part().empty());

    auto* controlFirst = dynamic_cast<rdui::Field*>(fieldset->children()[2].get());
    ensure("second direct child is the control-first Field", controlFirst && controlFirst->hint());
    rdui::StyleSheet stylesheet;
    const char* kFieldset =
        "fieldset { width: 120px; height: 90px; padding: 3px 6px; border: 1px #ffffff24; gap: 10px; & > legend { font-weight: bold; font-size: 10px; line-height: 10px; } } field { width: 100px; height: 30px; gap: 2px; } switch { size: 10px; } label, field > hint { width: 20px; height: 10px; }";
    ensure("Fieldset and control-first Field test style compiles", stylesheet.loadRadia(kFieldset).ok());
    rdui::FixedTextMetrics text;
    fieldset->setRect({0.f, 0.f, 120.f, 90.f});
    rdui::layoutTree(*fieldset, stylesheet, text, rdui::LayoutDirection::LeftToRight);
    const rdui::Style legendStyle = rdui::resolveWidgetStyle(stylesheet, *fieldset->legend());
    ensure("direct-child selector styles Legend", legendStyle.fontWeight == 700 && legendStyle.fontSize == 10.f);
    ensure_equals("Legend stays intrinsic instead of stretching across Fieldset", fieldset->legend()->rect().w, fieldset->legend()->desiredSize().x);
    ensure_equals("Legend begins at the Fieldset logical-start padding", fieldset->legend()->rect().left(), 6.f);
    ensure_equals("Fieldset top padding separates content without pushing Legend inward", fieldset->legend()->rect().top(), fieldset->rect().top());
    const rdui::Widget& firstField = *fieldset->children()[1];
    const rdui::Widget& secondField = *fieldset->children()[2];
    ensure_equals("Fieldset gap separates adjacent Fields", firstField.rect().bottom() - secondField.rect().top(), 10.f);

    rdui::RecordingPaintContext recording(text);
    const rdui::Style fieldsetStyle = rdui::resolveWidgetStyle(stylesheet, *fieldset);
    fieldset->paint(recording, fieldsetStyle, 1.f);
    const rdui::PaintCommand* fieldsetBox = recording.last(rdui::PaintCommandKind::Box);
    ensure("Fieldset emits one box with a top-border Legend gap", fieldsetBox && fieldsetBox->topBorderGap.has_value());
    ensure_equals("Fieldset top padding starts at the painted border's inner edge",
                  fieldsetBox->rect.top() - fieldsetStyle.borderWidth.top - firstField.rect().top(), 3.f);
    ensure_equals("Fieldset border crosses the Legend vertical center", fieldsetBox->rect.top(),
                  fieldset->legend()->rect().y + fieldset->legend()->rect().h * .5f);
    ensure("Legend gap clears both text edges",
           fieldsetBox->topBorderGap->left < fieldset->legend()->rect().left()
               && fieldsetBox->topBorderGap->right > fieldset->legend()->rect().right());

    rdui::StyleSheet obsoletePartStylesheet;
    const rdui::StyleSheetLoadResult obsoletePart = obsoletePartStylesheet.loadRadia("fieldset::legend { font-size: 10px; }");
    ensure("removed Fieldset Legend Part selector is rejected", !obsoletePart.ok());
    ensure_equals("removed Legend Part diagnostic is stable", obsoletePart.errors.front().code, std::string("stylesheet.selector.part_unknown"));

    rdui::StyleSheet obsoleteFieldPartStylesheet;
    const rdui::StyleSheetLoadResult obsoleteFieldPart = obsoleteFieldPartStylesheet.loadRadia("field::hint { font-size: 10px; }");
    ensure("authored Field Hint is not exposed as a Part", !obsoleteFieldPart.ok());
    ensure_equals("removed Field Hint Part diagnostic is stable", obsoleteFieldPart.errors.front().code,
                  std::string("stylesheet.selector.part_unknown"));

    rdui::layoutTree(*fieldset, stylesheet, text, rdui::LayoutDirection::RightToLeft);
    ensure_equals("Legend follows logical start in RTL", fieldset->legend()->rect().right(), fieldset->rect().right() - 6.f);

    controlFirst->setRect({0.f, 0.f, 100.f, 30.f});
    rdui::layoutTree(*controlFirst, stylesheet, text, rdui::LayoutDirection::LeftToRight);
    ensure_equals("control-first Hint aligns with Label in LTR", controlFirst->hint()->rect().left(), controlFirst->label()->rect().left());
    rdui::layoutTree(*controlFirst, stylesheet, text, rdui::LayoutDirection::RightToLeft);
    ensure_equals("control-first Hint aligns with Label in RTL", controlFirst->hint()->rect().right(), controlFirst->label()->rect().right());

    const char* kMissingLegendLayout = "<fieldset><field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>";
    const rdui::LayoutBuildResult missingLegend = factory.buildWidgetTreeFromString(kMissingLegendLayout, "missing-legend.xml");
    ensure("Fieldset requires Legend", !missingLegend.ok());
    ensure_equals("missing Legend diagnostic is stable", missingLegend.errors.front().code, "layout.fieldset.legend_required");

    const rdui::LayoutBuildResult missingField =
        factory.buildWidgetTreeFromString("<fieldset><legend>Empty</legend></fieldset>", "missing-field.xml");
    ensure("Fieldset requires at least one direct Field", !missingField.ok());
    ensure_equals("missing Field diagnostic is stable", missingField.errors.front().code, "layout.fieldset.field_required");

    const char* kDuplicateLegendLayout =
        "<fieldset><legend>One</legend><legend>Two</legend><field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>";
    const rdui::LayoutBuildResult duplicateLegend = factory.buildWidgetTreeFromString(kDuplicateLegendLayout, "duplicate-legend.xml");
    ensure("Fieldset rejects duplicate Legend", !duplicateLegend.ok());
    ensure_equals("duplicate Legend diagnostic is stable", duplicateLegend.errors.front().code, "layout.fieldset.legend_duplicate");

    const rdui::LayoutBuildResult unsupportedChild =
        factory.buildWidgetTreeFromString("<fieldset><legend>Settings</legend><panel/></fieldset>", "fieldset-child.xml");
    ensure("Fieldset rejects non-Field children", !unsupportedChild.ok());
    ensure_equals("Fieldset child diagnostic is stable", unsupportedChild.errors.front().code, "layout.fieldset.child_unsupported");

    const char* kFieldsetBreakLayout =
        "<fieldset><legend>Settings</legend><br/><field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>";
    const rdui::LayoutBuildResult flowBreak = factory.buildWidgetTreeFromString(kFieldsetBreakLayout, "fieldset-break.xml");
    ensure("Fieldset rejects Flow Break", !flowBreak.ok());
    ensure_equals("Fieldset Flow Break diagnostic is stable", flowBreak.errors.front().code, "layout.fieldset.flow_break_unsupported");

    const rdui::LayoutBuildResult standaloneLegend = factory.buildWidgetTreeFromString("<legend>Orphan</legend>", "legend.xml");
    ensure("Legend is not a standalone Widget", !standaloneLegend.ok());
    ensure_equals("standalone Legend reports its scoped grammar", standaloneLegend.errors.front().code, "layout.element.scoped");
}

template<> template<> void fieldObject::test<5>() {
    const char* kReorderedFieldsetLayout =
        "<fieldset><field class=\"late\"><label for=\"late\">Late</label><switch id=\"late\"/></field><legend>Settings</legend><field class=\"early\"><label for=\"early\">Early</label><switch id=\"early\"/></field></fieldset>";
    rdui::LayoutBuildResult result = factory.buildWidgetTreeFromString(kReorderedFieldsetLayout, "reordered-fieldset.xml");
    auto* fieldset = result.rootAs<rdui::Fieldset>();
    ensure("reordered Fieldset compiles", result.ok() && fieldset && fieldset->legend());
    auto* late = dynamic_cast<rdui::Field*>(fieldset->children()[0].get());
    auto* early = dynamic_cast<rdui::Field*>(fieldset->children()[2].get());
    ensure("reordered Fieldset fixture retains source order", late && early);

    rdui::StyleSheet stylesheet;
    const char* kReorderedFieldset =
        "fieldset { width: 120px; height: 90px; padding: 3px 6px; border: 1px #ffffff24; gap: 10px; & > legend { order: 100; font-size: 10px; line-height: 10px; } } field { width: 100px; height: 20px; } .early { order: -1; } .late { order: 2; }";
    ensure("reordered Fieldset style compiles", stylesheet.loadRadia(kReorderedFieldset).ok());

    rdui::FixedTextMetrics text;
    fieldset->setRect({0.f, 0.f, 120.f, 90.f});
    rdui::layoutTree(*fieldset, stylesheet, text, rdui::LayoutDirection::LeftToRight);
    ensure_equals("Legend remains the intrinsic first Fieldset item", fieldset->legend()->rect().top(), fieldset->rect().top());
    const float contentTop = fieldset->legend()->rect().y + fieldset->legend()->rect().h * .5f - 1.f - 3.f;
    ensure_equals("topmost ordered Field starts at the border-relative content inset", early->rect().top(), contentTop);
    ensure_equals("Field order remains effective below the Legend", early->rect().bottom() - late->rect().top(), 10.f);
}

template<> template<> void fieldObject::test<6>() {
    rdui::Panel owner;
    auto existingParent = std::make_unique<rdui::Panel>();
    rdui::Panel* parent = existingParent.get();
    auto existingChild = std::make_unique<rdui::Panel>();
    rdui::Panel* child = existingChild.get();
    rdui::detail::WidgetCompilerAccess::setStyleIdentity(*parent, owner.styleElement(), "parent");
    rdui::detail::WidgetCompilerAccess::setStyleIdentity(*child, owner.styleElement(), "parent::child");
    parent->addChild(std::move(existingChild));
    owner.addChild(std::move(existingParent));

    rdui::WidgetContract contract;
    rdui::CompositePartContract nested;
    nested.path = "parent::child";
    nested.parentPath = "parent";
    nested.create = [] { return std::make_unique<rdui::Panel>(); };
    rdui::CompositePartContract root;
    root.path = "parent";
    root.create = [] { return std::make_unique<rdui::Panel>(); };
    contract.compositeParts = {nested, root};

    rdui::detail::instantiateCompositeParts(owner, contract);
    ensure("child-before-parent declaration reuses the existing parent", owner.children().size() == 1U && owner.children().front().get() == parent);
    ensure("child-before-parent declaration reuses the existing child", parent->children().size() == 1U && parent->children().front().get() == child);
    rdui::detail::instantiateCompositePart(owner, contract, "parent::child");
    ensure("explicit nested instantiation remains idempotent", parent->children().size() == 1U);
}
} // namespace tut
