#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdtext.h"
#include "rduirecordingpaintcontext.h"
#include "rduiskincompiler.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"

namespace tut
{
    struct rduiwidget_data {};
    typedef test_group<rduiwidget_data> rduiwidget_test;
    typedef rduiwidget_test::object rduiwidget_object;
    rduiwidget_test rduiwidget_testcase("rduiwidget");

    template<> template<>
    void rduiwidget_object::test<1>()
    {
        rdui::Panel root;
        auto button = std::make_unique<rdui::Button>();
        rdui::WidgetRef<rdui::Button> reference(button.get());
        root.addChild(std::move(button));
        ensure("mounted widget reference remains valid", static_cast<bool>(reference));
        root.clearChildren();
        ensure("destroyed widget expires its reference", !reference);
    }

    template<> template<>
    void rduiwidget_object::test<2>()
    {
        rdui::Button button;
        int activations = 0;
        button.setOnActivate([&](rdui::Widget&) { ++activations; });
        button.activate();
        button.setDisabled(true).activate();
        ensure_equals("disabled button does not activate", activations, 1);
        ensure("button is focusable", button.focusable());
    }

    template<> template<>
    void rduiwidget_object::test<3>()
    {
        rdui::Panel root;
        auto first = std::make_unique<rdui::Button>();
        first->setId("first");
        root.addChild(std::move(first));
        auto leading = std::make_unique<rdui::Button>();
        leading->setId("leading");
        root.prependChild(std::move(leading));
        ensure_equals("prepend changes order", root.children().front()->id(), "leading");
        ensure("parent assigned", root.children().front()->parent() == &root);
        root.clearChildren();
        ensure("clear removes children", root.children().empty());
    }

    template<> template<>
    void rduiwidget_object::test<4>()
    {
        rdui::RecordingPaintContext recording;
        rdui::Style style;

        rdui::Label label("hello");
        label.setRect({1.f, 2.f, 30.f, 10.f});
        label.paint(recording, style, 1.f);
        ensure_equals("label emits box primitive", recording.count(rdui::PaintCommandKind::Box), 1U);
        ensure_equals("label emits text primitive", recording.count(rdui::PaintCommandKind::Text), 1U);
        const rdui::PaintCommand* text = recording.last(rdui::PaintCommandKind::Text);
        ensure("text primitive recorded", text != nullptr);
        ensure_equals("text primitive carries content", text->value, std::string("hello"));
        ensure_equals("text primitive carries measured run width", text->rect.w, 38.f);

        rdui::Icon icon("search");
        icon.setRect({4.f, 5.f, 16.f, 16.f});
        icon.paint(recording, style, 2.f);
        ensure_equals("icon emits box primitive", recording.count(rdui::PaintCommandKind::Box), 2U);
        ensure_equals("icon emits icon primitive", recording.count(rdui::PaintCommandKind::Icon), 1U);
        const rdui::PaintCommand* icon_command = recording.last(rdui::PaintCommandKind::Icon);
        ensure("icon primitive recorded", icon_command != nullptr);
        ensure_equals("icon primitive carries resource name", icon_command->value, std::string("search"));
        ensure_equals("icon primitive carries device scale", icon_command->scale, 2.f);
    }

    template<> template<>
    void rduiwidget_object::test<5>()
    {
        rdui::System system;
        rdui::ResourceSnapshot resources;
        resources.add("localization.yaml", R"YAML(
defaultLocale: en
locales:
  en:
    name: English
    strings: {}
  ar:
    name: العربية
    direction: rtl
    strings: {}
)YAML");
        resources.add("skin.radia", "panel { opacity: .5; effect: background-blur(3px), layer-blur(to right, 0px 0%, 4px 100%); }"
                                    "label { opacity: .5; text-align: start; } icon { size: 16px; }");
        resources.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M2 2 L22 22\"/></svg>");
        rdui::SkinGenerationPrepareResult prepared = rdui::SkinCompiler().prepare(std::move(resources));
        ensure("paint resources compile", prepared.ok());
        system.publish(prepared.generation);
        ensure("RTL paint locale selected", system.setLocale("ar"));

