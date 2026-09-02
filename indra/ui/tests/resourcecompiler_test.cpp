/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <type_traits>
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "event.h"
#include "eventcall.h"
#include "floater_test_helpers.h"
#include "html/button.h"
#include "html/floater.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/document.h"
#include "layout/engine.h"
#include "layout/resourcecompiler.h"
#include "layout_test_helpers.h"
#include "resourceprovider.h"
#include "skin/compiler.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace {
using radia::ui::DiagnosticResult;
using radia::ui::Element;
using radia::ui::ElementRef;
using radia::ui::authoredEventCall;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLFloaterElement;
using radia::ui::HTMLIconElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::HTMLTag;
using radia::ui::kChangeEvent;
using radia::ui::kClickEvent;
using radia::ui::kContextMenuEvent;
using radia::ui::kDoubleClickEvent;
using radia::ui::kInputEvent;
using radia::ui::kPointerDownEvent;
using radia::ui::kWheelEvent;
using radia::ui::resolveElementStyle;
using radia::ui::ResourceBuildResult;
using radia::ui::ResourceCompiler;
using radia::ui::ResourceId;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::SourceDocumentParser;
using radia::ui::SourceDocumentParseResult;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::Visibility;
using radia::ui::detail::appendText;
using radia::ui::detail::findElementInScope;
using radia::ui::detail::makeElement;
using radia::ui::detail::NodeAccess;
using radia::ui::detail::nodes;
using radia::ui::test::ResourceCompilerTestHelper;
using ::testing::Message;
using ::testing::Test;

static_assert(std::is_same_v<decltype(SourceDocumentParseResult::document), std::shared_ptr<const radia::ui::SourceDocument>>);

class ResourceCompilerTest : public Test {
protected:
    template<typename ElementT> ElementRef<ElementT> requireElement(Element& root, const std::string& id) const {
        return ElementRef<ElementT>(dynamic_cast<ElementT*>(findElementInScope(root, id)));
    }

    ResourceCompilerTestHelper factory;
    std::map<std::string, std::string>& resources = factory.resources;
};
} // namespace

TEST(ResourceIdTest, CanonicalizesLogicalPathsWithoutSkinPrefixes) {
    const ResourceId path("./views\\panel/../foo.html");
    EXPECT_TRUE(path.valid());
    EXPECT_EQ(path.value(), "views/foo.html");

    const ResourceId legacyPrefix("xui/foo.html");
    EXPECT_TRUE(legacyPrefix.valid());
    EXPECT_EQ(legacyPrefix.value(), "xui/foo.html");

    EXPECT_EQ(ResourceId::resolve(ResourceId("nested/outer.html"), "../shared.html").value(), "shared.html");
    EXPECT_EQ(ResourceId::resolve(ResourceId("nested/outer.html"), "/shared.html").value(), "shared.html");
    EXPECT_FALSE(ResourceId::resolve(ResourceId("nested/outer.html"), "").valid());
    EXPECT_FALSE(ResourceId::resolve(ResourceId("outer.html"), "../outside.html").valid());
}

TEST(ResourceIdTest, KeepsLogicalLookupSeparateFromPhysicalProvenance) {
    ResourceSnapshot snapshot;
    snapshot.add(ResourceId("foo.html"), "<panel></panel>", "skin.views/foo.html");

    const std::optional<radia::ui::ResourceSource> loaded = snapshot.load(ResourceId("./foo.html"));
    ASSERT_TRUE(loaded.has_value());
    EXPECT_EQ(loaded->content, "<panel></panel>");
    EXPECT_EQ(loaded->provenance, "skin.views/foo.html");
}

TEST(ResourceSnapshotTest, EqualityIncludesPrefixAliases) {
    ResourceSnapshot baseline;
    ResourceSnapshot candidate;
    ASSERT_TRUE(baseline.add("view.html", "<panel></panel>", "skin/view.html"));
    ASSERT_TRUE(candidate.add("view.html", "<panel></panel>", "skin/view.html"));
    EXPECT_EQ(baseline, candidate);

    ASSERT_TRUE(candidate.addPrefixAlias("skins/base"));
    EXPECT_NE(baseline, candidate);

    ASSERT_TRUE(baseline.addPrefixAlias("skins/base"));
    EXPECT_EQ(baseline, candidate);

    ASSERT_TRUE(candidate.addPrefixAlias("skins/overlay", ResourceId("views")));
    EXPECT_NE(baseline, candidate);
}

TEST_F(ResourceCompilerTest, ConsumesCanonicalLogicalIdsWithoutProviderTranslation) {
    ResourceSnapshot snapshot;
    ASSERT_TRUE(snapshot.add("root.html", "<panel><panel filename=\"views/child.html\"></panel></panel>"));
    ASSERT_TRUE(snapshot.add("child.html", "<panel id=\"child\"></panel>"));
    ASSERT_TRUE(snapshot.addPrefixAlias("views"));

    const ResourceBuildResult physicalRoot = ResourceCompiler(&snapshot).buildElementTreeFromResource(ResourceId("views/root.html"));
    ASSERT_FALSE(physicalRoot.ok());
    ASSERT_FALSE(physicalRoot.errors.empty());
    EXPECT_EQ(physicalRoot.errors.front().code, "layout.resource.missing");
    EXPECT_EQ(physicalRoot.errors.front().source, "views/root.html");

    const ResourceBuildResult physicalReference = ResourceCompiler(&snapshot).buildElementTreeFromResource(ResourceId("root.html"));
    ASSERT_FALSE(physicalReference.ok());
    ASSERT_FALSE(physicalReference.errors.empty());
    EXPECT_EQ(physicalReference.errors.front().code, "layout.resource.missing");
    EXPECT_EQ(physicalReference.errors.front().source, "views/child.html");
}

