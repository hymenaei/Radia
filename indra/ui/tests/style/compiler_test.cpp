/**
 * @file compiler_test.cpp
 * @brief Tests RSL property compilation and shorthand expansion.
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
#include "layout/engine.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace {
using radia::ui::AlignItems;
using radia::ui::Button;
using radia::ui::Flow;
using radia::ui::FontFamily;
using radia::ui::Icon;
using radia::ui::JustifyContent;
using radia::ui::Panel;
using radia::ui::PointerEvents;
using radia::ui::resolveWidgetStyle;
using radia::ui::Style;
using radia::ui::StyleSheet;
using radia::ui::StyleSheetLoadResult;
using radia::ui::Text;
using radia::ui::TextAlign;
using radia::ui::TextOverflow;
using radia::ui::TextWrap;
using radia::ui::VerticalAlign;
using radia::ui::WidgetState;
using radia::ui::detail::matchingBlock;
using radia::ui::detail::tokenizeTopLevel;
using radia::ui::detail::WidgetCompilerAccess;
} // namespace

namespace tut {
struct compilerData {};
using compilerTest = test_group<compilerData>;
using compilerObject = compilerTest::object;
compilerTest compilerTestCase("compiler");

template<> template<> void compilerObject::test<1>() {
    StyleSheet stylesheet;
    ensure("selector stylesheet loads", stylesheet.loadRadia("button.primary:hover > icon { width: 17px; }").ok());
    Button button;
    button.addClass("primary");
    WidgetCompilerAccess::setState(button, WidgetState::Hovered, true);
    Icon& icon = button.setIcon("search");
    ensure_equals("compiled selector matches element, class, state, and child", resolveWidgetStyle(stylesheet, icon).width.pixels(), 17.f);
}

template<> template<> void compilerObject::test<2>() {
    StyleSheet styleSheet;
    const char* kTokenStyles =
        ":root { --accent: #204060ff; --space: 12px; } button { background-color: var(--accent); padding: var(--space); border-radius: 5px; }";
    styleSheet.loadRadia(kTokenStyles);
    const Style style = styleSheet.resolve("button", "", {}, 0);
    ensure_approximately_equals("color token", style.backgroundColor.b, 96.f / 255.f, 6);
    ensure_equals("number token", style.padding.left, 12.f);
    ensure_equals("radius", style.borderRadius, 5.f);
}

template<> template<> void compilerObject::test<3>() {
    StyleSheet styleSheet;
    styleSheet.loadRadia("button { width: 10px; size: 20px 30px; width: 40px; }");
    const Style style = styleSheet.resolve("button", "", {}, 0);
    ensure_equals("later declaration wins", style.width.pixels(), 40.f);
    ensure_equals("size height retained", style.height.pixels(), 20.f);
}

template<> template<> void compilerObject::test<4>() {
    StyleSheet styleSheet;
    styleSheet.loadRadia("button.primary { width: 30px; } button { width: 10px; } #save { width: 50px; }");
    const std::set<std::string> classes{"primary"};
    ensure_equals("specificity sorted at load", styleSheet.resolve("button", "save", classes, 0).width.pixels(), 50.f);
    ensure_equals("class beats later element", styleSheet.resolve("button", "", classes, 0).width.pixels(), 30.f);
}

template<> template<> void compilerObject::test<5>() {
    StyleSheet styleSheet;
    const char* kNestedStyles =
        "button { background-color: #101010ff; &:hover { background-color: #202020ff; } > icon { size: 16px; } &:hover > icon { stroke-width: 3px; } }";
    ensure("nested stylesheet loads", styleSheet.loadRadia(kNestedStyles).ok());
    const uint8_t hover = WidgetState::Hovered | WidgetState::Default;
    ensure_equals("nested state", styleSheet.resolve("button", "", {}, hover).backgroundColor.r, 32.f / 255.f);
    Button button;
    WidgetCompilerAccess::setState(button, WidgetState::Hovered, true);
    const Style icon = resolveWidgetStyle(styleSheet, button.setIcon("search"));
    ensure_equals("nested child width", icon.width.pixels(), 16.f);
    ensure("nested child stroke width is set", icon.svgStrokeWidth.has_value());
    ensure_equals("nested owner state", icon.svgStrokeWidth->pixels, 3.f);
}

template<> template<> void compilerObject::test<6>() {
    StyleSheet styleSheet;
    styleSheet.loadRadia("panel { flow: row; vertical-align: middle; pointer-events: none; } label { text-align: right; pointer-events: auto; }");
    const Style panel = styleSheet.resolve("panel", "", {}, 0);
    ensure_equals("row enum", static_cast<int>(panel.flow), static_cast<int>(Flow::Row));
    ensure_equals("pointer enum", static_cast<int>(panel.pointerEvents), static_cast<int>(PointerEvents::PassThrough));
    const Style label = styleSheet.resolve("label", "", {}, 0);
    ensure_equals("horizontal enum", static_cast<int>(label.textAlign), static_cast<int>(TextAlign::Right));
    ensure_equals("container vertical enum", static_cast<int>(panel.verticalAlign), static_cast<int>(VerticalAlign::Middle));
    ensure_equals("vertical alignment remains local", static_cast<int>(label.verticalAlign), static_cast<int>(VerticalAlign::Top));
    ensure_equals("auto pointer enum", static_cast<int>(label.pointerEvents), static_cast<int>(PointerEvents::Auto));
    ensure("logical text alignment compiles", styleSheet.loadRadia("label { text-align: start; }").ok());
    ensure_equals("logical text alignment remains distinct", static_cast<int>(styleSheet.resolve("label", "", {}, 0).textAlign),
                  static_cast<int>(TextAlign::Start));
    const char* kCrossAxisStyles =
        "panel { align-items: end; } panel.normal { align-items: normal; } button { align-self: start; } button.auto { align-self: auto; }";
    ensure("cross-axis alignment compiles", styleSheet.loadRadia(kCrossAxisStyles).ok());
    ensure_equals("container cross-axis alignment is typed", static_cast<int>(styleSheet.resolve("panel", "", {}, 0).alignItems),
                  static_cast<int>(AlignItems::End));
    ensure_equals("flow-item cross-axis override is typed", static_cast<int>(styleSheet.resolve("button", "", {}, 0).alignSelf),
                  static_cast<int>(radia::ui::AlignSelf::Start));
    ensure_equals("normal container alignment can be authored", static_cast<int>(styleSheet.resolve("panel", "", {"normal"}, 0).alignItems),
                  static_cast<int>(AlignItems::Normal));
    ensure_equals("auto flow-item alignment can be authored", static_cast<int>(styleSheet.resolve("button", "", {"auto"}, 0).alignSelf),
                  static_cast<int>(radia::ui::AlignSelf::Auto));
}

template<> template<> void compilerObject::test<7>() {
    StyleSheet styleSheet;
    ensure("independent typography properties compile",
           styleSheet.loadRadia("label#a { font-family: sans; font-size: 19px; font-weight: bold; font-style: italic; }").ok());
    const Style a = styleSheet.resolve("label", "a", {}, 0);
    ensure_equals("font family", static_cast<int>(a.fontFamily), static_cast<int>(FontFamily::Sans));
    ensure_equals("font size", a.fontSize, 19.f);
    ensure_equals("bold", a.fontWeight, static_cast<U16>(700));
    ensure("italic", a.fontItalic);

    StyleSheet variableWeight;
    ensure("numeric variable font weights compile", variableWeight.loadRadia("label { font-weight: 525; }").ok());
    ensure_equals("numeric weight is preserved", variableWeight.resolve("label", "", {}, 0).fontWeight, static_cast<U16>(525));

    StyleSheet pseudoFamily;
    ensure("weight is not accepted as a pseudo font family", !pseudoFamily.loadRadia("label { font-family: sans-bold; }").ok());
    ensure("fractional Radia weights are rejected", !pseudoFamily.loadRadia("label { font-weight: 525.5; }").ok());
    ensure("Radia weights are unitless", !pseudoFamily.loadRadia("label { font-weight: 700px; }").ok());
}

template<> template<> void compilerObject::test<8>() {
    StyleSheet styleSheet;
    const char* kBorderStyles =
        "button { border: 1px #112233ff; border-width: 2px 3px; border-color: #ffffffff; } button > icon { stroke: 4px #abcdef88; stroke-linecap: square; }";
    styleSheet.loadRadia(kBorderStyles);
    const Style style = styleSheet.resolve("button", "", {}, 0);
    ensure_equals("ordered border width top", style.borderWidth.top, 2.f);
    ensure_equals("ordered border width right", style.borderWidth.right, 3.f);
    ensure_equals("border color override", style.borderColor.r, 1.f);
    Button button;
    Icon& buttonIcon = button.setIcon("search");
    const Style icon = resolveWidgetStyle(styleSheet, buttonIcon);
    ensure("svg width is set", icon.svgStrokeWidth.has_value());
    ensure_equals("svg width", icon.svgStrokeWidth->pixels, 4.f);
    ensure_equals("cap", static_cast<int>(icon.svgStrokeCap), static_cast<int>(radia::ui::StrokeCap::Square));
}

template<> template<> void compilerObject::test<9>() {
    StyleSheet styleSheet;
    const StyleSheetLoadResult result = styleSheet.loadRadia("panel { flow: grid; } panel#bad { flow: sideways; }", "test.radia");
    ensure_equals("grid falls back", static_cast<int>(styleSheet.resolve("panel", "", {}, 0).flow), static_cast<int>(Flow::Free));
    ensure_equals("unknown falls back", static_cast<int>(styleSheet.resolve("panel", "bad", {}, 0).flow), static_cast<int>(Flow::Free));
    ensure("unknown flow prevents commit", !result.ok());
    ensure_equals("unsupported flow warns", result.warnings.size(), 1U);
    ensure_equals("unknown flow errors", result.errors.size(), 1U);
    ensure_equals("flow diagnostic identifies source", result.errors.front().source, "test.radia");
}

template<> template<> void compilerObject::test<10>() {
    StyleSheet styleSheet;
    styleSheet.loadRadia("button, switch { height: 32px; } button > icon { width: 14px; } button:disabled { opacity: .5; }");
    ensure_equals("selector list button", styleSheet.resolve("button", "", {}, 0).height.pixels(), 32.f);
    ensure_equals("selector list switch", styleSheet.resolve("switch", "", {}, 0).height.pixels(), 32.f);
    Button button;
    ensure_equals("direct icon child", resolveWidgetStyle(styleSheet, button.setIcon("search")).width.pixels(), 14.f);
    const uint8_t disabled = static_cast<uint8_t>(WidgetState::Disabled);
    ensure_equals("state selector", styleSheet.resolve("button", "", {}, disabled).opacity, .5f);
}

template<> template<> void compilerObject::test<11>() {
    StyleSheet styleSheet;
    const char* kFlowItemStyles =
        "panel { padding: 1px 2px 3px 4px; min-width: 20px; min-height: 10px; gap: 7px; flex: 2 3 40%; order: -2; } panel.auto { flex: auto; } panel.none { flex: none; } panel.one { flex: 4; } panel.two { flex: 5 6; } panel.basis { flex: 10px; }";
    ensure("flow item stylesheet compiles", styleSheet.loadRadia(kFlowItemStyles).ok());
    const Style style = styleSheet.resolve("panel", "", {}, 0);
    ensure_equals("padding top", style.padding.top, 1.f);
    ensure_equals("padding right", style.padding.right, 2.f);
    ensure_equals("padding bottom", style.padding.bottom, 3.f);
    ensure_equals("padding left", style.padding.left, 4.f);
    ensure("min width is set", style.minWidth.has_value());
    ensure_equals("min width", style.minWidth->pixels, 20.f);
    ensure_equals("gap", style.gap.fixedPixels(), 7.f);
    ensure_equals("flex grow", style.flexGrow, 2.f);
    ensure_equals("flex shrink", style.flexShrink, 3.f);
    ensure_approximately_equals("flex basis percentage", style.flexBasis.resolve(0.f, 200.f), 80.f, 6);
    ensure_equals("order", style.order, -2);
    const Style automatic = styleSheet.resolve("panel", "", {"auto"}, 0);
    ensure_equals("flex auto grows", automatic.flexGrow, 1.f);
    ensure_equals("flex auto shrinks", automatic.flexShrink, 1.f);
    ensure("flex auto basis remains automatic", automatic.flexBasis.isAuto());
    const Style none = styleSheet.resolve("panel", "", {"none"}, 0);
    ensure_equals("flex none does not grow", none.flexGrow, 0.f);
    ensure_equals("flex none does not shrink", none.flexShrink, 0.f);
    ensure("flex none basis remains automatic", none.flexBasis.isAuto());
    ensure_equals("one-number flex sets grow", styleSheet.resolve("panel", "", {"one"}, 0).flexGrow, 4.f);
    ensure_equals("one-number flex defaults basis to zero", styleSheet.resolve("panel", "", {"one"}, 0).flexBasis.resolve(1.f), 0.f);
    const Style two = styleSheet.resolve("panel", "", {"two"}, 0);
    ensure_equals("two-number flex sets grow", two.flexGrow, 5.f);
    ensure_equals("two-number flex sets shrink", two.flexShrink, 6.f);
    const Style basis = styleSheet.resolve("panel", "", {"basis"}, 0);
    ensure_equals("one-length flex defaults grow", basis.flexGrow, 1.f);
    ensure_equals("one-length flex sets basis", basis.flexBasis.resolve(0.f), 10.f);
    StyleSheet legacy;
    ensure("legacy grow property is rejected", !legacy.loadRadia("panel { grow: 1; }").ok());
    ensure("flex grow remains unitless", !legacy.loadRadia("panel { flex-grow: 1px; }").ok());
}

template<> template<> void compilerObject::test<12>() {
    const Style style;
    ensure_equals("default flow", static_cast<int>(style.flow), static_cast<int>(Flow::Free));
    ensure_equals("default justification", static_cast<int>(style.justifyContent), static_cast<int>(JustifyContent::Start));
    ensure_equals("default container alignment", static_cast<int>(style.alignItems), static_cast<int>(AlignItems::Normal));
    ensure_equals("default flow-item alignment", static_cast<int>(style.alignSelf), static_cast<int>(radia::ui::AlignSelf::Auto));
    ensure_equals("default flex grow", style.flexGrow, 0.f);
    ensure_equals("default flex shrink", style.flexShrink, 1.f);
    ensure("default flex basis is automatic", style.flexBasis.isAuto());
    ensure_equals("default flow-item order", style.order, 0);
    ensure("default gap is fixed", !style.gap.isAuto());
    ensure_equals("default gap is zero", style.gap.fixedPixels(), 0.f);
    ensure_equals("default pointer behavior", static_cast<int>(style.pointerEvents), static_cast<int>(PointerEvents::Default));
    ensure_equals("default font", static_cast<int>(style.fontFamily), static_cast<int>(FontFamily::Sans));
    ensure_equals("default container vertical alignment is top", static_cast<int>(style.verticalAlign), static_cast<int>(VerticalAlign::Top));
    StyleSheet buttonTheme;
    Button button;
    const Style buttonDefaults = resolveWidgetStyle(buttonTheme, button);
    ensure_equals("Button intrinsically uses row flow", static_cast<int>(buttonDefaults.flow), static_cast<int>(Flow::Row));
    ensure_equals("Button intrinsically centers content horizontally", static_cast<int>(buttonDefaults.justifyContent),
                  static_cast<int>(JustifyContent::Center));
    ensure_equals("Button intrinsically aligns content to the middle", static_cast<int>(buttonDefaults.verticalAlign),
                  static_cast<int>(VerticalAlign::Middle));
    ensure("explicit Button layout compiles", buttonTheme.loadRadia("button { flow: column; justify-content: end; vertical-align: top; }").ok());
    const Style authoredButton = resolveWidgetStyle(buttonTheme, button);
    ensure_equals("authored Button flow overrides its intrinsic default", static_cast<int>(authoredButton.flow), static_cast<int>(Flow::Column));
    ensure_equals("authored Button justification overrides its intrinsic default", static_cast<int>(authoredButton.justifyContent),
                  static_cast<int>(JustifyContent::End));
    ensure_equals("authored Button vertical alignment overrides its intrinsic default", static_cast<int>(authoredButton.verticalAlign),
                  static_cast<int>(VerticalAlign::Top));

    StyleSheet floaterTheme;
    radia::ui::Floater floater;
    Panel* customHeader = nullptr;
    for (const auto& child : floater.header()->children())
        if (child->part() == "header::custom") customHeader = static_cast<Panel*>(child.get());
    ensure("Floater custom-header Part exists", customHeader != nullptr);
    ensure_equals("Floater header intrinsically aligns content to the middle",
                  static_cast<int>(resolveWidgetStyle(floaterTheme, *floater.header()).verticalAlign), static_cast<int>(VerticalAlign::Middle));
    ensure_equals("Floater custom header intrinsically aligns content to the middle",
                  static_cast<int>(resolveWidgetStyle(floaterTheme, *customHeader).verticalAlign), static_cast<int>(VerticalAlign::Middle));

    StyleSheet fieldTheme;
    radia::ui::Field field;
    ensure_equals("Field intrinsically aligns content to the middle", static_cast<int>(resolveWidgetStyle(fieldTheme, field).verticalAlign),
                  static_cast<int>(VerticalAlign::Middle));
    ensure("explicit Field vertical alignment compiles", fieldTheme.loadRadia("field { vertical-align: bottom; }").ok());
    ensure_equals("authored Field vertical alignment overrides its intrinsic default",
                  static_cast<int>(resolveWidgetStyle(fieldTheme, field).verticalAlign), static_cast<int>(VerticalAlign::Bottom));
    ensure_equals("initial box fill is presentation-neutral", style.backgroundColor.a, 0.f);
}

template<> template<> void compilerObject::test<13>() {
    StyleSheet styleSheet;
    const char* kTextPresentationStyles =
        "panel { letter-spacing: 50%; word-spacing: 25%; text-wrap: nowrap; } text { text-overflow: ellipsis-center; } label { font: italic 525 17px/21px sans; } label.reset { font-style: italic; font-weight: bold; line-height: 30px; font: 12px sans; }";
    ensure("text presentation properties compile", styleSheet.loadRadia(kTextPresentationStyles).ok());

    auto parent = std::make_unique<Panel>();
    auto text = std::make_unique<Text>("inventory item");
    Text* child = text.get();
    parent->addChild(std::move(text));
    const Style inherited = resolveWidgetStyle(styleSheet, *child);
    ensure_equals("letter-spacing percentage is retained for used-value resolution", inherited.letterSpacing.percent, .5f);
    ensure_equals("word-spacing percentage is retained for used-value resolution", inherited.wordSpacing.percent, .25f);
    ensure_equals("text-wrap inherits", static_cast<int>(inherited.textWrap), static_cast<int>(TextWrap::NoWrap));
    ensure_equals("text-overflow remains local", static_cast<int>(inherited.textOverflow), static_cast<int>(TextOverflow::EllipsisCenter));

    const Style shorthand = styleSheet.resolve("label", "", {}, 0);
    ensure("font shorthand sets style", shorthand.fontItalic);
    ensure_equals("font shorthand sets variable weight", shorthand.fontWeight, static_cast<U16>(525));
    ensure_equals("font shorthand sets size", shorthand.fontSize, 17.f);
    ensure("font shorthand sets line-height", shorthand.lineHeight.has_value());
    ensure_equals("font shorthand line-height value", shorthand.lineHeight->pixels, 21.f);

    const Style reset = styleSheet.resolve("label", "", {"reset"}, 0);
    ensure("font shorthand resets omitted style", !reset.fontItalic);
    ensure_equals("font shorthand resets omitted weight", reset.fontWeight, static_cast<U16>(400));
    ensure("font shorthand resets omitted line-height", !reset.lineHeight.has_value());

    StyleSheet invalid;
    ensure("unknown center truncation spelling is rejected", !invalid.loadRadia("text { text-overflow: middle; }").ok());
    ensure("font shorthand requires a family", !invalid.loadRadia("text { font: 13px; }").ok());
    ensure("normal word spacing compiles", invalid.loadRadia("text { word-spacing: normal; }").ok());
    ensure_equals("normal word spacing resets to zero", invalid.resolve("text", "", {}, 0).wordSpacing.pixels, 0.f);
}

template<> template<> void compilerObject::test<14>() {
    const char* kProperties[] = {"padding", "border-width"};
    const char* kValues[] = {"nan 2px 3px 4px",  "1px inf 3px 4px",  "1px 2px -inf 4px", "1px 2px 3px nan",
                             "-nan 2px 3px 4px", "1px -inf 3px 4px", "1px 2px -nan 4px", "1px 2px 3px -inf"};
    for (const char* property : kProperties)
        for (const char* value : kValues) {
            StyleSheet styleSheet;
            const std::string declaration = std::string("panel { ") + property + ": " + value + "; }";
            ensure("non-finite edge values reject the declaration", !styleSheet.loadRadia(declaration, "nonfinite-edge.radia").ok());
        }
}

template<> template<> void compilerObject::test<15>() {
    const std::vector<std::string> tokens = tokenizeTopLevel("italic 17px/21px sans", true);
    ensure_equals("top-level scanner preserves slash tokens", tokens.size(), 5U);
    ensure_equals("top-level scanner slash", tokens[2], "/");
    ensure("top-level scanner rejects unclosed parentheses", tokenizeTopLevel("var(--accent", true).empty());
    ensure("top-level scanner rejects unmatched close", radia::ui::detail::splitTopLevel("rgb(1, 2)), blue", ',').empty());
    const std::string kSource = "button { icon { width: 1px; } }";
    const std::size_t open = kSource.find('{');
    const std::optional<std::size_t> close = matchingBlock(kSource, open);
    ensure("block scanner finds nested close", close.has_value());
    ensure_equals("block scanner closes outer block", *close, kSource.size() - 1);
    ensure("block scanner rejects unclosed block", !matchingBlock("button {", 7).has_value());
}

template<> template<> void compilerObject::test<16>() {
    const std::set<std::string_view> shorthandNames{"font", "flex", "min-size", "overflow"};
    std::set<std::string_view> names;
    for (const radia::ui::detail::StylePropertyDefinition* property = radia::ui::detail::stylePropertyBegin();
         property != radia::ui::detail::stylePropertyEnd(); ++property) {
        ensure("registry property names are unique", names.insert(property->name).second);
        ensure("registry property has a compiler", property->compile != nullptr);
        ensure_equals("shorthand apply is explicit", property->apply == nullptr, shorthandNames.count(property->name) != 0);
    }
    ensure_equals("registry contains all style properties", names.size(), 53U);
}
} // namespace tut
