#include "linden_common.h"
#include "../test/lltut.h"
#include "../test/test.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdfield.h"
#include "rdfieldset.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rdtext.h"
#include "rduibinder.h"
#include "rduilayoutdocument.h"
#include "rduilayout.h"
#include "rduilayoutresourcecompiler.h"
#include "rduirecordingpaintcontext.h"
#include "rduiskincompiler.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"
#include <algorithm>
#include <fstream>
#include <map>
#include <sstream>

namespace tut
{
    struct LayoutCompilerFixture
    {
        std::map<std::string, std::string> resources;

        rdui::LayoutDocumentMap documents(rdui::DiagnosticResult& diagnostics) const
        {
            rdui::LayoutDocumentMap result;
            for (const auto& [resource_id, source] : resources)
            {
                rdui::LayoutDocumentParseResult parsed = rdui::LayoutDocumentParser().parse(source, resource_id);
                diagnostics.warnings.insert(diagnostics.warnings.end(), parsed.warnings.begin(), parsed.warnings.end());
                diagnostics.errors.insert(diagnostics.errors.end(), parsed.errors.begin(), parsed.errors.end());
                if (parsed.document)
                    result.emplace(resource_id, std::shared_ptr<const rdui::LayoutDocument>(std::move(parsed.document)));
            }
            return result;
        }

        rdui::ViewBuildResult createFromResource(const std::string& resource_id) const
        {
            rdui::ViewBuildResult result;
            rdui::LayoutDocumentMap parsed = documents(result);
            if (result.hasErrors()) return result;
            return rdui::LayoutResourceCompiler(&parsed).createFromResource(resource_id);
        }

        rdui::ViewBuildResult createFromString(const std::string& xml, const std::string& source = {}) const
        {
            rdui::ViewBuildResult result;
            rdui::LayoutDocumentMap parsed = documents(result);
            if (result.hasErrors()) return result;
            return rdui::LayoutResourceCompiler(&parsed).createFromString(xml, source);
        }

        rdui::DiagnosticResult validateWidgetDefaults(const std::string& element) const
        {
            rdui::DiagnosticResult result;
            rdui::LayoutDocumentMap parsed = documents(result);
            if (result.hasErrors()) return result;
            return rdui::LayoutResourceCompiler(&parsed).validateWidgetDefaults(element);
        }
    };

    struct rduilayoutresourcecompiler_data
    {
        LayoutCompilerFixture factory;
        std::map<std::string, std::string>& resources = factory.resources;

        template<typename WidgetT>
        rdui::WidgetRef<WidgetT> requireWidget(rdui::Widget& root, const std::string& id)
        {
            rdui::WidgetRef<WidgetT> output;
            rdui::Binder binder(root);
            binder.bind(id, output);
            rdui::BindingResult result = binder.finish();
            if (!result.ok()) output.set(nullptr);
            return output;
        }
    };
    typedef test_group<rduilayoutresourcecompiler_data> rduicomponentcontract_test;
    typedef rduicomponentcontract_test::object rduicomponentcontract_object;
    rduicomponentcontract_test rduicomponentcontract_testcase("rduicomponentcontract");