TEST_F(ResourceCompilerTest, BuildsFloaterWithControlsAndEventCalls) {
    resources["elements/minimize.html"] = "<minimize><icon src=\"minimize\"></icon></minimize>";
    resources["elements/close.html"] = "<close><icon src=\"close\"></icon></close>";
    constexpr char kFloaterLayout[] = "<floater resizeable><head><title>title</title><minimize></minimize><close></close></head><body>"
                                      "<p id=\"status\">Ready</p>"
                                      "<button id=\"go\" onClick=\"demoGo()\" onDoubleClick=\"demoDouble()\" "
                                      "onPointerDown=\"demoPress()\" onContextMenu=\"demoMenu()\" onWheel=\"demoWheel()\">"
                                      "<icon src=\"search\"></icon>Go</button><input type=\"checkbox\" switch=\"true\" name=\"mode\" id=\"toggle\" "
                                      "checked=\"true\" onInput=\"demoInput()\" onChange=\"demoChanged()\"></body></floater>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "floater.html");
    ASSERT_TRUE(result.ok());

    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    EXPECT_EQ(floater->title(), "title");
    ASSERT_NE(floater->head(), nullptr);
    ASSERT_NE(floater->body(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    EXPECT_TRUE(floater->resizeable());

    const ElementRef<HTMLButtonElement> go = requireElement<HTMLButtonElement>(*floater, "go");
    const ElementRef<HTMLInputElement> toggle = requireElement<HTMLInputElement>(*floater, "toggle");
    ASSERT_NE(go.get(), nullptr);
    ASSERT_NE(toggle.get(), nullptr);
    EXPECT_TRUE(toggle->checked());
    EXPECT_EQ(toggle->name(), "mode");

    ASSERT_NE(authoredEventCall(*go, kClickEvent), nullptr);
    ASSERT_NE(authoredEventCall(*go, kDoubleClickEvent), nullptr);
    ASSERT_NE(authoredEventCall(*go, kPointerDownEvent), nullptr);
    ASSERT_NE(authoredEventCall(*go, kContextMenuEvent), nullptr);
    ASSERT_NE(authoredEventCall(*go, kWheelEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*go, kClickEvent)->name(), "demoGo");
    EXPECT_EQ(authoredEventCall(*go, kDoubleClickEvent)->name(), "demoDouble");
    EXPECT_EQ(authoredEventCall(*go, kPointerDownEvent)->name(), "demoPress");
    EXPECT_EQ(authoredEventCall(*go, kContextMenuEvent)->name(), "demoMenu");
    EXPECT_EQ(authoredEventCall(*go, kWheelEvent)->name(), "demoWheel");

    ASSERT_NE(authoredEventCall(*toggle, kChangeEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*toggle, kChangeEvent)->name(), "demoChanged");
    ASSERT_NE(authoredEventCall(*toggle, kInputEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*toggle, kInputEvent)->name(), "demoInput");
}

TEST_F(ResourceCompilerTest, BuildsStructuralDivs) {
    constexpr char kDivLayout[] = "<div id=\"root\" class=\"stack\"><div id=\"group\"><p id=\"child\">content</p></div></div>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kDivLayout, "div.html");
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

TEST_F(ResourceCompilerTest, RefreshesCachedDocumentWhenResourceChanges) {
    ResourceSnapshot resources;
    resources.add("panel.html", "<panel><p id=\"first\"></p></panel>");
    ResourceCompiler compiler(&resources);

    const ResourceBuildResult first = compiler.buildElementTreeFromResource(ResourceId("panel.html"));
    ASSERT_TRUE(first.ok());
    const HTMLPanelElement* firstPanel = first.rootAs<HTMLPanelElement>();
    ASSERT_NE(firstPanel, nullptr);
    ASSERT_EQ(firstPanel->children().size(), std::size_t{1});
    EXPECT_EQ(firstPanel->children().front()->id(), "first");

    resources.add("panel.html", "<panel><p id=\"second\"></p></panel>");
    const ResourceBuildResult second = compiler.buildElementTreeFromResource(ResourceId("panel.html"));
    ASSERT_TRUE(second.ok());
    const HTMLPanelElement* secondPanel = second.rootAs<HTMLPanelElement>();
    ASSERT_NE(secondPanel, nullptr);
    ASSERT_EQ(secondPanel->children().size(), std::size_t{1});
    EXPECT_EQ(secondPanel->children().front()->id(), "second");
}

TEST_F(ResourceCompilerTest, RefreshesCachedDiagnosticsWhenProvenanceChanges) {
    ResourceSnapshot resources;
    resources.add(ResourceId("panel.html"), "<panel><unknown></unknown></panel>", "base/views/panel.html");
    ResourceCompiler compiler(&resources);

    const ResourceBuildResult first = compiler.buildElementTreeFromResource(ResourceId("panel.html"));
    ASSERT_FALSE(first.ok());
    ASSERT_FALSE(first.errors.empty());
    EXPECT_EQ(first.errors.front().source, "base/views/panel.html");

    resources.add(ResourceId("panel.html"), "<panel><unknown></unknown></panel>", "derived/html/panel.html");
    const ResourceBuildResult second = compiler.buildElementTreeFromResource(ResourceId("panel.html"));
    ASSERT_FALSE(second.ok());
    ASSERT_FALSE(second.errors.empty());
    EXPECT_EQ(second.errors.front().source, "derived/html/panel.html");
}

TEST_F(ResourceCompilerTest, InstantiatesIndependentEmbeddedResourcePanels) {
    constexpr char kSharedResourceLayout[] = "<panel id=\"base\" class=\"shared\"><p id=\"resourceChild\">base</p></panel>";
    constexpr char kEmbeddedPanelsLayout[] = "<panel><panel filename=\"shared.html\" id=\"one\" class=\"first\">"
                                             "<p id=\"inlineChild\"></p></panel>"
                                             "<panel filename=\"shared.html\" id=\"two\"></panel></panel>";
    resources["shared.html"] = kSharedResourceLayout;
    ResourceBuildResult result = factory.buildElementTreeFromString(kEmbeddedPanelsLayout, "outer.html");
    ASSERT_TRUE(result.ok());

    const ElementRef<HTMLPanelElement> first = requireElement<HTMLPanelElement>(*result.document->documentElement(), "one");
    const ElementRef<HTMLPanelElement> second = requireElement<HTMLPanelElement>(*result.document->documentElement(), "two");
    ASSERT_NE(first.get(), nullptr);
    ASSERT_NE(second.get(), nullptr);
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

TEST_F(ResourceCompilerTest, ResolvesNestedResourceReferences) {
    constexpr char kInnerResourceLayout[] = "<panel id=\"inner\"></panel>";
    constexpr char kMiddleResourceLayout[] = "<panel><panel filename=\"inner.html\"></panel></panel>";
    constexpr char kOuterResourceLayout[] = "<panel><panel filename=\"nested/middle.html\"></panel></panel>";
    resources["nested/inner.html"] = kInnerResourceLayout;
    resources["nested/middle.html"] = kMiddleResourceLayout;
    resources["outer.html"] = kOuterResourceLayout;

    const ResourceBuildResult result = factory.buildElementTreeFromResource(ResourceId("outer.html"));
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    ASSERT_EQ(result.document->documentElement()->children().size(), 1U);
    ASSERT_EQ(result.document->documentElement()->children()[0]->children().size(), 1U);
    EXPECT_EQ(result.document->documentElement()->children()[0]->children()[0]->id(), "inner");
}

TEST_F(ResourceCompilerTest, ResolvesRootedLayoutResourceReferences) {
    resources["shared.html"] = "<panel id=\"shared\"></panel>";

    const ResourceBuildResult result =
        factory.buildElementTreeFromString("<panel><panel filename=\"/shared.html\"></panel></panel>", "nested/outer.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    ASSERT_EQ(result.document->documentElement()->children().size(), 1U);
    EXPECT_EQ(result.document->documentElement()->children().front()->id(), "shared");
}

TEST_F(ResourceCompilerTest, RejectsResourceCycles) {
    constexpr char kFirstCycleLayout[] = "<panel><panel filename=\"b.html\"></panel></panel>";
    constexpr char kSecondCycleLayout[] = "<panel><panel filename=\"a.html\"></panel></panel>";
    resources["a.html"] = kFirstCycleLayout;
    resources["b.html"] = kSecondCycleLayout;

    const ResourceBuildResult result = factory.buildElementTreeFromResource(ResourceId("a.html"));
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.resource.cycle");
    EXPECT_EQ(result.errors.front().source, "a.html");
}

TEST_F(ResourceCompilerTest, RejectsInvalidResourceReferences) {
    struct InvalidReferenceCase {
        const char* name;
        const char* html;
        const char* id;
        const char* resourceHTML;
        const char* diagnostic;
    };

    const InvalidReferenceCase cases[] = {
        {"missing resource", "<panel><panel filename=\"missing.html\"></panel></panel>", nullptr, nullptr, "layout.resource.missing"},
        {"non-panel resource", "<panel><panel filename=\"wrong.html\"></panel></panel>", "wrong.html", "<p></p>", "layout.resource.root_invalid"},
        {"filename on non-panel", "<button filename=\"x.html\"></button>", nullptr, nullptr, "layout.filename.unsupported"},
        {"resource root escape", "<panel><panel filename=\"../outside.html\"></panel></panel>", nullptr, nullptr, "layout.resource.path_invalid"},
    };

    for (const auto& test : cases) {
        resources.clear();
        if (test.id != nullptr) resources[test.id] = test.resourceHTML;

        SCOPED_TRACE(Message() << "invalid resource reference: " << test.name);
        const ResourceBuildResult result = factory.buildElementTreeFromString(test.html, "root.html");
        ASSERT_FALSE(result.ok());
        EXPECT_FALSE(result.document);
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    resources["empty.html"] = "";
    const ResourceBuildResult empty = factory.buildElementTreeFromResource(ResourceId("empty.html"));
    ASSERT_FALSE(empty.ok());
    ASSERT_FALSE(empty.errors.empty());
    EXPECT_EQ(empty.errors.front().code, "layout.html.invalid");
}

TEST_F(ResourceCompilerTest, PreservesCompilerSourceLocations) {
    constexpr char kSourceLocationLayout[] = "<panel>\n"
                                             "  <unknown></unknown>\n"
                                             "</panel>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kSourceLocationLayout, "source_ranges.html");
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().line, 2U);
    EXPECT_EQ(result.errors.front().column, 3U);
}

TEST_F(ResourceCompilerTest, PreservesMixedContentOrderAndSourceRanges) {
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
    EXPECT_GT(parsed.document->root->content[1].source.end.offset, parsed.document->root->content[1].source.begin.offset);
}

TEST_F(ResourceCompilerTest, BuildsMixedTextAndElementsAsOneOrderedRuntimeTree) {
    constexpr char kMixedContentLayout[] = "<p>Hello <b>world</b><br>Again</p>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kMixedContentLayout, "mixed-runtime.html");
    ASSERT_TRUE(result.ok());
    Element* root = result.document->documentElement();
    ASSERT_NE(root, nullptr);

    const auto runtimeChildren = nodes(*root);
    ASSERT_EQ(runtimeChildren.size(), 4U);
    ASSERT_NE(runtimeChildren.begin()->asText(), nullptr);
    EXPECT_EQ(runtimeChildren.begin()->asText()->data(), "Hello ");
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
    EXPECT_TRUE(NodeAccess::flowBreakBefore(*child));
    EXPECT_EQ(child->asText()->data(), "Again");
    EXPECT_EQ(root->textContent(), "Hello world\nAgain");
}

TEST_F(ResourceCompilerTest, PreservesWhitespaceInTextNodes) {
    const ResourceBuildResult result = factory.buildElementTreeFromString("<p>  before   after  </p>", "whitespace.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    EXPECT_EQ(result.document->documentElement()->textContent(), "  before   after  ");
}

TEST_F(ResourceCompilerTest, IgnoresFormattingWhitespaceWhenValidatingFlowBreaks) {
    const ResourceBuildResult result = factory.buildElementTreeFromString("<p>\n  <br>\n  after\n</p>", "flow-break-whitespace.html");
    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.flow_break.leading");
}

TEST_F(ResourceCompilerTest, ParsesCanonicalTagsAndRejectsUnknownTags) {
    const SourceDocumentParseResult known = SourceDocumentParser().parse("<PaNeL><BuTtOn></BuTtOn><P></P></PaNeL>", "tags.html");
    ASSERT_TRUE(known.ok());
    ASSERT_NE(known.document, nullptr);
    ASSERT_EQ(known.document->root->tag, HTMLTag::Panel);
    ASSERT_EQ(known.document->root->content.size(), 2U);
    ASSERT_EQ(known.document->root->content[0].node->tag, HTMLTag::Button);
    ASSERT_EQ(known.document->root->content[1].node->tag, HTMLTag::Paragraph);

    const SourceDocumentParseResult unknown = SourceDocumentParser().parse("<panel>\n  <madeUp></madeUp>\n</panel>", "unknown.html");
    ASSERT_FALSE(unknown.ok());
    ASSERT_FALSE(unknown.errors.empty());
    EXPECT_EQ(unknown.errors.front().code, "layout.element.unknown");
    EXPECT_EQ(unknown.errors.front().source, "unknown.html");
    EXPECT_EQ(unknown.errors.front().line, 2U);
    EXPECT_EQ(unknown.errors.front().column, 3U);
}

TEST_F(ResourceCompilerTest, RejectsSelfClosingSyntaxOnNormalElements) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<panel/>", "self-closing.html");

    ASSERT_FALSE(parsed.ok());
    ASSERT_FALSE(parsed.errors.empty());
    EXPECT_EQ(parsed.errors.front().code, "layout.html.invalid");
}

TEST_F(ResourceCompilerTest, KeepsSlashInUnquotedAttributeValues) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<input name=mode/>", "unquoted-slash.html");

    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    EXPECT_EQ(parsed.document->root->attributes.at("name").value, "mode/");
}

TEST_F(ResourceCompilerTest, ComposesButtonInlineChildren) {
    constexpr char kButtonLayout[] = "<button>first"
                                     "<icon src=\"one\"></icon></button>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kButtonLayout);
    ASSERT_TRUE(result.ok());
    HTMLButtonElement* button = result.rootAs<HTMLButtonElement>();
    ASSERT_NE(button, nullptr);
    const auto buttonNodes = nodes(*button);
    ASSERT_EQ(buttonNodes.size(), 2U);
    ASSERT_NE(buttonNodes.begin()->asText(), nullptr);
    EXPECT_EQ(buttonNodes.begin()->asText()->data(), "first");
    auto iconNode = buttonNodes.begin();
    ++iconNode;
    ASSERT_NE(iconNode->asElement(), nullptr);
    auto* icon = dynamic_cast<radia::ui::HTMLIconElement*>(iconNode->asElement());
    ASSERT_NE(icon, nullptr);
    EXPECT_EQ(icon->name(), "one");
    ASSERT_EQ(button->children().size(), 1U);
    EXPECT_EQ(button->children().front(), icon);

    constexpr char kIconFirstLayout[] = "<button><icon src=\"search\"></icon>"
                                        "second</button>";
    ResourceBuildResult iconFirst = factory.buildElementTreeFromString(kIconFirstLayout);
    ASSERT_TRUE(iconFirst.ok());
    HTMLButtonElement* reversed = iconFirst.rootAs<HTMLButtonElement>();
    ASSERT_NE(reversed, nullptr);
    const auto reversedNodes = nodes(*reversed);
    ASSERT_EQ(reversedNodes.size(), 2U);
    ASSERT_NE(reversedNodes.begin()->asElement(), nullptr);
    EXPECT_EQ(reversedNodes.begin()->asElement()->elementName(), "icon");
    auto reversedText = reversedNodes.begin();
    ++reversedText;
    ASSERT_NE(reversedText->asText(), nullptr);
    EXPECT_EQ(reversedText->asText()->data(), "second");

    button->append(makeElement<HTMLIconElement>("updated"));
    appendText(*button, "updated");
    EXPECT_EQ(nodes(*button).size(), 4U);
    button->replaceChildren();
    EXPECT_TRUE(nodes(*button).empty());
    auto rebuilt = makeElement<HTMLIconElement>("rebuilt");
    radia::ui::HTMLIconElement* rebuiltIcon = rebuilt.get();
    button->append(std::move(rebuilt));
    ASSERT_EQ(button->children().size(), 1U);
    EXPECT_EQ(button->children().front(), rebuiltIcon);
    EXPECT_EQ(rebuiltIcon->name(), "rebuilt");
}

TEST_F(ResourceCompilerTest, RefreshesLocalizedElementsAcrossLocaleChanges) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalizationYaml[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {title: Title, status: Ready, press: Press}}, "
                                         "pt: {strings: {title: Título, status: Pronto, press: Pressione}}}\n";
    constexpr char kStyles[] = "label { color: #ffffffff; }";
    constexpr char kLocalizedLayout[] = "<floater><head><title>{{title}}</title></head><body>"
                                        "<label id=\"status\" for=\"target\">{{status}}</label>"
                                        "<input type=\"checkbox\" switch=\"true\" id=\"target\">"
                                        "<button id=\"press\">{{press}}</button></body></floater>";
    resources.add("localization.yaml", kLocalizationYaml);
    resources.add("skin.css", kStyles);
    resources.add("localized.html", kLocalizedLayout);

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));

    ResourceBuildResult result = system.buildElementTree(ResourceId("localized.html"));
    ASSERT_TRUE(result.ok());
    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    const ElementRef<HTMLLabelElement> status = requireElement<HTMLLabelElement>(*floater, "status");
    const ElementRef<HTMLButtonElement> press = requireElement<HTMLButtonElement>(*floater, "press");
    ASSERT_NE(status.get(), nullptr);
    ASSERT_NE(press.get(), nullptr);
    auto pressNodes = nodes(*press);
    ASSERT_EQ(pressNodes.size(), 1U);
    ASSERT_NE(pressNodes.begin()->asText(), nullptr);
    EXPECT_EQ(floater->title(), "Title");
    EXPECT_EQ(status->textContent(), "Ready");
    EXPECT_EQ(pressNodes.begin()->asText()->data(), "Press");

    std::unique_ptr<Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    ASSERT_TRUE(result.document);
    surface->mountFloater(*result.document);
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(floater->title(), "Título");
    EXPECT_EQ(status->textContent(), "Pronto");
    pressNodes = nodes(*press);
    ASSERT_EQ(pressNodes.size(), 1U);
    ASSERT_NE(pressNodes.begin()->asText(), nullptr);
    EXPECT_EQ(pressNodes.begin()->asText()->data(), "Pressione");

    auto programmatic = makeElement<HTMLFloaterElement>();
    HTMLFloaterElement* programmaticFloater = programmatic.get();
    radia::ui::test::appendFloaterStructure(*programmatic);
    programmatic->head()->children().front()->innerHTML(system.t("title"));
    surface->mountFloater(std::move(programmatic));
    EXPECT_EQ(programmaticFloater->title(), "Título");

    auto localizedBody = radia::ui::test::makeFloater();
    HTMLFloaterElement* localizedBodyFloater = localizedBody.get();
    surface->mountFloater(std::move(localizedBody));
    auto bodyChild = makeElement<Element>("p");
    bodyChild->setId("body-child");
    localizedBodyFloater->body()->append(std::move(bodyChild));
    ASSERT_EQ(localizedBodyFloater->body()->children().size(), 1U);
    localizedBodyFloater->body()->children().front()->innerHTML(system.t("status"));
    EXPECT_EQ(localizedBodyFloater->body()->textContent(), "Pronto");

    status->innerHTML(system.t("status"));
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

TEST_F(ResourceCompilerTest, RefreshesLocalizedRichTextAcrossLocaleChanges) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalizationYaml[] = "defaultLocale: en\n"
                                         "locales: {en: {strings: {message: 'Hello <b>bold</b><br>Again'}}, "
                                         "pt: {strings: {message: 'Olá <i>itálico</i><br>Novamente'}}}\n";
    resources.add("localization.yaml", kLocalizationYaml);
    resources.add("skin.css", "p { display: block; } b { font-weight: bold; } i { font-style: italic; }");
    resources.add("rich.html", "<p>{{message}}</p>");

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(resources);
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));

    ResourceBuildResult result = system.buildElementTree(ResourceId("rich.html"));
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
        EXPECT_EQ(child->asText()->data(), first);
        ++child;
        ASSERT_NE(child->asElement(), nullptr);
        EXPECT_EQ(child->asElement()->elementName(), inlineElement);
        EXPECT_EQ(child->asElement()->textContent(), inlineText);
        ++child;
        ASSERT_NE(child->asElement(), nullptr);
        EXPECT_EQ(child->asElement()->elementName(), "br");
        ++child;
        ASSERT_NE(child->asText(), nullptr);
        EXPECT_TRUE(NodeAccess::flowBreakBefore(*child));
        EXPECT_EQ(child->asText()->data(), last);
    };

    EXPECT_EQ(root->textContent(), "Hello bold\nAgain");
    assertRuntime("Hello ", "b", "bold", "Again");
    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(root->textContent(), "Olá itálico\nNovamente");
    assertRuntime("Olá ", "i", "itálico", "Novamente");
}

