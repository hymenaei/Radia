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
#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include "layout/document.h"
#include "layout/resourcecompiler.h"
#include "skin/compiler.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "test_layout_helpers.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/div.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"

namespace {
using radia::ui::Button;
using radia::ui::DiagnosticResult;
using radia::ui::Div;
using radia::ui::fixedTextMetrics;
using radia::ui::Floater;
using radia::ui::Label;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutDocumentParser;
using radia::ui::LayoutDocumentParseResult;
using radia::ui::Panel;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::Switch;
using radia::ui::Style;
using radia::ui::System;
using radia::ui::Visibility;
using radia::ui::Widget;
using radia::ui::WidgetEventKind;
using radia::ui::WidgetRef;
using radia::ui::detail::findWidgetInScope;
using radia::ui::test::LayoutCompilerTestHelper;
using ::testing::Message;

class LayoutResourceCompilerTest : public ::testing::Test {
protected:
    template<typename WidgetT> WidgetRef<WidgetT> requireWidget(Widget& root, const std::string& id) const {
        return WidgetRef<WidgetT>(dynamic_cast<WidgetT*>(findWidgetInScope(root, id)));
    }

    LayoutCompilerTestHelper factory;
    std::map<std::string, std::string>& resources = factory.resources;
};
} // namespace

TEST_F(LayoutResourceCompilerTest, BuildsFloaterWithControlsAndEventCalls) {
    constexpr char kFloaterLayout[] = "<floater title=\"title\" closeIcon=\"close\" minimizeIcon=\"minimize\" canMinimize=\"true\">"
                                      "<p id=\"status\">Ready</p>"
                                      "<button id=\"go\" onClick=\"demoGo()\" onDoubleClick=\"demoDouble()\" "
                                      "onMouseDown=\"demoPress()\" onLongClick=\"demoHold()\" "
                                      "onContextMenu=\"demoMenu()\" longClickDelay=\"750ms\">"
                                      "<icon src=\"search\"/>Go</button><switch id=\"toggle\" checked=\"true\" "
                                      "onChange=\"demoChanged()\"/></floater>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kFloaterLayout, "floater.xml");
    ASSERT_TRUE(result.ok());

    Floater* floater = result.rootAs<Floater>();
    ASSERT_NE(floater, nullptr);
    EXPECT_EQ(floater->title(), "title");
    ASSERT_NE(floater->header(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);

    const WidgetRef<Button> go = requireWidget<Button>(*floater, "go");
    const WidgetRef<Switch> toggle = requireWidget<Switch>(*floater, "toggle");
    ASSERT_TRUE(go);
    ASSERT_TRUE(toggle);
    EXPECT_TRUE(toggle->checked());

    ASSERT_NE(go->eventCall(WidgetEventKind::Click), nullptr);
    ASSERT_NE(go->eventCall(WidgetEventKind::DoubleClick), nullptr);
    ASSERT_NE(go->eventCall(WidgetEventKind::MouseDown), nullptr);
    ASSERT_NE(go->eventCall(WidgetEventKind::LongClick), nullptr);
    ASSERT_NE(go->eventCall(WidgetEventKind::ContextMenu), nullptr);
    EXPECT_EQ(go->eventCall(WidgetEventKind::Click)->name(), "demoGo");
    EXPECT_EQ(go->eventCall(WidgetEventKind::DoubleClick)->name(), "demoDouble");
    EXPECT_EQ(go->eventCall(WidgetEventKind::MouseDown)->name(), "demoPress");
    EXPECT_EQ(go->eventCall(WidgetEventKind::LongClick)->name(), "demoHold");
    EXPECT_EQ(go->eventCall(WidgetEventKind::ContextMenu)->name(), "demoMenu");
    ASSERT_TRUE(go->longClickDelay().has_value());
    EXPECT_EQ(go->longClickDelay()->count(), 750LL);

    ASSERT_NE(toggle->eventCall(WidgetEventKind::Change), nullptr);
    EXPECT_EQ(toggle->eventCall(WidgetEventKind::Change)->name(), "demoChanged");
}

TEST_F(LayoutResourceCompilerTest, BuildsStructuralDivs) {
    constexpr char kDivLayout[] = "<div id=\"root\" class=\"stack\"><div id=\"group\"><p id=\"child\">content</p></div></div>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kDivLayout, "div.xml");
    ASSERT_TRUE(result.ok());

    const Div* root = result.rootAs<Div>();
    ASSERT_NE(root, nullptr);
    EXPECT_EQ(root->elementName(), "div");
    EXPECT_EQ(root->classes().count("stack"), 1U);
    ASSERT_EQ(root->children().size(), 1U);

    const Div* group = dynamic_cast<const Div*>(root->children().front().get());
    ASSERT_NE(group, nullptr);
    ASSERT_EQ(group->children().size(), 1U);
    EXPECT_EQ(group->children().front()->id(), "child");
}

TEST_F(LayoutResourceCompilerTest, InstantiatesIndependentEmbeddedResourcePanels) {
    constexpr char kSharedResourceLayout[] = "<panel id=\"base\" class=\"shared\"><p id=\"resourceChild\">base</p></panel>";
    constexpr char kEmbeddedPanelsLayout[] = "<panel><panel filename=\"shared.xml\" id=\"one\" class=\"first\">"
                                             "<p id=\"inlineChild\"/></panel>"
                                             "<panel filename=\"shared.xml\" id=\"two\"/></panel>";
    resources["shared.xml"] = kSharedResourceLayout;
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kEmbeddedPanelsLayout, "outer.xml");
    ASSERT_TRUE(result.ok());

    const WidgetRef<Panel> first = requireWidget<Panel>(*result.root, "one");
    const WidgetRef<Panel> second = requireWidget<Panel>(*result.root, "two");
    ASSERT_TRUE(first);
    ASSERT_TRUE(second);
    EXPECT_NE(first.get(), second.get());
    EXPECT_EQ(first->classes().count("shared"), 1U);
    EXPECT_EQ(first->classes().count("first"), 1U);
    ASSERT_FALSE(first->children().empty());
    ASSERT_FALSE(second->children().empty());
    EXPECT_EQ(first->children().front()->id(), "resourceChild");
    EXPECT_EQ(first->children().back()->id(), "inlineChild");

    first->setVisibility(Visibility::Collapse);
    EXPECT_EQ(second->visibility(), Visibility::Visible);
}

