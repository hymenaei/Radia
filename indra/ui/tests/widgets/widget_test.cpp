/**
 * @file widget_test.cpp
 * @brief Tests Widget identity, state, tree ownership, and event behavior.
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
#include "../test/lltut.h"
#include "render/recordingpaintcontext.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"

namespace tut {
std::string paintedText(const radia::ui::RecordingPaintContext& recording) {
    std::string text;
    for (const radia::ui::PaintCommand& command : recording.commands())
        if (command.kind == radia::ui::PaintCommandKind::Text) text += command.value;
    return text;
}

struct widgetData {};
using widgetTest = test_group<widgetData>;
using widgetObject = widgetTest::object;
widgetTest widgetTestCase("widget");

template<> template<> void widgetObject::test<1>() {
    radia::ui::Panel root;
    auto button = std::make_unique<radia::ui::Button>();
    radia::ui::WidgetRef<radia::ui::Button> reference(button.get());
    root.addChild(std::move(button));
    ensure("mounted widget reference remains valid", static_cast<bool>(reference));
    root.clearChildren();
    ensure("destroyed widget expires its reference", !reference);
}

template<> template<> void widgetObject::test<2>() {
    radia::ui::Button button;
    int activations = 0;
    button.setOnActivate([&](radia::ui::Widget&) { ++activations; });
    button.activate();
    button.setDisabled(true).activate();
    ensure_equals("disabled button does not activate", activations, 1);
    ensure("button is focusable", button.focusable());
}

template<> template<> void widgetObject::test<3>() {
    radia::ui::Panel root;
    auto first = std::make_unique<radia::ui::Button>();
    first->setId("first");
    root.addChild(std::move(first));
    auto leading = std::make_unique<radia::ui::Button>();
    leading->setId("leading");
    root.prependChild(std::move(leading));
    ensure_equals("prepend changes order", root.children().front()->id(), "leading");
    ensure("parent assigned", root.children().front()->parent() == &root);
    root.clearChildren();
    ensure("clear removes children", root.children().empty());
}

template<> template<> void widgetObject::test<4>() {
    radia::ui::RecordingPaintContext recording;
    radia::ui::Style style;

    radia::ui::Label label("hello");
    label.setRect({1.f, 2.f, 30.f, 10.f});
    label.paint(recording, style, 1.f);
    ensure_equals("label emits box primitive", recording.count(radia::ui::PaintCommandKind::Box), 1U);
    ensure_equals("label emits text primitive", recording.count(radia::ui::PaintCommandKind::Text), 1U);
    const radia::ui::PaintCommand* text = recording.last(radia::ui::PaintCommandKind::Text);
    ensure("text primitive recorded", text != nullptr);
    ensure_equals("text primitive carries content", text->value, std::string("hello"));
    ensure_equals("text primitive carries measured run width", text->rect.w, 38.f);

    radia::ui::Icon icon("search");
    icon.setRect({4.f, 5.f, 16.f, 16.f});
    icon.paint(recording, style, 2.f);
    ensure_equals("icon emits box primitive", recording.count(radia::ui::PaintCommandKind::Box), 2U);
    ensure_equals("icon emits icon primitive", recording.count(radia::ui::PaintCommandKind::Icon), 1U);
    const radia::ui::PaintCommand* iconCommand = recording.last(radia::ui::PaintCommandKind::Icon);
    ensure("icon primitive recorded", iconCommand != nullptr);
    ensure_equals("icon primitive carries resource name", iconCommand->value, std::string("search"));
    ensure_equals("icon primitive carries device scale", iconCommand->scale, 2.f);
}

template<> template<> void widgetObject::test<5>() {
    radia::ui::System system;
    radia::ui::ResourceSnapshot resources;
    const char* kLocalization = "defaultLocale: en\nlocales: {en: {name: English, strings: {}}, ar: {name: العربية, direction: rtl, strings: {}}}\n";
    const char* kStyles =
        "panel { opacity: .5; effect: background-blur(3px), layer-blur(to right, 0px 0%, 4px 100%); } label { opacity: .5; text-align: start; } icon { size: 16px; }";
    resources.add("localization.yaml", kLocalization);
    resources.add("skin.radia", kStyles);
    resources.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M2 2 L22 22\"/></svg>");
    radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(std::move(resources));
    ensure("paint resources compile", prepared.ok());
    system.publish(prepared.generation);
    ensure("RTL paint locale selected", system.setLocale("ar"));

    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    surface->setViewport(100.f, 100.f);
    auto panel = std::make_unique<radia::ui::Panel>();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto label = std::make_unique<radia::ui::Label>("hello");
    label->setRect({0.f, 20.f, 30.f, 10.f});
    panel->addChild(std::move(label));
    auto icon = std::make_unique<radia::ui::Icon>("search");
    icon->setRect({0.f, 0.f, 16.f, 16.f});
    panel->addChild(std::move(icon));
    surface->root().addChild(std::move(panel));

    radia::ui::RecordingPaintContext recording;
    surface->paint(recording, 2.f);
    ensure_equals("Surface begins one paint frame", recording.count(radia::ui::PaintCommandKind::BeginFrame), 1U);
    ensure_equals("Surface ends one paint frame", recording.count(radia::ui::PaintCommandKind::EndFrame), 1U);
    ensure_equals("Surface begins one composed effect scope", recording.count(radia::ui::PaintCommandKind::BeginEffects), 1U);
    ensure_equals("Surface ends one composed effect scope", recording.count(radia::ui::PaintCommandKind::EndEffects), 1U);
    ensure_equals("Surface traverses text widget", recording.count(radia::ui::PaintCommandKind::Text), 1U);
    ensure_equals("Surface emits compiled icon identifier", recording.count(radia::ui::PaintCommandKind::Icon), 1U);
    const radia::ui::PaintCommand* textCommand = recording.last(radia::ui::PaintCommandKind::Text);
    const radia::ui::PaintCommand* iconCommand = recording.last(radia::ui::PaintCommandKind::Icon);
    const radia::ui::PaintCommand* effectCommand = recording.last(radia::ui::PaintCommandKind::BeginEffects);
    ensure("Surface records text, icon, and effect primitives", textCommand && iconCommand && effectCommand);
    ensure_equals("effect scope preserves device scale", effectCommand->scale, 2.f);
    ensure_equals("effect scope preserves the composed list", effectCommand->style.effects.size(), 2U);
    ensure_equals("Surface preserves icon resource name", iconCommand->value, std::string("search"));
    ensure_equals("ancestor opacity multiplies child opacity", textCommand->style.textColor.a, .25f);
    ensure_equals("Text Host consumes resolved RTL alignment before painting the run", static_cast<int>(textCommand->style.textAlign),
                  static_cast<int>(radia::ui::TextAlign::Left));
    ensure_equals("resolved RTL alignment positions the run against the right edge", textCommand->rect.x, -8.f);
    ensure_equals("Surface primitive order begins with frame", static_cast<int>(recording.commands().front().kind),
                  static_cast<int>(radia::ui::PaintCommandKind::BeginFrame));
    ensure_equals("Surface primitive order ends with frame", static_cast<int>(recording.commands().back().kind),
                  static_cast<int>(radia::ui::PaintCommandKind::EndFrame));
    ensure("text is emitted before the later icon", textCommand < iconCommand);
    ensure("successful Surface paint clears invalidation", !surface->needsPaint());
}

template<> template<> void widgetObject::test<6>() {
    std::vector<radia::ui::InlineContentNode> nodes;
    nodes.push_back(radia::ui::InlineContentNode::text("alpha"));
    nodes.push_back(radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::B, {radia::ui::InlineContentNode::text("beta")}));
    nodes.push_back(radia::ui::InlineContentNode::br());
    nodes.push_back(radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::I, {radia::ui::InlineContentNode::text("gamma")}));

    radia::ui::Text text("initial");
    ensure_equals("Text accepts a string literal", text.content().nodes()[0].value(), std::string("initial"));
    text.setContent(radia::ui::InlineContent(std::move(nodes)));
    text.setRect({0.f, 0.f, 100.f, 40.f});
    radia::ui::Style style;
    style.fontSize = 10.f;
    const radia::ui::FixedTextMetrics metrics(.5f, .75f);
    const radia::ui::Vec2 measured = text.intrinsicSize(radia::ui::StyleSheet(), style, metrics);
    ensure_equals("inline measurement uses widest explicit line", measured.x, 55.f);
    ensure_equals("inline measurement accumulates line heights", measured.y, 20.f);

    style.verticalAlign = radia::ui::VerticalAlign::Bottom;
    radia::ui::RecordingPaintContext recording(metrics);
    text.paint(recording, style, 1.f);
    ensure_equals("Text paints its own box", recording.count(radia::ui::PaintCommandKind::Box), 1U);
    ensure_equals("each inline run emits one text primitive", recording.count(radia::ui::PaintCommandKind::Text), 3U);
    const auto& commands = recording.commands();
    ensure_equals("plain run is first", commands[1].value, std::string("alpha"));
    ensure_equals("text block starts at the top regardless of container alignment", commands[1].rect.y, 30.f);
    ensure_equals("text runs preserve the authored content vertical alignment", static_cast<int>(commands[1].style.verticalAlign),
                  static_cast<int>(radia::ui::VerticalAlign::Bottom));
    ensure_equals("bold run follows on the same line", commands[2].value, std::string("beta"));
    ensure_equals("B content strengthens the run style", commands[2].style.fontWeight, static_cast<U16>(700));
    ensure_equals("Br advances the following run downward", commands[3].rect.y, 20.f);
    ensure("I content emphasizes the following run", commands[3].style.fontItalic);

    radia::ui::Style trackedStyle;
    trackedStyle.fontSize = 10.f;
    trackedStyle.letterSpacing = radia::ui::Length{2.f};
    radia::ui::Text tracked;
    tracked.setContent(radia::ui::InlineContent({
        radia::ui::InlineContentNode::text("a"),
        radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::B, {radia::ui::InlineContentNode::text("b")}),
    }));
    tracked.setRect({0.f, 0.f, 100.f, 10.f});
    ensure_equals("letter spacing contributes across inline run boundaries", tracked.intrinsicSize(radia::ui::StyleSheet(), trackedStyle, metrics).x,
                  15.f);
    radia::ui::RecordingPaintContext trackedRecording(metrics);
    tracked.paint(trackedRecording, trackedStyle, 1.f);
    ensure_equals("inline paint preserves the boundary letter spacing", trackedRecording.commands()[2].rect.x, 7.f);
    trackedStyle.textColor = radia::ui::Color(1.f, 0.f, 0.f);
    radia::ui::RecordingPaintContext recolored(metrics);
    tracked.paint(recolored, trackedStyle, 1.f);
    ensure_equals("cached inline runs refresh their text color", recolored.commands()[1].style.textColor.r, 1.f);

    radia::ui::Text trackedCombining;
    trackedCombining.setContent(radia::ui::InlineContent({
        radia::ui::InlineContentNode::text("a"),
        radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::I, {radia::ui::InlineContentNode::text("\xCC\x81")}),
        radia::ui::InlineContentNode::text("b"),
    }));
    trackedCombining.setRect({0.f, 0.f, 100.f, 10.f});
    ensure_equals("fixed metrics count grapheme rather than codepoint gaps",
                  trackedCombining.intrinsicSize(radia::ui::StyleSheet(), trackedStyle, metrics).x, 17.f);
    radia::ui::RecordingPaintContext trackedCombiningRecording(metrics);
    trackedCombining.paint(trackedCombiningRecording, trackedStyle, 1.f);
    ensure_equals("letter spacing does not split a cross-style grapheme", trackedCombiningRecording.commands()[2].rect.x, 5.f);

    style.textAlign = radia::ui::TextAlign::Center;
    radia::ui::RecordingPaintContext centered(metrics);
    text.paint(centered, style, 1.f);
    ensure_equals("text-align center positions the complete first line", centered.commands()[1].rect.x, 22.5f);

    std::vector<radia::ui::InlineContentNode> rtlNodes;
    rtlNodes.push_back(radia::ui::InlineContentNode::text("الأول"));
    rtlNodes.push_back(radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::B, {radia::ui::InlineContentNode::text("الثاني")}));
    radia::ui::Text rtlText;
    rtlText.setContent(radia::ui::InlineContent(std::move(rtlNodes)));
    rtlText.setRect({0.f, 0.f, 100.f, 20.f});
    style.direction = radia::ui::LayoutDirection::RightToLeft;
    radia::ui::RecordingPaintContext rtl(metrics);
    rtlText.paint(rtl, style, 1.f);
    ensure_equals("RTL inline spans are emitted in visual order", rtl.commands()[1].value, std::string("الثاني"));
    ensure_equals("RTL visual order retains the preceding logical span", rtl.commands()[2].value, std::string("الأول"));

    std::vector<radia::ui::InlineContentNode> mixedNodes;
    mixedNodes.push_back(radia::ui::InlineContentNode::text("الأول 123"));
    mixedNodes.push_back(radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::B, {radia::ui::InlineContentNode::text(" الثاني")}));
    radia::ui::Text mixed;
    mixed.setContent(radia::ui::InlineContent(std::move(mixedNodes)));
    mixed.setRect({0.f, 0.f, 100.f, 20.f});
    radia::ui::RecordingPaintContext mixedRtl(metrics);
    mixed.paint(mixedRtl, style, 1.f);
    ensure_equals("mixed bidi content splits at embedding and style boundaries", mixedRtl.count(radia::ui::PaintCommandKind::Text), 3U);
    ensure("rightmost logical style run is painted first in visual order", mixedRtl.commands()[1].style.fontWeight == 700);
    ensure_equals("embedded LTR segment keeps its own visual run", mixedRtl.commands()[2].value, std::string("123"));
    ensure("leading logical RTL segment is painted last", mixedRtl.commands()[3].style.fontWeight == 400);

    radia::ui::Text struck;
    struck.setContent(
        radia::ui::InlineContent({radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::S, {radia::ui::InlineContentNode::text("obsolete")})}));
    struck.setRect({0.f, 0.f, 100.f, 20.f});
    radia::ui::RecordingPaintContext struckRecording(metrics);
    struck.paint(struckRecording, style, 1.f);
    ensure("S content marks only its text run for strike-through", struckRecording.commands()[1].style.fontStrike);
}

template<> template<> void widgetObject::test<7>() {
    radia::ui::StyleSheet stylesheet;
    const char* kKbd =
        "text { font-size: 10px; line-height: 10px; } kbd { gap: 2px; padding: 1px; background-color: #111111ff; > kbd { padding: 1px 2px; background-color: #222222ff; } }";
    ensure("Kbd paint styles compile", stylesheet.loadRadia(kKbd).ok());

    radia::ui::Text projected;
    projected.setContent(radia::ui::InlineContent({
        radia::ui::InlineContentNode::text("Press"),
        radia::ui::InlineContentNode::kbd("example", radia::ui::KeybindingPresentation{{"Ctrl", "Shift", "F"}}),
    }));
    ensure_equals("Kbd preserves authored surrounding spacing while separating chord keys", projected.text(), "PressCtrl Shift F");

    radia::ui::Surface surface(stylesheet);
    surface.setViewport(200.f, 40.f);
    auto text = std::make_unique<radia::ui::Text>();
    text->setContent(radia::ui::InlineContent({radia::ui::InlineContentNode::kbd("example", radia::ui::KeybindingPresentation{{"Ctrl", "Shift", "F"}})}));
    surface.mount(std::move(text));

    radia::ui::RecordingPaintContext recording;
    surface.paint(recording);
    ensure_equals("Kbd paints its chord and three independent key boxes", recording.count(radia::ui::PaintCommandKind::Box), 5U);
    ensure_equals("each generated Kbd key paints independently", recording.count(radia::ui::PaintCommandKind::Text), 3U);

    std::vector<std::string> keys;
    std::vector<radia::ui::Color> boxColors;
    for (const radia::ui::PaintCommand& command : recording.commands()) {
        if (command.kind == radia::ui::PaintCommandKind::Text) keys.push_back(command.value);
        if (command.kind == radia::ui::PaintCommandKind::Box && command.style.backgroundColor.a > 0.f) boxColors.push_back(command.style.backgroundColor);
    }
    ensure_equals("primary chord exposes three key labels", keys.size(), 3U);
    ensure_equals("first Kbd key is Ctrl", keys[0], std::string("Ctrl"));
    ensure_equals("second Kbd key is Shift", keys[1], std::string("Shift"));
    ensure_equals("third Kbd key is F", keys[2], std::string("F"));
    ensure_equals("one styled chord surrounds three styled keys", boxColors.size(), 4U);
    ensure("outer and nested Kbd selectors produce distinct surfaces", boxColors.front().r != boxColors.back().r);
}

template<> template<> void widgetObject::test<8>() {
    const radia::ui::FixedTextMetrics metrics(.5f, .5f);
    radia::ui::Style style;
    ensure_equals("text wraps by default like Radia", static_cast<int>(style.textWrap), static_cast<int>(radia::ui::TextWrap::Wrap));

    class ShapedRunMetrics final : public radia::ui::TextMetrics {
    public:
        radia::ui::Vec2 measureText(const std::string& value, const radia::ui::Style&) const override {
            if (value == "Radia UI Demo") return {50.f, 10.f};
            if (value == "Radia UI") return {28.f, 10.f};
            if (value == " ") return {6.f, 10.f};
            if (value == "Demo") return {20.f, 10.f};
            return {0.f, 10.f};
        }
    } shapedMetrics;
    radia::ui::Text naturallySized("Radia UI Demo");
    naturallySized.setRect({0.f, 0.f, 50.f, 10.f});
    radia::ui::RecordingPaintContext naturalWrap(shapedMetrics);
    naturallySized.paint(naturalWrap, style, 1.f);
    ensure_equals("a shaped line that fits is not rewrapped after token measurement", naturalWrap.count(radia::ui::PaintCommandKind::Text), 1U);
    ensure_equals("the fitting shaped line remains intact", naturalWrap.commands()[1].value, std::string("Radia UI Demo"));

    style.fontSize = 10.f;
    style.textWrap = radia::ui::TextWrap::NoWrap;
    style.textOverflow = radia::ui::TextOverflow::EllipsisCenter;
    style.overflowX = radia::ui::Overflow::Hidden;

    radia::ui::Text inventory("abcdefghij");
    inventory.setRect({0.f, 0.f, 35.f, 10.f});
    radia::ui::RecordingPaintContext centered(metrics);
    inventory.paint(centered, style, 1.f);
    const std::string centeredText = paintedText(centered);
    ensure_equals("center ellipsis preserves a balanced prefix and suffix", centeredText, std::string("abc\xE2\x80\xA6hij"));

    style.textOverflow = radia::ui::TextOverflow::Ellipsis;
    radia::ui::RecordingPaintContext ended(metrics);
    inventory.paint(ended, style, 1.f);
    const std::string endedText = paintedText(ended);
    ensure_equals("end ellipsis preserves the visual start", endedText, std::string("abcdef\xE2\x80\xA6"));

    style.direction = radia::ui::LayoutDirection::RightToLeft;
    radia::ui::Text rtlInventory("ابتثجحخدذر");
    rtlInventory.setRect({0.f, 0.f, 35.f, 10.f});
    radia::ui::RecordingPaintContext rtlEnded(metrics);
    rtlInventory.paint(rtlEnded, style, 1.f);
    ensure_equals("RTL end ellipsis paints at the visual left edge", rtlEnded.commands()[1].value, std::string("\xE2\x80\xA6"));
    ensure_equals("RTL end ellipsis retains the logical prefix", rtlEnded.commands()[2].value, std::string("ابتثجح"));
    style.direction = radia::ui::LayoutDirection::LeftToRight;

    radia::ui::Text combining("a\u0301bcdef");
    combining.setRect({0.f, 0.f, 20.f, 10.f});
    radia::ui::RecordingPaintContext combiningEnded(metrics);
    combining.paint(combiningEnded, style, 1.f);
    const std::string combiningText = paintedText(combiningEnded);
    ensure_equals("end ellipsis does not split a combining sequence", combiningText, std::string("a\u0301b\u2026"));

    radia::ui::Text styledCombining;
    styledCombining.setContent(radia::ui::InlineContent({
        radia::ui::InlineContentNode::text("a"),
        radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::I, {radia::ui::InlineContentNode::text("\xCC\x81")}),
        radia::ui::InlineContentNode::text("bcdef"),
    }));
    styledCombining.setRect({0.f, 0.f, 10.f, 10.f});
    radia::ui::RecordingPaintContext styledCombiningEnded(metrics);
    styledCombining.paint(styledCombiningEnded, style, 1.f);
    const std::string styledCombiningText = paintedText(styledCombiningEnded);
    ensure_equals("ellipsis does not split a grapheme across inline styles", styledCombiningText, std::string("\xE2\x80\xA6"));

    radia::ui::Text family("\U0001F468\u200D\U0001F469\u200D\U0001F467ABC");
    family.setRect({0.f, 0.f, 35.f, 10.f});
    radia::ui::RecordingPaintContext familyEnded(metrics);
    family.paint(familyEnded, style, 1.f);
    const std::string familyText = paintedText(familyEnded);
    ensure_equals("end ellipsis does not split an emoji ZWJ sequence", familyText, std::string("\U0001F468\u200D\U0001F469\u200D\U0001F467A\u2026"));

    style.textOverflow = radia::ui::TextOverflow::Clip;
    radia::ui::Text clipped("abc");
    clipped.setRect({0.f, 0.f, 2.f, 10.f});
    radia::ui::RecordingPaintContext clippedRecording(metrics);
    clipped.paint(clippedRecording, style, 1.f);
    ensure_equals("clip preserves the full run for the overflow scissor", clippedRecording.commands()[1].value, std::string("abc"));

    style.textWrap = radia::ui::TextWrap::Wrap;
    radia::ui::Text wrapped("alpha beta");
    wrapped.setRect({0.f, 0.f, 30.f, 20.f});
    radia::ui::RecordingPaintContext wrapping(metrics);
    wrapped.paint(wrapping, style, 1.f);
    ensure_equals("text-wrap creates two painted lines", wrapping.count(radia::ui::PaintCommandKind::Text), 2U);
    ensure_equals("first wrapped line ends before the breaking space", wrapping.commands()[1].value, std::string("alpha"));
    ensure_equals("second wrapped line begins with the next word", wrapping.commands()[2].value, std::string("beta"));

    radia::ui::Text zeroWidthWrapped("alpha beta");
    zeroWidthWrapped.setRect({0.f, 0.f, 0.f, 20.f});
    radia::ui::RecordingPaintContext zeroWidthWrapping(metrics);
    zeroWidthWrapped.paint(zeroWidthWrapping, style, 1.f);
    ensure_equals("a zero-width constraint preserves word wrap opportunities", zeroWidthWrapping.count(radia::ui::PaintCommandKind::Text), 2U);

    radia::ui::Text multiword("alpha beta gamma");
    multiword.setRect({0.f, 0.f, 50.f, 20.f});
    radia::ui::RecordingPaintContext shapedWrapping(metrics);
    multiword.paint(shapedWrapping, style, 1.f);
    ensure_equals("wrapping shapes each visual line instead of separate word tokens", shapedWrapping.count(radia::ui::PaintCommandKind::Text), 2U);
    ensure_equals("the first visual line retains its internal space", shapedWrapping.commands()[1].value, std::string("alpha beta"));
    ensure_equals("the following word starts the second visual line", shapedWrapping.commands()[2].value, std::string("gamma"));

    radia::ui::Text cjk("你好世界");
    cjk.setRect({0.f, 0.f, 10.f, 20.f});
    radia::ui::RecordingPaintContext cjkWrapping(metrics);
    cjk.paint(cjkWrapping, style, 1.f);
    ensure_equals("Unicode line-break opportunities wrap CJK text", cjkWrapping.count(radia::ui::PaintCommandKind::Text), 2U);

    radia::ui::Text styledWord;
    styledWord.setContent(radia::ui::InlineContent({
        radia::ui::InlineContentNode::text("al"),
        radia::ui::InlineContentNode::container(radia::ui::InlineContentKind::B, {radia::ui::InlineContentNode::text("pha")}),
    }));
    styledWord.setRect({0.f, 0.f, 15.f, 20.f});
    radia::ui::RecordingPaintContext styledWordWrapping(metrics);
    styledWord.paint(styledWordWrapping, style, 1.f);
    ensure_equals("inline styling does not create a line-break opportunity", styledWordWrapping.commands()[1].rect.y,
                  styledWordWrapping.commands()[2].rect.y);

    class VariableTextMetrics final : public radia::ui::TextMetrics {
    public:
        radia::ui::Vec2 measureText(const std::string& value, const radia::ui::Style&) const override {
            if (value.empty()) return {0.f, 10.f};
            if (value == "W") return {20.f, 10.f};
            if (value == "\xE2\x80\xA6") return {5.f, 10.f};
            return {
                static_cast<float>(value.size()) * 5.f,
                10.f,
            };
        }
    } variableMetrics;
    style.textWrap = radia::ui::TextWrap::NoWrap;
    style.textOverflow = radia::ui::TextOverflow::EllipsisCenter;
    radia::ui::Text asymmetric("Wabc");
    asymmetric.setRect({0.f, 0.f, 10.f, 10.f});
    radia::ui::RecordingPaintContext asymmetricCenter(variableMetrics);
    asymmetric.paint(asymmetricCenter, style, 1.f);
    const std::string asymmetricText = paintedText(asymmetricCenter);
    ensure_equals("center ellipsis tries the suffix when the prefix is too wide", asymmetricText, std::string("\u2026c"));

    style.textWrap = radia::ui::TextWrap::Wrap;
    style.textOverflow = radia::ui::TextOverflow::Clip;
    style.width = radia::ui::Dimension::fromPixels(30.f);
    ensure_equals("an explicit wrapping width contributes both lines to intrinsic height",
                  wrapped.intrinsicSize(radia::ui::StyleSheet(), style, metrics).y, 20.f);

    style.letterSpacing = radia::ui::Length{2.f};
    ensure_equals("pixel letter spacing contributes between characters only", metrics.measureText("abc", style).x, 19.f);
    style.letterSpacing = radia::ui::Length{0.f, .5f};
    ensure_equals("percentage letter spacing uses the fixed font's space advance", metrics.measureText("abc", style).x, 20.f);
    style.letterSpacing = {};
    style.wordSpacing = radia::ui::Length{3.f};
    ensure_equals("word spacing contributes once per word separator", metrics.measureText("a b", style).x, 18.f);
    style.wordSpacing = radia::ui::Length{0.f, .5f};
    ensure_equals("percentage word spacing resolves against font-size", metrics.measureText("a b", style).x, 20.f);
    style.wordSpacing = radia::ui::Length{3.f};
    ensure_equals("word spacing recognizes Unicode space separators", metrics.measureText("a\u2003b", style).x, 18.f);
}

template<> template<> void widgetObject::test<9>() {
    radia::ui::Button button;

    button.setDisabled(true);
    ensure("setDisabled(true) disables the Widget", button.disabled());
    button.setDisabled(false);
    ensure("setDisabled(false) enables the Widget", !button.disabled());

    button.setHidden(true);
    ensure("setHidden(true) selects Hidden", button.visibility() == radia::ui::Visibility::Hidden);
    button.setHidden(false);
    ensure("setHidden(false) selects Visible", button.visibility() == radia::ui::Visibility::Visible);
}

template<> template<> void widgetObject::test<10>() {
    radia::ui::StyleSheet styleSheet;
    ensure("switch intrinsic layout stylesheet compiles",
           styleSheet.loadRadia("panel { flow: row; } switch { width: 40px; height: 20px; background-color: #000000ff; }").ok());
    radia::ui::Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = std::make_unique<radia::ui::Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = std::make_unique<radia::ui::Switch>();
    radia::ui::Switch* target = control.get();
    panel->addChild(std::move(control));
    surface.mount(std::move(panel));

    surface.updateLayout();
    const float uncheckedLeft = target->thumb()->rect().left();
    surface.pointerDown({{5.f, 10.f}, radia::ui::PointerButton::Left});
    surface.pointerUp({{5.f, 10.f}, radia::ui::PointerButton::Left});
    surface.updateLayout();

    ensure("pointer activation checks the switch", target->checked());
    ensure("pointer activation repositions the switch thumb", target->thumb()->rect().left() > uncheckedLeft);
}
} // namespace tut