TEST_F(ResourceCompilerTest, RejectsUnknownElementsAndAttributes) {
    constexpr char kUnknownElementLayout[] = "<panel>"
                                             "<unknown></unknown></panel>";
    constexpr char kUnsupportedAttributeLayout[] = "<panel width=\"10\"></panel>";
    constexpr char kUnknownAttributeLayout[] = "<floater invented=\"true\"></floater>";
    const ResourceBuildResult unknownElement = factory.buildElementTreeFromString(kUnknownElementLayout, "unknown.html");
    ASSERT_FALSE(unknownElement.ok());
    EXPECT_FALSE(unknownElement.document);
    ASSERT_FALSE(unknownElement.errors.empty());
    EXPECT_EQ(unknownElement.errors.front().code, "layout.element.unknown");
    EXPECT_EQ(unknownElement.errors.front().source, "unknown.html");

    const ResourceBuildResult unsupportedAttribute = factory.buildElementTreeFromString(kUnsupportedAttributeLayout, "attribute.html");
    ASSERT_FALSE(unsupportedAttribute.ok());
    EXPECT_FALSE(unsupportedAttribute.document);
    ASSERT_FALSE(unsupportedAttribute.errors.empty());
    EXPECT_EQ(unsupportedAttribute.errors.front().code, "layout.attribute.unsupported");

    const ResourceBuildResult unknownAttribute = factory.buildElementTreeFromString(kUnknownAttributeLayout, "unknown_attribute.html");
    ASSERT_FALSE(unknownAttribute.ok());
    EXPECT_FALSE(unknownAttribute.document);
    ASSERT_FALSE(unknownAttribute.errors.empty());
    EXPECT_EQ(unknownAttribute.errors.front().code, "layout.attribute.unknown");
}

