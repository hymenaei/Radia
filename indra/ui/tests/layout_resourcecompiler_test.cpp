/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include "floater_test_helpers.h"
#include "elements/button.h"
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "event.h"
#include "layout/document.h"
#include "layout/engine.h"
#include "layout/resourcecompiler.h"
#include "skin/compiler.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "test_layout_helpers.h"
#include "text/metrics.h"

namespace {
using radia::ui::ButtonElement;
using radia::ui::DiagnosticResult;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::fixedTextMetrics;
using radia::ui::FloaterElement;
using radia::ui::InputElement;
using radia::ui::kChangeEvent;
using radia::ui::kClickEvent;
using radia::ui::kContextMenuEvent;
using radia::ui::kDoubleClickEvent;
using radia::ui::kInputEvent;
using radia::ui::kPointerDownEvent;
using radia::ui::kWheelEvent;
using radia::ui::LabelElement;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutResourceCompiler;
using radia::ui::PanelElement;
using radia::ui::resolveElementStyle;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::SourceDocumentParser;
using radia::ui::SourceDocumentParseResult;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::Tag;
using radia::ui::Visibility;
using radia::ui::detail::findElementInScope;
using radia::ui::detail::nodes;
using radia::ui::test::LayoutCompilerTestHelper;
using ::testing::Message;

class LayoutResourceCompilerTest : public ::testing::Test {
protected:
    template<typename ElementT> ElementRef<ElementT> requireElement(Element& root, const std::string& id) const {
        return ElementRef<ElementT>(dynamic_cast<ElementT*>(findElementInScope(root, id)));
    }

    LayoutCompilerTestHelper factory;
    std::map<std::string, std::string>& resources = factory.resources;
};
} // namespace

