/**
 * @file resourcecompiler_test.cpp
 * @brief Tests Layout Resource compilation and Widget Contract diagnostics.
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
#include <cctype>
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

namespace {
using radia::ui::Button;
using radia::ui::DiagnosticResult;
using radia::ui::FixedTextMetrics;
using radia::ui::fixedTextMetrics;
using radia::ui::Floater;
using radia::ui::Label;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutDocumentParser;
using radia::ui::LayoutDocumentParseResult;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::Switch;
using radia::ui::System;
using radia::ui::Widget;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetRef;
using radia::ui::detail::findWidgetInScope;
} // namespace

namespace tut {
struct resourceCompilerData {
    LayoutCompilerFixture factory;
    std::map<std::string, std::string>& resources = factory.resources;

    template<typename WidgetT> WidgetRef<WidgetT> requireWidget(Widget& root, const std::string& id) {
        return WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(findWidgetInScope(root, id)));
    }
};
using resourceCompilerTest = test_group<resourceCompilerData>;
using resourceCompilerObject = resourceCompilerTest::object;
resourceCompilerTest resourceCompilerTestCase("resourcecompiler");

template<> template<> void resourceCompilerObject::test<1>() {
    const char* kXml =
        "<floater title=\"title\" closeIcon=\"close\" minimizeIcon=\"minimize\" canMinimize=\"true\"><text id=\"status\">Ready</text><button id=\"go\" onClick=\"demoGo()\" onDoubleClick=\"demoDouble()\" onMouseDown=\"demoPress()\" onLongClick=\"demoHold()\" onContextMenu=\"demoMenu()\" longClickDelay=\"750ms\"><icon src=\"search\"/>Go</button><switch id=\"toggle\" checked=\"true\" onChange=\"demoChanged()\"/></floater>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kXml, "floater.xml");
    auto* floater = result.rootAs<Floater>();
    ensure("arbitrary floater root parsed", result.ok() && floater);
    ensure_equals("title localization key retained without a System", floater->title(), "title");
    ensure("declared chrome composed", floater->header() && floater->minimizeButton());
    auto go = requireWidget<Button>(*floater, "go");
    auto toggle = requireWidget<Switch>(*floater, "toggle");
    ensure("Binder resolves parsed controls", go && toggle);
    ensure("typed lookup", toggle->checked());
    ensure_equals("click Handler Call parsed", go->eventCall(WidgetEventKind::Click)->name(), "demoGo");
    ensure_equals("double-click Handler Call parsed", go->eventCall(WidgetEventKind::DoubleClick)->name(), "demoDouble");
    ensure_equals("pointer Handler Call parsed", go->eventCall(WidgetEventKind::MouseDown)->name(), "demoPress");
    ensure_equals("long-click Handler Call parsed", go->eventCall(WidgetEventKind::LongClick)->name(), "demoHold");
    ensure_equals("context-menu Handler Call parsed", go->eventCall(WidgetEventKind::ContextMenu)->name(), "demoMenu");
    ensure_equals("long click delay parsed", go->longClickDelay()->count(), 750LL);
    ensure_equals("Switch Handler Call parsed", toggle->eventCall(WidgetEventKind::Change)->name(), "demoChanged");
}

template<> template<> void resourceCompilerObject::test<2>() {
    resources["shared.xml"] = "<panel id=\"base\" class=\"shared\"><text id=\"resourceChild\">base</text></panel>";
    const char* kXml =
        "<panel><panel filename=\"shared.xml\" id=\"one\" class=\"first\"><text id=\"inlineChild\"/></panel><panel filename=\"shared.xml\" id=\"two\"/></panel>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kXml, "outer.xml");
    ensure("embedded panels parsed", result.ok());
    auto first = requireWidget<radia::ui::Panel>(*result.root, "one");
    auto second = requireWidget<radia::ui::Panel>(*result.root, "two");
    ensure("independent panel instances", first && second && first.get() != second.get());
    ensure("referenced class retained", first->classes().count("shared") == 1);
    ensure("inline class appended", first->classes().count("first") == 1);
    ensure("referenced child first", first->children().front()->id() == "resourceChild");
    ensure("inline child appended", first->children().back()->id() == "inlineChild");
    first->setVisibility(radia::ui::Visibility::Collapsed);
    ensure("second visibility independent", second->visibility() == radia::ui::Visibility::Visible);
}

template<> template<> void resourceCompilerObject::test<3>() {
    resources["nested/inner.xml"] = "<panel id=\"inner\"/>";
    resources["nested/middle.xml"] = "<panel><panel filename=\"inner.xml\"/></panel>";
    resources["outer.xml"] = "<panel><panel filename=\"nested/middle.xml\"/></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromResource("outer.xml");
    ensure("nested relative panels load", result.ok());
    ensure("deep child present",
           result.root->children().size() == 1
               && result.root->children()[0]->children().size() == 1
               && result.root->children()[0]->children()[0]->id() == "inner");
}

template<> template<> void resourceCompilerObject::test<4>() {
    resources["a.xml"] = "<panel><panel filename=\"b.xml\"/></panel>";
    resources["b.xml"] = "<panel><panel filename=\"a.xml\"/></panel>";
    LayoutBuildResult cycle = factory.buildWidgetTreeFromResource("a.xml");
    ensure("cycle rejected", !cycle.ok());
    ensure("failed build never exposes a partial tree", !cycle.root);
    ensure_equals("cycle has stable diagnostic code", cycle.errors.front().code, "layout.resource.cycle");
    ensure_equals("cycle diagnostic identifies source", cycle.errors.front().source, "a.xml");
    ensure("missing panel rejected", !factory.buildWidgetTreeFromString("<panel><panel filename=\"missing.xml\"/></panel>", "root.xml").ok());
    resources["wrong.xml"] = "<text/>";
    ensure("non-panel reference rejected", !factory.buildWidgetTreeFromString("<panel><panel filename=\"wrong.xml\"/></panel>", "root.xml").ok());
    ensure("filename on non-panel rejected", !factory.buildWidgetTreeFromString("<button filename=\"x.xml\"/>").ok());
    ensure("resource-root escape rejected",
           !factory.buildWidgetTreeFromString("<panel><panel filename=\"../outside.xml\"/></panel>", "root.xml").ok());

    resources["empty.xml"] = "";
    const LayoutBuildResult empty = factory.buildWidgetTreeFromResource("empty.xml");
    ensure("empty resource rejected as invalid XML", !empty.ok());
    ensure_equals("empty resource differs from missing resource", empty.errors.front().code, "layout.xml.invalid");
}

template<> template<> void resourceCompilerObject::test<5>() {
    const LayoutBuildResult invalid = factory.buildWidgetTreeFromString("<panel>\n  <unknown/>\n</panel>", "source_ranges.xml");
    ensure("source-aware compiler rejects unknown child", !invalid.ok());
    ensure_equals("compiler diagnostic retains source line", invalid.errors.front().line, 2U);
    ensure_equals("compiler diagnostic retains source column", invalid.errors.front().column, 3U);

    LayoutDocumentParseResult parsed = LayoutDocumentParser().parse("<panel>before<label>middle</label>after</panel>", "mixed.xml");
    ensure("one document tree parses", parsed.ok());
    ensure_equals("mixed content order is represented once", parsed.document->root->content.size(), 3U);
    ensure("text-child-text order retained",
           parsed.document->root->content[0].isText() && !parsed.document->root->content[1].isText() && parsed.document->root->content[2].isText());
    ensure("element source range retained",
           parsed.document->root->content[1].source.begin.line == 1
               && parsed.document->root->content[1].source.end.offset >= parsed.document->root->content[1].source.begin.offset);
}

template<> template<> void resourceCompilerObject::test<6>() {
    LayoutBuildResult result = factory.buildWidgetTreeFromString("<button>first<icon src=\"one\"/></button>");
    auto* button = result.rootAs<Button>();
    ensure("button parses", result.ok() && button);
    ensure_equals("inline icon retained", button->icon()->name(), "one");
    ensure_equals("inline label retained", button->label()->text(), "first");
    ensure_equals("button caption is not a standalone label style target", button->label()->elementName(), std::string("button-caption"));
    ensure("text before icon preserves authored order", button->children()[0].get() == button->label());
    LayoutBuildResult iconFirst = factory.buildWidgetTreeFromString("<button><icon src=\"search\"/>second</button>");
    auto* reversed = iconFirst.rootAs<Button>();
    ensure("icon before text preserves authored order", iconFirst.ok() && reversed->children()[0].get() == reversed->icon());
    button->setIcon("updated");
    button->setLabel("updated");
    ensure_equals("icon updated in place", button->children().size(), 2U);
    button->clearChildren();
    ensure("clearing Button children clears typed refs", !button->icon() && !button->label());
    button->setIcon("rebuilt");
    ensure_equals("programmatic icon child can be recreated", button->icon()->name(), "rebuilt");
    ensure("button.label authoring syntax is removed", !factory.buildWidgetTreeFromString("<button><button.label value=\"old\"/></button>").ok());
    ensure("button.icon authoring syntax is removed", !factory.buildWidgetTreeFromString("<button><button.icon name=\"old\"/></button>").ok());
    ensure("switch.label authoring syntax is removed", !factory.buildWidgetTreeFromString("<switch><switch.label value=\"old\"/></switch>").ok());
    ensure("label text is no longer authored through value", !factory.buildWidgetTreeFromString("<label value=\"old\"/>").ok());
}

template<> template<> void resourceCompilerObject::test<7>() {
    std::ifstream vertexFile(tut::sSourceDir + "../newview/app_settings/shaders/class1/interface/uiV.glsl");
    std::ifstream fragmentFile(tut::sSourceDir + "../newview/app_settings/shaders/class1/interface/uiF.glsl");
    std::ifstream paintProtocolFile(tut::sSourceDir + "render/paintprotocol.def");
    ensure("paint protocol sources are readable", vertexFile.good() && fragmentFile.good() && paintProtocolFile.good());
    std::ostringstream vertex, fragment, paintProtocol;
    vertex << vertexFile.rdbuf();
    fragment << fragmentFile.rdbuf();
    paintProtocol << paintProtocolFile.rdbuf();
    const std::string vertexSource = vertex.str();
    const std::string fragmentSource = fragment.str();
    const std::string paintProtocolSource = paintProtocol.str();
    const auto contains = [](const std::string& source, const char* text) { return source.find(text) != std::string::npos; };
    const auto trimToken = [](std::string token) {
        const std::size_t first = token.find_first_not_of(" \t\r\n");
        const std::size_t last = token.find_last_not_of(" \t\r\n");
        return first == std::string::npos ? std::string() : token.substr(first, last - first + 1);
    };
    const auto shaderName = [](const std::string& name) { return name; };
    const auto parseDefinition = [&](const char* macro) {
        std::map<std::string, int> entries;
        const std::string marker = std::string(macro) + "(";
        std::size_t position = 0;
        while ((position = paintProtocolSource.find(marker, position)) != std::string::npos) {
            const std::size_t nameStart = position + marker.size();
            const std::size_t firstComma = paintProtocolSource.find(',', nameStart);
            const std::size_t close = paintProtocolSource.find(')', firstComma + 1);
            if (firstComma == std::string::npos || close == std::string::npos) break;
            entries.emplace(trimToken(paintProtocolSource.substr(nameStart, firstComma - nameStart)),
                            std::stoi(trimToken(paintProtocolSource.substr(firstComma + 1, close - firstComma - 1))));
            position = close + 1;
        }
        return entries;
    };
    const auto parseShaderConstants = [&](const char* prefix) {
        std::map<std::string, int> entries;
        const std::string marker = std::string("const int ") + prefix;
        std::size_t position = 0;
        while ((position = fragmentSource.find(marker, position)) != std::string::npos) {
            const std::size_t nameStart = position + std::string("const int ").size();
            const std::size_t equals = fragmentSource.find('=', nameStart);
            const std::size_t semicolon = fragmentSource.find(';', equals);
            if (equals == std::string::npos || semicolon == std::string::npos) break;
            const std::string name = trimToken(fragmentSource.substr(nameStart, equals - nameStart));
            entries.emplace(name.substr(std::string(prefix).size()), std::stoi(trimToken(fragmentSource.substr(equals + 1, semicolon - equals - 1))));
            position = semicolon + 1;
        }
        return entries;
    };
    const auto ensureProtocol = [&](const char* macro, const char* shaderPrefix) {
        const auto definitions = parseDefinition(macro);
        const auto constants = parseShaderConstants(shaderPrefix);
        ensure((std::string("shader protocol count matches ") + macro).c_str(), definitions.size() == constants.size());
        for (const auto& [name, value] : definitions) {
            const auto found = constants.find(shaderName(name));
            ensure((std::string("shader protocol entry matches ") + name).c_str(), found != constants.end() && found->second == value);
        }
    };
    ensureProtocol("PAINT_OP_ENTRY", "kPaintOp");
    ensureProtocol("GRADIENT_OP_ENTRY", "kGradient");
    ensureProtocol("OUTLINE_OP_ENTRY", "kOutline");

    ensure("paint shader variant is guarded", contains(vertexSource, "#ifdef PAINT_SHADER") && contains(fragmentSource, "#ifdef PAINT_SHADER"));
    ensure("vertex shader forwards shape coordinates", contains(vertexSource, "shapeCoord = texcoord0"));
    ensure("fragment shader names direct and fill modes",
           contains(fragmentSource, "kPaintOpDirect = 0")
               && contains(fragmentSource, "kPaintOpFill = 1")
               && contains(fragmentSource, "paintOp == kPaintOpDirect"));
    ensure("paint protocol list contains the shader operation values",
           contains(paintProtocolSource, "PAINT_OP_ENTRY(Direct, 0)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(Fill, 1)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(Border, 2)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(Gradient, 3)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(OuterShadow, 4)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(InsetShadow, 5)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(GradientBorder, 6)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(Blur, 7)")
               && contains(paintProtocolSource, "PAINT_OP_ENTRY(Composite, 8)"));
    ensure("fragment shader declares the paint protocol",
           contains(fragmentSource, "uniform int paintOp;")
               && contains(fragmentSource, "uniform vec4 shapeRect;")
               && contains(fragmentSource, "uniform vec2 effectTextureSize;")
               && contains(fragmentSource, "uniform int gradientKind;")
               && contains(fragmentSource, "uniform int outlineStyle;")
               && !contains(fragmentSource, "rduiPaintOp"));
    ensure("fragment shader has analytic border mode",
           contains(fragmentSource, "kPaintOpBorder = 2")
               && contains(fragmentSource, "paintOp == kPaintOpBorder")
               && contains(fragmentSource, "fwidth"));
    ensure("fragment shader names gradient and outline modes",
           contains(fragmentSource, "kGradientLinear = 0")
               && contains(fragmentSource, "kGradientRadial = 1")
               && contains(fragmentSource, "kGradientConic = 2")
               && contains(fragmentSource, "kOutlineSolid = 0")
               && contains(fragmentSource, "kOutlineDashed = 1"));
    ensure("fragment shader supports a Fieldset Legend border gap", contains(fragmentSource, "topBorderGap"));
    ensure("fragment shader supports radial and conic gradients",
           contains(fragmentSource, "gradientKind") && contains(fragmentSource, "atan(delta.x, delta.y)"));
    ensure("fragment shader supports repeating gradient paint",
           contains(fragmentSource, "gradientRepeating")
               && contains(fragmentSource, "underlyingGradientIntegral")
               && contains(fragmentSource, "cycles * repeatingTotal"));
    ensure("fragment shader antialiases gradient stops and repeating seams",
           contains(fragmentSource, "gradientPixelWidth")
               && contains(fragmentSource, "filteredGradientColor")
               && contains(fragmentSource, "gradientIntervalIntegral")
               && contains(fragmentSource, "dFdx(delta)"));
    ensure("fragment shader supports gradient borders",
           contains(fragmentSource, "kPaintOpGradientBorder = 6")
               && contains(fragmentSource, "paintOp == kPaintOpGradientBorder")
               && contains(fragmentSource, "borderWidths"));
    ensure("fragment shader supports composited blur effects",
           contains(fragmentSource, "kPaintOpBlur = 7")
               && contains(fragmentSource, "kPaintOpComposite = 8")
               && contains(fragmentSource, "paintOp == kPaintOpBlur")
               && contains(fragmentSource, "paintOp == kPaintOpComposite")
               && contains(fragmentSource, "blurredEffectColor")
               && contains(fragmentSource, "maxSamplesPerSide")
               && contains(fragmentSource, "totalWeight"));
    ensure("rounded background blur uses the shape mask for coverage",
           contains(fragmentSource, "return vec4(color.rgb, mask);") && !contains(fragmentSource, "color.a * mask"));
}

template<> template<> void resourceCompilerObject::test<8>() {
    System system;
    ResourceSnapshot resources;
    const char* kLocalization =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {title: Title, status: Ready, press: Press}}, pt: {name: Português, strings: {title: Título, status: Pronto, press: Pressione}}}\n";
    const char* kLocalizedLayout =
        "<floater title=\"title\"><label id=\"status\" for=\"target\">status</label><switch id=\"target\"/><button id=\"press\">press</button></floater>";
    resources.add("localization.yaml", kLocalization);
    resources.add("skin.radia", "label { text-color: #ffffffff; }");
    resources.add("localized.xml", kLocalizedLayout);
    SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ensure("localizations load", prepared.ok());
    system.publish(prepared.generation);
    LayoutBuildResult result = system.buildWidgetTree("localized.xml");
    auto* floater = result.rootAs<Floater>();
    ensure("localized tree builds", result.ok() && floater);
    auto status = requireWidget<Label>(*floater, "status");
    auto press = requireWidget<Button>(*floater, "press");
    ensure("Binder resolves localized controls", status && press);
    Label* buttonLabel = press->label();
    ensure_equals("initial title localized", floater->title(), "Title");
    ensure_equals("initial label localized", status->text(), "Ready");
    ensure_equals("initial button localized", buttonLabel->text(), "Press");

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    auto localizedFloater = std::unique_ptr<Floater>(static_cast<Floater*>(result.root.release()));
    surface->mountFloater(std::move(localizedFloater));
    ensure("second language selected", system.setLocale("pt"));
    ensure_equals("visible title refreshed", floater->title(), "Título");
    ensure_equals("visible label refreshed", status->text(), "Pronto");
    ensure_equals("visible button refreshed", buttonLabel->text(), "Pressione");

    auto programmatic = std::make_unique<Floater>();
    Floater* programmaticFloater = programmatic.get();
    programmatic->setTitle("title");
    surface->mountFloater(std::move(programmatic));
    ensure_equals("programmatic title resolves when attached", programmaticFloater->title(), "Título");

    status->setContent(system.localize("status"));
    ensure("default language restored", system.setLocale("en"));
    ensure_equals("programmatic title remains locale-bound", programmaticFloater->title(), "Title");
    ensure_equals("C++ localized assignment stays bound", status->text(), "Ready");
    status->setText("Literal");
    ensure("Portuguese restored", system.setLocale("pt"));
    ensure_equals("literal assignment clears binding", status->text(), "Literal");

    resources.add("missing.xml", "<text>missing</text>");
    const SkinGenerationPrepareResult missing = SkinCompiler().prepare(std::move(resources));
    ensure("missing default-language key rejects generation", !missing.ok() && !missing.generation);
}

template<> template<> void resourceCompilerObject::test<9>() {
    const LayoutBuildResult unknownElement = factory.buildWidgetTreeFromString("<panel><unknown/></panel>", "unknown.xml");
    ensure("unknown element rejects document", !unknownElement.ok() && !unknownElement.root);
    ensure_equals("unknown element diagnostic code", unknownElement.errors.front().code, "layout.element.unknown");
    ensure_equals("unknown element diagnostic source", unknownElement.errors.front().source, "unknown.xml");

    const LayoutBuildResult unsupportedAttribute = factory.buildWidgetTreeFromString("<panel width=\"10\"/>", "attribute.xml");
    ensure("unsupported attribute rejects document", !unsupportedAttribute.ok() && !unsupportedAttribute.root);
    ensure_equals("unsupported attribute diagnostic code", unsupportedAttribute.errors.front().code, "layout.attribute.unsupported");

    const LayoutBuildResult unknownAttribute = factory.buildWidgetTreeFromString("<floater invented=\"true\"/>", "unknown_attribute.xml");
    ensure("unknown widget attribute rejects document", !unknownAttribute.ok() && !unknownAttribute.root);
    ensure_equals("unknown attribute diagnostic code", unknownAttribute.errors.front().code, "layout.attribute.unknown");

    const LayoutBuildResult unsupportedEvent = factory.buildWidgetTreeFromString("<text onClick=\"click()\">copy</text>", "event.xml");
    ensure("unsupported Widget Event leaves the Widget tree usable", unsupportedEvent.ok() && unsupportedEvent.root);
    ensure_equals("unsupported Event reports one warning", unsupportedEvent.warnings.size(), 1U);
    ensure_equals("unsupported Event diagnostic code", unsupportedEvent.warnings.front().code, "layout.event.unsupported");

    const LayoutBuildResult expressionCall = factory.buildWidgetTreeFromString("<button onClick=\"save(force=true)\"/>", "expression.xml");
    ensure("Event expressions leave the Widget tree usable with a no-op binding", expressionCall.ok() && expressionCall.root);
    ensure_equals("expression reports one warning", expressionCall.warnings.size(), 1U);
    ensure_equals("unsupported literal diagnostic code", expressionCall.warnings.front().code, "layout.event.literal_unsupported");
}

template<> template<> void resourceCompilerObject::test<10>() {
    const LayoutBuildResult duplicate =
        factory.buildWidgetTreeFromString("<panel><text id=\"same\"/><button id=\"same\">Same</button></panel>", "duplicates.xml");
    ensure("duplicate ids reject whole widget tree", !duplicate.ok() && !duplicate.root);
    ensure_equals("duplicate id diagnostic code", duplicate.errors.front().code, "layout.id.duplicate");
    ensure_equals("duplicate id diagnostic source", duplicate.errors.front().source, "duplicates.xml");
}

template<> template<> void resourceCompilerObject::test<11>() {
    const LayoutBuildResult invalid =
        factory.buildWidgetTreeFromString("<floater canClose=\"sometimes\"><switch checked=\"yes\"/></floater>", "booleans.xml");
    ensure("invalid booleans reject whole widget tree", !invalid.ok() && !invalid.root);
    ensure_equals("both invalid booleans diagnosed", invalid.errors.size(), 2U);
    ensure_equals("boolean diagnostic code", invalid.errors.front().code, "layout.attribute.boolean_invalid");
    ensure_equals("boolean diagnostic source", invalid.errors.front().source, "booleans.xml");
}

template<> template<> void resourceCompilerObject::test<12>() {
    const LayoutBuildResult missingHandler = factory.buildWidgetTreeFromString("<button longClickDelay=\"1s\"/>");
    ensure("delay without an Event Handler leaves the Widget tree usable", missingHandler.ok());
    ensure_equals("missing long-click Handler warns", missingHandler.warnings.front().code, "layout.long_click.event_missing");
    ensure("duration requires unit", !factory.buildWidgetTreeFromString("<button onLongClick=\"hold()\" longClickDelay=\"500\"/>").ok());
    const LayoutBuildResult unsupported = factory.buildWidgetTreeFromString("<text onLongClick=\"hold()\">copy</text>");
    ensure("Text ignores unsupported long click", unsupported.ok());
    ensure_equals("unsupported long click warns", unsupported.warnings.front().code, "layout.event.unsupported");
}

template<> template<> void resourceCompilerObject::test<13>() {
    const char* kCustomHeaderLayout =
        "<floater title=\"tools\" icon=\"search\" canMinimize=\"true\" showHeaderIdentity=\"false\"><header><button id=\"refresh\">Refresh</button></header><panel id=\"content\"/></floater>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kCustomHeaderLayout, "custom_header.xml");
    auto* floater = result.rootAs<Floater>();
    ensure("custom-header floater builds", result.ok() && floater);
    auto content = requireWidget<radia::ui::Panel>(*floater, "content");
    auto refresh = requireWidget<Button>(*floater, "refresh");
    ensure("Binder resolves authored Floater children", content && refresh);
    ensure("direct children route into content box", content->parent() == floater->content());
    ensure_equals("custom header widget remains findable", refresh->parent()->part(), "header::custom");
    ensure("custom header precedes minimize control", refresh->parent() == floater->header()->children()[2].get());
    ensure("built-in controls follow custom header", floater->header()->children()[3].get() == floater->minimizeButton());
    ensure_equals("minimize icon keeps the Floater part path", floater->minimizeButton()->children()[0]->part(), "header::minimize::icon");
    ensure("nested Floater icon also fulfills the Button icon role",
           floater->minimizeButton()->icon() == floater->minimizeButton()->children()[0].get());
    ensure("expanded identity icon collapsed", floater->header()->children()[0]->visibility() == radia::ui::Visibility::Collapsed);
    ensure("expanded identity title collapsed", floater->header()->children()[1]->visibility() == radia::ui::Visibility::Collapsed);

    floater->setMinimized(true);
    ensure("minimized identity icon shown", floater->header()->children()[0]->visibility() == radia::ui::Visibility::Visible);
    ensure("minimized identity title shown", floater->header()->children()[1]->visibility() == radia::ui::Visibility::Visible);
    ensure("custom header box collapsed while minimized", refresh->parent()->visibility() == radia::ui::Visibility::Collapsed);
    ensure("content box collapsed while minimized", floater->content()->visibility() == radia::ui::Visibility::Collapsed);
    floater->setMinimized(false);
    ensure("custom header box restored on expansion", refresh->parent()->visibility() == radia::ui::Visibility::Visible);
    ensure("expanded identity returns collapsed", floater->header()->children()[1]->visibility() == radia::ui::Visibility::Collapsed);

    floater->clearChildren();
    ensure("clearing authored children preserves owned header", floater->header() != nullptr);
    ensure("clearing authored children preserves owned content box", floater->content() != nullptr);
    ensure("clearing authored children expires content reference", !content);
    ensure("clearing authored children expires custom header reference", !refresh);

    const LayoutBuildResult missingTitle = factory.buildWidgetTreeFromString("<floater canMinimize=\"true\"/>", "missing_title.xml");
    ensure("minimizable floater requires title", !missingTitle.ok());
    ensure_equals("title diagnostic is stable", missingTitle.errors.front().code, "layout.floater.title_required");

    const LayoutBuildResult duplicateHeader = factory.buildWidgetTreeFromString("<floater><header/><header/></floater>", "duplicate_header.xml");
    ensure("duplicate custom header rejects widget tree", !duplicateHeader.ok());
    ensure_equals("duplicate header diagnostic is stable", duplicateHeader.errors.front().code, "layout.part.duplicate");

    const LayoutBuildResult attachedOnly = factory.buildWidgetTreeFromString("<floater canDetach=\"false\"/>", "attached_only.xml");
    ensure("floater accepts detach policy", attachedOnly.ok());
    ensure("floater detach policy defaults on and can opt out", !attachedOnly.rootAs<Floater>()->canDetach());
}

template<> template<> void resourceCompilerObject::test<14>() {
    resources["widgets/floater.xml"] = "<floater closeIcon=\"close\" minimizeIcon=\"minimize\" canClose=\"false\"/>";
    resources["defaulted.xml"] = "<floater title=\"defaulted\" canClose=\"true\"/>";

    ensure("Widget Defaults validate independently with case-insensitive lookup", !factory.validateWidgetDefaults("FLOATER").hasErrors());

    LayoutBuildResult result = factory.buildWidgetTreeFromResource("defaulted.xml");
    auto* floater = result.rootAs<Floater>();
    ensure("Widget Defaults apply", result.ok() && floater);
    ensure_equals("default close icon applied", floater->closeIcon(), std::string("close"));
    ensure_equals("default minimize icon applied", floater->minimizeIcon(), std::string("minimize"));
    ensure_equals("default close icon reaches declared part", floater->closeButton()->icon()->name(), std::string("close"));
    ensure_equals("default minimize icon reaches declared part", floater->minimizeButton()->icon()->name(), std::string("minimize"));
    ensure("Widget tree attribute overrides Widget Default", floater->canClose());

    resources["widgets/floater.xml"] = "<panel/>";
    ensure("Widget Defaults validation reports invalid root", factory.validateWidgetDefaults("floater").hasErrors());
    const LayoutBuildResult invalid = factory.buildWidgetTreeFromResource("defaulted.xml");
    ensure("wrong Widget Defaults root rejects Widget tree", !invalid.ok());
    ensure("invalid Widget Defaults expose no partial root", invalid.root == nullptr);
}

template<> template<> void resourceCompilerObject::test<15>() {
    const char* kVisibilityLayout =
        "<panel><text id=\"shown\" visibility=\"visible\"/><text id=\"hidden\" visibility=\"hidden\"/><text id=\"collapsed\" visibility=\"collapsed\"/></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kVisibilityLayout, "visibility.xml");
    ensure("typed visibility values compile", result.ok());
    ensure_equals("Visible value is typed", static_cast<int>(result.root->children()[0]->visibility()),
                  static_cast<int>(radia::ui::Visibility::Visible));
    ensure_equals("Hidden value is typed", static_cast<int>(result.root->children()[1]->visibility()),
                  static_cast<int>(radia::ui::Visibility::Hidden));
    ensure_equals("Collapsed value is typed", static_cast<int>(result.root->children()[2]->visibility()),
                  static_cast<int>(radia::ui::Visibility::Collapsed));

    const LayoutBuildResult invalid = factory.buildWidgetTreeFromString("<text visibility=\"invisible\"/>", "invalid_visibility.xml");
    ensure("invalid visibility is rejected", !invalid.ok());
    ensure_equals("invalid visibility diagnostic is stable", invalid.errors.front().code, "layout.attribute.visibility_invalid");

    const LayoutBuildResult legacy = factory.buildWidgetTreeFromString("<text visible=\"false\"/>", "legacy_visibility.xml");
    ensure("legacy boolean visibility is rejected", !legacy.ok());
    ensure_equals("legacy visibility is an unknown attribute", legacy.errors.front().code, "layout.attribute.unknown");

    const LayoutBuildResult legacyBinding = factory.buildWidgetTreeFromString("<switch bind=\"old-setting\"/>", "legacy-bind.xml");
    ensure("legacy provider binding syntax is rejected", !legacyBinding.ok());
    ensure_equals("legacy provider binding is an unknown attribute", legacyBinding.errors.front().code, "layout.attribute.unknown");
}

template<> template<> void resourceCompilerObject::test<16>() {
    resources["widgets/label.xml"] = "<label visibility=\"sometimes\"/>";
    const DiagnosticResult visibility = factory.validateWidgetDefaults("label");
    ensure("Widget Defaults validate typed visibility values", visibility.hasErrors());
    ensure_equals("Widget Defaults preserve visibility diagnostics", visibility.errors.front().code, "layout.attribute.visibility_invalid");

    resources["widgets/label.xml"] = "<label/>";
    resources["widgets/switch.xml"] = "<switch checked=\"sometimes\"/>";
    const DiagnosticResult widget_attribute = factory.validateWidgetDefaults("switch");
    ensure("Widget Defaults validate widget-specific typed values", widget_attribute.hasErrors());
    ensure_equals("Widget Defaults preserve widget attribute diagnostics", widget_attribute.errors.front().code, "layout.attribute.boolean_invalid");
}

template<> template<> void resourceCompilerObject::test<17>() {
    const char* kCaseInsensitiveLayout =
        "<FlOaTeR TiTlE=\"tools\" CaNMiNiMiZe=\"true\"><HeAdEr><BuTtOn ID=\"saveFile\" ONCLICK=\"saveFile()\"><IcOn SrC=\"search\"/>Save</BuTtOn></HeAdEr></FlOaTeR>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kCaseInsensitiveLayout, "case-insensitive.xml");
    auto* floater = result.rootAs<Floater>();
    ensure("element and attribute lookup is ASCII case-insensitive", result.ok() && floater);
    auto button = requireWidget<Button>(*floater, "saveFile");
    ensure("mixed-case schema names retain canonical widget behavior", button && button->icon());
    ensure_equals("contract retains canonical element spelling", button->elementName(), std::string("button"));
    ensure_equals("mixed-case Event attribute resolves", button->eventCall(WidgetEventKind::Click)->name(), std::string("saveFile"));
}

template<> template<> void resourceCompilerObject::test<18>() {
    const LayoutBuildResult snake = factory.buildWidgetTreeFromString("<BuTtOn\n  on_click=\"saveFile()\"/>", "legacy-snake.xml");
    ensure("legacy snake_case attribute is rejected", !snake.ok());
    ensure_equals("legacy attribute is not an alias", snake.errors.front().code, std::string("layout.attribute.unknown"));
    ensure_equals("legacy attribute diagnostic retains source line", snake.errors.front().line, std::size_t(1));
    ensure("known element names use canonical spelling in diagnostics", snake.errors.front().message.find("<button>") != std::string::npos);

    const LayoutBuildResult duplicate =
        factory.buildWidgetTreeFromString("<button onClick=\"saveOne()\" ONCLICK=\"saveTwo()\"/>", "duplicate-case.xml");
    ensure("attributes colliding after case folding are rejected", !duplicate.ok());
    ensure_equals("case-folded duplicate has a stable diagnostic", duplicate.errors.front().code, std::string("layout.attribute.duplicate"));

    const LayoutBuildResult invalidId = factory.buildWidgetTreeFromString("<panel id=\"bad.id\"/>", "id.xml");
    ensure("invalid Widget ID characters are rejected", !invalidId.ok());
    ensure_equals("invalid ID diagnostic is stable", invalidId.errors.front().code, std::string("layout.id.invalid"));

    const LayoutBuildResult invalidClass = factory.buildWidgetTreeFromString("<panel class=\"BadClass\"/>", "class.xml");
    ensure("non-kebab Widget class is rejected", !invalidClass.ok());
    ensure_equals("invalid class diagnostic is stable", invalidClass.errors.front().code, std::string("layout.class.invalid"));

    const LayoutBuildResult invalidHandler = factory.buildWidgetTreeFromString("<button onClick=\"bad_action()\"/>", "handler-name.xml");
    ensure("invalid Handler name leaves the Widget tree usable", invalidHandler.ok());
    ensure_equals("invalid Handler name reports one warning", invalidHandler.warnings.size(), 1U);
    ensure_equals("invalid Handler diagnostic is stable", invalidHandler.warnings.front().code, std::string("layout.event.name_invalid"));

    ensure("Icon name alias is removed", !factory.buildWidgetTreeFromString("<icon name=\"search\"/>").ok());
    ensure("Icon icon alias is removed", !factory.buildWidgetTreeFromString("<icon icon=\"search\"/>").ok());
    ensure("Icon source alias is removed", !factory.buildWidgetTreeFromString("<icon source=\"search\"/>").ok());
    ensure("legacy qualified header element is removed", !factory.buildWidgetTreeFromString("<floater><floater.header/></floater>").ok());
}

template<> template<> void resourceCompilerObject::test<19>() {
    const char* kEventCallLayout =
        "<panel><button id=\"inspect\" onClick=\"inspect(4, 'settings', true, this, event)\"/><button id=\"bare\" onClick=\"press\"/><button id=\"lifecycle\" onClick=\"postBuild()\"/></panel>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kEventCallLayout, "event-calls.xml");
    ensure("valid and invalid Event Handler Calls keep the Widget tree usable", result.ok());
    ensure_equals("bare and lifecycle names each warn", result.warnings.size(), 2U);
    ensure_equals("bare name requires call syntax", result.warnings[0].code, std::string("layout.event.call_required"));
    ensure_equals("lifecycle Handler name is reserved", result.warnings[1].code, std::string("layout.event.handler_reserved"));

    auto inspect = requireWidget<Button>(*result.root, "inspect");
    auto bare = requireWidget<Button>(*result.root, "bare");
    auto lifecycle = requireWidget<Button>(*result.root, "lifecycle");
    ensure("parsed Event Handler Call is attached", inspect && inspect->eventCall(WidgetEventKind::Click));
    ensure_equals("parsed Event Handler name retained", inspect->eventCall(WidgetEventKind::Click)->name(), std::string("inspect"));
    ensure_equals("all parsed arguments retained", inspect->eventCall(WidgetEventKind::Click)->arguments().size(), 5U);
    ensure("invalid and reserved calls attach no runtime binding",
           bare && lifecycle && !bare->eventCall(WidgetEventKind::Click) && !lifecycle->eventCall(WidgetEventKind::Click));
}
} // namespace tut