TEST_F(LayoutResourceCompilerTest, ResolvesNestedResourceReferences) {
    constexpr char kInnerResourceLayout[] = "<panel id=\"inner\"/>";
    constexpr char kMiddleResourceLayout[] = "<panel><panel filename=\"inner.xml\"/></panel>";
    constexpr char kOuterResourceLayout[] = "<panel><panel filename=\"nested/middle.xml\"/></panel>";
    resources["nested/inner.xml"] = kInnerResourceLayout;
    resources["nested/middle.xml"] = kMiddleResourceLayout;
    resources["outer.xml"] = kOuterResourceLayout;

    const LayoutBuildResult result = factory.buildWidgetTreeFromResource("outer.xml");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.root);
    ASSERT_EQ(result.root->children().size(), 1U);
    ASSERT_EQ(result.root->children()[0]->children().size(), 1U);
    EXPECT_EQ(result.root->children()[0]->children()[0]->id(), "inner");
}

TEST_F(LayoutResourceCompilerTest, RejectsResourceCycles) {
    constexpr char kFirstCycleLayout[] = "<panel><panel filename=\"b.xml\"/></panel>";
    constexpr char kSecondCycleLayout[] = "<panel><panel filename=\"a.xml\"/></panel>";
    resources["a.xml"] = kFirstCycleLayout;
    resources["b.xml"] = kSecondCycleLayout;

    const LayoutBuildResult result = factory.buildWidgetTreeFromResource("a.xml");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.root);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.resource.cycle");
    EXPECT_EQ(result.errors.front().source, "a.xml");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidResourceReferences) {
    struct InvalidReferenceCase {
        const char* name;
        const char* markup;
        const char* resourceId;
        const char* resourceMarkup;
    };

    const InvalidReferenceCase cases[] = {
        {"missing resource", "<panel><panel filename=\"missing.xml\"/></panel>", nullptr, nullptr},
        {"non-panel resource", "<panel><panel filename=\"wrong.xml\"/></panel>", "wrong.xml", "<p/>"},
        {"filename on non-panel", "<button filename=\"x.xml\"/>", nullptr, nullptr},
        {"resource root escape", "<panel><panel filename=\"../outside.xml\"/></panel>", nullptr, nullptr},
    };

    for (const auto& test : cases) {
        resources.clear();
        if (test.resourceId != nullptr) resources[test.resourceId] = test.resourceMarkup;

        SCOPED_TRACE(Message() << "invalid resource reference: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.markup, "root.xml");
        ASSERT_FALSE(result.ok());
        EXPECT_FALSE(result.root);
    }

    resources["empty.xml"] = "";
    const LayoutBuildResult empty = factory.buildWidgetTreeFromResource("empty.xml");
    ASSERT_FALSE(empty.ok());
    ASSERT_FALSE(empty.errors.empty());
    EXPECT_EQ(empty.errors.front().code, "layout.xml.invalid");
}

TEST_F(LayoutResourceCompilerTest, PreservesCompilerSourceLocations) {
    constexpr char kSourceLocationLayout[] = "<panel>\n"
                                             "  <unknown/>\n"
                                             "</panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kSourceLocationLayout, "source_ranges.xml");
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().line, 2U);
    EXPECT_EQ(result.errors.front().column, 3U);
}