TEST_F(LayoutResourceCompilerTest, BuildsFloaterWithControlsAndEventCalls) {
    resources["elements/minimize.html"] = "<minimize><icon src=\"minimize\"/></minimize>";
    resources["elements/close.html"] = "<close><icon src=\"close\"/></close>";
    constexpr char kFloaterLayout[] = "<floater resizeable><head><title>title</title><minimize/><close/></head><body>"
                                      "<p id=\"status\">Ready</p>"
                                      "<button id=\"go\" onClick=\"demoGo()\" onDoubleClick=\"demoDouble()\" "
                                      "onPointerDown=\"demoPress()\" onContextMenu=\"demoMenu()\" onWheel=\"demoWheel()\">"
                                      "<icon src=\"search\"/>Go</button><input type=\"checkbox\" switch=\"true\" name=\"mode\" id=\"toggle\" "
                                      "checked=\"true\" onInput=\"demoInput()\" onChange=\"demoChanged()\"/></body></floater>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "floater.html");
    ASSERT_TRUE(result.ok());

    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    EXPECT_EQ(floater->title(), "title");
    ASSERT_NE(floater->head(), nullptr);
    ASSERT_NE(floater->body(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    EXPECT_TRUE(floater->resizeable());

    const ElementRef<ButtonElement> go = requireElement<ButtonElement>(*floater, "go");
    const ElementRef<InputElement> toggle = requireElement<InputElement>(*floater, "toggle");
    ASSERT_TRUE(go);
    ASSERT_TRUE(toggle);
    EXPECT_TRUE(toggle->checked());
    EXPECT_EQ(toggle->name(), "mode");

    ASSERT_NE(go->eventCall(kClickEvent), nullptr);
    ASSERT_NE(go->eventCall(kDoubleClickEvent), nullptr);
    ASSERT_NE(go->eventCall(kPointerDownEvent), nullptr);
    ASSERT_NE(go->eventCall(kContextMenuEvent), nullptr);
    ASSERT_NE(go->eventCall(kWheelEvent), nullptr);
    EXPECT_EQ(go->eventCall(kClickEvent)->name(), "demoGo");
    EXPECT_EQ(go->eventCall(kDoubleClickEvent)->name(), "demoDouble");
    EXPECT_EQ(go->eventCall(kPointerDownEvent)->name(), "demoPress");
    EXPECT_EQ(go->eventCall(kContextMenuEvent)->name(), "demoMenu");
    EXPECT_EQ(go->eventCall(kWheelEvent)->name(), "demoWheel");

    ASSERT_NE(toggle->eventCall(kChangeEvent), nullptr);
    EXPECT_EQ(toggle->eventCall(kChangeEvent)->name(), "demoChanged");
    ASSERT_NE(toggle->eventCall(kInputEvent), nullptr);
    EXPECT_EQ(toggle->eventCall(kInputEvent)->name(), "demoInput");
}

TEST_F(LayoutResourceCompilerTest, BuildsStructuralDivs) {
    constexpr char kDivLayout[] = "<div id=\"root\" class=\"stack\"><div id=\"group\"><p id=\"child\">content</p></div></div>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kDivLayout, "div.html");
    ASSERT_TRUE(result.ok());

    const Element* root = result.rootAs<Element>();
    ASSERT_NE(root, nullptr);
    EXPECT_EQ(root->elementName(), "div");
    EXPECT_EQ(root->classes().count("stack"), 1U);
    ASSERT_EQ(root->children().size(), 1U);

    const Element* group = root->children().front();
    ASSERT_NE(group, nullptr);
    EXPECT_EQ(group->elementName(), "div");
    ASSERT_EQ(group->children().size(), 1U);
    EXPECT_EQ(group->children().front()->id(), "child");
}

TEST_F(LayoutResourceCompilerTest, RefreshesCachedDocumentWhenResourceChanges) {
    ResourceSnapshot resources;
    resources.add("panel.html", "<panel><p id=\"first\"/></panel>");
    LayoutResourceCompiler compiler(&resources);

    const LayoutBuildResult first = compiler.buildElementTreeFromResource("panel.html");
    ASSERT_TRUE(first.ok());
    ASSERT_EQ(first.rootAs<PanelElement>()->children().size(), std::size_t{1});
    EXPECT_EQ(first.rootAs<PanelElement>()->children().front()->id(), "first");

    resources.add("panel.html", "<panel><p id=\"second\"/></panel>");
    const LayoutBuildResult second = compiler.buildElementTreeFromResource("panel.html");
    ASSERT_TRUE(second.ok());
    ASSERT_EQ(second.rootAs<PanelElement>()->children().size(), std::size_t{1});
    EXPECT_EQ(second.rootAs<PanelElement>()->children().front()->id(), "second");
}

TEST_F(LayoutResourceCompilerTest, InstantiatesIndependentEmbeddedResourcePanels) {
    constexpr char kSharedResourceLayout[] = "<panel id=\"base\" class=\"shared\"><p id=\"resourceChild\">base</p></panel>";
    constexpr char kEmbeddedPanelsLayout[] = "<panel><panel filename=\"shared.html\" id=\"one\" class=\"first\">"
                                             "<p id=\"inlineChild\"/></panel>"
                                             "<panel filename=\"shared.html\" id=\"two\"/></panel>";
    resources["shared.html"] = kSharedResourceLayout;
    LayoutBuildResult result = factory.buildElementTreeFromString(kEmbeddedPanelsLayout, "outer.html");
    ASSERT_TRUE(result.ok());

    const ElementRef<PanelElement> first = requireElement<PanelElement>(*result.document->documentElement(), "one");
    const ElementRef<PanelElement> second = requireElement<PanelElement>(*result.document->documentElement(), "two");
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
    constexpr char kMiddleResourceLayout[] = "<panel><panel filename=\"inner.html\"/></panel>";
    constexpr char kOuterResourceLayout[] = "<panel><panel filename=\"nested/middle.html\"/></panel>";
    resources["nested/inner.html"] = kInnerResourceLayout;
    resources["nested/middle.html"] = kMiddleResourceLayout;
    resources["outer.html"] = kOuterResourceLayout;

    const LayoutBuildResult result = factory.buildElementTreeFromResource("outer.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    ASSERT_EQ(result.document->documentElement()->children().size(), 1U);
    ASSERT_EQ(result.document->documentElement()->children()[0]->children().size(), 1U);
    EXPECT_EQ(result.document->documentElement()->children()[0]->children()[0]->id(), "inner");
}

TEST_F(LayoutResourceCompilerTest, RejectsResourceCycles) {
    constexpr char kFirstCycleLayout[] = "<panel><panel filename=\"b.html\"/></panel>";
    constexpr char kSecondCycleLayout[] = "<panel><panel filename=\"a.html\"/></panel>";
    resources["a.html"] = kFirstCycleLayout;
    resources["b.html"] = kSecondCycleLayout;

    const LayoutBuildResult result = factory.buildElementTreeFromResource("a.html");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.resource.cycle");
    EXPECT_EQ(result.errors.front().source, "a.html");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidResourceReferences) {
    struct InvalidReferenceCase {
        const char* name;
        const char* markup;
        const char* resourceId;
        const char* resourceMarkup;
        const char* diagnostic;
    };

    const InvalidReferenceCase cases[] = {
        {"missing resource", "<panel><panel filename=\"missing.html\"/></panel>", nullptr, nullptr, "layout.resource.missing"},
        {"non-panel resource", "<panel><panel filename=\"wrong.html\"/></panel>", "wrong.html", "<p/>", "layout.resource.root_invalid"},
        {"filename on non-panel", "<button filename=\"x.html\"/>", nullptr, nullptr, "layout.filename.unsupported"},
        {"resource root escape", "<panel><panel filename=\"../outside.html\"/></panel>", nullptr, nullptr, "layout.resource.path_invalid"},
    };

    for (const auto& test : cases) {
        resources.clear();
        if (test.resourceId != nullptr) resources[test.resourceId] = test.resourceMarkup;

        SCOPED_TRACE(Message() << "invalid resource reference: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.markup, "root.html");
        ASSERT_FALSE(result.ok());
        EXPECT_FALSE(result.document);
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    resources["empty.html"] = "";
    const LayoutBuildResult empty = factory.buildElementTreeFromResource("empty.html");
    ASSERT_FALSE(empty.ok());
    ASSERT_FALSE(empty.errors.empty());
    EXPECT_EQ(empty.errors.front().code, "layout.html.invalid");
}

TEST_F(LayoutResourceCompilerTest, PreservesCompilerSourceLocations) {
    constexpr char kSourceLocationLayout[] = "<panel>\n"
                                             "  <unknown/>\n"
                                             "</panel>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kSourceLocationLayout, "source_ranges.html");
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().line, 2U);
    EXPECT_EQ(result.errors.front().column, 3U);
}

TEST_F(LayoutResourceCompilerTest, PreservesMixedContentOrderAndSourceRanges) {
    constexpr char kMixedContentLayout[] = "<panel>before"
                                           "<label>middle</label>"
                                           "after</panel>";
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse(kMixedContentLayout, "mixed.html");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 3U);
    EXPECT_TRUE(parsed.document->root->content[0].isText());
    EXPECT_FALSE(parsed.document->root->content[1].isText());
    EXPECT_TRUE(parsed.document->root->content[2].isText());
    EXPECT_EQ(parsed.document->root->content[1].source.begin.line, 1U);
    EXPECT_GE(parsed.document->root->content[1].source.end.offset, parsed.document->root->content[1].source.begin.offset);
}

TEST_F(LayoutResourceCompilerTest, BuildsMixedTextAndElementsAsOneOrderedRuntimeTree) {
    constexpr char kMixedContentLayout[] = "<p>Hello <b>world</b><br/>Again</p>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kMixedContentLayout, "mixed-runtime.html");
    ASSERT_TRUE(result.ok());
    Element* root = result.document->documentElement();
    ASSERT_NE(root, nullptr);

    const auto runtimeChildren = nodes(*root);
    ASSERT_EQ(runtimeChildren.size(), 4U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->getData(), "Hello ");
    auto child = runtimeChildren.begin();
    ++child;
    ASSERT_NE(child->asElement(), nullptr);
    EXPECT_EQ(child->asElement()->elementName(), "b");
    EXPECT_EQ(child->asElement()->textContent(), "world");
    ++child;
    ASSERT_NE(child->asElement(), nullptr);
    EXPECT_EQ(child->asElement()->elementName(), "br");
    ++child;
    ASSERT_NE(child->asText(), nullptr);
    EXPECT_TRUE(radia::ui::detail::NodeAccess::flowBreakBefore(*child));
    EXPECT_EQ(child->asText()->getData(), "Again");
    EXPECT_EQ(root->textContent(), "Hello world\nAgain");
}

TEST_F(LayoutResourceCompilerTest, PreservesWhitespaceInTextNodes) {
    const LayoutBuildResult result = factory.buildElementTreeFromString("<p>  before   after  </p>", "whitespace.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    EXPECT_EQ(result.document->documentElement()->textContent(), "  before   after  ");
}

TEST_F(LayoutResourceCompilerTest, IgnoresFormattingWhitespaceWhenValidatingFlowBreaks) {
    const LayoutBuildResult result = factory.buildElementTreeFromString("<p>\n  <br/>\n  after\n</p>", "flow-break-whitespace.html");
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.flow_break.leading");
}

TEST_F(LayoutResourceCompilerTest, ParsesCanonicalTagsAndRejectsUnknownTags) {
    const SourceDocumentParseResult known = SourceDocumentParser().parse("<PaNeL><BuTtOn/><P/></PaNeL>", "tags.html");
    ASSERT_TRUE(known.ok());
    ASSERT_NE(known.document, nullptr);
    ASSERT_EQ(known.document->root->tag, Tag::Panel);
    ASSERT_EQ(known.document->root->content.size(), 2U);
    ASSERT_EQ(known.document->root->content[0].node->tag, Tag::Button);
    ASSERT_EQ(known.document->root->content[1].node->tag, Tag::Paragraph);

    const SourceDocumentParseResult unknown = SourceDocumentParser().parse("<panel>\n  <madeUp/>\n</panel>", "unknown.html");
    ASSERT_FALSE(unknown.ok());
    ASSERT_FALSE(unknown.errors.empty());
    EXPECT_EQ(unknown.errors.front().code, "layout.element.unknown");
    EXPECT_EQ(unknown.errors.front().source, "unknown.html");
    EXPECT_EQ(unknown.errors.front().line, 2U);
    EXPECT_EQ(unknown.errors.front().column, 3U);
}

TEST_F(LayoutResourceCompilerTest, ComposesButtonInlineChildren) {
    constexpr char kButtonLayout[] = "<button>first"
                                     "<icon src=\"one\"/></button>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kButtonLayout);
    ASSERT_TRUE(result.ok());
    ButtonElement* button = result.rootAs<ButtonElement>();
    ASSERT_NE(button, nullptr);
    const auto buttonNodes = nodes(*button);
    ASSERT_EQ(buttonNodes.size(), 2U);
    ASSERT_NE(buttonNodes.begin()->asText(), nullptr);
    EXPECT_EQ(buttonNodes.begin()->asText()->getData(), "first");
    auto iconNode = buttonNodes.begin();
    ++iconNode;
    ASSERT_NE(iconNode->asElement(), nullptr);
    auto* icon = dynamic_cast<radia::ui::IconElement*>(iconNode->asElement());
    ASSERT_NE(icon, nullptr);
    EXPECT_EQ(icon->name(), "one");
    ASSERT_EQ(button->children().size(), 1U);
    EXPECT_EQ(button->children().front(), icon);

    constexpr char kIconFirstLayout[] = "<button><icon src=\"search\"/>"
                                        "second</button>";
    LayoutBuildResult iconFirst = factory.buildElementTreeFromString(kIconFirstLayout);
    ASSERT_TRUE(iconFirst.ok());
    ButtonElement* reversed = iconFirst.rootAs<ButtonElement>();
    ASSERT_NE(reversed, nullptr);
    const auto reversedNodes = nodes(*reversed);
    ASSERT_EQ(reversedNodes.size(), 2U);
    ASSERT_NE(reversedNodes.begin()->asElement(), nullptr);
    EXPECT_EQ(reversedNodes.begin()->asElement()->elementName(), "icon");
    auto reversedText = reversedNodes.begin();
    ++reversedText;
    ASSERT_NE(reversedText->asText(), nullptr);
    EXPECT_EQ(reversedText->asText()->getData(), "second");

    button->append(std::make_unique<radia::ui::IconElement>("updated"));
    radia::ui::detail::appendText(*button, "updated");
    EXPECT_EQ(nodes(*button).size(), 4U);
    button->replaceChildren();
    EXPECT_TRUE(nodes(*button).empty());
    auto rebuilt = std::make_unique<radia::ui::IconElement>("rebuilt");
    radia::ui::IconElement* rebuiltIcon = rebuilt.get();
    button->append(std::move(rebuilt));
    ASSERT_EQ(button->children().size(), 1U);
    EXPECT_EQ(button->children().front(), rebuiltIcon);
    EXPECT_EQ(rebuiltIcon->name(), "rebuilt");
}

TEST_F(LayoutResourceCompilerTest, MatchesPaintProtocolWithShaderConstants) {
    const std::filesystem::path uiSourceRoot = std::filesystem::path(__FILE__).parent_path().parent_path();
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
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Composite, 8)")
                && contains(paintProtocolSource, "PAINT_OP_ENTRY(Arrow, 9)"));
    EXPECT_TRUE(contains(fragmentSource, "uniform int paintOp;")
                && contains(fragmentSource, "uniform vec4 shapeRect;")
                && contains(fragmentSource, "uniform vec4 shapeRadiusX;")
                && contains(fragmentSource, "uniform vec4 shapeRadiusY;")
                && contains(fragmentSource, "uniform vec4 innerRadiusX;")
                && contains(fragmentSource, "uniform vec4 innerRadiusY;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRect;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRadiusX;")
                && contains(fragmentSource, "uniform vec4 scrollbarClipRadiusY;")
                && contains(fragmentSource, "uniform int scrollbarClipEnabled;")
                && contains(fragmentSource, "uniform vec4 clipCoverageRect;")
                && contains(fragmentSource, "uniform int clipCoverageEnabled;")
                && contains(fragmentSource, "uniform vec2 effectTextureSize;")
                && contains(fragmentSource, "uniform int gradientKind;")
                && contains(fragmentSource, "uniform int outlineStyle;")
                && !contains(fragmentSource, "rduiPaintOp"));
    EXPECT_TRUE(contains(fragmentSource, "kPaintOpBorder = 2")
                && contains(fragmentSource, "paintOp == kPaintOpBorder")
                && contains(fragmentSource, "fwidth"));
    EXPECT_TRUE(contains(fragmentSource, "roundedTriangleCornerDistance")
                && contains(fragmentSource, "roundedTriangleCoverage(shapeCoord, a, b, c, arrowRadius)"));
    EXPECT_TRUE(contains(fragmentSource, "scrollbarClipEnabled != 0") && contains(fragmentSource, "coverageFromDistance(clipDistance)"));
    EXPECT_TRUE(contains(fragmentSource, "applyClipCoverage")
                && contains(fragmentSource, "gl_FragCoord.xy")
                && contains(fragmentSource, "clipCoverageEnabled == 0"));
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

TEST_F(LayoutResourceCompilerTest, RefreshesLocalizedElementsAcrossLocaleChanges) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalizationYaml[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {title: Title, status: Ready, press: Press}}, "
                                         "pt: {strings: {title: Título, status: Pronto, press: Pressione}}}\n";
    constexpr char kStyles[] = "label { color: #ffffffff; }";
    constexpr char kLocalizedLayout[] = "<floater><head><title>{{title}}</title></head><body>"
                                        "<label id=\"status\" for=\"target\">{{status}}</label>"
                                        "<input type=\"checkbox\" switch=\"true\" id=\"target\"/>"
                                        "<button id=\"press\">{{press}}</button></body></floater>";
    resources.add("localization.yaml", kLocalizationYaml);
    resources.add("skin.css", kStyles);
    resources.add("localized.html", kLocalizedLayout);

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ASSERT_TRUE(prepared.ok());
    system.publish(prepared.generation);

    LayoutBuildResult result = system.buildElementTree("localized.html");
    ASSERT_TRUE(result.ok());
    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    const ElementRef<LabelElement> status = requireElement<LabelElement>(*floater, "status");
    const ElementRef<ButtonElement> press = requireElement<ButtonElement>(*floater, "press");
    ASSERT_TRUE(status);
    ASSERT_TRUE(press);
    auto pressNodes = nodes(*press);
    ASSERT_EQ(pressNodes.size(), 1U);
    ASSERT_NE(pressNodes.begin()->asText(), nullptr);
    EXPECT_EQ(floater->title(), "Title");
    EXPECT_EQ(status->textContent(), "Ready");
    EXPECT_EQ(pressNodes.begin()->asText()->getData(), "Press");

    std::unique_ptr<Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    ASSERT_TRUE(result.document);
    surface->mountFloater(*result.document);
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(floater->title(), "Título");
    EXPECT_EQ(status->textContent(), "Pronto");
    pressNodes = nodes(*press);
    ASSERT_EQ(pressNodes.size(), 1U);
    ASSERT_NE(pressNodes.begin()->asText(), nullptr);
    EXPECT_EQ(pressNodes.begin()->asText()->getData(), "Pressione");

    auto programmatic = std::make_unique<FloaterElement>();
    FloaterElement* programmaticFloater = programmatic.get();
    radia::ui::test::appendFloaterStructure(*programmatic);
    programmatic->head()->children().front()->content(system.t("title"));
    surface->mountFloater(std::move(programmatic));
    EXPECT_EQ(programmaticFloater->title(), "Título");

    auto localizedBody = radia::ui::test::makeFloater();
    FloaterElement* localizedBodyFloater = localizedBody.get();
    surface->mountFloater(std::move(localizedBody));
    auto bodyChild = std::make_unique<Element>("p");
    bodyChild->setId("body-child");
    localizedBodyFloater->body()->append(std::move(bodyChild));
    localizedBodyFloater->body()->children().front()->content(system.t("status"));
    EXPECT_EQ(localizedBodyFloater->body()->textContent(), "Pronto");
    ASSERT_EQ(localizedBodyFloater->body()->children().size(), 1U);

    status->content(system.t("status"));
    ASSERT_TRUE(system.setLocale("en"));
    EXPECT_EQ(programmaticFloater->title(), "Title");
    EXPECT_EQ(localizedBodyFloater->body()->textContent(), "Ready");
    EXPECT_EQ(status->textContent(), "Ready");
    ASSERT_EQ(localizedBodyFloater->body()->children().size(), 1U);
    EXPECT_EQ(localizedBodyFloater->body()->children().front()->id(), "body-child");
    status->textContent("Literal");
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(status->textContent(), "Literal");

    constexpr char kMissingLayout[] = "<p>{{missing}}</p>";
    resources.add("missing.html", kMissingLayout);
    const SkinGenerationPrepareResult missing = SkinCompiler().prepare(std::move(resources));
    ASSERT_FALSE(missing.ok());
    EXPECT_FALSE(missing.generation);
    ASSERT_FALSE(missing.errors.empty());
    EXPECT_EQ(missing.errors.front().code, "layout.localization.missing");
}

TEST_F(LayoutResourceCompilerTest, RefreshesLocalizedRichTextAcrossLocaleChanges) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalizationYaml[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {message: 'Hello <b>bold</b><br/>Again'}}, "
                                         "pt: {strings: {message: 'Olá <i>itálico</i><br/>Novamente'}}}\n";
    resources.add("localization.yaml", kLocalizationYaml);
    resources.add("skin.css", "p { display: block; } b { font-weight: bold; } i { font-style: italic; }");
    resources.add("rich.html", "<p>{{message}}</p>");

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ASSERT_TRUE(prepared.ok());
    system.publish(prepared.generation);

    LayoutBuildResult result = system.buildElementTree("rich.html");
    ASSERT_TRUE(result.ok());
    Element* root = result.document->documentElement();
    ASSERT_NE(root, nullptr);
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->mount(*result.document);

    auto assertRuntime = [root](const char* first, const char* inlineElement, const char* inlineText, const char* last) {
        const auto runtimeChildren = nodes(*root);
        ASSERT_EQ(runtimeChildren.size(), 4U);
        auto child = runtimeChildren.begin();
        ASSERT_NE(child->asText(), nullptr);
        EXPECT_EQ(child->asText()->getData(), first);
        ++child;
        ASSERT_NE(child->asElement(), nullptr);
        EXPECT_EQ(child->asElement()->elementName(), inlineElement);
        EXPECT_EQ(child->asElement()->textContent(), inlineText);
        ++child;
        ASSERT_NE(child->asElement(), nullptr);
        EXPECT_EQ(child->asElement()->elementName(), "br");
        ++child;
        ASSERT_NE(child->asText(), nullptr);
        EXPECT_TRUE(radia::ui::detail::NodeAccess::flowBreakBefore(*child));
        EXPECT_EQ(child->asText()->getData(), last);
    };

    EXPECT_EQ(root->textContent(), "Hello bold\nAgain");
    assertRuntime("Hello ", "b", "bold", "Again");
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(root->textContent(), "Olá itálico\nNovamente");
    assertRuntime("Olá ", "i", "itálico", "Novamente");
}

TEST_F(LayoutResourceCompilerTest, RejectsUnknownElementsAndAttributes) {
    constexpr char kUnknownElementLayout[] = "<panel>"
                                             "<unknown/></panel>";
    constexpr char kUnsupportedAttributeLayout[] = "<panel width=\"10\"/>";
    constexpr char kUnknownAttributeLayout[] = "<floater invented=\"true\"/>";
    const LayoutBuildResult unknownElement = factory.buildElementTreeFromString(kUnknownElementLayout, "unknown.html");
    ASSERT_FALSE(unknownElement.ok());
    EXPECT_FALSE(unknownElement.document);
    ASSERT_FALSE(unknownElement.errors.empty());
    EXPECT_EQ(unknownElement.errors.front().code, "layout.element.unknown");
    EXPECT_EQ(unknownElement.errors.front().source, "unknown.html");

    const LayoutBuildResult unsupportedAttribute = factory.buildElementTreeFromString(kUnsupportedAttributeLayout, "attribute.html");
    ASSERT_FALSE(unsupportedAttribute.ok());
    EXPECT_FALSE(unsupportedAttribute.document);
    ASSERT_FALSE(unsupportedAttribute.errors.empty());
    EXPECT_EQ(unsupportedAttribute.errors.front().code, "layout.attribute.unsupported");

    const LayoutBuildResult unknownAttribute = factory.buildElementTreeFromString(kUnknownAttributeLayout, "unknown_attribute.html");
    ASSERT_FALSE(unknownAttribute.ok());
    EXPECT_FALSE(unknownAttribute.document);
    ASSERT_FALSE(unknownAttribute.errors.empty());
    EXPECT_EQ(unknownAttribute.errors.front().code, "layout.attribute.unknown");
}

TEST_F(LayoutResourceCompilerTest, AcceptsEventsOnEveryElementAndWarnsForExpressions) {
    constexpr char kUniversalEventLayout[] = "<p onClick=\"click()\">copy</p>";
    constexpr char kExpressionCallLayout[] = "<button onClick=\"save(force=true)\"/>";
    const LayoutBuildResult universalEvent = factory.buildElementTreeFromString(kUniversalEventLayout, "event.html");
    ASSERT_TRUE(universalEvent.ok());
    ASSERT_TRUE(universalEvent.document);
    EXPECT_TRUE(universalEvent.warnings.empty());
    ASSERT_NE(universalEvent.document->documentElement()->eventCall(kClickEvent), nullptr);
    EXPECT_EQ(universalEvent.document->documentElement()->eventCall(kClickEvent)->name(), "click");

    const LayoutBuildResult expressionCall = factory.buildElementTreeFromString(kExpressionCallLayout, "expression.html");
    ASSERT_TRUE(expressionCall.ok());
    ASSERT_TRUE(expressionCall.document);
    ASSERT_EQ(expressionCall.warnings.size(), 1U);
    EXPECT_EQ(expressionCall.warnings.front().code, "layout.event.literal_unsupported");
}

TEST_F(LayoutResourceCompilerTest, RejectsDuplicateElementIds) {
    constexpr char kDuplicateIdLayout[] = "<panel><p id=\"same\"/>"
                                          "<button id=\"same\">Same</button></panel>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kDuplicateIdLayout, "duplicates.html");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.id.duplicate");
    EXPECT_EQ(result.errors.front().source, "duplicates.html");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidBooleanAttributes) {
    constexpr char kInvalidBooleanLayout[] = "<floater resizeable=\"sometimes\"><head/><body>"
                                             "<input type=\"checkbox\" switch=\"true\" checked=\"yes\"/></body></floater>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kInvalidBooleanLayout, "booleans.html");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_EQ(result.errors.size(), 2U);
    EXPECT_EQ(result.errors.front().code, "layout.attribute.boolean_invalid");
    EXPECT_EQ(result.errors.front().source, "booleans.html");
}