TEST_F(ResourceCompilerTest, AcceptsEventsOnEveryElementAndWarnsForExpressions) {
    constexpr char kUniversalEventLayout[] = "<p onClick=\"click()\">copy</p>";
    constexpr char kExpressionCallLayout[] = "<button onClick=\"save(force=true)\"></button>";
    const ResourceBuildResult universalEvent = factory.buildElementTreeFromString(kUniversalEventLayout, "event.html");
    ASSERT_TRUE(universalEvent.ok());
    ASSERT_TRUE(universalEvent.document);
    EXPECT_TRUE(universalEvent.warnings.empty());
    ASSERT_NE(authoredEventCall(*universalEvent.document->documentElement(), kClickEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*universalEvent.document->documentElement(), kClickEvent)->name(), "click");

    const ResourceBuildResult expressionCall = factory.buildElementTreeFromString(kExpressionCallLayout, "expression.html");
    ASSERT_TRUE(expressionCall.ok());
    ASSERT_TRUE(expressionCall.document);
    ASSERT_EQ(expressionCall.warnings.size(), 1U);
    EXPECT_EQ(expressionCall.warnings.front().code, "layout.event.literal_unsupported");
}

TEST_F(ResourceCompilerTest, AcceptsDuplicateElementIdsAndUsesTreeOrder) {
    constexpr char kDuplicateIdLayout[] = "<panel><p id=\"same\"></p>"
                                          "<button id=\"same\">Same</button></panel>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kDuplicateIdLayout, "duplicates.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    const Element* first = result.document->getElementById("same");
    ASSERT_NE(first, nullptr);
    EXPECT_EQ(first->elementName(), "p");
    ASSERT_EQ(result.document->getElementById("same"), first);
}