TEST_F(LayoutResourceCompilerTest, PreservesMixedContentOrderAndSourceRanges) {
    constexpr char kMixedContentLayout[] = "<panel>before"
                                           "<label>middle</label>"
                                           "after</panel>";
    const LayoutDocumentParseResult parsed = LayoutDocumentParser().parse(kMixedContentLayout, "mixed.xml");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 3U);
    EXPECT_TRUE(parsed.document->root->content[0].isText());
    EXPECT_FALSE(parsed.document->root->content[1].isText());
    EXPECT_TRUE(parsed.document->root->content[2].isText());
    EXPECT_EQ(parsed.document->root->content[1].source.begin.line, 1U);
    EXPECT_GE(parsed.document->root->content[1].source.end.offset, parsed.document->root->content[1].source.begin.offset);
}

TEST_F(LayoutResourceCompilerTest, ComposesButtonInlineChildrenAndRejectsLegacyPartMarkup) {
    constexpr char kButtonLayout[] = "<button>first"
                                     "<icon src=\"one\"/></button>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kButtonLayout);
    ASSERT_TRUE(result.ok());
    Button* button = result.rootAs<Button>();
    ASSERT_NE(button, nullptr);
    ASSERT_NE(button->icon(), nullptr);
    ASSERT_NE(button->label(), nullptr);
    EXPECT_EQ(button->icon()->name(), "one");
    EXPECT_EQ(button->label()->text(), "first");
    EXPECT_EQ(button->label()->elementName(), "button-caption");
    ASSERT_GE(button->children().size(), 2U);
    EXPECT_EQ(button->children()[0].get(), button->label());

    constexpr char kIconFirstLayout[] = "<button><icon src=\"search\"/>"
                                        "second</button>";
    LayoutBuildResult iconFirst = factory.buildWidgetTreeFromString(kIconFirstLayout);
    ASSERT_TRUE(iconFirst.ok());
    Button* reversed = iconFirst.rootAs<Button>();
    ASSERT_NE(reversed, nullptr);
    ASSERT_FALSE(reversed->children().empty());
    EXPECT_EQ(reversed->children()[0].get(), reversed->icon());

    button->setIcon("updated");
    button->setLabel("updated");
    EXPECT_EQ(button->children().size(), 2U);
    button->clearChildren();
    EXPECT_EQ(button->icon(), nullptr);
    EXPECT_EQ(button->label(), nullptr);
    button->setIcon("rebuilt");
    ASSERT_NE(button->icon(), nullptr);
    EXPECT_EQ(button->icon()->name(), "rebuilt");

    struct LegacyMarkupCase {
        const char* name;
        const char* markup;
    };
    const LegacyMarkupCase cases[] = {
        {"button label part", "<button><button.label value=\"old\"/></button>"},
        {"button icon part", "<button><button.icon name=\"old\"/></button>"},
        {"switch label part", "<switch><switch.label value=\"old\"/></switch>"},
        {"label value attribute", "<label value=\"old\"/>"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "removed inline markup: " << test.name);
        EXPECT_FALSE(factory.buildWidgetTreeFromString(test.markup).ok());
    }
}

