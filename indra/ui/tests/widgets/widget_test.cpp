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
#include <gtest/gtest.h>
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

namespace {
using radia::ui::Button;
using radia::ui::Color;
using radia::ui::Dimension;
using radia::ui::FixedTextMetrics;
using radia::ui::fixedTextMetrics;
using radia::ui::Icon;
using radia::ui::InlineContent;
using radia::ui::InlineContentKind;
using radia::ui::InlineContentNode;
using radia::ui::KeybindingPresentation;
using radia::ui::Label;
using radia::ui::LayoutDirection;
using radia::ui::Length;
using radia::ui::Overflow;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::Panel;
using radia::ui::PointerButton;
using radia::ui::RecordingPaintContext;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::Switch;
using radia::ui::System;
using radia::ui::Text;
using radia::ui::TextAlign;
using radia::ui::TextMetrics;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::Vec2;
using radia::ui::VerticalAlign;
using radia::ui::Visibility;
using radia::ui::Widget;
using radia::ui::WidgetRef;
} // namespace

namespace {
std::string paintedText(const RecordingPaintContext& recording) {
    std::string text;
    for (const PaintCommand& command : recording.commands())
        if (command.kind == PaintCommandKind::Text) text += command.textOrIconName;
    return text;
}
} // namespace

TEST(WidgetTest, WidgetReferencesExpireAfterUnmount) {
    Panel root;
    auto button = std::make_unique<Button>();
    WidgetRef<Button> reference(button.get());
    root.addChild(std::move(button));
    ASSERT_TRUE(reference);
    root.clearChildren();
    EXPECT_FALSE(reference);
}

TEST(WidgetTest, DisabledButtonsDoNotActivate) {
    Button button;
    int activations = 0;
    button.setOnActivate([&](Widget&) { ++activations; });
    button.activate();
    button.setDisabled(true).activate();
    EXPECT_EQ(activations, 1);
    EXPECT_TRUE(button.focusable());
}

TEST(WidgetTest, ChildInsertionMaintainsOrderAndOwnership) {
    Panel root;
    auto first = std::make_unique<Button>();
    first->setId("first");
    root.addChild(std::move(first));
    auto leading = std::make_unique<Button>();
    leading->setId("leading");
    root.prependChild(std::move(leading));
    EXPECT_EQ(root.children().front()->id(), "leading");
    EXPECT_EQ(root.children().front()->parent(), &root);
    root.clearChildren();
    EXPECT_TRUE(root.children().empty());
}

TEST(WidgetPaintTest, RecordsLabelAndIconPrimitives) {
    RecordingPaintContext recording;
    Style style;

    Label label("hello");
    label.setRect({1.f, 2.f, 30.f, 10.f});
    label.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 1U);
    const PaintCommand* text = recording.last(PaintCommandKind::Text);
    ASSERT_NE(text, nullptr);
    EXPECT_EQ(text->textOrIconName, "hello");
    EXPECT_EQ(text->rect.w, 38.f);

    Icon icon("search");
    icon.setRect({4.f, 5.f, 16.f, 16.f});
    icon.paint(recording, style, 2.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 2U);
    EXPECT_EQ(recording.count(PaintCommandKind::Icon), 1U);
    const PaintCommand* iconCommand = recording.last(PaintCommandKind::Icon);
    ASSERT_NE(iconCommand, nullptr);
    EXPECT_EQ(iconCommand->textOrIconName, "search");
    EXPECT_EQ(iconCommand->scale, 2.f);
}