TEST_F(ResourceCompilerTest, RejectsInvalidBooleanAttributes) {
    constexpr char kInvalidBooleanLayout[] = "<floater resizeable=\"sometimes\"><head></head><body>"
                                             "<input type=\"checkbox\" switch=\"true\" checked=\"yes\"></body></floater>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kInvalidBooleanLayout, "booleans.html");
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_EQ(result.errors.size(), 2U);
    EXPECT_EQ(result.errors[0].code, "layout.attribute.boolean_invalid");
    EXPECT_EQ(result.errors[0].message, "Invalid boolean value for resizeable: sometimes.");
    EXPECT_EQ(result.errors[0].source, "booleans.html");
    EXPECT_EQ(result.errors[0].line, 1U);
    EXPECT_EQ(result.errors[0].column, 1U);
    EXPECT_EQ(result.errors[1].code, "layout.attribute.boolean_invalid");
    EXPECT_EQ(result.errors[1].message, "Invalid boolean value for checked: yes.");
    EXPECT_EQ(result.errors[1].source, "booleans.html");
    EXPECT_EQ(result.errors[1].line, 1U);
    EXPECT_EQ(result.errors[1].column, 52U);
}

TEST_F(ResourceCompilerTest, AcceptsBooleanAttributesWithoutValue) {
    const ResourceBuildResult result = factory.buildElementTreeFromString("<button disabled>Save</button>", "boolean.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    const HTMLButtonElement* button = result.rootAs<HTMLButtonElement>();
    ASSERT_NE(button, nullptr);
    EXPECT_TRUE(button->disabled());
    EXPECT_EQ(button->textContent(), "Save");
}

TEST_F(ResourceCompilerTest, ParsesRadiaHTMLSyntaxDirectly) {
    constexpr char kHTML[] = "<panel data-kind=demo><!-- ignored --><input type=checkbox switch checked><br>After</panel>";
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse(kHTML, "direct.html");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 3U);

    const radia::ui::SourceNode* input = parsed.document->root->content[0].node.get();
    ASSERT_NE(input, nullptr);
    EXPECT_EQ(input->attributes.at("type").value, "checkbox");
    EXPECT_TRUE(input->attributes.contains("switch"));
    EXPECT_TRUE(input->attributes.contains("checked"));
    EXPECT_EQ(parsed.document->root->content[1].node->tag, HTMLTag::Br);
    EXPECT_EQ(parsed.document->root->content[2].text, "After");
    EXPECT_EQ(parsed.document->root->attributes.at("data-kind").value, "demo");
}