TEST_F(LayoutResourceCompilerTest, MatchesPaintProtocolWithShaderConstants) {
    const std::filesystem::path uiSourceRoot = std::filesystem::path(__FILE__).parent_path().parent_path().parent_path();
    const std::filesystem::path newviewSourceRoot = uiSourceRoot.parent_path() / "newview";
    std::ifstream vertexFile(newviewSourceRoot / "app_settings/shaders/class1/interface/uiV.glsl");
    std::ifstream fragmentFile(newviewSourceRoot / "app_settings/shaders/class1/interface/uiF.glsl");
    std::ifstream paintProtocolFile(uiSourceRoot / "render/paintprotocol.def");
    ASSERT_TRUE(vertexFile.good());
    ASSERT_TRUE(fragmentFile.good());
    ASSERT_TRUE(paintProtocolFile.good());

    std::ostringstream vertex;
    std::ostringstream fragment;
    std::ostringstream paintProtocol;
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
    const auto expectProtocol = [&](const char* macro, const char* shaderPrefix) {
        const auto definitions = parseDefinition(macro);
        const auto constants = parseShaderConstants(shaderPrefix);
        EXPECT_EQ(definitions.size(), constants.size()) << macro;
        for (const auto& [name, value] : definitions) {
            SCOPED_TRACE(Message() << "paint protocol entry: " << name);
            const auto found = constants.find(name);
            ASSERT_NE(found, constants.end());
            EXPECT_EQ(found->second, value);
        }
    };

    expectProtocol("PAINT_OP_ENTRY", "kPaintOp");
    expectProtocol("GRADIENT_OP_ENTRY", "kGradient");
    expectProtocol("OUTLINE_OP_ENTRY", "kOutline");

    EXPECT_TRUE(contains(vertexSource, "#ifdef PAINT_SHADER"));
    EXPECT_TRUE(contains(fragmentSource, "#ifdef PAINT_SHADER"));
    EXPECT_TRUE(contains(vertexSource, "shapeCoord = texcoord0"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpDirect = 0")
                && contains(fragmentSource, "kPaintOpFill = 1")
                && contains(fragmentSource, "paintOp == kPaintOpDirect"));
    EXPECT_TRUE(contains(paintProtocolSource, "PAINT_OP_ENTRY(Direct, 0)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Fill, 1)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Border, 2)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Gradient, 3)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(OuterShadow, 4)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(InsetShadow, 5)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(GradientBorder, 6)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Blur, 7)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Composite, 8)"));
    EXPECT_TRUE(contains(fragmentSource, "uniform int paintOp;")
                && contains(fragmentSource, "uniform vec4 shapeRect;")
                && contains(fragmentSource, "uniform vec2 effectTextureSize;")
                && contains(fragmentSource, "uniform int gradientKind;")
                && contains(fragmentSource, "uniform int outlineStyle;")
                && !contains(fragmentSource, "rduiPaintOp"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpBorder = 2")
                && contains(fragmentSource, "paintOp == kPaintOpBorder")
                && contains(fragmentSource, "fwidth"));
    EXPECT_TRUE(contains(fragmentSource, "kGradientLinear = 0")
                && contains(fragmentSource, "kGradientRadial = 1")
                && contains(fragmentSource, "kGradientConic = 2")
                && contains(fragmentSource, "kOutlineSolid = 0")
                && contains(fragmentSource, "kOutlineDashed = 1"));
    EXPECT_TRUE(contains(fragmentSource, "topBorderGap"));
    EXPECT_TRUE(contains(fragmentSource, "gradientKind") && contains(fragmentSource, "atan(delta.x, delta.y)"));
    EXPECT_TRUE(contains(fragmentSource, "gradientRepeating")
                && contains(fragmentSource, "underlyingGradientIntegral")
                && contains(fragmentSource, "cycles * repeatingTotal"));
    EXPECT_TRUE(contains(fragmentSource, "gradientPixelWidth")
                && contains(fragmentSource, "filteredGradientColor")
                && contains(fragmentSource, "gradientIntervalIntegral")
                && contains(fragmentSource, "dFdx(delta)"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpGradientBorder = 6")
                && contains(fragmentSource, "paintOp == kPaintOpGradientBorder")
                && contains(fragmentSource, "borderWidths"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpBlur = 7")
                && contains(fragmentSource, "kPaintOpComposite = 8")
                && contains(fragmentSource, "paintOp == kPaintOpBlur")
                && contains(fragmentSource, "paintOp == kPaintOpComposite")
                && contains(fragmentSource, "blurredEffectColor")
                && contains(fragmentSource, "maxSamplesPerSide")
                && contains(fragmentSource, "totalWeight"));
    EXPECT_TRUE(contains(fragmentSource, "return vec4(color.rgb, mask);") && !contains(fragmentSource, "color.a * mask"));
}

TEST_F(LayoutResourceCompilerTest, RefreshesLocalizedWidgetsAcrossLocaleChanges) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalizationYaml[] = "defaultLocale: en\n"
                                         "locales: {en: {name: English, strings: {title: Title, status: Ready, press: Press}}, "
                                         "pt: {name: Português, strings: {title: Título, status: Pronto, press: Pressione}}}\n";
    constexpr char kStyles[] = "label { text-color: #ffffffff; }";
    constexpr char kLocalizedLayout[] = "<floater title=\"title\"><label id=\"status\" for=\"target\">status</label>"
                                        "<switch id=\"target\"/>"
                                        "<button id=\"press\">press</button></floater>";
    resources.add("localization.yaml", kLocalizationYaml);
    resources.add("skin.radia", kStyles);
    resources.add("localized.xml", kLocalizedLayout);

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ASSERT_TRUE(prepared.ok());
    system.publish(prepared.generation);

    LayoutBuildResult result = system.buildWidgetTree("localized.xml");
    ASSERT_TRUE(result.ok());
    Floater* floater = result.rootAs<Floater>();
    ASSERT_NE(floater, nullptr);
    const WidgetRef<Label> status = requireWidget<Label>(*floater, "status");
    const WidgetRef<Button> press = requireWidget<Button>(*floater, "press");
    ASSERT_TRUE(status);
    ASSERT_TRUE(press);
    ASSERT_NE(press->label(), nullptr);
    EXPECT_EQ(floater->title(), "Title");
    EXPECT_EQ(status->text(), "Ready");
    EXPECT_EQ(press->label()->text(), "Press");

    std::unique_ptr<Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    auto localizedFloater = std::unique_ptr<Floater>(static_cast<Floater*>(result.root.release()));
    ASSERT_NE(localizedFloater, nullptr);
    surface->mountFloater(std::move(localizedFloater));
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(floater->title(), "Título");
    EXPECT_EQ(status->text(), "Pronto");
    EXPECT_EQ(press->label()->text(), "Pressione");

    auto programmatic = std::make_unique<Floater>();
    Floater* programmaticFloater = programmatic.get();
    programmatic->setTitle("title");
    surface->mountFloater(std::move(programmatic));
    EXPECT_EQ(programmaticFloater->title(), "Título");

    status->setContent(system.localize("status"));
    ASSERT_TRUE(system.setLocale("en"));
    EXPECT_EQ(programmaticFloater->title(), "Title");
    EXPECT_EQ(status->text(), "Ready");
    status->setText("Literal");
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(status->text(), "Literal");

    constexpr char kMissingLayout[] = "<p>missing</p>";
    resources.add("missing.xml", kMissingLayout);
    const SkinGenerationPrepareResult missing = SkinCompiler().prepare(std::move(resources));
    EXPECT_FALSE(missing.ok());
    EXPECT_FALSE(missing.generation);
}

TEST_F(LayoutResourceCompilerTest, RejectsUnknownElementsAndAttributes) {
    constexpr char kUnknownElementLayout[] = "<panel>"
                                             "<unknown/></panel>";
    constexpr char kLegacyTextElementLayout[] = "<text>legacy</text>";
    constexpr char kUnsupportedAttributeLayout[] = "<panel width=\"10\"/>";
    constexpr char kUnknownAttributeLayout[] = "<floater invented=\"true\"/>";
    const LayoutBuildResult unknownElement = factory.buildWidgetTreeFromString(kUnknownElementLayout, "unknown.xml");
    ASSERT_FALSE(unknownElement.ok());
    EXPECT_FALSE(unknownElement.root);
    ASSERT_FALSE(unknownElement.errors.empty());
    EXPECT_EQ(unknownElement.errors.front().code, "layout.element.unknown");
    EXPECT_EQ(unknownElement.errors.front().source, "unknown.xml");

    const LayoutBuildResult legacyTextElement = factory.buildWidgetTreeFromString(kLegacyTextElementLayout, "legacy-text.xml");
    ASSERT_FALSE(legacyTextElement.ok());
    ASSERT_FALSE(legacyTextElement.errors.empty());
    EXPECT_EQ(legacyTextElement.errors.front().code, "layout.element.unknown");

    const LayoutBuildResult unsupportedAttribute = factory.buildWidgetTreeFromString(kUnsupportedAttributeLayout, "attribute.xml");
    ASSERT_FALSE(unsupportedAttribute.ok());
    EXPECT_FALSE(unsupportedAttribute.root);
    ASSERT_FALSE(unsupportedAttribute.errors.empty());
    EXPECT_EQ(unsupportedAttribute.errors.front().code, "layout.attribute.unsupported");

    const LayoutBuildResult unknownAttribute = factory.buildWidgetTreeFromString(kUnknownAttributeLayout, "unknown_attribute.xml");
    ASSERT_FALSE(unknownAttribute.ok());
    EXPECT_FALSE(unknownAttribute.root);
    ASSERT_FALSE(unknownAttribute.errors.empty());
    EXPECT_EQ(unknownAttribute.errors.front().code, "layout.attribute.unknown");
}

TEST_F(LayoutResourceCompilerTest, WarnsForUnsupportedEventsAndExpressions) {
    constexpr char kUnsupportedEventLayout[] = "<p onClick=\"click()\">copy</p>";
    constexpr char kExpressionCallLayout[] = "<button onClick=\"save(force=true)\"/>";
    const LayoutBuildResult unsupportedEvent = factory.buildWidgetTreeFromString(kUnsupportedEventLayout, "event.xml");
    ASSERT_TRUE(unsupportedEvent.ok());
    ASSERT_TRUE(unsupportedEvent.root);
    ASSERT_EQ(unsupportedEvent.warnings.size(), 1U);
    EXPECT_EQ(unsupportedEvent.warnings.front().code, "layout.event.unsupported");

    const LayoutBuildResult expressionCall = factory.buildWidgetTreeFromString(kExpressionCallLayout, "expression.xml");
    ASSERT_TRUE(expressionCall.ok());
    ASSERT_TRUE(expressionCall.root);
    ASSERT_EQ(expressionCall.warnings.size(), 1U);
    EXPECT_EQ(expressionCall.warnings.front().code, "layout.event.literal_unsupported");
}

TEST_F(LayoutResourceCompilerTest, RejectsDuplicateWidgetIds) {
    constexpr char kDuplicateIdLayout[] = "<panel><p id=\"same\"/>"
                                          "<button id=\"same\">Same</button></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kDuplicateIdLayout, "duplicates.xml");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.root);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.id.duplicate");
    EXPECT_EQ(result.errors.front().source, "duplicates.xml");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidBooleanAttributes) {
    constexpr char kInvalidBooleanLayout[] = "<floater canClose=\"sometimes\">"
                                             "<switch checked=\"yes\"/></floater>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kInvalidBooleanLayout, "booleans.xml");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.root);
    ASSERT_EQ(result.errors.size(), 2U);
    EXPECT_EQ(result.errors.front().code, "layout.attribute.boolean_invalid");
    EXPECT_EQ(result.errors.front().source, "booleans.xml");
}

TEST_F(LayoutResourceCompilerTest, ValidatesLongClickDelayAndUnsupportedEvents) {
    constexpr char kMissingHandlerLayout[] = "<button longClickDelay=\"1s\"/>";
    constexpr char kInvalidLongClickLayout[] = "<button onLongClick=\"hold()\" "
                                               "longClickDelay=\"500\"/>";
    constexpr char kUnsupportedLongClickLayout[] = "<p onLongClick=\"hold()\">"
                                                   "copy</p>";
    const LayoutBuildResult missingHandler = factory.buildWidgetTreeFromString(kMissingHandlerLayout);
    ASSERT_TRUE(missingHandler.ok());
    ASSERT_FALSE(missingHandler.warnings.empty());
    EXPECT_EQ(missingHandler.warnings.front().code, "layout.long_click.event_missing");

    EXPECT_FALSE(factory.buildWidgetTreeFromString(kInvalidLongClickLayout).ok());

    const LayoutBuildResult unsupported = factory.buildWidgetTreeFromString(kUnsupportedLongClickLayout);
    ASSERT_TRUE(unsupported.ok());
    ASSERT_FALSE(unsupported.warnings.empty());
    EXPECT_EQ(unsupported.warnings.front().code, "layout.event.unsupported");
}

TEST_F(LayoutResourceCompilerTest, ManagesCustomFloaterHeaderLifecycle) {
    constexpr char kCustomHeaderLayout[] = "<floater title=\"tools\" icon=\"search\" canMinimize=\"true\" showHeaderIdentity=\"false\">"
                                           "<header><button id=\"refresh\">Refresh</button></header>"
                                           "<panel id=\"content\"/></floater>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kCustomHeaderLayout, "custom_header.xml");
    ASSERT_TRUE(result.ok());
    Floater* floater = result.rootAs<Floater>();
    ASSERT_NE(floater, nullptr);

    WidgetRef<Panel> content = requireWidget<Panel>(*floater, "content");
    WidgetRef<Button> refresh = requireWidget<Button>(*floater, "refresh");
    ASSERT_TRUE(content);
    ASSERT_TRUE(refresh);
    ASSERT_NE(floater->header(), nullptr);
    ASSERT_NE(floater->content(), nullptr);
    EXPECT_EQ(content->parent(), floater->content());
    EXPECT_EQ(refresh->parent()->part(), "header::custom");
    EXPECT_EQ(refresh->parent(), floater->header()->children()[2].get());
    EXPECT_EQ(floater->header()->children()[3].get(), floater->minimizeButton());
    EXPECT_EQ(floater->minimizeButton()->children()[0]->part(), "header::minimize::icon");
    EXPECT_EQ(floater->minimizeButton()->icon(), floater->minimizeButton()->children()[0].get());
    EXPECT_EQ(floater->header()->children()[0]->visibility(), Visibility::Visible);
    EXPECT_EQ(floater->header()->children()[1]->visibility(), Visibility::Visible);

    floater->setMinimized(true);
    EXPECT_EQ(floater->header()->children()[0]->visibility(), Visibility::Visible);
    EXPECT_EQ(floater->header()->children()[1]->visibility(), Visibility::Visible);
    EXPECT_EQ(refresh->parent()->visibility(), Visibility::Visible);
    EXPECT_EQ(floater->content()->visibility(), Visibility::Visible);
    EXPECT_FALSE(refresh->parent()->isDisplayed(Style{}));
    EXPECT_FALSE(floater->content()->isDisplayed(Style{}));

    floater->setMinimized(false);
    EXPECT_EQ(refresh->parent()->visibility(), Visibility::Visible);
    EXPECT_EQ(floater->header()->children()[1]->visibility(), Visibility::Visible);
    EXPECT_TRUE(refresh->parent()->isDisplayed(Style{}));
    EXPECT_TRUE(floater->content()->isDisplayed(Style{}));

    floater->clearChildren();
    EXPECT_NE(floater->header(), nullptr);
    EXPECT_NE(floater->content(), nullptr);
    EXPECT_FALSE(content);
    EXPECT_FALSE(refresh);
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidFloaterHeaderConfiguration) {
    constexpr char kMissingTitleLayout[] = "<floater canMinimize=\"true\"/>";
    constexpr char kDuplicateHeaderLayout[] = "<floater>"
                                              "<header/><header/></floater>";
    const LayoutBuildResult missingTitle = factory.buildWidgetTreeFromString(kMissingTitleLayout, "missing_title.xml");
    ASSERT_FALSE(missingTitle.ok());
    ASSERT_FALSE(missingTitle.errors.empty());
    EXPECT_EQ(missingTitle.errors.front().code, "layout.floater.title_required");

    const LayoutBuildResult duplicateHeader = factory.buildWidgetTreeFromString(kDuplicateHeaderLayout, "duplicate_header.xml");
    ASSERT_FALSE(duplicateHeader.ok());
    ASSERT_FALSE(duplicateHeader.errors.empty());
    EXPECT_EQ(duplicateHeader.errors.front().code, "layout.part.duplicate");
}

TEST_F(LayoutResourceCompilerTest, AppliesWidgetDefaultsToFloater) {
    constexpr char kFloaterDefaultsLayout[] = "<floater closeIcon=\"close\" minimizeIcon=\"minimize\" canClose=\"false\"/>";
    constexpr char kDefaultedFloaterLayout[] = "<floater title=\"defaulted\" canClose=\"true\"/>";
    resources["widgets/floater.xml"] = kFloaterDefaultsLayout;
    resources["defaulted.xml"] = kDefaultedFloaterLayout;

    EXPECT_FALSE(factory.validateWidgetDefaults("FLOATER").hasErrors());
    LayoutBuildResult result = factory.buildWidgetTreeFromResource("defaulted.xml");
    ASSERT_TRUE(result.ok());
    Floater* floater = result.rootAs<Floater>();
    ASSERT_NE(floater, nullptr);
    EXPECT_EQ(floater->closeIcon(), "close");
    EXPECT_EQ(floater->minimizeIcon(), "minimize");
    ASSERT_NE(floater->closeButton(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_NE(floater->closeButton()->icon(), nullptr);
    ASSERT_NE(floater->minimizeButton()->icon(), nullptr);
    EXPECT_EQ(floater->closeButton()->icon()->name(), "close");
    EXPECT_EQ(floater->minimizeButton()->icon()->name(), "minimize");
    EXPECT_TRUE(floater->canClose());
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidWidgetDefaults) {
    constexpr char kInvalidFloaterDefaultsLayout[] = "<panel/>";
    constexpr char kDefaultedLayout[] = "<floater title=\"defaulted\"/>";
    resources["widgets/floater.xml"] = kInvalidFloaterDefaultsLayout;
    resources["defaulted.xml"] = kDefaultedLayout;

    EXPECT_TRUE(factory.validateWidgetDefaults("floater").hasErrors());
    const LayoutBuildResult result = factory.buildWidgetTreeFromResource("defaulted.xml");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.root);
}

TEST_F(LayoutResourceCompilerTest, CompilesTypedVisibilityValues) {
    constexpr char kVisibilityLayout[] = "<panel><p id=\"shown\" visibility=\"visible\"/>"
                                         "<p id=\"hidden\" visibility=\"hidden\"/>"
                                         "<p id=\"collapsed\" visibility=\"collapse\"/></panel>";
    const LayoutBuildResult result = factory.buildWidgetTreeFromString(kVisibilityLayout, "visibility.xml");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.root);
    ASSERT_EQ(result.root->children().size(), 3U);
    EXPECT_EQ(result.root->children()[0]->visibility(), Visibility::Visible);
    EXPECT_EQ(result.root->children()[1]->visibility(), Visibility::Hidden);
    EXPECT_EQ(result.root->children()[2]->visibility(), Visibility::Collapse);
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidAndLegacyVisibilitySyntax) {
    struct InvalidVisibilityCase {
        const char* name;
        const char* markup;
        const char* diagnostic;
    };
    const InvalidVisibilityCase cases[] = {
        {"invalid enum value", "<p visibility=\"invisible\"/>", "layout.attribute.visibility_invalid"},
        {"legacy collapsed spelling", "<p visibility=\"collapsed\"/>", "layout.attribute.visibility_invalid"},
        {"legacy boolean visibility", "<p visible=\"false\"/>", "layout.attribute.unknown"},
        {"legacy provider binding", "<switch bind=\"old-setting\"/>", "layout.attribute.unknown"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "visibility markup: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.markup, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(LayoutResourceCompilerTest, ValidatesWidgetDefaultDiagnostics) {
    constexpr char kInvalidVisibilityDefaultsLayout[] = "<label visibility=\"sometimes\"/>";
    constexpr char kLabelDefaultsLayout[] = "<label/>";
    constexpr char kInvalidSwitchDefaultsLayout[] = "<switch checked=\"sometimes\"/>";
    resources["widgets/label.xml"] = kInvalidVisibilityDefaultsLayout;
    const DiagnosticResult visibility = factory.validateWidgetDefaults("label");
    ASSERT_TRUE(visibility.hasErrors());
    EXPECT_EQ(visibility.errors.front().code, "layout.attribute.visibility_invalid");

    resources["widgets/label.xml"] = kLabelDefaultsLayout;
    resources["widgets/switch.xml"] = kInvalidSwitchDefaultsLayout;
    const DiagnosticResult widgetAttribute = factory.validateWidgetDefaults("switch");
    ASSERT_TRUE(widgetAttribute.hasErrors());
    EXPECT_EQ(widgetAttribute.errors.front().code, "layout.attribute.boolean_invalid");
}

TEST_F(LayoutResourceCompilerTest, AcceptsCaseInsensitiveMarkupNames) {
    constexpr char kCaseInsensitiveLayout[] = "<FlOaTeR TiTlE=\"tools\" CaNMiNiMiZe=\"true\"><HeAdEr>"
                                              "<BuTtOn ID=\"saveFile\" ONCLICK=\"saveFile()\">"
                                              "<IcOn SrC=\"search\"/>Save</BuTtOn></HeAdEr></FlOaTeR>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kCaseInsensitiveLayout, "case-insensitive.xml");
    ASSERT_TRUE(result.ok());
    Floater* floater = result.rootAs<Floater>();
    ASSERT_NE(floater, nullptr);
    const WidgetRef<Button> button = requireWidget<Button>(*floater, "saveFile");
    ASSERT_TRUE(button);
    ASSERT_NE(button->icon(), nullptr);
    EXPECT_EQ(button->elementName(), "button");
    ASSERT_NE(button->eventCall(WidgetEventKind::Click), nullptr);
    EXPECT_EQ(button->eventCall(WidgetEventKind::Click)->name(), "saveFile");
}

TEST_F(LayoutResourceCompilerTest, RejectsLegacyNamesAndCaseFoldedConflicts) {
    struct InvalidMarkupCase {
        const char* name;
        const char* markup;
        const char* diagnostic;
    };
    const InvalidMarkupCase cases[] = {
        {"snake-case event attribute", "<BuTtOn\n  on_click=\"saveFile()\"/>", "layout.attribute.unknown"},
        {"case-folded duplicate attribute", "<button onClick=\"saveOne()\" ONCLICK=\"saveTwo()\"/>", "layout.attribute.duplicate"},
        {"invalid widget id", "<panel id=\"bad.id\"/>", "layout.id.invalid"},
        {"invalid widget class", "<panel class=\"BadClass\"/>", "layout.class.invalid"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid markup: " << test.name);
        const LayoutBuildResult result = factory.buildWidgetTreeFromString(test.markup, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    constexpr char kInvalidHandlerLayout[] = "<button onClick=\"bad_action()\"/>";
    const LayoutBuildResult invalidHandler = factory.buildWidgetTreeFromString(kInvalidHandlerLayout, "handler-name.xml");
    ASSERT_TRUE(invalidHandler.ok());
    ASSERT_EQ(invalidHandler.warnings.size(), 1U);
    EXPECT_EQ(invalidHandler.warnings.front().code, "layout.event.name_invalid");

    const char* removedMarkup[] = {
        "<icon name=\"search\"/>",
        "<icon icon=\"search\"/>",
        "<icon source=\"search\"/>",
        "<floater><floater.header/></floater>",
    };
    for (const char* markup : removedMarkup) {
        SCOPED_TRACE(Message() << "removed legacy markup: " << markup);
        EXPECT_FALSE(factory.buildWidgetTreeFromString(markup).ok());
    }
}

TEST_F(LayoutResourceCompilerTest, PreservesValidEventCallsAndWarnsForInvalidCalls) {
    constexpr char kEventCallsLayout[] = "<panel><button id=\"inspect\" "
                                         "onClick=\"inspect(4, 'settings', true, this, event)\"/>"
                                         "<button id=\"bare\" onClick=\"press\"/>"
                                         "<button id=\"lifecycle\" onClick=\"postBuild()\"/></panel>";
    LayoutBuildResult result = factory.buildWidgetTreeFromString(kEventCallsLayout, "event-calls.xml");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), 2U);
    EXPECT_EQ(result.warnings[0].code, "layout.event.call_required");
    EXPECT_EQ(result.warnings[1].code, "layout.event.handler_reserved");

    const WidgetRef<Button> inspect = requireWidget<Button>(*result.root, "inspect");
    const WidgetRef<Button> bare = requireWidget<Button>(*result.root, "bare");
    const WidgetRef<Button> lifecycle = requireWidget<Button>(*result.root, "lifecycle");
    ASSERT_TRUE(inspect);
    ASSERT_TRUE(bare);
    ASSERT_TRUE(lifecycle);
    ASSERT_NE(inspect->eventCall(WidgetEventKind::Click), nullptr);
    EXPECT_EQ(inspect->eventCall(WidgetEventKind::Click)->name(), "inspect");
    EXPECT_EQ(inspect->eventCall(WidgetEventKind::Click)->arguments().size(), 5U);
    EXPECT_EQ(bare->eventCall(WidgetEventKind::Click), nullptr);
    EXPECT_EQ(lifecycle->eventCall(WidgetEventKind::Click), nullptr);
}