TEST(WidgetPaintTest, PaintsCompiledResourcesWithLocaleAndEffects) {
    System system;
    ResourceSnapshot resources;
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {name: English, strings: {}}, "
                                     "ar: {name: العربية, direction: rtl, strings: {}}}\n";
    constexpr char kStyles[] = "panel { opacity: .5; effect: background-blur(3px), layer-blur(to right, 0px 0%, 4px 100%); } "
                               "label { opacity: .5; text-align: start; } icon { size: 16px; }";
    constexpr char kSearchIconSvg[] = "<svg viewBox=\"0 0 24 24\">"
                                      "<path d=\"M2 2 L22 22\"/></svg>";
    resources.add("localization.yaml", kLocalization);
    resources.add("skin.radia", kStyles);
    resources.add("resources/icons/search.svg", kSearchIconSvg);
    SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(resources));
    ASSERT_TRUE(prepared.ok());
    system.publish(prepared.generation);
    ASSERT_TRUE(system.setLocale("ar"));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(100.f, 100.f);
    auto panel = std::make_unique<Panel>();
    panel->setRect({0.f, 0.f, 100.f, 100.f});
    auto label = std::make_unique<Label>("hello");
    label->setRect({0.f, 20.f, 30.f, 10.f});
    panel->addChild(std::move(label));
    auto icon = std::make_unique<Icon>("search");
    icon->setRect({0.f, 0.f, 16.f, 16.f});
    panel->addChild(std::move(icon));
    surface->root().addChild(std::move(panel));

    RecordingPaintContext recording;
    surface->paint(recording, 2.f);
    EXPECT_EQ(recording.count(PaintCommandKind::BeginFrame), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::EndFrame), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::BeginEffects), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::EndEffects), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 1U);
    EXPECT_EQ(recording.count(PaintCommandKind::Icon), 1U);
    const PaintCommand* textCommand = recording.last(PaintCommandKind::Text);
    const PaintCommand* iconCommand = recording.last(PaintCommandKind::Icon);
    const PaintCommand* effectCommand = recording.last(PaintCommandKind::BeginEffects);
    ASSERT_NE(textCommand, nullptr);
    ASSERT_NE(iconCommand, nullptr);
    ASSERT_NE(effectCommand, nullptr);
    EXPECT_EQ(effectCommand->scale, 2.f);
    EXPECT_EQ(effectCommand->style.effects.size(), 2U);
    EXPECT_EQ(iconCommand->textOrIconName, "search");
    EXPECT_EQ(textCommand->style.textColor.a, .25f);
    EXPECT_EQ(static_cast<int>(textCommand->style.textAlign), static_cast<int>(TextAlign::Left));
    EXPECT_EQ(textCommand->rect.x, -8.f);
    EXPECT_EQ(static_cast<int>(recording.commands().front().kind), static_cast<int>(PaintCommandKind::BeginFrame));
    EXPECT_EQ(static_cast<int>(recording.commands().back().kind), static_cast<int>(PaintCommandKind::EndFrame));
    EXPECT_LT(textCommand, iconCommand);
    EXPECT_FALSE(surface->needsPaint());
}

TEST(TextWidgetTest, MeasuresAndPaintsStyledInlineContent) {
    std::vector<InlineContentNode> nodes;
    nodes.push_back(InlineContentNode::text("alpha"));
    nodes.push_back(InlineContentNode::container(InlineContentKind::B, {InlineContentNode::text("beta")}));
    nodes.push_back(InlineContentNode::br());
    nodes.push_back(InlineContentNode::container(InlineContentKind::I, {InlineContentNode::text("gamma")}));

    Text text("initial");
    EXPECT_EQ(text.content().nodes()[0].value(), "initial");
    text.setContent(InlineContent(std::move(nodes)));
    text.setRect({0.f, 0.f, 100.f, 40.f});
    Style style;
    style.fontSize = 10.f;
    const FixedTextMetrics metrics(.5f, .75f);
    const Vec2 measured = text.intrinsicSize(StyleSheet(), style, metrics);
    EXPECT_EQ(measured.x, 55.f);
    EXPECT_EQ(measured.y, 20.f);

    style.verticalAlign = VerticalAlign::Bottom;
    RecordingPaintContext recording(metrics);
    text.paint(recording, style, 1.f);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
    ASSERT_EQ(recording.count(PaintCommandKind::Text), 3U);
    const auto& commands = recording.commands();
    ASSERT_GE(commands.size(), 4U);
    EXPECT_EQ(commands[1].textOrIconName, "alpha");
    EXPECT_EQ(commands[1].rect.y, 30.f);
    EXPECT_EQ(static_cast<int>(commands[1].style.verticalAlign), static_cast<int>(VerticalAlign::Bottom));
    EXPECT_EQ(commands[2].textOrIconName, "beta");
    EXPECT_EQ(commands[2].style.fontWeight, static_cast<U16>(700));
    EXPECT_EQ(commands[3].rect.y, 20.f);
    EXPECT_TRUE(commands[3].style.fontItalic);

    style.textAlign = TextAlign::Center;
    RecordingPaintContext centered(metrics);
    text.paint(centered, style, 1.f);
    ASSERT_GE(centered.commands().size(), 2U);
    EXPECT_EQ(centered.commands()[1].rect.x, 22.5f);
}