TEST_F(ResourceCompilerTest, DecodesHTMLEntitiesWithoutRewritingHTML) {
    constexpr char kHTML[] = "<p title=\"a&amp;b\">&lt; &quot; &apos;</p>";
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse(kHTML, "entities.html");
    ASSERT_TRUE(parsed.ok());
    ASSERT_NE(parsed.document, nullptr);
    ASSERT_EQ(parsed.document->root->content.size(), 1U);
    EXPECT_EQ(parsed.document->root->attributes.at("title").value, "a&b");
    EXPECT_EQ(parsed.document->root->content.front().text, "< \" '");
}

TEST_F(ResourceCompilerTest, RejectsMismatchedHTMLTags) {
    const SourceDocumentParseResult parsed = SourceDocumentParser().parse("<panel><p></panel>", "mismatched.html");
    ASSERT_FALSE(parsed.ok());
    ASSERT_FALSE(parsed.errors.empty());
    EXPECT_EQ(parsed.errors.front().code, "layout.html.invalid");
}

TEST_F(ResourceCompilerTest, ManagesAuthoredFloaterHeadLifecycle) {
    constexpr char kFloaterLayout[] = "<floater><head><title><icon src=\"search\"></icon>tools</title>"
                                      "<minimize><icon src=\"minimize\"></icon></minimize><close><icon src=\"close\"></icon></close></head>"
                                      "<body><panel id=\"content\"><button id=\"refresh\">Refresh</button></panel></body></floater>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "floater_head.html");
    ASSERT_TRUE(result.ok());
    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);

    ElementRef<HTMLPanelElement> content = requireElement<HTMLPanelElement>(*floater, "content");
    ElementRef<HTMLButtonElement> refresh = requireElement<HTMLButtonElement>(*floater, "refresh");
    ASSERT_NE(content.get(), nullptr);
    ASSERT_NE(refresh.get(), nullptr);
    ASSERT_NE(floater->head(), nullptr);
    ASSERT_NE(floater->body(), nullptr);
    EXPECT_EQ(content->parentElement(), floater->body());
    EXPECT_EQ(refresh->parentElement(), content.get());
    EXPECT_EQ(floater->head()->children()[1], floater->minimizeButton());
    EXPECT_EQ(floater->head()->children()[2], floater->closeButton());
    EXPECT_EQ(floater->minimizeButton()->children()[0]->elementName(), "icon");
    EXPECT_EQ(floater->closeButton()->children()[0]->elementName(), "icon");
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
    EXPECT_EQ(content.get(), nullptr);
    EXPECT_EQ(refresh.get(), nullptr);
}

TEST_F(ResourceCompilerTest, FindsFloaterControlsThroughHeadWrappers) {
    constexpr char kFloaterLayout[] =
        "<floater><head><title>tools</title><div><minimize></minimize><close></close></div></head><body></body></floater>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kFloaterLayout, "nested_floater_controls.html");
    ASSERT_TRUE(result.ok());
    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    EXPECT_EQ(floater->minimizeButton()->parentElement()->elementName(), "div");
    EXPECT_EQ(floater->closeButton()->parentElement()->elementName(), "div");
}

TEST_F(ResourceCompilerTest, RejectsInvalidFloaterStructure) {
    constexpr char kMissingTitleLayout[] = "<floater><head><minimize></minimize></head><body></body></floater>";
    constexpr char kDuplicateHeadLayout[] = "<floater><head></head><head></head><body></body></floater>";
    constexpr char kDuplicateTitleLayout[] = "<floater><head><title>one</title><div><title>two</title></div></head><body></body></floater>";
    constexpr char kDuplicateMinimizeLayout[] =
        "<floater><head><title>one</title><minimize></minimize><div><minimize></minimize></div></head><body></body></floater>";
    constexpr char kDuplicateCloseLayout[] =
        "<floater><head><title>one</title><close></close><div><close></close></div></head><body></body></floater>";
    constexpr char kNestedHeadLayout[] = "<floater><body><head></head></body></floater>";
    constexpr char kNestedBodyLayout[] = "<floater><head><body></body></head></floater>";
    constexpr char kBodyTitleLayout[] = "<floater><head><title>one</title></head><body><div><title>two</title></div></body></floater>";
    constexpr char kBodyMinimizeLayout[] = "<floater><head><title>one</title></head><body><div><minimize></minimize></div></body></floater>";
    constexpr char kBodyCloseLayout[] = "<floater><head><title>one</title></head><body><div><close></close></div></body></floater>";
    const ResourceBuildResult missingTitle = factory.buildElementTreeFromString(kMissingTitleLayout, "missing_title.html");
    ASSERT_FALSE(missingTitle.ok());
    ASSERT_FALSE(missingTitle.errors.empty());
    EXPECT_EQ(missingTitle.errors.front().code, "layout.floater.title_required");

    const ResourceBuildResult duplicateHead = factory.buildElementTreeFromString(kDuplicateHeadLayout, "duplicate_head.html");
    ASSERT_FALSE(duplicateHead.ok());
    ASSERT_FALSE(duplicateHead.errors.empty());
    EXPECT_EQ(duplicateHead.errors.front().code, "layout.floater.head_duplicate");

    const ResourceBuildResult duplicateTitle = factory.buildElementTreeFromString(kDuplicateTitleLayout, "duplicate_title.html");
    ASSERT_FALSE(duplicateTitle.ok());
    ASSERT_FALSE(duplicateTitle.errors.empty());
    EXPECT_EQ(duplicateTitle.errors.front().code, "layout.floater.title_duplicate");

    const ResourceBuildResult duplicateMinimize = factory.buildElementTreeFromString(kDuplicateMinimizeLayout, "duplicate_minimize.html");
    ASSERT_FALSE(duplicateMinimize.ok());
    ASSERT_FALSE(duplicateMinimize.errors.empty());
    EXPECT_EQ(duplicateMinimize.errors.front().code, "layout.floater.minimize_duplicate");

    const ResourceBuildResult duplicateClose = factory.buildElementTreeFromString(kDuplicateCloseLayout, "duplicate_close.html");
    ASSERT_FALSE(duplicateClose.ok());
    ASSERT_FALSE(duplicateClose.errors.empty());
    EXPECT_EQ(duplicateClose.errors.front().code, "layout.floater.close_duplicate");

    const ResourceBuildResult nestedHead = factory.buildElementTreeFromString(kNestedHeadLayout, "nested_head.html");
    ASSERT_FALSE(nestedHead.ok());
    ASSERT_FALSE(nestedHead.errors.empty());
    EXPECT_EQ(nestedHead.errors.front().code, "layout.floater.head_required");

    const ResourceBuildResult nestedBody = factory.buildElementTreeFromString(kNestedBodyLayout, "nested_body.html");
    ASSERT_FALSE(nestedBody.ok());
    ASSERT_FALSE(nestedBody.errors.empty());
    EXPECT_EQ(nestedBody.errors.front().code, "layout.floater.body_required");

    const ResourceBuildResult bodyTitle = factory.buildElementTreeFromString(kBodyTitleLayout, "body_title.html");
    ASSERT_FALSE(bodyTitle.ok());
    ASSERT_FALSE(bodyTitle.errors.empty());
    EXPECT_EQ(bodyTitle.errors.front().code, "layout.floater.head_only");

    const ResourceBuildResult bodyMinimize = factory.buildElementTreeFromString(kBodyMinimizeLayout, "body_minimize.html");
    ASSERT_FALSE(bodyMinimize.ok());
    ASSERT_FALSE(bodyMinimize.errors.empty());
    EXPECT_EQ(bodyMinimize.errors.front().code, "layout.floater.head_only");

    const ResourceBuildResult bodyClose = factory.buildElementTreeFromString(kBodyCloseLayout, "body_close.html");
    ASSERT_FALSE(bodyClose.ok());
    ASSERT_FALSE(bodyClose.errors.empty());
    EXPECT_EQ(bodyClose.errors.front().code, "layout.floater.head_only");
}