TEST_F(LayoutResourceCompilerTest, AcceptsBooleanAttributesWithoutValue) {
    const LayoutBuildResult result = factory.buildElementTreeFromString("<button disabled>Save</button>", "boolean.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    const ButtonElement* button = result.rootAs<ButtonElement>();
    ASSERT_NE(button, nullptr);
    EXPECT_TRUE(button->disabled());
    EXPECT_EQ(button->textContent(), "Save");
}

TEST_F(LayoutResourceCompilerTest, ParsesRadiaHtmlSyntaxDirectly) {
    constexpr char kHtml[] = "<panel data-kind=demo><!-- ignored --><input type=checkbox switch checked><br>After</panel>";
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse(kHtml, "direct.html");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 3U);

    const radia::ui::SourceNode* input = parsed.document->root->content[0].node.get();
    ASSERT_NE(input, nullptr);
    EXPECT_EQ(input->attributes.at("type").value, "checkbox");
    EXPECT_TRUE(input->attributes.contains("switch"));
    EXPECT_TRUE(input->attributes.contains("checked"));
    EXPECT_EQ(parsed.document->root->content[1].node->tag, Tag::Br);
    EXPECT_EQ(parsed.document->root->content[2].text, "After");
    EXPECT_EQ(parsed.document->root->attributes.at("data-kind").value, "demo");
}