TEST(TextWidgetTest, PreservesSpacingAndGraphemeBoundariesAcrossInlineStyles) {
    const FixedTextMetrics metrics(.5f, .75f);
    Style style;
    style.fontSize = 10.f;
    style.letterSpacing = Length{2.f};

    Text tracked;
    tracked.setContent(InlineContent({
        InlineContentNode::text("a"),
        InlineContentNode::container(InlineContentKind::B, {InlineContentNode::text("b")}),
    }));
    tracked.setRect({0.f, 0.f, 100.f, 10.f});
    EXPECT_EQ(tracked.intrinsicSize(StyleSheet(), style, metrics).x, 15.f);

    RecordingPaintContext trackedRecording(metrics);
    tracked.paint(trackedRecording, style, 1.f);
    ASSERT_GE(trackedRecording.commands().size(), 3U);
    EXPECT_EQ(trackedRecording.commands()[2].rect.x, 7.f);

    style.textColor = Color(1.f, 0.f, 0.f);
    RecordingPaintContext recolored(metrics);
    tracked.paint(recolored, style, 1.f);
    ASSERT_GE(recolored.commands().size(), 2U);
    EXPECT_EQ(recolored.commands()[1].style.textColor.r, 1.f);

    Text trackedCombining;
    trackedCombining.setContent(InlineContent({
        InlineContentNode::text("a"),
        InlineContentNode::container(InlineContentKind::I, {InlineContentNode::text("\xCC\x81")}),
        InlineContentNode::text("b"),
    }));
    trackedCombining.setRect({0.f, 0.f, 100.f, 10.f});
    EXPECT_EQ(trackedCombining.intrinsicSize(StyleSheet(), style, metrics).x, 17.f);

    RecordingPaintContext trackedCombiningRecording(metrics);
    trackedCombining.paint(trackedCombiningRecording, style, 1.f);
    ASSERT_GE(trackedCombiningRecording.commands().size(), 3U);
    EXPECT_EQ(trackedCombiningRecording.commands()[2].rect.x, 5.f);
}

TEST(TextWidgetTest, PaintsBidirectionalInlineContentInVisualOrder) {
    const FixedTextMetrics metrics(.5f, .75f);
    Style style;
    style.fontSize = 10.f;
    style.direction = LayoutDirection::RightToLeft;

    Text rtlText;
    rtlText.setContent(InlineContent({
        InlineContentNode::text("الأول"),
        InlineContentNode::container(InlineContentKind::B, {InlineContentNode::text("الثاني")}),
    }));
    rtlText.setRect({0.f, 0.f, 100.f, 20.f});
    RecordingPaintContext rtl(metrics);
    rtlText.paint(rtl, style, 1.f);
    ASSERT_GE(rtl.commands().size(), 3U);
    EXPECT_EQ(rtl.commands()[1].textOrIconName, "الثاني");
    EXPECT_EQ(rtl.commands()[2].textOrIconName, "الأول");

    Text mixed;
    mixed.setContent(InlineContent({
        InlineContentNode::text("الأول 123"),
        InlineContentNode::container(InlineContentKind::B, {InlineContentNode::text(" الثاني")}),
    }));
    mixed.setRect({0.f, 0.f, 100.f, 20.f});
    RecordingPaintContext mixedRtl(metrics);
    mixed.paint(mixedRtl, style, 1.f);
    ASSERT_EQ(mixedRtl.count(PaintCommandKind::Text), 3U);
    ASSERT_GE(mixedRtl.commands().size(), 4U);
    EXPECT_EQ(mixedRtl.commands()[1].style.fontWeight, static_cast<U16>(700));
    EXPECT_EQ(mixedRtl.commands()[2].textOrIconName, "123");
    EXPECT_EQ(mixedRtl.commands()[3].style.fontWeight, static_cast<U16>(400));
}