TEST_F(ResourceCompilerTest, AppliesChildBearingElementDefaults) {
    resources["elements/minimize.html"] = "<minimize><icon src=\"minimize\"></icon></minimize>";
    resources["elements/close.html"] = "<close><icon src=\"close\"></icon></close>";
    constexpr char kDefaultedFloaterLayout[] =
        "<floater><head><title>defaulted</title><minimize></minimize><close></close></head><body></body></floater>";

    ASSERT_FALSE(factory.validateElementDefaults("MINIMIZE").hasErrors());
    ASSERT_FALSE(factory.validateElementDefaults("close").hasErrors());
    ResourceBuildResult result = factory.buildElementTreeFromString(kDefaultedFloaterLayout, "defaulted.html");
    ASSERT_TRUE(result.ok());
    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->closeButton(), nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_EQ(floater->closeButton()->children().size(), 1U);
    ASSERT_EQ(floater->minimizeButton()->children().size(), 1U);
    const auto* closeIcon = dynamic_cast<radia::ui::HTMLIconElement*>(floater->closeButton()->children().front());
    const auto* minimizeIcon = dynamic_cast<radia::ui::HTMLIconElement*>(floater->minimizeButton()->children().front());
    ASSERT_NE(closeIcon, nullptr);
    ASSERT_NE(minimizeIcon, nullptr);
    EXPECT_EQ(closeIcon->name(), "close");
    EXPECT_EQ(minimizeIcon->name(), "minimize");
    EXPECT_TRUE(floater->closable());
    EXPECT_TRUE(floater->minimizable());

    constexpr char kOverrideLayout[] = "<floater><head><title>override</title><minimize><icon src=\"custom\"></icon></minimize><close></close></head>"
                                       "<body></body></floater>";
    result = factory.buildElementTreeFromString(kOverrideLayout, "override.html");
    ASSERT_TRUE(result.ok());
    floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    ASSERT_NE(floater->minimizeButton(), nullptr);
    ASSERT_EQ(floater->minimizeButton()->children().size(), 1U);
    const auto* customIcon = dynamic_cast<radia::ui::HTMLIconElement*>(floater->minimizeButton()->children().front());
    ASSERT_NE(customIcon, nullptr);
    EXPECT_EQ(customIcon->name(), "custom");
}

TEST_F(ResourceCompilerTest, RejectsInvalidElementDefaults) {
    constexpr char kInvalidFloaterDefaultsLayout[] = "<panel></panel>";
    constexpr char kDefaultedLayout[] = "<floater title=\"defaulted\"></floater>";
    resources["elements/floater.html"] = kInvalidFloaterDefaultsLayout;
    resources["defaulted.html"] = kDefaultedLayout;

    const DiagnosticResult defaults = factory.validateElementDefaults("floater");
    ASSERT_TRUE(defaults.hasErrors());
    ASSERT_FALSE(defaults.errors.empty());
    EXPECT_EQ(defaults.errors.front().code, "layout.defaults.root_invalid");
    const ResourceBuildResult result = factory.buildElementTreeFromResource(ResourceId("defaulted.html"));
    ASSERT_FALSE(result.ok());
    EXPECT_FALSE(result.document);
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().code, "layout.defaults.root_invalid");
}

TEST_F(ResourceCompilerTest, CompilesTypedVisibilityValues) {
    constexpr char kVisibilityLayout[] = "<panel><p id=\"shown\" visibility=\"visible\"></p>"
                                         "<p id=\"hidden\" visibility=\"hidden\"></p>"
                                         "<p id=\"collapsed\" visibility=\"collapse\"></p></panel>";
    const ResourceBuildResult result = factory.buildElementTreeFromString(kVisibilityLayout, "visibility.html");
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.document);
    ASSERT_EQ(result.document->documentElement()->children().size(), 3U);
    EXPECT_EQ(result.document->documentElement()->children()[0]->visibility(), Visibility::Visible);
    EXPECT_EQ(result.document->documentElement()->children()[1]->visibility(), Visibility::Hidden);
    EXPECT_EQ(result.document->documentElement()->children()[2]->visibility(), Visibility::Collapse);
}