TEST_F(LayoutResourceCompilerTest, DecodesHtmlEntitiesWithoutRewritingMarkup) {
    constexpr char kHtml[] = "<p title=\"a&amp;b\">&lt; &quot; &apos;</p>";
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse(kHtml, "entities.html");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 1U);
    EXPECT_EQ(parsed.document->root->attributes.at("title").value, "a&b");
    EXPECT_EQ(parsed.document->root->content.front().text, "< \" '");
}

TEST_F(LayoutResourceCompilerTest, RejectsMismatchedHtmlTags) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<panel><p></panel>", "mismatched.html");
    ASSERT_FALSE(parsed.ok());
    ASSERT_FALSE(parsed.errors.empty());
    EXPECT_EQ(parsed.errors.front().code, "layout.html.invalid");
}

TEST_F(LayoutResourceCompilerTest, ManagesAuthoredFloaterHeadLifecycle) {
    constexpr char kFloaterLayout[] = "<floater><head><title><icon src=\"search\"/>tools</title>"
                                      "<minimize><icon src=\"minimize\"/></minimize><close><icon src=\"close\"/></close></head>"
                                      "<body><panel id=\"content\"><button id=\"refresh\">Refresh</button></panel></body></floater>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "floater_head.html");
    ASSERT_TRUE(result.ok());
    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);

    ElementRef<PanelElement> content = requireElement<PanelElement>(*floater, "content");
    ElementRef<ButtonElement> refresh = requireElement<ButtonElement>(*floater, "refresh");
    ASSERT_TRUE(content);
    ASSERT_TRUE(refresh);
    ASSERT_NE(floater->head(), nullptr);
    ASSERT_NE(floater->body(), nullptr);
    EXPECT_EQ(content->parentElement(), floater->body());
    EXPECT_EQ(refresh->parentElement(), content.get());
    EXPECT_EQ(floater->head()->children()[1], floater->minimizeButton());
    EXPECT_EQ(floater->head()->children()[2], floater->closeButton());
    EXPECT_EQ(floater->minimizeButton()->children()[0]->part(), "");
    EXPECT_EQ(floater->closeButton()->children()[0]->part(), "");
    EXPECT_TRUE(floater->closable());
    EXPECT_TRUE(floater->minimizable());

    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia("floater:minimized > body { display: none; }").ok());
    floater->setMinimized(true);
    EXPECT_EQ(floater->head()->visibility(), Visibility::Visible);
    EXPECT_EQ(floater->body()->visibility(), Visibility::Visible);
    EXPECT_FALSE(floater->body()->isDisplayed(resolveElementStyle(styleSheet, *floater->body())));

    floater->setMinimized(false);
    EXPECT_TRUE(floater->body()->isDisplayed(resolveElementStyle(styleSheet, *floater->body())));

    floater->replaceChildren();
    EXPECT_EQ(floater->head(), nullptr);
    EXPECT_EQ(floater->body(), nullptr);
    EXPECT_FALSE(content);
    EXPECT_FALSE(refresh);
}