TEST(TextWidgetTest, AppliesStrikeThroughOnlyToStruckRuns) {
    const FixedTextMetrics metrics(.5f, .75f);
    Style style;
    Text struck;
    struck.setContent(InlineContent({InlineContentNode::container(InlineContentKind::S, {InlineContentNode::text("obsolete")})}));
    struck.setRect({0.f, 0.f, 100.f, 20.f});
    RecordingPaintContext recording(metrics);
    struck.paint(recording, style, 1.f);
    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_TRUE(recording.commands()[1].style.fontStrike);
}

TEST(KbdWidgetTest, PaintsKeybindingAsStyledKeys) {
    StyleSheet stylesheet;
    constexpr char kKbd[] = "text { font-size: 10px; line-height: 10px; } "
                            "kbd { gap: 2px; padding: 1px; background-color: #111111ff; "
                            "> kbd { padding: 1px 2px; background-color: #222222ff; } }";
    ASSERT_TRUE(stylesheet.loadRadia(kKbd).ok());

    Text projected;
    projected.setContent(InlineContent({
        InlineContentNode::text("Press"),
        InlineContentNode::kbd("example", KeybindingPresentation{{"Ctrl", "Shift", "F"}}),
    }));
    EXPECT_EQ(projected.text(), "PressCtrl Shift F");

    Surface surface(stylesheet);
    surface.setViewport(200.f, 40.f);
    auto text = std::make_unique<Text>();
    text->setContent(InlineContent({InlineContentNode::kbd("example", KeybindingPresentation{{"Ctrl", "Shift", "F"}})}));
    surface.mount(std::move(text));

    RecordingPaintContext recording;
    surface.paint(recording);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 5U);
    EXPECT_EQ(recording.count(PaintCommandKind::Text), 3U);

    std::vector<std::string> keys;
    std::vector<Color> boxColors;
    for (const PaintCommand& command : recording.commands()) {
        if (command.kind == PaintCommandKind::Text) keys.push_back(command.textOrIconName);
        if (command.kind == PaintCommandKind::Box && command.style.backgroundColor.a > 0.f) boxColors.push_back(command.style.backgroundColor);
    }
    ASSERT_EQ(keys.size(), 3U);
    EXPECT_EQ(keys[0], "Ctrl");
    EXPECT_EQ(keys[1], "Shift");
    EXPECT_EQ(keys[2], "F");
    EXPECT_EQ(boxColors.size(), 4U);
    ASSERT_FALSE(boxColors.empty());
    EXPECT_NE(boxColors.front().r, boxColors.back().r);
}

TEST(TextWidgetTest, PreservesFittingShapedLines) {
    class ShapedRunMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string& value, const Style&) const override {
            if (value == "Radia UI Demo") return {50.f, 10.f};
            if (value == "Radia UI") return {28.f, 10.f};
            if (value == " ") return {6.f, 10.f};
            if (value == "Demo") return {20.f, 10.f};
            return {0.f, 10.f};
        }
    } shapedMetrics;

    Text naturallySized("Radia UI Demo");
    naturallySized.setRect({0.f, 0.f, 50.f, 10.f});
    RecordingPaintContext recording(shapedMetrics);
    naturallySized.paint(recording, Style{}, 1.f);

    ASSERT_EQ(recording.count(PaintCommandKind::Text), 1U);
    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].textOrIconName, "Radia UI Demo");
}

TEST(TextWidgetTest, EllipsizesAccordingToTextDirection) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::EllipsisCenter;
    style.overflowX = Overflow::Hidden;

    Text inventory("abcdefghij");
    inventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext centered(metrics);
    inventory.paint(centered, style, 1.f);
    EXPECT_EQ(paintedText(centered), "abc\xE2\x80\xA6hij");

    style.textOverflow = TextOverflow::Ellipsis;
    RecordingPaintContext ended(metrics);
    inventory.paint(ended, style, 1.f);
    EXPECT_EQ(paintedText(ended), "abcdef\xE2\x80\xA6");

    style.direction = LayoutDirection::RightToLeft;
    Text rtlInventory("ابتثجحخدذر");
    rtlInventory.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext rtlEnded(metrics);
    rtlInventory.paint(rtlEnded, style, 1.f);
    ASSERT_GE(rtlEnded.commands().size(), 3U);
    EXPECT_EQ(rtlEnded.commands()[1].textOrIconName, "\xE2\x80\xA6");
    EXPECT_EQ(rtlEnded.commands()[2].textOrIconName, "ابتثجح");
}