TEST_F(ResourceCompilerTest, RejectsInvalidVisibilitySyntax) {
    struct InvalidVisibilityCase {
        const char* name;
        const char* html;
        const char* diagnostic;
    };
    const InvalidVisibilityCase cases[] = {
        {"invalid enum value", "<p visibility=\"invisible\"></p>", "layout.attribute.visibility_invalid"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "visibility HTML: " << test.name);
        const ResourceBuildResult result = factory.buildElementTreeFromString(test.html, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }
}

TEST_F(ResourceCompilerTest, ValidatesElementDefaultDiagnostics) {
    constexpr char kInvalidVisibilityDefaultsLayout[] = "<label visibility=\"sometimes\"></label>";
    constexpr char kLabelDefaultsLayout[] = "<label></label>";
    constexpr char kInvalidSwitchDefaultsLayout[] = "<input type=\"checkbox\" switch=\"true\" checked=\"sometimes\">";
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

TEST_F(ResourceCompilerTest, PreservesPhysicalProvenanceForElementDefaultDiagnostics) {
    ResourceSnapshot snapshot;
    ASSERT_TRUE(snapshot.add("elements/label.html", "<label visibility=\"sometimes\"></label>", "skins/views/elements/label.html"));

    const DiagnosticResult result = ResourceCompiler(&snapshot).validateElementDefaults("label");
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.front().source, "skins/views/elements/label.html");
}

TEST_F(ResourceCompilerTest, AcceptsCaseInsensitiveHTMLNames) {
    constexpr char kCaseInsensitiveLayout[] = "<FlOaTeR ReSiZeAbLe><HeAd><TiTlE>tools</TiTlE><MiNiMiZe></MiNiMiZe></HeAd><BoDy>"
                                              "<BuTtOn ID=\"saveFile\" ONCLICK=\"saveFile()\">"
                                              "<IcOn SrC=\"search\"></IcOn>Save</BuTtOn></BoDy></FlOaTeR>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kCaseInsensitiveLayout, "case-insensitive.html");
    ASSERT_TRUE(result.ok());
    HTMLFloaterElement* floater = result.rootAs<HTMLFloaterElement>();
    ASSERT_NE(floater, nullptr);
    const ElementRef<HTMLButtonElement> button = requireElement<HTMLButtonElement>(*floater, "saveFile");
    ASSERT_NE(button.get(), nullptr);
    ASSERT_EQ(button->children().size(), 1U);
    ASSERT_NE(dynamic_cast<radia::ui::HTMLIconElement*>(button->children().front()), nullptr);
    EXPECT_EQ(button->elementName(), "button");
    ASSERT_NE(authoredEventCall(*button, kClickEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*button, kClickEvent)->name(), "saveFile");
}

TEST_F(ResourceCompilerTest, RejectsMalformedHTMLAttributesAndCaseFoldedConflicts) {
    struct InvalidHTMLCase {
        const char* name;
        const char* html;
        const char* diagnostic;
    };
    const InvalidHTMLCase cases[] = {
        {"snake-case event attribute", "<BuTtOn\n  on_click=\"saveFile()\"></BuTtOn>", "layout.attribute.unknown"},
        {"case-folded duplicate attribute", "<button onClick=\"saveOne()\" ONCLICK=\"saveTwo()\"></button>", "layout.attribute.duplicate"},
        {"empty element id", "<panel id=\"\"></panel>", "layout.id.invalid"},
        {"whitespace in element id", "<panel id=\"bad id\"></panel>", "layout.id.invalid"},
    };
    for (const auto& test : cases) {
        SCOPED_TRACE(Message() << "invalid HTML: " << test.name);
        const ResourceBuildResult result = factory.buildElementTreeFromString(test.html, test.name);
        ASSERT_FALSE(result.ok());
        ASSERT_FALSE(result.errors.empty());
        EXPECT_EQ(result.errors.front().code, test.diagnostic);
    }

    const ResourceBuildResult standardValues =
        factory.buildElementTreeFromString("<panel id=\"123:bad.id\" class=\"bad.class @token\"></panel>", "standard-values.html");
    ASSERT_TRUE(standardValues.ok());
    ASSERT_TRUE(standardValues.document);
    EXPECT_EQ(standardValues.document->documentElement()->id(), "123:bad.id");
    EXPECT_TRUE(standardValues.document->documentElement()->classes().contains("bad.class"));
    EXPECT_TRUE(standardValues.document->documentElement()->classes().contains("@token"));

    constexpr char kInvalidHandlerLayout[] = "<button onClick=\"bad_action()\"></button>";
    const ResourceBuildResult invalidHandler = factory.buildElementTreeFromString(kInvalidHandlerLayout, "handler-name.html");
    ASSERT_TRUE(invalidHandler.ok());
    ASSERT_EQ(invalidHandler.warnings.size(), 1U);
    EXPECT_EQ(invalidHandler.warnings.front().code, "layout.event.name_invalid");
}

TEST_F(ResourceCompilerTest, PreservesValidEventCallsAndWarnsForInvalidCalls) {
    constexpr char kEventCallsLayout[] = "<panel><button id=\"inspect\" "
                                         "onClick=\"inspect(4, 'settings', true, this, event)\"></button>"
                                         "<button id=\"bare\" onClick=\"press\"></button>"
                                         "<button id=\"lifecycle\" onClick=\"postBuild()\"></button></panel>";
    ResourceBuildResult result = factory.buildElementTreeFromString(kEventCallsLayout, "event-calls.html");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), 2U);
    EXPECT_EQ(result.warnings[0].code, "layout.event.call_required");
    EXPECT_EQ(result.warnings[1].code, "layout.event.handler_reserved");

    const ElementRef<HTMLButtonElement> inspect = requireElement<HTMLButtonElement>(*result.document->documentElement(), "inspect");
    const ElementRef<HTMLButtonElement> bare = requireElement<HTMLButtonElement>(*result.document->documentElement(), "bare");
    const ElementRef<HTMLButtonElement> lifecycle = requireElement<HTMLButtonElement>(*result.document->documentElement(), "lifecycle");
    ASSERT_NE(inspect.get(), nullptr);
    ASSERT_NE(bare.get(), nullptr);
    ASSERT_NE(lifecycle.get(), nullptr);
    ASSERT_NE(authoredEventCall(*inspect, kClickEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*inspect, kClickEvent)->name(), "inspect");
    EXPECT_EQ(authoredEventCall(*inspect, kClickEvent)->arguments().size(), 5U);
    EXPECT_EQ(authoredEventCall(*bare, kClickEvent), nullptr);
    EXPECT_EQ(authoredEventCall(*lifecycle, kClickEvent), nullptr);
}