    template<> template<>
    void rduicomponentcontract_object::test<1>()
    {
        const rdui::ViewBuildResult result = factory.createFromString(
            "<field id=\"example-field\"><label for=\"field-switch\">label.example</label>"
            "<switch id=\"field-switch\" checked=\"true\" onChange=\"switch-changed\"/><br/>"
            "<hint>Helpful <b>detail</b></hint><br/><error>Fallback error</error></field>", "field.xml");
        auto* field = result.rootAs<rdui::Field>();
        ensure("field markup builds", result.ok() && field);
        ensure("Field exposes its direct Label and Value Control", field->label() && field->control());
        ensure("Field exposes scoped Hint and Error Text Hosts", field->hint() && field->error());
        ensure("authored Hint and Error remain ordinary direct children",
               field->hint()->parent() == field && field->hint()->part().empty()
               && field->error()->parent() == field && field->error()->part().empty());
        ensure_equals("Hint preserves shared Inline Content", field->hint()->text(), "Helpful detail");
        ensure_equals("authored Error is retained as fallback", field->error()->text(), "Fallback error");
        ensure_equals("Error starts collapsed while the Value Control is valid",
                      static_cast<int>(field->error()->visibility()), static_cast<int>(rdui::Visibility::Collapsed));
        ensure("Flow Break marks Hint for a new row", field->hint()->flowBreakBefore());
        ensure("Flow Break marks Error for a new row", field->error()->flowBreakBefore());

        const rdui::ViewBuildResult link_result = factory.createFromString(
            "<field><label for=\"field-switch\">Label</label><switch id=\"field-switch\"/>"
            "<hint><link href=\"https://example.com\">link</link></hint></field>", "link.xml");
        ensure("Hint rejects interactive Inline Content", !link_result.ok());
        ensure_equals("unsupported Hint inline node diagnostic is stable", link_result.errors.front().code,
                      "view.inline.unsupported");

        const rdui::ViewBuildResult standalone_hint = factory.createFromString("<hint>orphan</hint>", "hint.xml");
        ensure("Hint is not a standalone Widget", !standalone_hint.ok());
        ensure_equals("standalone Hint reports its Field-scoped grammar", standalone_hint.errors.front().code,
                      "view.element.scoped");

        const rdui::ViewBuildResult standalone_error = factory.createFromString("<error>orphan</error>", "error.xml");
        ensure("Error is not a standalone Widget", !standalone_error.ok());
        ensure_equals("standalone Error reports its Field-scoped grammar", standalone_error.errors.front().code,
                      "view.element.scoped");

        const rdui::ViewBuildResult duplicate_hint = factory.createFromString(
            "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/>"
            "<hint>one</hint><hint>two</hint></field>", "duplicate-hint.xml");
        ensure("Field rejects duplicate Hint content", !duplicate_hint.ok());
        ensure_equals("duplicate Hint diagnostic is stable", duplicate_hint.errors.front().code,
                      "view.field.hint_duplicate");

        const rdui::ViewBuildResult missing = factory.createFromString("<field/>", "field-missing.xml");
        ensure("Field requires its direct Label and Value Control", !missing.ok());
        ensure_equals("missing Field Label diagnostic is stable", missing.errors.front().code,
                      "view.field.label_required");

        const rdui::ViewBuildResult unsupported = factory.createFromString(
            "<field><label for=\"button\">Label</label><button id=\"button\"/></field>", "field-child.xml");
        ensure("Field rejects a non-Value-Control child", !unsupported.ok());
        ensure("unsupported Field child diagnostic is reported",
               std::any_of(unsupported.errors.begin(), unsupported.errors.end(), [](const rdui::Diagnostic& diagnostic)
               {
                   return diagnostic.code == "view.field.child_unsupported";
               }));

        const rdui::ViewBuildResult mismatch = factory.createFromString(
            "<panel><switch id=\"other\"/><field><label for=\"other\">Label</label>"
            "<switch id=\"direct\"/></field></panel>", "field-target.xml");
        ensure("Field Label must target its direct Value Control", !mismatch.ok());
        ensure("Field target mismatch diagnostic is reported",
               std::any_of(mismatch.errors.begin(), mismatch.errors.end(), [](const rdui::Diagnostic& diagnostic)
               {
                   return diagnostic.code == "view.field.label_target_mismatch";
               }));

        ensure("Switch rejects two authored value sources", !factory.createFromString(
            "<switch bind=\"demo-enabled\" checked=\"true\"/>", "switch-sources.xml").ok());
        ensure("Switch rejects an invalid binding identifier", !factory.createFromString(
            "<switch bind=\"Bad_Name\"/>", "switch-bind.xml").ok());

        const rdui::ViewBuildResult leading_break = factory.createFromString(
            "<panel><br/><button/></panel>", "leading-break.xml");
        ensure("Flow Break rejects a leading directive", !leading_break.ok());
        ensure_equals("leading Flow Break diagnostic is stable", leading_break.errors.front().code,
                      "view.flow_break.leading");
        const rdui::ViewBuildResult trailing_break = factory.createFromString(
            "<panel><button/><br/></panel>", "trailing-break.xml");
        ensure("Flow Break rejects a trailing directive", !trailing_break.ok());
        ensure_equals("trailing Flow Break diagnostic is stable", trailing_break.errors.front().code,
                      "view.flow_break.trailing");
        const rdui::ViewBuildResult consecutive_break = factory.createFromString(
            "<panel><button/><br/><br/><button/></panel>", "consecutive-break.xml");
        ensure("Flow Break rejects consecutive directives", !consecutive_break.ok());
        ensure_equals("consecutive Flow Break diagnostic is stable", consecutive_break.errors.front().code,
                      "view.flow_break.consecutive");
        const rdui::ViewBuildResult attributed_break = factory.createFromString(
            "<panel><button/><br class=\"invalid\"/><button/></panel>", "attributed-break.xml");
        ensure("Flow Break rejects Widget attributes", !attributed_break.ok());
        ensure_equals("Flow Break attribute diagnostic is stable", attributed_break.errors.front().code,
                      "view.attribute.unknown");

        const rdui::ViewBuildResult attributed_hint = factory.createFromString(
            "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/>"
            "<hint visibility=\"hidden\">Hint</hint></field>", "attributed-hint.xml");
        ensure("Field-scoped Inline Content rejects Widget attributes", !attributed_hint.ok());
        ensure_equals("Hint attribute diagnostic is stable", attributed_hint.errors.front().code,
                      "view.attribute.unknown");

        const rdui::ViewBuildResult chrome_break = factory.createFromString(
            "<floater title=\"title\"><header/><br/><text>content</text></floater>", "chrome-break.xml");
        ensure("non-layout composite slots do not satisfy Flow Break", !chrome_break.ok());
        ensure_equals("chrome before Flow Break remains a leading directive", chrome_break.errors.front().code,
                      "view.flow_break.leading");
    }