TEST(TextWidgetTest, PreservesGraphemeBoundariesWhenEllipsizing) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::Ellipsis;
    style.overflowX = Overflow::Hidden;

    Text combining("a\u0301bcdef");
    combining.setRect({0.f, 0.f, 20.f, 10.f});
    RecordingPaintContext combiningRecording(metrics);
    combining.paint(combiningRecording, style, 1.f);
    EXPECT_EQ(paintedText(combiningRecording), "a\u0301b\u2026");

    Text styledCombining;
    styledCombining.setContent(InlineContent({
        InlineContentNode::text("a"),
        InlineContentNode::container(InlineContentKind::I, {InlineContentNode::text("\xCC\x81")}),
        InlineContentNode::text("bcdef"),
    }));
    styledCombining.setRect({0.f, 0.f, 10.f, 10.f});
    RecordingPaintContext styledRecording(metrics);
    styledCombining.paint(styledRecording, style, 1.f);
    EXPECT_EQ(paintedText(styledRecording), "\xE2\x80\xA6");

    Text family("\U0001F468\u200D\U0001F469\u200D\U0001F467ABC");
    family.setRect({0.f, 0.f, 35.f, 10.f});
    RecordingPaintContext familyRecording(metrics);
    family.paint(familyRecording, style, 1.f);
    EXPECT_EQ(paintedText(familyRecording), "\U0001F468\u200D\U0001F469\u200D\U0001F467A\u2026");
}

TEST(TextWidgetTest, ClipsOverflowWithoutRewritingText) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::Clip;
    style.overflowX = Overflow::Hidden;

    Text clipped("abc");
    clipped.setRect({0.f, 0.f, 2.f, 10.f});
    RecordingPaintContext recording(metrics);
    clipped.paint(recording, style, 1.f);

    ASSERT_GE(recording.commands().size(), 2U);
    EXPECT_EQ(recording.commands()[1].textOrIconName, "abc");
}

TEST(TextWidgetTest, WrapsAtWordAndUnicodeBoundaries) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;
    style.textOverflow = TextOverflow::Clip;
    EXPECT_EQ(static_cast<int>(style.textWrap), static_cast<int>(TextWrap::Wrap));

    Text wrapped("alpha beta");
    wrapped.setRect({0.f, 0.f, 30.f, 20.f});
    RecordingPaintContext wrapping(metrics);
    wrapped.paint(wrapping, style, 1.f);
    ASSERT_EQ(wrapping.count(PaintCommandKind::Text), 2U);
    ASSERT_GE(wrapping.commands().size(), 3U);
    EXPECT_EQ(wrapping.commands()[1].textOrIconName, "alpha");
    EXPECT_EQ(wrapping.commands()[2].textOrIconName, "beta");

    Text zeroWidthWrapped("alpha beta");
    zeroWidthWrapped.setRect({0.f, 0.f, 0.f, 20.f});
    RecordingPaintContext zeroWidthWrapping(metrics);
    zeroWidthWrapped.paint(zeroWidthWrapping, style, 1.f);
    EXPECT_EQ(zeroWidthWrapping.count(PaintCommandKind::Text), 2U);

    Text multiword("alpha beta gamma");
    multiword.setRect({0.f, 0.f, 50.f, 20.f});
    RecordingPaintContext shapedWrapping(metrics);
    multiword.paint(shapedWrapping, style, 1.f);
    ASSERT_EQ(shapedWrapping.count(PaintCommandKind::Text), 2U);
    ASSERT_GE(shapedWrapping.commands().size(), 3U);
    EXPECT_EQ(shapedWrapping.commands()[1].textOrIconName, "alpha beta");
    EXPECT_EQ(shapedWrapping.commands()[2].textOrIconName, "gamma");

    Text cjk("你好世界");
    cjk.setRect({0.f, 0.f, 10.f, 20.f});
    RecordingPaintContext cjkWrapping(metrics);
    cjk.paint(cjkWrapping, style, 1.f);
    EXPECT_EQ(cjkWrapping.count(PaintCommandKind::Text), 2U);

    Text styledWord;
    styledWord.setContent(InlineContent({
        InlineContentNode::text("al"),
        InlineContentNode::container(InlineContentKind::B, {InlineContentNode::text("pha")}),
    }));
    styledWord.setRect({0.f, 0.f, 15.f, 20.f});
    RecordingPaintContext styledWordWrapping(metrics);
    styledWord.paint(styledWordWrapping, style, 1.f);
    ASSERT_GE(styledWordWrapping.commands().size(), 3U);
    EXPECT_EQ(styledWordWrapping.commands()[1].rect.y, styledWordWrapping.commands()[2].rect.y);

    style.textOverflow = TextOverflow::Clip;
    style.width = Dimension::fromPixels(30.f);
    EXPECT_EQ(wrapped.intrinsicSize(StyleSheet(), style, metrics).y, 20.f);
}