TEST_F(LayoutResourceCompilerTest, FindsFloaterControlsThroughHeadWrappers) {
    constexpr char kFloaterLayout[] = "<floater><head><title>tools</title><div><minimize/><close/></div></head><body/></floater>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "nested_floater_controls.html");
    ASSERT_TRUE(result.ok());
    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    EXPECT_EQ(floater->minimizeButton()->parentElement()->elementName(), "div");
    EXPECT_EQ(floater->closeButton()->parentElement()->elementName(), "div");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidFloaterStructure) {
    constexpr char kMissingTitleLayout[] = "<floater><head><minimize/></head><body/></floater>";
    constexpr char kDuplicateHeadLayout[] = "<floater><head/><head/><body/></floater>";
    constexpr char kDuplicateTitleLayout[] = "<floater><head><title>one</title><div><title>two</title></div></head><body/></floater>";
    constexpr char kDuplicateMinimizeLayout[] = "<floater><head><title>one</title><minimize/><div><minimize/></div></head><body/></floater>";
    constexpr char kDuplicateCloseLayout[] = "<floater><head><title>one</title><close/><div><close/></div></head><body/></floater>";
    constexpr char kNestedHeadLayout[] = "<floater><body><head/></body></floater>";
    constexpr char kNestedBodyLayout[] = "<floater><head><body/></head></floater>";
    constexpr char kBodyTitleLayout[] = "<floater><head><title>one</title></head><body><div><title>two</title></div></body></floater>";
    constexpr char kBodyMinimizeLayout[] = "<floater><head><title>one</title></head><body><div><minimize/></div></body></floater>";
    constexpr char kBodyCloseLayout[] = "<floater><head><title>one</title></head><body><div><close/></div></body></floater>";
    const LayoutBuildResult missingTitle = factory.buildElementTreeFromString(kMissingTitleLayout, "missing_title.html");
    ASSERT_FALSE(missingTitle.ok());
    ASSERT_FALSE(missingTitle.errors.empty());
    EXPECT_EQ(missingTitle.errors.front().code, "layout.floater.title_required");

    const LayoutBuildResult duplicateHead = factory.buildElementTreeFromString(kDuplicateHeadLayout, "duplicate_head.html");
    ASSERT_FALSE(duplicateHead.ok());
    ASSERT_FALSE(duplicateHead.errors.empty());
    EXPECT_EQ(duplicateHead.errors.front().code, "layout.floater.head_duplicate");

    const LayoutBuildResult duplicateTitle = factory.buildElementTreeFromString(kDuplicateTitleLayout, "duplicate_title.html");
    ASSERT_FALSE(duplicateTitle.ok());
    ASSERT_FALSE(duplicateTitle.errors.empty());
    EXPECT_EQ(duplicateTitle.errors.front().code, "layout.floater.title_duplicate");

    const LayoutBuildResult duplicateMinimize = factory.buildElementTreeFromString(kDuplicateMinimizeLayout, "duplicate_minimize.html");
    ASSERT_FALSE(duplicateMinimize.ok());
    ASSERT_FALSE(duplicateMinimize.errors.empty());
    EXPECT_EQ(duplicateMinimize.errors.front().code, "layout.floater.minimize_duplicate");

    const LayoutBuildResult duplicateClose = factory.buildElementTreeFromString(kDuplicateCloseLayout, "duplicate_close.html");
    ASSERT_FALSE(duplicateClose.ok());
    ASSERT_FALSE(duplicateClose.errors.empty());
    EXPECT_EQ(duplicateClose.errors.front().code, "layout.floater.close_duplicate");

    const LayoutBuildResult nestedHead = factory.buildElementTreeFromString(kNestedHeadLayout, "nested_head.html");
    ASSERT_FALSE(nestedHead.ok());
    ASSERT_FALSE(nestedHead.errors.empty());
    EXPECT_EQ(nestedHead.errors.front().code, "layout.floater.head_required");

    const LayoutBuildResult nestedBody = factory.buildElementTreeFromString(kNestedBodyLayout, "nested_body.html");
    ASSERT_FALSE(nestedBody.ok());
    ASSERT_FALSE(nestedBody.errors.empty());
    EXPECT_EQ(nestedBody.errors.front().code, "layout.floater.body_required");

    const LayoutBuildResult bodyTitle = factory.buildElementTreeFromString(kBodyTitleLayout, "body_title.html");
    ASSERT_FALSE(bodyTitle.ok());
    ASSERT_FALSE(bodyTitle.errors.empty());
    EXPECT_EQ(bodyTitle.errors.front().code, "layout.floater.head_only");

    const LayoutBuildResult bodyMinimize = factory.buildElementTreeFromString(kBodyMinimizeLayout, "body_minimize.html");
    ASSERT_FALSE(bodyMinimize.ok());
    ASSERT_FALSE(bodyMinimize.errors.empty());
    EXPECT_EQ(bodyMinimize.errors.front().code, "layout.floater.head_only");

    const LayoutBuildResult bodyClose = factory.buildElementTreeFromString(kBodyCloseLayout, "body_close.html");
    ASSERT_FALSE(bodyClose.ok());
    ASSERT_FALSE(bodyClose.errors.empty());
    EXPECT_EQ(bodyClose.errors.front().code, "layout.floater.head_only");
}