    template<> template<>
    void rduicomponentcontract_object::test<2>()
    {
        rdui::ViewBuildResult result = factory.createFromString(
            "<panel><text id=\"copy\">before <b>bold<i>both</i></b><br/><i>after</i></text>"
            "<text id=\"title\">Title</text></panel>", "inline.xml");
        auto text = result.root ? requireWidget<rdui::Text>(*result.root, "copy") : rdui::WidgetRef<rdui::Text>();
        auto title = result.root ? requireWidget<rdui::Text>(*result.root, "title") : rdui::WidgetRef<rdui::Text>();
        ensure("multiple Text inline-content hosts build", result.ok() && text && title);
        ensure("inline formatting does not create child Widgets", text->children().empty());
        ensure_equals("mixed inline order is retained", text->content().nodes().size(), 5U);
        ensure_equals("plain content remains a text value", static_cast<int>(text->content().nodes()[0].kind()),
                      static_cast<int>(rdui::InlineContentKind::Text));
        ensure_equals("whitespace before a formatted span stays in the parent flow",
                      text->content().nodes()[1].value(), std::string(" "));
        ensure_equals("nested semantic content is retained", text->content().nodes()[2].children().size(), 2U);
        ensure_equals("explicit line break remains an inline value",
                      static_cast<int>(text->content().nodes()[3].kind()), static_cast<int>(rdui::InlineContentKind::Br));
        rdui::ViewBuildResult label_result = factory.createFromString(
            "<panel><label id=\"label\" for=\"target\">name <b>important</b><br/><i>detail</i></label>"
            "<switch id=\"target\"/></panel>", "label-inline.xml");
        auto label = label_result.root
                   ? requireWidget<rdui::Label>(*label_result.root, "label") : rdui::WidgetRef<rdui::Label>();
        ensure("Label reuses Inline Content without child Widgets",
               label_result.ok() && label && label->children().empty());
        ensure_equals("Label exposes flattened semantic text", label->text(),
                      std::string("name important\ndetail"));

        rdui::LocalizationCatalog localization;
        ensure("inline localization fixture loads", localization.loadYaml(R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      inlineExample: "First <b>Second</b>"
)YAML").ok());
        const rdui::ViewBuildContext context(localization, "en");
        rdui::ViewBuildResult localized_result = rdui::LayoutResourceCompiler().createFromString(
            "<text>inlineExample</text>", "localized-inline.xml", &context);
        auto* localized = localized_result.rootAs<rdui::Text>();
        ensure("localized inline content builds", localized_result.ok() && localized);
        const auto& localized_nodes = localized->content().nodes();
        ensure_equals("localized inline structure is retained", localized_nodes.size(), 2U);
        ensure_equals("first localized inline run resolves", localized_nodes[0].value(),
                      std::string("First "));
        ensure_equals("second localized inline run resolves",
                      localized_nodes[1].children()[0].value(), std::string("Second"));

        ensure("removed Heading element rejects the View",
               !factory.createFromString("<heading>Title</heading>", "heading.xml").ok());

        const rdui::ViewBuildResult decoration = factory.createFromString(
            "<text><s>outdated</s> <kbd binding=\"toggle-fly\"/></text>", "decoration.xml");
        const auto* decorated = decoration.rootAs<rdui::Text>();
        ensure("strike and keybinding Inline Content compile", decoration.ok() && decorated);
        ensure_equals("strike remains a semantic inline node",
                      static_cast<int>(decorated->content().nodes()[0].kind()),
                      static_cast<int>(rdui::InlineContentKind::S));
        ensure_equals("keybinding command id remains presentation metadata",
                      decorated->content().nodes()[2].metadata(), std::string("toggle-fly"));

        const rdui::ViewBuildResult strike_spacing = factory.createFromString(
            "<text>keep <s>remove</s></text>", "strike-spacing.xml");
        const auto* struck = strike_spacing.rootAs<rdui::Text>();
        ensure("strike-spacing fixture compiles", strike_spacing.ok() && struck);
        ensure_equals("space before S remains a plain sibling", struck->content().nodes()[1].value(),
                      std::string(" "));
        ensure_equals("S begins with its authored text rather than the preceding space",
                      struck->content().nodes()[2].children()[0].value(), std::string("remove"));

        const rdui::ViewBuildResult missing_binding = factory.createFromString(
            "<text><kbd/></text>", "missing-kbd-binding.xml");
        ensure("Kbd requires a binding", !missing_binding.ok());
        ensure_equals("missing Kbd binding diagnostic is stable", missing_binding.errors.front().code,
                      std::string("view.inline.kbd.binding_required"));

        const rdui::ViewBuildResult invalid_binding = factory.createFromString(
            "<text><kbd binding=\"toggle_fly\"/></text>", "invalid-kbd-binding.xml");
        ensure("Kbd requires a canonical command id", !invalid_binding.ok());
        ensure_equals("invalid Kbd binding diagnostic is stable", invalid_binding.errors.front().code,
                      std::string("view.inline.kbd.binding_invalid"));

        const rdui::ViewBuildResult widget_child = factory.createFromString(
            "<text><label>not-inline</label></text>", "widget-child.xml");
        ensure("Text rejects Widget children", !widget_child.ok());
        ensure_equals("inline vocabulary diagnostic is stable", widget_child.errors.front().code,
                      std::string("view.inline.element_unknown"));
    }

    template<> template<>
    void rduicomponentcontract_object::test<3>()
    {
        rdui::ViewBuildResult result = factory.createFromString(
            "<panel><label id=\"toggle-label\" for=\"toggle\">Enable</label>"
            "<switch id=\"toggle\" onChange=\"toggle-changed\"/></panel>", "label-target.xml");
        auto label = result.root
                   ? requireWidget<rdui::Label>(*result.root, "toggle-label") : rdui::WidgetRef<rdui::Label>();
        auto target = result.root
                    ? requireWidget<rdui::Switch>(*result.root, "toggle") : rdui::WidgetRef<rdui::Switch>();
        ensure("same-scope Label target resolves", result.ok() && label && target);
        ensure("resolved Label accepts pointer activation", label->defaultPointerEvents());

        int changes = 0;
        rdui::Binder binder(*result.root);
        binder.onChange("toggle-changed", [&](const rdui::ChangeActionEvent& event)
        {
            ensure("Label activation reports the completed Switch value", event.checked);
            ++changes;
        });
        rdui::BindingResult binding = binder.finish();
        ensure("Label activation action binding commits", binding.ok());
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

        const rdui::ViewBuildResult required = factory.createFromString(
            "<panel><label>Missing relationship</label><switch id=\"toggle\"/></panel>", "label-required.xml");
        ensure("Label without for rejects the View", !required.ok());
        ensure_equals("required Label relationship diagnostic is stable", required.errors.front().code,
                      std::string("view.label.for_required"));

        const rdui::ViewBuildResult invalid = factory.createFromString(
            "<panel><label for=\"Bad_Target\">Invalid relationship</label>"
            "<switch id=\"toggle\"/></panel>", "label-invalid.xml");
        ensure("invalid Label target id rejects the View", !invalid.ok());
        ensure_equals("invalid Label relationship diagnostic is stable", invalid.errors.front().code,
                      std::string("view.label.for_invalid"));

        const rdui::ViewBuildResult missing = factory.createFromString(
            "<panel><label for=\"missing\">Missing target</label></panel>", "label-missing.xml");
        ensure("missing same-scope Label target rejects the View", !missing.ok());
        ensure_equals("missing Label target diagnostic is stable", missing.errors.front().code,
                      std::string("view.label.target_missing"));

        const rdui::ViewBuildResult wrong_type = factory.createFromString(
            "<panel><label for=\"copy\">Wrong target</label><text id=\"copy\">Copy</text></panel>",
            "label-type.xml");
        ensure("non-labelable Label target rejects the View", !wrong_type.ok());
        ensure_equals("non-labelable target diagnostic is stable", wrong_type.errors.front().code,
                      std::string("view.label.target_not_labelable"));

        resources["nested-target.xml"] = "<panel><switch id=\"nested-target\"/></panel>";
        const rdui::ViewBuildResult cross_scope = factory.createFromString(
            "<panel><label for=\"nested-target\">Cross scope</label>"
            "<panel filename=\"nested-target.xml\"/></panel>", "label-scope.xml");
        ensure("cross-scope Label target rejects the View", !cross_scope.ok());
        ensure_equals("cross-scope target is unavailable in the Label scope", cross_scope.errors.front().code,
                      std::string("view.label.target_missing"));

        resources["nested-valid.xml"] =
            "<panel><label for=\"nested-switch\">Nested</label><switch id=\"nested-switch\"/></panel>";
        const rdui::ViewBuildResult nested = factory.createFromString(
            "<panel><panel filename=\"nested-valid.xml\"/></panel>", "label-nested.xml");
        ensure("Label resolves a target inside its own included-resource scope", nested.ok());
    }

    template<> template<>
    void rduicomponentcontract_object::test<4>()
    {
        rdui::ViewBuildResult result = factory.createFromString(
            "<fieldset id=\"settings\"><legend>Settings <b>demo</b></legend>"
            "<field><label for=\"first\">First</label><switch id=\"first\"/></field>"
            "<field><switch id=\"second\"/><label for=\"second\">Second</label><br/>"
            "<hint>Second hint</hint></field></fieldset>", "fieldset.xml");
        auto* fieldset = result.rootAs<rdui::Fieldset>();
        ensure("Fieldset with Legend and direct Fields compiles", result.ok() && fieldset);
        ensure("Fieldset exposes its scoped Legend", fieldset->legend());
        ensure_equals("Legend preserves Inline Content", fieldset->legend()->text(), "Settings demo");
        ensure_equals("Fieldset contains one direct Legend and two direct Fields",
                      fieldset->children().size(), 3U);
        ensure("Legend is an ordinary direct child rather than a Part",
               fieldset->legend()->element() == "legend" && fieldset->legend()->part().empty());

        auto* control_first = dynamic_cast<rdui::Field*>(fieldset->children()[2].get());
        ensure("second direct child is the control-first Field", control_first && control_first->hint());
        rdui::StyleSheet theme;
        ensure("Fieldset and control-first Field test style compiles", theme.loadRadia(
            "fieldset { width: 120px; height: 90px; padding: 3px 6px; border: 1px #ffffff24; gap: 10px; "
            "  & > legend { font-weight: bold; font-size: 10px; line-height: 10px; } } "
            "field { width: 100px; height: 30px; gap: 2px; } "
            "switch { size: 10px; } label, field > hint { width: 20px; height: 10px; }").ok());
        rdui::FixedTextMetrics text;
        fieldset->setRect({0.f, 0.f, 120.f, 90.f});
        rdui::layoutTree(*fieldset, theme, text, rdui::LayoutDirection::LeftToRight);
        const rdui::Style legend_style = rdui::resolveWidgetStyle(theme, *fieldset->legend());
        ensure("direct-child selector styles Legend",
               legend_style.font_bold && legend_style.font_size == 10.f);
        ensure_equals("Legend stays intrinsic instead of stretching across Fieldset",
                      fieldset->legend()->rect().w, fieldset->legend()->desiredSize().x);
        ensure_equals("Legend begins at the Fieldset logical-start padding",
                      fieldset->legend()->rect().left(), 6.f);
        ensure_equals("Fieldset top padding separates content without pushing Legend inward",
                      fieldset->legend()->rect().top(), fieldset->rect().top());
        const rdui::Widget& first_field = *fieldset->children()[1];
        const rdui::Widget& second_field = *fieldset->children()[2];
        ensure_equals("Fieldset gap separates adjacent Fields",
                      first_field.rect().bottom() - second_field.rect().top(), 10.f);

        rdui::RecordingPaintContext recording(text);
        const rdui::Style fieldset_style = rdui::resolveWidgetStyle(theme, *fieldset);
        fieldset->paint(recording, fieldset_style, 1.f);
        const rdui::PaintCommand* fieldset_box = recording.last(rdui::PaintCommandKind::Box);
        ensure("Fieldset emits one box with a top-border Legend gap",
               fieldset_box && fieldset_box->top_border_gap.has_value());
        ensure_equals("Fieldset top padding starts at the painted border's inner edge",
                      fieldset_box->rect.top() - fieldset_style.border_width.top
                          - first_field.rect().top(),
                      3.f);
        ensure_equals("Fieldset border crosses the Legend vertical center",
                      fieldset_box->rect.top(), fieldset->legend()->rect().y + fieldset->legend()->rect().h * .5f);
        ensure("Legend gap clears both text edges",
               fieldset_box->top_border_gap->left < fieldset->legend()->rect().left()
               && fieldset_box->top_border_gap->right > fieldset->legend()->rect().right());

        rdui::StyleSheet obsolete_part_theme;
        const rdui::StyleSheetLoadResult obsolete_part =
            obsolete_part_theme.loadRadia("fieldset::legend { font-size: 10px; }");
        ensure("removed Fieldset Legend Part selector is rejected", !obsolete_part.ok());
        ensure_equals("removed Legend Part diagnostic is stable", obsolete_part.errors.front().code,
                      std::string("stylesheet.selector.part_unknown"));

        rdui::StyleSheet obsolete_field_part_theme;
        const rdui::StyleSheetLoadResult obsolete_field_part =
            obsolete_field_part_theme.loadRadia("field::hint { font-size: 10px; }");
        ensure("authored Field Hint is not exposed as a Part", !obsolete_field_part.ok());
        ensure_equals("removed Field Hint Part diagnostic is stable", obsolete_field_part.errors.front().code,
                      std::string("stylesheet.selector.part_unknown"));

        rdui::layoutTree(*fieldset, theme, text, rdui::LayoutDirection::RightToLeft);
        ensure_equals("Legend follows logical start in RTL",
                      fieldset->legend()->rect().right(), fieldset->rect().right() - 6.f);

        control_first->setRect({0.f, 0.f, 100.f, 30.f});
        rdui::layoutTree(*control_first, theme, text, rdui::LayoutDirection::LeftToRight);
        ensure_equals("control-first Hint aligns with Label in LTR",
                      control_first->hint()->rect().left(), control_first->label()->rect().left());
        rdui::layoutTree(*control_first, theme, text, rdui::LayoutDirection::RightToLeft);
        ensure_equals("control-first Hint aligns with Label in RTL",
                      control_first->hint()->rect().right(), control_first->label()->rect().right());

        const rdui::ViewBuildResult missing_legend = factory.createFromString(
            "<fieldset><field><label for=\"toggle\">Label</label>"
            "<switch id=\"toggle\"/></field></fieldset>", "missing-legend.xml");
        ensure("Fieldset requires Legend", !missing_legend.ok());
        ensure_equals("missing Legend diagnostic is stable", missing_legend.errors.front().code,
                      "view.fieldset.legend_required");

        const rdui::ViewBuildResult missing_field = factory.createFromString(
            "<fieldset><legend>Empty</legend></fieldset>", "missing-field.xml");
        ensure("Fieldset requires at least one direct Field", !missing_field.ok());
        ensure_equals("missing Field diagnostic is stable", missing_field.errors.front().code,
                      "view.fieldset.field_required");

        const rdui::ViewBuildResult duplicate_legend = factory.createFromString(
            "<fieldset><legend>One</legend><legend>Two</legend>"
            "<field><label for=\"toggle\">Label</label><switch id=\"toggle\"/></field></fieldset>",
            "duplicate-legend.xml");
        ensure("Fieldset rejects duplicate Legend", !duplicate_legend.ok());
        ensure_equals("duplicate Legend diagnostic is stable", duplicate_legend.errors.front().code,
                      "view.fieldset.legend_duplicate");

        const rdui::ViewBuildResult unsupported_child = factory.createFromString(
            "<fieldset><legend>Settings</legend><panel/></fieldset>", "fieldset-child.xml");
        ensure("Fieldset rejects non-Field children", !unsupported_child.ok());
        ensure_equals("Fieldset child diagnostic is stable", unsupported_child.errors.front().code,
                      "view.fieldset.child_unsupported");

        const rdui::ViewBuildResult flow_break = factory.createFromString(
            "<fieldset><legend>Settings</legend><br/><field><label for=\"toggle\">Label</label>"
            "<switch id=\"toggle\"/></field></fieldset>", "fieldset-break.xml");
        ensure("Fieldset rejects Flow Break", !flow_break.ok());
        ensure_equals("Fieldset Flow Break diagnostic is stable", flow_break.errors.front().code,
                      "view.fieldset.flow_break_unsupported");

        const rdui::ViewBuildResult standalone_legend = factory.createFromString(
            "<legend>Orphan</legend>", "legend.xml");
        ensure("Legend is not a standalone Widget", !standalone_legend.ok());
        ensure_equals("standalone Legend reports its scoped grammar", standalone_legend.errors.front().code,
                      "view.element.scoped");
    }

    template<> template<>
    void rduicomponentcontract_object::test<5>()
    {
        rdui::ViewBuildResult result = factory.createFromString(
            "<fieldset>"
            "<field class=\"late\"><label for=\"late\">Late</label><switch id=\"late\"/></field>"
            "<legend>Settings</legend>"
            "<field class=\"early\"><label for=\"early\">Early</label><switch id=\"early\"/></field>"
            "</fieldset>", "reordered-fieldset.xml");
        auto* fieldset = result.rootAs<rdui::Fieldset>();
        ensure("reordered Fieldset compiles", result.ok() && fieldset && fieldset->legend());
        auto* late = dynamic_cast<rdui::Field*>(fieldset->children()[0].get());
        auto* early = dynamic_cast<rdui::Field*>(fieldset->children()[2].get());
        ensure("reordered Fieldset fixture retains source order", late && early);

        rdui::StyleSheet theme;
        ensure("reordered Fieldset style compiles", theme.loadRadia(
            "fieldset { width: 120px; height: 90px; padding: 3px 6px; "
            "border: 1px #ffffff24; gap: 10px; "
            "& > legend { order: 100; font-size: 10px; line-height: 10px; } } "
            "field { width: 100px; height: 20px; } "
            ".early { order: -1; } .late { order: 2; }").ok());

        rdui::FixedTextMetrics text;
        fieldset->setRect({0.f, 0.f, 120.f, 90.f});
        rdui::layoutTree(*fieldset, theme, text, rdui::LayoutDirection::LeftToRight);
        ensure_equals("Legend remains the intrinsic first Fieldset item",
                      fieldset->legend()->rect().top(), fieldset->rect().top());
        const float content_top = fieldset->legend()->rect().y
            + fieldset->legend()->rect().h * .5f - 1.f - 3.f;
        ensure_equals("topmost ordered Field starts at the border-relative content inset",
                      early->rect().top(), content_top);
        ensure_equals("Field order remains effective below the Legend",
                      early->rect().bottom() - late->rect().top(), 10.f);
    }

}