        std::unique_ptr<rdui::Surface> surface = system.createSurface(rdui::fixedTextMetrics());
        surface->setViewport(100.f, 100.f);
        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({0.f, 0.f, 100.f, 100.f});
        auto label = std::make_unique<rdui::Label>("hello");
        label->setRect({0.f, 20.f, 30.f, 10.f});
        panel->addChild(std::move(label));
        auto icon = std::make_unique<rdui::Icon>("search");
        icon->setRect({0.f, 0.f, 16.f, 16.f});
        panel->addChild(std::move(icon));
        surface->root().addChild(std::move(panel));

        rdui::RecordingPaintContext recording;
        surface->paint(recording, 2.f);
        ensure_equals("Surface begins one paint frame", recording.count(rdui::PaintCommandKind::BeginFrame), 1U);
        ensure_equals("Surface ends one paint frame", recording.count(rdui::PaintCommandKind::EndFrame), 1U);
        ensure_equals("Surface begins one composed effect scope", recording.count(rdui::PaintCommandKind::BeginEffects), 1U);
        ensure_equals("Surface ends one composed effect scope", recording.count(rdui::PaintCommandKind::EndEffects), 1U);
        ensure_equals("Surface traverses text widget", recording.count(rdui::PaintCommandKind::Text), 1U);
        ensure_equals("Surface emits compiled icon identifier", recording.count(rdui::PaintCommandKind::Icon), 1U);
        const rdui::PaintCommand* text_command = recording.last(rdui::PaintCommandKind::Text);
        const rdui::PaintCommand* icon_command = recording.last(rdui::PaintCommandKind::Icon);
        const rdui::PaintCommand* effect_command = recording.last(rdui::PaintCommandKind::BeginEffects);
        ensure("Surface records text, icon, and effect primitives", text_command && icon_command && effect_command);
        ensure_equals("effect scope preserves device scale", effect_command->scale, 2.f);
        ensure_equals("effect scope preserves the composed list", effect_command->style.effects.size(), 2U);
        ensure_equals("Surface preserves icon resource name", icon_command->value, std::string("search"));
        ensure_equals("ancestor opacity multiplies child opacity", text_command->style.text_color.a, .25f);
        ensure_equals("Text Host consumes resolved RTL alignment before painting the run",
                      static_cast<int>(text_command->style.text_align), static_cast<int>(rdui::TextAlign::Left));
        ensure_equals("resolved RTL alignment positions the run against the right edge", text_command->rect.x, -8.f);
        ensure_equals("Surface primitive order begins with frame", static_cast<int>(recording.commands().front().kind),
                      static_cast<int>(rdui::PaintCommandKind::BeginFrame));
        ensure_equals("Surface primitive order ends with frame", static_cast<int>(recording.commands().back().kind),
                      static_cast<int>(rdui::PaintCommandKind::EndFrame));
        ensure("text is emitted before the later icon",
               text_command < icon_command);
        ensure("successful Surface paint clears invalidation", !surface->needsPaint());
    }

    template<> template<>
    void rduiwidget_object::test<6>()
    {
        std::vector<rdui::InlineContentNode> nodes;
        nodes.push_back(rdui::InlineContentNode::text("alpha"));
        nodes.push_back(rdui::InlineContentNode::container(rdui::InlineContentKind::B,
            {rdui::InlineContentNode::text("beta")}));
        nodes.push_back(rdui::InlineContentNode::br());
        nodes.push_back(rdui::InlineContentNode::container(rdui::InlineContentKind::I,
            {rdui::InlineContentNode::text("gamma")}));

        rdui::Text text("initial");
        ensure_equals("Text accepts a string literal", text.content().nodes()[0].value(),
                      std::string("initial"));
        text.setContent(rdui::InlineContent(std::move(nodes)));
        text.setRect({0.f, 0.f, 100.f, 40.f});
        rdui::Style style;
        style.font_size = 10.f;
        const rdui::FixedTextMetrics metrics(.5f, .75f);
        const rdui::Vec2 measured = text.intrinsicSize(rdui::StyleSheet(), style, metrics);
        ensure_equals("inline measurement uses widest explicit line", measured.x, 55.f);
        ensure_equals("inline measurement accumulates line heights", measured.y, 20.f);

        style.vertical_align = rdui::VerticalAlign::Bottom;
        rdui::RecordingPaintContext recording(metrics);
        text.paint(recording, style, 1.f);
        ensure_equals("Text paints its own box", recording.count(rdui::PaintCommandKind::Box), 1U);
        ensure_equals("each inline run emits one text primitive", recording.count(rdui::PaintCommandKind::Text), 3U);
        const auto& commands = recording.commands();
        ensure_equals("plain run is first", commands[1].value, std::string("alpha"));
        ensure_equals("text block starts at the top regardless of container alignment", commands[1].rect.y, 30.f);
        ensure_equals("text runs preserve the authored content vertical alignment",
                      static_cast<int>(commands[1].style.vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Bottom));
        ensure_equals("bold run follows on the same line", commands[2].value, std::string("beta"));
        ensure_equals("B content strengthens the run style",
                      commands[2].style.font_weight, static_cast<U16>(700));
        ensure_equals("Br advances the following run downward", commands[3].rect.y, 20.f);
        ensure("I content emphasizes the following run", commands[3].style.font_italic);

        rdui::Style tracked_style;
        tracked_style.font_size = 10.f;
        tracked_style.letter_spacing = rdui::Length{2.f};
        rdui::Text tracked;
        tracked.setContent(rdui::InlineContent({
            rdui::InlineContentNode::text("a"),
            rdui::InlineContentNode::container(
                rdui::InlineContentKind::B,
                {rdui::InlineContentNode::text("b")}),
        }));
        tracked.setRect({0.f, 0.f, 100.f, 10.f});
        ensure_equals("letter spacing contributes across inline run boundaries",
                      tracked.intrinsicSize(
                          rdui::StyleSheet(), tracked_style, metrics).x,
                      15.f);
        rdui::RecordingPaintContext tracked_recording(metrics);
        tracked.paint(tracked_recording, tracked_style, 1.f);
        ensure_equals("inline paint preserves the boundary letter spacing",
                      tracked_recording.commands()[2].rect.x, 7.f);

        rdui::Text tracked_combining;
        tracked_combining.setContent(rdui::InlineContent({
            rdui::InlineContentNode::text("a"),
            rdui::InlineContentNode::container(
                rdui::InlineContentKind::I,
                {rdui::InlineContentNode::text("\xCC\x81")}),
            rdui::InlineContentNode::text("b"),
        }));
        tracked_combining.setRect({0.f, 0.f, 100.f, 10.f});
        ensure_equals(
            "fixed metrics count grapheme rather than codepoint gaps",
            tracked_combining.intrinsicSize(
                rdui::StyleSheet(), tracked_style, metrics).x,
            17.f);
        rdui::RecordingPaintContext tracked_combining_recording(
            metrics);
        tracked_combining.paint(
            tracked_combining_recording, tracked_style, 1.f);
        ensure_equals(
            "letter spacing does not split a cross-style grapheme",
            tracked_combining_recording.commands()[2].rect.x,
            5.f);

        style.text_align = rdui::TextAlign::Center;
        rdui::RecordingPaintContext centered(metrics);
        text.paint(centered, style, 1.f);
        ensure_equals("text-align center positions the complete first line", centered.commands()[1].rect.x, 22.5f);

        std::vector<rdui::InlineContentNode> rtl_nodes;
        rtl_nodes.push_back(rdui::InlineContentNode::text("الأول"));
        rtl_nodes.push_back(rdui::InlineContentNode::container(rdui::InlineContentKind::B,
            {rdui::InlineContentNode::text("الثاني")}));
        rdui::Text rtl_text;
        rtl_text.setContent(rdui::InlineContent(std::move(rtl_nodes)));
        rtl_text.setRect({0.f, 0.f, 100.f, 20.f});
        style.direction = rdui::LayoutDirection::RightToLeft;
        rdui::RecordingPaintContext rtl(metrics);
        rtl_text.paint(rtl, style, 1.f);
        ensure_equals("RTL inline spans are emitted in visual order", rtl.commands()[1].value,
                      std::string("الثاني"));
        ensure_equals("RTL visual order retains the preceding logical span", rtl.commands()[2].value,
                      std::string("الأول"));

        std::vector<rdui::InlineContentNode> mixed_nodes;
        mixed_nodes.push_back(rdui::InlineContentNode::text("الأول 123"));
        mixed_nodes.push_back(rdui::InlineContentNode::container(rdui::InlineContentKind::B,
            {rdui::InlineContentNode::text(" الثاني")}));
        rdui::Text mixed;
        mixed.setContent(rdui::InlineContent(std::move(mixed_nodes)));
        mixed.setRect({0.f, 0.f, 100.f, 20.f});
        rdui::RecordingPaintContext mixed_rtl(metrics);
        mixed.paint(mixed_rtl, style, 1.f);
        ensure_equals("mixed bidi content splits at embedding and style boundaries",
                      mixed_rtl.count(rdui::PaintCommandKind::Text), 3U);
        ensure("rightmost logical style run is painted first in visual order",
               mixed_rtl.commands()[1].style.font_weight == 700);
        ensure_equals("embedded LTR segment keeps its own visual run", mixed_rtl.commands()[2].value,
                      std::string("123"));
        ensure("leading logical RTL segment is painted last",
               mixed_rtl.commands()[3].style.font_weight == 400);

        rdui::Text struck;
        struck.setContent(rdui::InlineContent({rdui::InlineContentNode::container(
            rdui::InlineContentKind::S,
            {rdui::InlineContentNode::text("obsolete")})}));
        struck.setRect({0.f, 0.f, 100.f, 20.f});
        rdui::RecordingPaintContext struck_recording(metrics);
        struck.paint(struck_recording, style, 1.f);
        ensure("S content marks only its text run for strike-through",
               struck_recording.commands()[1].style.font_strike);
    }

    template<> template<>
    void rduiwidget_object::test<7>()
    {
        rdui::StyleSheet stylesheet;
        ensure("Kbd paint styles compile", stylesheet.loadRadia(
            "text { font-size: 10px; line-height: 10px; } "
            "kbd { gap: 2px; padding: 1px; background-color: #111111ff; "
            "  > kbd { padding: 1px 2px; background-color: #222222ff; } "
            "}").ok());

        rdui::Text projected;
        projected.setContent(rdui::InlineContent({
            rdui::InlineContentNode::text("Press"),
            rdui::InlineContentNode::kbd(
                "example", rdui::KeybindingPresentation{{"Ctrl", "Shift", "F"}}),
        }));
        ensure_equals("Kbd preserves authored surrounding spacing while separating chord keys",
                      projected.text(), "PressCtrl Shift F");

        rdui::Surface surface(stylesheet);
        surface.setViewport(200.f, 40.f);
        auto text = std::make_unique<rdui::Text>();
        text->setContent(rdui::InlineContent({rdui::InlineContentNode::kbd(
            "example", rdui::KeybindingPresentation{{"Ctrl", "Shift", "F"}})}));
        surface.mount(std::move(text));

        rdui::RecordingPaintContext recording;
        surface.paint(recording);
        ensure_equals("Kbd paints its chord and three independent key boxes",
                      recording.count(rdui::PaintCommandKind::Box), 5U);
        ensure_equals("each generated Kbd key paints independently",
                      recording.count(rdui::PaintCommandKind::Text), 3U);

        std::vector<std::string> keys;
        std::vector<rdui::Color> box_colors;
        for (const rdui::PaintCommand& command : recording.commands())
        {
            if (command.kind == rdui::PaintCommandKind::Text) keys.push_back(command.value);
            if (command.kind == rdui::PaintCommandKind::Box
                && command.style.background_color.a > 0.f)
                box_colors.push_back(command.style.background_color);
        }
        ensure_equals("primary chord exposes three key labels", keys.size(), 3U);
        ensure_equals("first Kbd key is Ctrl", keys[0], std::string("Ctrl"));
        ensure_equals("second Kbd key is Shift", keys[1], std::string("Shift"));
        ensure_equals("third Kbd key is F", keys[2], std::string("F"));
        ensure_equals("one styled chord surrounds three styled keys", box_colors.size(), 4U);
        ensure("outer and nested Kbd selectors produce distinct surfaces",
               box_colors.front().r != box_colors.back().r);
    }

    template<> template<>
    void rduiwidget_object::test<8>()
    {
        const rdui::FixedTextMetrics metrics(.5f, .5f);
        rdui::Style style;
        ensure_equals("text wraps by default like CSS",
                      static_cast<int>(style.text_wrap),
                      static_cast<int>(rdui::TextWrap::Wrap));

        class ShapedRunMetrics final : public rdui::TextMetrics
        {
            public:
                rdui::Vec2 measureText(const std::string& value, const rdui::Style&) const override
                {
                    if (value == "RDUI Demo") return {50.f, 10.f};
                    if (value == "RDUI") return {28.f, 10.f};
                    if (value == " ") return {6.f, 10.f};
                    if (value == "Demo") return {20.f, 10.f};
                    return {0.f, 10.f};
                }
        } shaped_metrics;
        rdui::Text naturally_sized("RDUI Demo");
        naturally_sized.setRect({0.f, 0.f, 50.f, 10.f});
        rdui::RecordingPaintContext natural_wrap(shaped_metrics);
        naturally_sized.paint(natural_wrap, style, 1.f);
        ensure_equals("a shaped line that fits is not rewrapped after token measurement",
                      natural_wrap.count(rdui::PaintCommandKind::Text), 1U);
        ensure_equals("the fitting shaped line remains intact",
                      natural_wrap.commands()[1].value, std::string("RDUI Demo"));

        style.font_size = 10.f;
        style.text_wrap = rdui::TextWrap::NoWrap;
        style.text_overflow = rdui::TextOverflow::EllipsisCenter;
        style.overflow_x = rdui::Overflow::Hidden;

        rdui::Text inventory("abcdefghij");
        inventory.setRect({0.f, 0.f, 35.f, 10.f});
        rdui::RecordingPaintContext centered(metrics);
        inventory.paint(centered, style, 1.f);
        std::string centered_text;
        for (const rdui::PaintCommand& command : centered.commands())
            if (command.kind == rdui::PaintCommandKind::Text) centered_text += command.value;
        ensure_equals("center ellipsis preserves a balanced prefix and suffix",
                      centered_text, std::string("abc\xE2\x80\xA6hij"));

        style.text_overflow = rdui::TextOverflow::Ellipsis;
        rdui::RecordingPaintContext ended(metrics);
        inventory.paint(ended, style, 1.f);
        std::string ended_text;
        for (const rdui::PaintCommand& command : ended.commands())
            if (command.kind == rdui::PaintCommandKind::Text) ended_text += command.value;
        ensure_equals("end ellipsis preserves the visual start",
                      ended_text, std::string("abcdef\xE2\x80\xA6"));

        style.direction = rdui::LayoutDirection::RightToLeft;
        rdui::Text rtl_inventory("ابتثجحخدذر");
        rtl_inventory.setRect({0.f, 0.f, 35.f, 10.f});
        rdui::RecordingPaintContext rtl_ended(metrics);
        rtl_inventory.paint(rtl_ended, style, 1.f);
        ensure_equals("RTL end ellipsis paints at the visual left edge",
                      rtl_ended.commands()[1].value,
                      std::string("\xE2\x80\xA6"));
        ensure_equals("RTL end ellipsis retains the logical prefix",
                      rtl_ended.commands()[2].value,
                      std::string("ابتثجح"));
        style.direction = rdui::LayoutDirection::LeftToRight;

        rdui::Text combining("a\xCC\x81" "bcdef");
        combining.setRect({0.f, 0.f, 20.f, 10.f});
        rdui::RecordingPaintContext combining_ended(metrics);
        combining.paint(combining_ended, style, 1.f);
        std::string combining_text;
        for (const rdui::PaintCommand& command : combining_ended.commands())
            if (command.kind == rdui::PaintCommandKind::Text)
                combining_text += command.value;
        ensure_equals("end ellipsis does not split a combining sequence",
                      combining_text,
                      std::string("a\xCC\x81" "b\xE2\x80\xA6"));

        rdui::Text styled_combining;
        styled_combining.setContent(rdui::InlineContent({
            rdui::InlineContentNode::text("a"),
            rdui::InlineContentNode::container(
                rdui::InlineContentKind::I,
                {rdui::InlineContentNode::text("\xCC\x81")}),
            rdui::InlineContentNode::text("bcdef"),
        }));
        styled_combining.setRect({0.f, 0.f, 10.f, 10.f});
        rdui::RecordingPaintContext styled_combining_ended(
            metrics);
        styled_combining.paint(
            styled_combining_ended, style, 1.f);
        std::string styled_combining_text;
        for (const rdui::PaintCommand& command :
             styled_combining_ended.commands())
            if (command.kind == rdui::PaintCommandKind::Text)
                styled_combining_text += command.value;
        ensure_equals(
            "ellipsis does not split a grapheme across inline styles",
            styled_combining_text,
            std::string("\xE2\x80\xA6"));

        rdui::Text family(
            "\xF0\x9F\x91\xA8\xE2\x80\x8D\xF0\x9F\x91\xA9"
            "\xE2\x80\x8D\xF0\x9F\x91\xA7" "ABC");
        family.setRect({0.f, 0.f, 35.f, 10.f});
        rdui::RecordingPaintContext family_ended(metrics);
        family.paint(family_ended, style, 1.f);
        std::string family_text;
        for (const rdui::PaintCommand& command : family_ended.commands())
            if (command.kind == rdui::PaintCommandKind::Text)
                family_text += command.value;
        ensure_equals("end ellipsis does not split an emoji ZWJ sequence",
                      family_text,
                      std::string(
                          "\xF0\x9F\x91\xA8\xE2\x80\x8D\xF0\x9F\x91\xA9"
                          "\xE2\x80\x8D\xF0\x9F\x91\xA7"
                          "A\xE2\x80\xA6"));

        style.text_overflow = rdui::TextOverflow::Clip;
        rdui::Text clipped("abc");
        clipped.setRect({0.f, 0.f, 2.f, 10.f});
        rdui::RecordingPaintContext clipped_recording(metrics);
        clipped.paint(clipped_recording, style, 1.f);
        ensure_equals(
            "clip preserves the full run for the overflow scissor",
            clipped_recording.commands()[1].value,
            std::string("abc"));

        style.text_wrap = rdui::TextWrap::Wrap;
        rdui::Text wrapped("alpha beta");
        wrapped.setRect({0.f, 0.f, 30.f, 20.f});
        rdui::RecordingPaintContext wrapping(metrics);
        wrapped.paint(wrapping, style, 1.f);
        ensure_equals("text-wrap creates two painted lines",
                      wrapping.count(rdui::PaintCommandKind::Text), 2U);
        ensure_equals("first wrapped line ends before the breaking space",
                      wrapping.commands()[1].value, std::string("alpha"));
        ensure_equals("second wrapped line begins with the next word",
                      wrapping.commands()[2].value, std::string("beta"));

        rdui::Text zero_width_wrapped("alpha beta");
        zero_width_wrapped.setRect({0.f, 0.f, 0.f, 20.f});
        rdui::RecordingPaintContext zero_width_wrapping(metrics);
        zero_width_wrapped.paint(zero_width_wrapping, style, 1.f);
        ensure_equals(
            "a zero-width constraint preserves word wrap opportunities",
            zero_width_wrapping.count(rdui::PaintCommandKind::Text), 2U);

        rdui::Text multiword("alpha beta gamma");
        multiword.setRect({0.f, 0.f, 50.f, 20.f});
        rdui::RecordingPaintContext shaped_wrapping(metrics);
        multiword.paint(shaped_wrapping, style, 1.f);
        ensure_equals("wrapping shapes each visual line instead of separate word tokens",
                      shaped_wrapping.count(rdui::PaintCommandKind::Text), 2U);
        ensure_equals("the first visual line retains its internal space",
                      shaped_wrapping.commands()[1].value, std::string("alpha beta"));
        ensure_equals("the following word starts the second visual line",
                      shaped_wrapping.commands()[2].value, std::string("gamma"));

        rdui::Text cjk("你好世界");
        cjk.setRect({0.f, 0.f, 10.f, 20.f});
        rdui::RecordingPaintContext cjk_wrapping(metrics);
        cjk.paint(cjk_wrapping, style, 1.f);
        ensure_equals("Unicode line-break opportunities wrap CJK text",
                      cjk_wrapping.count(rdui::PaintCommandKind::Text), 2U);

        rdui::Text styled_word;
        styled_word.setContent(rdui::InlineContent({
            rdui::InlineContentNode::text("al"),
            rdui::InlineContentNode::container(
                rdui::InlineContentKind::B,
                {rdui::InlineContentNode::text("pha")}),
        }));
        styled_word.setRect({0.f, 0.f, 15.f, 20.f});
        rdui::RecordingPaintContext styled_word_wrapping(metrics);
        styled_word.paint(styled_word_wrapping, style, 1.f);
        ensure_equals(
            "inline styling does not create a line-break opportunity",
            styled_word_wrapping.commands()[1].rect.y,
            styled_word_wrapping.commands()[2].rect.y);

        class VariableTextMetrics final : public rdui::TextMetrics
        {
            public:
                rdui::Vec2 measureText(
                    const std::string& value,
                    const rdui::Style&) const override
                {
                    if (value.empty()) return {0.f, 10.f};
                    if (value == "W") return {20.f, 10.f};
                    if (value == "\xE2\x80\xA6") return {5.f, 10.f};
                    return {
                        static_cast<float>(value.size()) * 5.f,
                        10.f,
                    };
                }
        } variable_metrics;
        style.text_wrap = rdui::TextWrap::NoWrap;
        style.text_overflow = rdui::TextOverflow::EllipsisCenter;
        rdui::Text asymmetric("Wabc");
        asymmetric.setRect({0.f, 0.f, 10.f, 10.f});
        rdui::RecordingPaintContext asymmetric_center(variable_metrics);
        asymmetric.paint(asymmetric_center, style, 1.f);
        std::string asymmetric_text;
        for (const rdui::PaintCommand& command :
             asymmetric_center.commands())
            if (command.kind == rdui::PaintCommandKind::Text)
                asymmetric_text += command.value;
        ensure_equals(
            "center ellipsis tries the suffix when the prefix is too wide",
            asymmetric_text, std::string("\xE2\x80\xA6" "c"));

        style.text_wrap = rdui::TextWrap::Wrap;
        style.text_overflow = rdui::TextOverflow::Clip;
        style.width = rdui::Dimension::fromPixels(30.f);
        ensure_equals("an explicit wrapping width contributes both lines to intrinsic height",
                      wrapped.intrinsicSize(rdui::StyleSheet(), style, metrics).y, 20.f);

        style.letter_spacing = rdui::Length{2.f};
        ensure_equals("pixel letter spacing contributes between characters only",
                      metrics.measureText("abc", style).x, 19.f);
        style.letter_spacing = rdui::Length{0.f, .5f};
        ensure_equals("percentage letter spacing uses the fixed font's space advance",
                      metrics.measureText("abc", style).x, 20.f);
        style.letter_spacing = {};
        style.word_spacing = rdui::Length{3.f};
        ensure_equals("word spacing contributes once per word separator",
                      metrics.measureText("a b", style).x, 18.f);
        style.word_spacing = rdui::Length{0.f, .5f};
        ensure_equals("percentage word spacing resolves against font-size",
                      metrics.measureText("a b", style).x, 20.f);
        style.word_spacing = rdui::Length{3.f};
        ensure_equals("word spacing recognizes Unicode space separators",
                      metrics.measureText("a\xE2\x80\x83" "b", style).x,
                      18.f);
    }

}