TEST_F(LayoutResourceCompilerTest, AppliesChildBearingElementDefaults) {
    resources["elements/minimize.html"] = "<minimize><icon src=\"minimize\"/></minimize>";
    resources["elements/close.html"] = "<close><icon src=\"close\"/></close>";
    constexpr char kDefaultedFloaterLayout[] = "<floater><head><title>defaulted</title><minimize/><close/></head><body/></floater>";

    EXPECT_FALSE(factory.validateElementDefaults("MINIMIZE").hasErrors());
    EXPECT_FALSE(factory.validateElementDefaults("close").hasErrors());
    LayoutBuildResult result = factory.buildElementTreeFromString(kDefaultedFloaterLayout, "defaulted.html");
    ASSERT_TRUE(result.ok());
    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_EQ(floater->closeButton()->children().size(), 1U);
    ASSERT_EQ(floater->minimizeButton()->children().size(), 1U);
    const auto* closeIcon = dynamic_cast<radia::ui::IconElement*>(floater->closeButton()->children().front());
    const auto* minimizeIcon = dynamic_cast<radia::ui::IconElement*>(floater->minimizeButton()->children().front());
    ASSERT_NE(closeIcon, nullptr);
    ASSERT_NE(minimizeIcon, nullptr);
    EXPECT_EQ(closeIcon->name(), "close");
    EXPECT_EQ(minimizeIcon->name(), "minimize");
    EXPECT_TRUE(floater->closable());
    EXPECT_TRUE(floater->minimizable());

    constexpr char kOverrideLayout[] = "<floater><head><title>override</title><minimize><icon src=\"custom\"/></minimize><close/></head>"
                                       "<body/></floater>";
    result = factory.buildElementTreeFromString(kOverrideLayout, "override.html");
    ASSERT_TRUE(result.ok());
    floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_EQ(floater->minimizeButton()->children().size(), 1U);
    const auto* customIcon = dynamic_cast<radia::ui::IconElement*>(floater->minimizeButton()->children().front());
    ASSERT_NE(customIcon, nullptr);
    EXPECT_EQ(customIcon->name(), "custom");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidElementDefaults) {
    constexpr char kInvalidFloaterDefaultsLayout[] = "<panel/>";
    constexpr char kDefaultedLayout[] = "<floater title=\"defaulted\"/>";
    resources["elements/floater.html"] = kInvalidFloaterDefaultsLayout;
    resources["defaulted.html"] = kDefaultedLayout;

    const DiagnosticResult defaults = factory.validateElementDefaults("floater");
    ASSERT_TRUE(defaults.hasErrors());
    ASSERT_FALSE(defaults.errors.empty());
    EXPECT_EQ(defaults.errors.front().code, "layout.defaults.root_invalid");
    const LayoutBuildResult result = factory.buildElementTreeFromResource("defaulted.html");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.defaults.root_invalid");
}