TEST(TextWidgetTest, UsesShapedWidthsForCenterEllipsis) {
    class VariableTextMetrics final : public TextMetrics {
    public:
        Vec2 measureText(const std::string& value, const Style&) const override {
            if (value.empty()) return {0.f, 10.f};
            if (value == "W") return {20.f, 10.f};
            if (value == "\xE2\x80\xA6") return {5.f, 10.f};
            return {
                static_cast<float>(value.size()) * 5.f,
                10.f,
            };
        }
    } variableMetrics;

    Style style;
    style.fontSize = 10.f;
    style.textWrap = TextWrap::NoWrap;
    style.textOverflow = TextOverflow::EllipsisCenter;
    style.overflowX = Overflow::Hidden;
    Text asymmetric("Wabc");
    asymmetric.setRect({0.f, 0.f, 10.f, 10.f});
    RecordingPaintContext recording(variableMetrics);
    asymmetric.paint(recording, style, 1.f);

    EXPECT_EQ(paintedText(recording), "\u2026c");
}

TEST(TextWidgetTest, AppliesLetterAndWordSpacingToMeasuredText) {
    const FixedTextMetrics metrics(.5f, .5f);
    Style style;
    style.fontSize = 10.f;

    style.letterSpacing = Length{2.f};
    EXPECT_EQ(metrics.measureText("abc", style).x, 19.f);
    style.letterSpacing = Length{0.f, .5f};
    EXPECT_EQ(metrics.measureText("abc", style).x, 20.f);

    style.letterSpacing = {};
    style.wordSpacing = Length{3.f};
    EXPECT_EQ(metrics.measureText("a b", style).x, 18.f);
    style.wordSpacing = Length{0.f, .5f};
    EXPECT_EQ(metrics.measureText("a b", style).x, 20.f);
    style.wordSpacing = Length{3.f};
    EXPECT_EQ(metrics.measureText("a\u2003b", style).x, 18.f);
}

TEST(WidgetTest, UpdatesDisabledAndVisibilityState) {
    Button button;

    button.setDisabled(true);
    EXPECT_TRUE(button.disabled());
    button.setDisabled(false);
    EXPECT_FALSE(button.disabled());

    button.setHidden(true);
    EXPECT_EQ(button.visibility(), Visibility::Hidden);
    button.setHidden(false);
    EXPECT_EQ(button.visibility(), Visibility::Visible);
}

TEST(SwitchWidgetTest, PointerActivationUpdatesStateAndThumb) {
    StyleSheet styleSheet;
    constexpr char kSwitchLayout[] = "panel { flow: row; } "
                                     "switch { width: 40px; height: 20px; background-color: #000000ff; }";
    ASSERT_TRUE(styleSheet.loadRadia(kSwitchLayout).ok());
    Surface surface(styleSheet);
    surface.setViewport(100.f, 20.f);
    auto panel = std::make_unique<Panel>();
    panel->setRect({0.f, 0.f, 100.f, 20.f});
    auto control = std::make_unique<Switch>();
    Switch* target = control.get();
    panel->addChild(std::move(control));
    surface.mount(std::move(panel));

    surface.updateLayout();
    const float uncheckedLeft = target->thumb()->rect().left();
    surface.pointerDown({{5.f, 10.f}, PointerButton::Left});
    surface.pointerUp({{5.f, 10.f}, PointerButton::Left});
    surface.updateLayout();

    EXPECT_TRUE(target->checked());
    EXPECT_GT(target->thumb()->rect().left(), uncheckedLeft);
}
