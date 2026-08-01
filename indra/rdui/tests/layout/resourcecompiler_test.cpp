/**
 * @file resourcecompiler_test.cpp
 * @brief
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
#include <map>
#include <sstream>
#include "../test/lltut.h"
#include "../test/test.h"
#include "binding/binder.h"
#include "fixture.h"
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

namespace tut {
struct rduilayoutresourcecompiler_data {
    LayoutCompilerFixture factory;
    std::map<std::string, std::string>& resources = factory.resources;

    template<typename WidgetT> rdui::WidgetRef<WidgetT> requireWidget(rdui::Widget& root, const std::string& id) {
        rdui::WidgetRef<WidgetT> output;
        rdui::Binder binder(root);
        binder.bind(id, output);
        rdui::BindingResult result = binder.finish();
        if (!result.ok()) output.set(nullptr);
        return output;
    }
};
typedef test_group<rduilayoutresourcecompiler_data> rduilayoutresourcecompiler_test;
typedef rduilayoutresourcecompiler_test::object rduilayoutresourcecompiler_object;
rduilayoutresourcecompiler_test rduilayoutresourcecompiler_testcase("rduilayoutresourcecompiler");

template<> template<> void rduilayoutresourcecompiler_object::test<1>() {
    const char* xml = "<floater title=\"title\" closeIcon=\"close\" minimizeIcon=\"minimize\" canMinimize=\"true\"><text "
                      "id=\"status\">Ready</text>"
                      "<button id=\"go\" onClick=\"demo-go\" onDoubleClick=\"demo-double\" onMouseDown=\"demo-press\" "
                      "onLongClick=\"demo-hold\" "
                      "onContextMenu=\"demo-menu\" longClickDelay=\"750ms\"><icon src=\"search\"/>Go</button>"
                      "<switch id=\"toggle\" checked=\"true\" onChange=\"demo-changed\"/></floater>";
    rdui::ViewBuildResult result = factory.createFromString(xml, "floater.xml");
    auto* floater = result.rootAs<rdui::Floater>();
    ensure("arbitrary floater root parsed", result.ok() && floater);
    ensure_equals("title localization key retained without a System", floater->title(), "title");
    ensure("declared chrome composed", floater->header() && floater->minimizeButton());
    auto go = requireWidget<rdui::Button>(*floater, "go");
    auto toggle = requireWidget<rdui::Switch>(*floater, "toggle");
    ensure("Binder resolves parsed controls", go && toggle);
    ensure("typed lookup", toggle->checked());
    ensure_equals("click action parsed", go->action(rdui::ActionEventKind::Click), "demo-go");
    ensure_equals("double click action parsed", go->action(rdui::ActionEventKind::DoubleClick), "demo-double");
    ensure_equals("mouse action parsed", go->action(rdui::ActionEventKind::MouseDown), "demo-press");
    ensure_equals("long click action parsed", go->action(rdui::ActionEventKind::LongClick), "demo-hold");
    ensure_equals("context menu action parsed", go->action(rdui::ActionEventKind::ContextMenu), "demo-menu");
    ensure_equals("long click delay parsed", go->longClickDelay()->count(), 750LL);
    ensure_equals("switch action parsed", toggle->action(rdui::ActionEventKind::Change), "demo-changed");
}

template<> template<> void rduilayoutresourcecompiler_object::test<2>() {
    resources["shared.xml"] = "<panel id=\"base\" class=\"shared\"><text id=\"resource-child\">base</text></panel>";
    const char* xml = "<panel><panel filename=\"shared.xml\" id=\"one\" class=\"first\"><text id=\"inline-child\"/></panel>"
                      "<panel filename=\"shared.xml\" id=\"two\"/></panel>";
    rdui::ViewBuildResult result = factory.createFromString(xml, "outer.xml");
    ensure("embedded panels parsed", result.ok());
    auto first = requireWidget<rdui::Panel>(*result.root, "one");
    auto second = requireWidget<rdui::Panel>(*result.root, "two");
    ensure("independent panel instances", first && second && first.get() != second.get());
    ensure("referenced class retained", first->classes().count("shared") == 1);
    ensure("inline class appended", first->classes().count("first") == 1);
    ensure("referenced child first", first->children().front()->id() == "resource-child");
    ensure("inline child appended", first->children().back()->id() == "inline-child");
    first->setVisibility(rdui::Visibility::Collapsed);
    ensure("second visibility independent", second->visibility() == rdui::Visibility::Visible);
}

template<> template<> void rduilayoutresourcecompiler_object::test<3>() {
    resources["nested/inner.xml"] = "<panel id=\"inner\"/>";
    resources["nested/middle.xml"] = "<panel><panel filename=\"inner.xml\"/></panel>";
    resources["outer.xml"] = "<panel><panel filename=\"nested/middle.xml\"/></panel>";
    const rdui::ViewBuildResult result = factory.createFromResource("outer.xml");
    ensure("nested relative panels load", result.ok());
    ensure("deep child present",
           result.root->children().size() == 1
               && result.root->children()[0]->children().size() == 1
               && result.root->children()[0]->children()[0]->id() == "inner");
}

template<> template<> void rduilayoutresourcecompiler_object::test<4>() {
    resources["a.xml"] = "<panel><panel filename=\"b.xml\"/></panel>";
    resources["b.xml"] = "<panel><panel filename=\"a.xml\"/></panel>";
    rdui::ViewBuildResult cycle = factory.createFromResource("a.xml");
    ensure("cycle rejected", !cycle.ok());
    ensure("failed build never exposes a partial tree", !cycle.root);
    ensure_equals("cycle has stable diagnostic code", cycle.errors.front().code, "view.resource.cycle");
    ensure_equals("cycle diagnostic identifies source", cycle.errors.front().source, "a.xml");
    ensure("missing panel rejected", !factory.createFromString("<panel><panel filename=\"missing.xml\"/></panel>", "root.xml").ok());
    resources["wrong.xml"] = "<text/>";
    ensure("non-panel reference rejected", !factory.createFromString("<panel><panel filename=\"wrong.xml\"/></panel>", "root.xml").ok());
    ensure("filename on non-panel rejected", !factory.createFromString("<button filename=\"x.xml\"/>").ok());
    ensure("resource-root escape rejected", !factory.createFromString("<panel><panel filename=\"../outside.xml\"/></panel>", "root.xml").ok());

    resources["empty.xml"] = "";
    const rdui::ViewBuildResult empty = factory.createFromResource("empty.xml");
    ensure("empty resource rejected as invalid XML", !empty.ok());
    ensure_equals("empty resource differs from missing resource", empty.errors.front().code, "view.xml.invalid");
}

template<> template<> void rduilayoutresourcecompiler_object::test<5>() {
    const rdui::ViewBuildResult invalid = factory.createFromString("<panel>\n  <unknown/>\n</panel>", "source_ranges.xml");
    ensure("source-aware compiler rejects unknown child", !invalid.ok());
    ensure_equals("compiler diagnostic retains source line", invalid.errors.front().line, 2U);
    ensure_equals("compiler diagnostic retains source column", invalid.errors.front().column, 3U);

    rdui::LayoutDocumentParseResult parsed = rdui::LayoutDocumentParser().parse("<panel>before<label>middle</label>after</panel>", "mixed.xml");
    ensure("one document tree parses", parsed.ok());
    ensure_equals("mixed content order is represented once", parsed.document->root->content.size(), 3U);
    ensure("text-child-text order retained",
           parsed.document->root->content[0].isText() && !parsed.document->root->content[1].isText() && parsed.document->root->content[2].isText());
    ensure("element source range retained",
           parsed.document->root->content[1].source.begin.line == 1
               && parsed.document->root->content[1].source.end.offset >= parsed.document->root->content[1].source.begin.offset);
}

template<> template<> void rduilayoutresourcecompiler_object::test<6>() {
    rdui::ViewBuildResult result = factory.createFromString("<button>first<icon src=\"one\"/></button>");
    auto* button = result.rootAs<rdui::Button>();
    ensure("button parses", result.ok() && button);
    ensure_equals("inline icon retained", button->icon()->name(), "one");
    ensure_equals("inline label retained", button->label()->text(), "first");
    ensure_equals("button caption is not a standalone label style target", button->label()->element(), std::string("button-caption"));
    ensure("text before icon preserves authored order", button->children()[0].get() == button->label());
    rdui::ViewBuildResult icon_first = factory.createFromString("<button><icon src=\"search\"/>second</button>");
    auto* reversed = icon_first.rootAs<rdui::Button>();
    ensure("icon before text preserves authored order", icon_first.ok() && reversed->children()[0].get() == reversed->icon());
    button->setIcon("updated");
    button->setLabel("updated");
    ensure_equals("icon updated in place", button->children().size(), 2U);
    button->clearChildren();
    ensure("clearing Button children clears typed refs", !button->icon() && !button->label());
    button->setIcon("rebuilt");
    ensure_equals("programmatic icon child can be recreated", button->icon()->name(), "rebuilt");
    ensure("button.label authoring syntax is removed", !factory.createFromString("<button><button.label value=\"old\"/></button>").ok());
    ensure("button.icon authoring syntax is removed", !factory.createFromString("<button><button.icon name=\"old\"/></button>").ok());
    ensure("switch.label authoring syntax is removed", !factory.createFromString("<switch><switch.label value=\"old\"/></switch>").ok());
    ensure("label text is no longer authored through value", !factory.createFromString("<label value=\"old\"/>").ok());
}

template<> template<> void rduilayoutresourcecompiler_object::test<7>() {
    std::ifstream vertex_file(tut::sSourceDir + "../newview/app_settings/shaders/class1/interface/rduiV.glsl");
    std::ifstream fragment_file(tut::sSourceDir + "../newview/app_settings/shaders/class1/interface/rduiF.glsl");
    std::ostringstream vertex, fragment;
    vertex << vertex_file.rdbuf();
    fragment << fragment_file.rdbuf();
    ensure("vertex shader forwards shape coordinates", vertex.str().find("shape_coord = texcoord0") != std::string::npos);
    ensure("fragment shader keeps passthrough mode", fragment.str().find("rduiShapeMode == 0") != std::string::npos);
    ensure("fragment shader has analytic border mode",
           fragment.str().find("rduiShapeMode == 2") != std::string::npos && fragment.str().find("fwidth") != std::string::npos);
    ensure("fragment shader supports a Fieldset Legend border gap", fragment.str().find("rduiTopBorderGap") != std::string::npos);
    ensure("fragment shader supports radial and conic gradients",
           fragment.str().find("rduiGradientKind") != std::string::npos && fragment.str().find("atan(delta.x, delta.y)") != std::string::npos);
    ensure("fragment shader supports repeating gradient paint",
           fragment.str().find("rduiGradientRepeating") != std::string::npos
               && fragment.str().find("underlyingGradientIntegral") != std::string::npos
               && fragment.str().find("cycles * repeating_total") != std::string::npos);
    ensure("fragment shader antialiases gradient stops and repeating seams",
           fragment.str().find("gradientPixelWidth") != std::string::npos
               && fragment.str().find("filteredGradientColor") != std::string::npos
               && fragment.str().find("gradientIntervalIntegral") != std::string::npos
               && fragment.str().find("dFdx(delta)") != std::string::npos);
    ensure("fragment shader supports gradient borders",
           fragment.str().find("rduiShapeMode == 6") != std::string::npos && fragment.str().find("rduiBorderWidths") != std::string::npos);
    ensure("fragment shader supports composited blur effects",
           fragment.str().find("rduiShapeMode == 7") != std::string::npos
               && fragment.str().find("rduiShapeMode == 8") != std::string::npos
               && fragment.str().find("blurredEffectColor") != std::string::npos
               && fragment.str().find("max_samples_per_side") != std::string::npos
               && fragment.str().find("total_weight") != std::string::npos);
}

template<> template<> void rduilayoutresourcecompiler_object::test<8>() {
    rdui::System system;
    rdui::ResourceSnapshot resources;
    resources.add("localization.yaml", R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings:
      title: Title
      status: Ready
      press: Press
  pt:
    name: Português
    strings:
      title: Título
      status: Pronto
      press: Pressione
)YAML");
    resources.add("skin.radia", "label { text-color: #ffffffff; }");
    resources.add("localized.xml",
                  "<floater title=\"title\"><label id=\"status\" for=\"target\">status</label>"
                  "<switch id=\"target\"/><button id=\"press\">press</button></floater>");
    rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(resources);
    ensure("localizations load", prepared.ok());
    system.publish(prepared.generation);
    rdui::ViewBuildResult result = system.createView("localized.xml");
    auto* floater = result.rootAs<rdui::Floater>();
    ensure("localized tree builds", result.ok() && floater);
    auto status = requireWidget<rdui::Label>(*floater, "status");
    auto press = requireWidget<rdui::Button>(*floater, "press");
    ensure("Binder resolves localized controls", status && press);
    rdui::Label* button_label = press->label();
    ensure_equals("initial title localized", floater->title(), "Title");
    ensure_equals("initial label localized", status->text(), "Ready");
    ensure_equals("initial button localized", button_label->text(), "Press");

    std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
    auto localized_floater = std::unique_ptr<rdui::Floater>(static_cast<rdui::Floater*>(result.root.release()));
    surface->mountFloater(std::move(localized_floater));
    ensure("second language selected", system.setLocale("pt"));
    ensure_equals("visible title refreshed", floater->title(), "Título");
    ensure_equals("visible label refreshed", status->text(), "Pronto");
    ensure_equals("visible button refreshed", button_label->text(), "Pressione");

    auto programmatic = std::make_unique<rdui::Floater>();
    rdui::Floater* programmatic_floater = programmatic.get();
    programmatic->setTitle("title");
    surface->mountFloater(std::move(programmatic));
    ensure_equals("programmatic title resolves when attached", programmatic_floater->title(), "Título");

    status->setContent(system.localized("status"));
    ensure("default language restored", system.setLocale("en"));
    ensure_equals("programmatic title remains locale-bound", programmatic_floater->title(), "Title");
    ensure_equals("C++ localized assignment stays bound", status->text(), "Ready");
    status->setText("Literal");
    ensure("Portuguese restored", system.setLocale("pt"));
    ensure_equals("literal assignment clears binding", status->text(), "Literal");

    resources.add("missing.xml", "<text>missing</text>");
    const rdui::SkinGenerationPrepareResult missing = rdui::SkinCompiler().prepare(std::move(resources));
    ensure("missing default-language key rejects generation", !missing.ok() && !missing.generation);
}

template<> template<> void rduilayoutresourcecompiler_object::test<9>() {
    const rdui::ViewBuildResult unknown_element = factory.createFromString("<panel><unknown/></panel>", "unknown.xml");
    ensure("unknown element rejects document", !unknown_element.ok() && !unknown_element.root);
    ensure_equals("unknown element diagnostic code", unknown_element.errors.front().code, "view.element.unknown");
    ensure_equals("unknown element diagnostic source", unknown_element.errors.front().source, "unknown.xml");

    const rdui::ViewBuildResult unsupported_attribute = factory.createFromString("<panel width=\"10\"/>", "attribute.xml");
    ensure("unsupported attribute rejects document", !unsupported_attribute.ok() && !unsupported_attribute.root);
    ensure_equals("unsupported attribute diagnostic code", unsupported_attribute.errors.front().code, "view.attribute.unsupported");

    const rdui::ViewBuildResult unknown_attribute = factory.createFromString("<floater invented=\"true\"/>", "unknown_attribute.xml");
    ensure("unknown widget attribute rejects document", !unknown_attribute.ok() && !unknown_attribute.root);
    ensure_equals("unknown attribute diagnostic code", unknown_attribute.errors.front().code, "view.attribute.unknown");

    const rdui::ViewBuildResult unsupported_action = factory.createFromString("<label onClick=\"click\"/>", "action.xml");
    ensure("unsupported widget event rejects view", !unsupported_action.ok() && !unsupported_action.root);
    ensure_equals("unsupported action diagnostic code", unsupported_action.errors.front().code, "view.action.unsupported");

    const rdui::ViewBuildResult expression_action = factory.createFromString("<button onClick=\"save(force=true)\"/>", "expression.xml");
    ensure("action expressions reject view", !expression_action.ok() && !expression_action.root);
    ensure_equals("invalid action name diagnostic code", expression_action.errors.front().code, "view.action.name_invalid");
}

template<> template<> void rduilayoutresourcecompiler_object::test<10>() {
    const rdui::ViewBuildResult duplicate =
        factory.createFromString("<panel><text id=\"same\"/><button id=\"same\">Same</button></panel>", "duplicates.xml");
    ensure("duplicate ids reject whole view", !duplicate.ok() && !duplicate.root);
    ensure_equals("duplicate id diagnostic code", duplicate.errors.front().code, "view.id.duplicate");
    ensure_equals("duplicate id diagnostic source", duplicate.errors.front().source, "duplicates.xml");
}

template<> template<> void rduilayoutresourcecompiler_object::test<11>() {
    const rdui::ViewBuildResult invalid =
        factory.createFromString("<floater canClose=\"sometimes\"><switch checked=\"yes\"/></floater>", "booleans.xml");
    ensure("invalid booleans reject whole view", !invalid.ok() && !invalid.root);
    ensure_equals("both invalid booleans diagnosed", invalid.errors.size(), 2U);
    ensure_equals("boolean diagnostic code", invalid.errors.front().code, "view.attribute.boolean_invalid");
    ensure_equals("boolean diagnostic source", invalid.errors.front().source, "booleans.xml");
}

template<> template<> void rduilayoutresourcecompiler_object::test<12>() {
    ensure("delay requires action", !factory.createFromString("<button longClickDelay=\"1s\"/>").ok());
    ensure("duration requires unit", !factory.createFromString("<button onLongClick=\"hold\" longClickDelay=\"500\"/>").ok());
    ensure("label rejects long click", !factory.createFromString("<label onLongClick=\"hold\"/>").ok());
}

template<> template<> void rduilayoutresourcecompiler_object::test<13>() {
    rdui::ViewBuildResult result = factory.createFromString("<floater title=\"tools\" icon=\"search\" canMinimize=\"true\" "
                                                            "showHeaderIdentity=\"false\">"
                                                            "<header><button id=\"refresh\">Refresh</button></header><panel "
                                                            "id=\"content\"/></floater>",
                                                            "custom_header.xml");
    auto* floater = result.rootAs<rdui::Floater>();
    ensure("custom-header floater builds", result.ok() && floater);
    auto content = requireWidget<rdui::Panel>(*floater, "content");
    auto refresh = requireWidget<rdui::Button>(*floater, "refresh");
    ensure("Binder resolves authored Floater children", content && refresh);
    ensure("direct children route into content box", content->parent() == floater->content());
    ensure_equals("custom header widget remains findable", refresh->parent()->part(), "header::custom");
    ensure("custom header precedes minimize control", refresh->parent() == floater->header()->children()[2].get());
    ensure("built-in controls follow custom header", floater->header()->children()[3].get() == floater->minimizeButton());
    ensure_equals("minimize icon keeps the Floater part path", floater->minimizeButton()->children()[0]->part(), "header::minimize::icon");
    ensure("nested Floater icon also fulfills the Button icon role",
           floater->minimizeButton()->icon() == floater->minimizeButton()->children()[0].get());
    ensure("expanded identity icon collapsed", floater->header()->children()[0]->visibility() == rdui::Visibility::Collapsed);
    ensure("expanded identity title collapsed", floater->header()->children()[1]->visibility() == rdui::Visibility::Collapsed);

    floater->setMinimized(true);
    ensure("minimized identity icon shown", floater->header()->children()[0]->visibility() == rdui::Visibility::Visible);
    ensure("minimized identity title shown", floater->header()->children()[1]->visibility() == rdui::Visibility::Visible);
    ensure("custom header box collapsed while minimized", refresh->parent()->visibility() == rdui::Visibility::Collapsed);
    ensure("content box collapsed while minimized", floater->content()->visibility() == rdui::Visibility::Collapsed);
    floater->setMinimized(false);
    ensure("custom header box restored on expansion", refresh->parent()->visibility() == rdui::Visibility::Visible);
    ensure("expanded identity returns collapsed", floater->header()->children()[1]->visibility() == rdui::Visibility::Collapsed);

    floater->clearChildren();
    ensure("clearing authored children preserves owned header", floater->header() != nullptr);
    ensure("clearing authored children preserves owned content box", floater->content() != nullptr);
    ensure("clearing authored children expires content reference", !content);
    ensure("clearing authored children expires custom header reference", !refresh);

    const rdui::ViewBuildResult missing_title = factory.createFromString("<floater canMinimize=\"true\"/>", "missing_title.xml");
    ensure("minimizable floater requires title", !missing_title.ok());
    ensure_equals("title diagnostic is stable", missing_title.errors.front().code, "view.floater.title_required");

    const rdui::ViewBuildResult duplicate_header = factory.createFromString("<floater><header/><header/></floater>", "duplicate_header.xml");
    ensure("duplicate custom header rejects view", !duplicate_header.ok());
    ensure_equals("duplicate header diagnostic is stable", duplicate_header.errors.front().code, "view.part.duplicate");

    const rdui::ViewBuildResult attached_only = factory.createFromString("<floater canDetach=\"false\"/>", "attached_only.xml");
    ensure("floater accepts detach policy", attached_only.ok());
    ensure("floater detach policy defaults on and can opt out", !attached_only.rootAs<rdui::Floater>()->canDetach());
}

template<> template<> void rduilayoutresourcecompiler_object::test<14>() {
    resources["widgets/floater.xml"] = "<floater closeIcon=\"close\" minimizeIcon=\"minimize\" canClose=\"false\"/>";
    resources["defaulted.xml"] = "<floater title=\"defaulted\" canClose=\"true\"/>";

    ensure("Widget Defaults validate independently with case-insensitive lookup", !factory.validateWidgetDefaults("FLOATER").hasErrors());

    rdui::ViewBuildResult result = factory.createFromResource("defaulted.xml");
    auto* floater = result.rootAs<rdui::Floater>();
    ensure("Widget Defaults apply", result.ok() && floater);
    ensure_equals("default close icon applied", floater->closeIcon(), std::string("close"));
    ensure_equals("default minimize icon applied", floater->minimizeIcon(), std::string("minimize"));
    ensure_equals("default close icon reaches declared part", floater->closeButton()->icon()->name(), std::string("close"));
    ensure_equals("default minimize icon reaches declared part", floater->minimizeButton()->icon()->name(), std::string("minimize"));
    ensure("View attribute overrides Widget Default", floater->canClose());

    resources["widgets/floater.xml"] = "<panel/>";
    ensure("Widget Defaults validation reports invalid root", factory.validateWidgetDefaults("floater").hasErrors());
    const rdui::ViewBuildResult invalid = factory.createFromResource("defaulted.xml");
    ensure("wrong Widget Defaults root rejects View", !invalid.ok());
    ensure("invalid Widget Defaults expose no partial root", invalid.root == nullptr);
}

template<> template<> void rduilayoutresourcecompiler_object::test<15>() {
    const rdui::ViewBuildResult result = factory.createFromString("<panel><text id=\"shown\" visibility=\"visible\"/>"
                                                                  "<text id=\"hidden\" visibility=\"hidden\"/>"
                                                                  "<text id=\"collapsed\" visibility=\"collapsed\"/></panel>",
                                                                  "visibility.xml");
    ensure("typed visibility values compile", result.ok());
    ensure_equals("Visible value is typed", static_cast<int>(result.root->children()[0]->visibility()), static_cast<int>(rdui::Visibility::Visible));
    ensure_equals("Hidden value is typed", static_cast<int>(result.root->children()[1]->visibility()), static_cast<int>(rdui::Visibility::Hidden));
    ensure_equals("Collapsed value is typed", static_cast<int>(result.root->children()[2]->visibility()),
                  static_cast<int>(rdui::Visibility::Collapsed));

    const rdui::ViewBuildResult invalid = factory.createFromString("<text visibility=\"invisible\"/>", "invalid_visibility.xml");
    ensure("invalid visibility is rejected", !invalid.ok());
    ensure_equals("invalid visibility diagnostic is stable", invalid.errors.front().code, "view.attribute.visibility_invalid");

    const rdui::ViewBuildResult legacy = factory.createFromString("<text visible=\"false\"/>", "legacy_visibility.xml");
    ensure("legacy boolean visibility is rejected", !legacy.ok());
    ensure_equals("legacy visibility is an unknown attribute", legacy.errors.front().code, "view.attribute.unknown");
}

template<> template<> void rduilayoutresourcecompiler_object::test<16>() {
    resources["widgets/label.xml"] = "<label visibility=\"sometimes\"/>";
    const rdui::DiagnosticResult visibility = factory.validateWidgetDefaults("label");
    ensure("Widget Defaults validate typed visibility values", visibility.hasErrors());
    ensure_equals("Widget Defaults preserve visibility diagnostics", visibility.errors.front().code, "view.attribute.visibility_invalid");

    resources["widgets/label.xml"] = "<label/>";
    resources["widgets/switch.xml"] = "<switch checked=\"sometimes\"/>";
    const rdui::DiagnosticResult widget_attribute = factory.validateWidgetDefaults("switch");
    ensure("Widget Defaults validate widget-specific typed values", widget_attribute.hasErrors());
    ensure_equals("Widget Defaults preserve widget attribute diagnostics", widget_attribute.errors.front().code, "view.attribute.boolean_invalid");
}

template<> template<> void rduilayoutresourcecompiler_object::test<17>() {
    rdui::ViewBuildResult result = factory.createFromString("<FlOaTeR TiTlE=\"tools\" CaNMiNiMiZe=\"true\">"
                                                            "<HeAdEr><BuTtOn ID=\"save-file\" ONCLICK=\"save-file\">"
                                                            "<IcOn SrC=\"search\"/>Save</BuTtOn></HeAdEr></FlOaTeR>",
                                                            "case-insensitive.xml");
    auto* floater = result.rootAs<rdui::Floater>();
    ensure("element and attribute lookup is ASCII case-insensitive", result.ok() && floater);
    auto button = requireWidget<rdui::Button>(*floater, "save-file");
    ensure("mixed-case schema names retain canonical widget behavior", button && button->icon());
    ensure_equals("contract retains canonical element spelling", button->element(), std::string("button"));
    ensure_equals("mixed-case action attribute resolves", button->action(rdui::ActionEventKind::Click), std::string("save-file"));
}

template<> template<> void rduilayoutresourcecompiler_object::test<18>() {
    const rdui::ViewBuildResult snake = factory.createFromString("<BuTtOn\n  on_click=\"save-file\"/>", "legacy-snake.xml");
    ensure("legacy snake_case attribute is rejected", !snake.ok());
    ensure_equals("legacy attribute is not an alias", snake.errors.front().code, std::string("view.attribute.unknown"));
    ensure_equals("legacy attribute diagnostic retains source line", snake.errors.front().line, std::size_t(1));
    ensure("known element names use canonical spelling in diagnostics", snake.errors.front().message.find("<button>") != std::string::npos);

    const rdui::ViewBuildResult duplicate = factory.createFromString("<button onClick=\"save-one\" ONCLICK=\"save-two\"/>", "duplicate-case.xml");
    ensure("attributes colliding after case folding are rejected", !duplicate.ok());
    ensure_equals("case-folded duplicate has a stable diagnostic", duplicate.errors.front().code, std::string("view.attribute.duplicate"));

    const rdui::ViewBuildResult invalid_id = factory.createFromString("<panel id=\"bad_id\"/>", "id.xml");
    ensure("non-kebab Widget ID is rejected", !invalid_id.ok());
    ensure_equals("invalid ID diagnostic is stable", invalid_id.errors.front().code, std::string("view.id.invalid"));

    const rdui::ViewBuildResult invalid_class = factory.createFromString("<panel class=\"BadClass\"/>", "class.xml");
    ensure("non-kebab Widget class is rejected", !invalid_class.ok());
    ensure_equals("invalid class diagnostic is stable", invalid_class.errors.front().code, std::string("view.class.invalid"));

    const rdui::ViewBuildResult invalid_action = factory.createFromString("<button onClick=\"bad_action\"/>", "action-name.xml");
    ensure("non-kebab Action is rejected", !invalid_action.ok());
    ensure_equals("invalid Action diagnostic is stable", invalid_action.errors.front().code, std::string("view.action.name_invalid"));

    ensure("Icon name alias is removed", !factory.createFromString("<icon name=\"search\"/>").ok());
    ensure("Icon icon alias is removed", !factory.createFromString("<icon icon=\"search\"/>").ok());
    ensure("Icon source alias is removed", !factory.createFromString("<icon source=\"search\"/>").ok());
    ensure("legacy qualified header element is removed", !factory.createFromString("<floater><floater.header/></floater>").ok());
}
} // namespace tut