TEST_F(LayoutResourceCompilerTest, CompilesTypedVisibilityValues) {
    constexpr char kVisibilityLayout[] = "<panel><p id=\"shown\" visibility=\"visible\"/>"
                                         "<p id=\"hidden\" visibility=\"hidden\"/>"
                                         "<p id=\"collapsed\" visibility=\"collapse\"/></panel>";
    const LayoutBuildResult result = factory.buildElementTreeFromString(kVisibilityLayout, "visibility.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    ASSERT_EQ(result.document->documentElement()->children().size(), 3U);
    EXPECT_EQ(result.document->documentElement()->children()[0]->visibility(), Visibility::Visible);
    EXPECT_EQ(result.document->documentElement()->children()[1]->visibility(), Visibility::Hidden);
    EXPECT_EQ(result.document->documentElement()->children()[2]->visibility(), Visibility::Collapse);
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidVisibilitySyntax) {
    struct InvalidVisibilityCase {
        const char* name;
        const char* markup;
        const char* diagnostic;
    };
    const InvalidVisibilityCase cases[] = {
        {"invalid enum value", "<p visibility=\"invisible\"/>", "layout.attribute.visibility_invalid"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "visibility markup: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.markup, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(LayoutResourceCompilerTest, ValidatesElementDefaultDiagnostics) {
    constexpr char kInvalidVisibilityDefaultsLayout[] = "<label visibility=\"sometimes\"/>";
    constexpr char kLabelDefaultsLayout[] = "<label/>";
    constexpr char kInvalidSwitchDefaultsLayout[] = "<input type=\"checkbox\" switch=\"true\" checked=\"sometimes\"/>";
    resources["elements/label.html"] = kInvalidVisibilityDefaultsLayout;
    const DiagnosticResult visibility = factory.validateElementDefaults("label");
    ASSERT_TRUE(visibility.hasErrors());
    EXPECT_EQ(visibility.errors.front().code, "layout.attribute.visibility_invalid");

    resources["elements/label.html"] = kLabelDefaultsLayout;
    resources["elements/input.html"] = kInvalidSwitchDefaultsLayout;
    const DiagnosticResult elementAttribute = factory.validateElementDefaults("input");
    ASSERT_TRUE(elementAttribute.hasErrors());
    EXPECT_EQ(elementAttribute.errors.front().code, "layout.attribute.boolean_invalid");
}

TEST_F(LayoutResourceCompilerTest, AcceptsCaseInsensitiveMarkupNames) {
    constexpr char kCaseInsensitiveLayout[] = "<FlOaTeR ReSiZeAbLe><HeAd><TiTlE>tools</TiTlE><MiNiMiZe/></HeAd><BoDy>"
                                              "<BuTtOn ID=\"saveFile\" ONCLICK=\"saveFile()\">"
                                              "<IcOn SrC=\"search\"/>Save</BuTtOn></BoDy></FlOaTeR>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kCaseInsensitiveLayout, "case-insensitive.html");
    ASSERT_TRUE(result.ok());
    FloaterElement* floater = result.rootAs<FloaterElement>();
    ASSERT_NE(floater, nullptr);
    const ElementRef<ButtonElement> button = requireElement<ButtonElement>(*floater, "saveFile");
    ASSERT_TRUE(button);
    ASSERT_EQ(button->children().size(), 1U);
    ASSERT_NE(dynamic_cast<radia::ui::IconElement*>(button->children().front()), nullptr);
    EXPECT_EQ(button->elementName(), "button");
    ASSERT_NE(button->eventCall(kClickEvent), nullptr);
    EXPECT_EQ(button->eventCall(kClickEvent)->name(), "saveFile");
}

TEST_F(LayoutResourceCompilerTest, RejectsInvalidNamesAndCaseFoldedConflicts) {
    struct InvalidMarkupCase {
        const char* name;
        const char* markup;
        const char* diagnostic;
    };
    const InvalidMarkupCase cases[] = {
        {"snake-case event attribute", "<BuTtOn\n  on_click=\"saveFile()\"/>", "layout.attribute.unknown"},
        {"case-folded duplicate attribute", "<button onClick=\"saveOne()\" ONCLICK=\"saveTwo()\"/>", "layout.attribute.duplicate"},
        {"invalid element id", "<panel id=\"bad.id\"/>", "layout.id.invalid"},
        {"invalid element class", "<panel class=\"bad.class\"/>", "layout.class.invalid"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid markup: " << test.name);
        const LayoutBuildResult result = factory.buildElementTreeFromString(test.markup, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    const LayoutBuildResult underscore =
        factory.buildElementTreeFromString("<panel id=\"bad_id\" class=\"good_class\"/>", "underscore-identifiers.html");
    ASSERT_TRUE(underscore.ok());
    ASSERT_TRUE(underscore.document);
    EXPECT_EQ(underscore.document->documentElement()->id(), "bad_id");
    EXPECT_TRUE(underscore.document->documentElement()->classes().contains("good_class"));

    constexpr char kInvalidHandlerLayout[] = "<button onClick=\"bad_action()\"/>";
    const LayoutBuildResult invalidHandler = factory.buildElementTreeFromString(kInvalidHandlerLayout, "handler-name.html");
    ASSERT_TRUE(invalidHandler.ok());
    ASSERT_EQ(invalidHandler.warnings.size(), 1U);
    EXPECT_EQ(invalidHandler.warnings.front().code, "layout.event.name_invalid");
}

TEST_F(LayoutResourceCompilerTest, PreservesValidEventCallsAndWarnsForInvalidCalls) {
    constexpr char kEventCallsLayout[] = "<panel><button id=\"inspect\" "
                                         "onClick=\"inspect(4, 'settings', true, this, event)\"/>"
                                         "<button id=\"bare\" onClick=\"press\"/>"
                                         "<button id=\"lifecycle\" onClick=\"postBuild()\"/></panel>";
    LayoutBuildResult result = factory.buildElementTreeFromString(kEventCallsLayout, "event-calls.html");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), 2U);
    EXPECT_EQ(result.warnings[0].code, "layout.event.call_required");
    EXPECT_EQ(result.warnings[1].code, "layout.event.handler_reserved");

    const ElementRef<ButtonElement> inspect = requireElement<ButtonElement>(*result.document->documentElement(), "inspect");
    const ElementRef<ButtonElement> bare = requireElement<ButtonElement>(*result.document->documentElement(), "bare");
    const ElementRef<ButtonElement> lifecycle = requireElement<ButtonElement>(*result.document->documentElement(), "lifecycle");
    ASSERT_TRUE(inspect);
    ASSERT_TRUE(bare);
    ASSERT_TRUE(lifecycle);
    ASSERT_NE(inspect->eventCall(kClickEvent), nullptr);
    EXPECT_EQ(inspect->eventCall(kClickEvent)->name(), "inspect");
    EXPECT_EQ(inspect->eventCall(kClickEvent)->arguments().size(), 5U);
    EXPECT_EQ(bare->eventCall(kClickEvent), nullptr);
    EXPECT_EQ(lifecycle->eventCall(kClickEvent), nullptr);
}
