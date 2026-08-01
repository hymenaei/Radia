/**
 * @file style_test.cpp
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
#include "../test/lltut.h"
#include "layout/engine.h"
#include "style/stylesheet.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace tut {
struct rduistyle_data {};
typedef test_group<rduistyle_data> rduistyle_test;
typedef rduistyle_test::object rduistyle_object;
rduistyle_test rduistyle_testcase("rduistyle");

template<> template<> void rduistyle_object::test<1>() {
    rdui::StyleSheet theme;
    ensure("valid colors compile",
           theme
               .loadRadia(":root { --accent: hsl(120 100% 50%); --ink: rgb(255, 0, 0, 50%); }"
                          "button { background-color: var(--accent); text-color: var(--ink); font-size: 17px; }"
                          "label { text-color: #00ff00ff; font-size: 29px; }")
               .ok());
    const rdui::Style button = theme.resolve("button", "", {}, 0);
    ensure_approximately_equals("hsl color token", button.background_color.g, 1.f, 6);
    rdui::Button control;
    rdui::Label& label = control.setLabel("Inherited");
    ensure_approximately_equals("rgb color token with alpha inherits into button label", rdui::resolveWidgetStyle(theme, label).text_color.a, .5f, 6);
    ensure_equals("button caption inherits button font size", rdui::resolveWidgetStyle(theme, label).font_size, 17.f);
    rdui::Label standalone("Standalone");
    ensure_equals("standalone Label keeps its own font size", rdui::resolveWidgetStyle(theme, standalone).font_size, 29.f);
    const rdui::StyleSheetLoadResult invalid = theme.loadRadia("switch { background-color: ##invalid; }", "invalid.radia");
    ensure("invalid color rejects candidate", !invalid.ok());
    ensure_equals("invalid value diagnostic code", invalid.errors.front().code, "stylesheet.property.value_invalid");
    ensure_approximately_equals("failed candidate keeps prior stylesheet", theme.resolve("button", "", {}, 0).background_color.g, 1.f, 6);
}

template<> template<> void rduistyle_object::test<2>() {
    rdui::StyleSheet theme;
    theme.loadRadia("panel { margin: 1px auto 3px -4px; padding: 5px 6px; gap: 7px; }");
    const rdui::Style style = theme.resolve("panel", "", {}, 0);
    ensure_equals("margin top", style.margin.top.fixedPixels(), 1.f);
    ensure("margin right auto", style.margin.right.isAuto());
    ensure_equals("margin bottom", style.margin.bottom.fixedPixels(), 3.f);
    ensure_equals("negative margin retained", style.margin.left.fixedPixels(), -4.f);
    ensure("non-auto margin edge", !style.margin.left.isAuto());
    ensure_equals("padding remains numeric", style.padding.left, 6.f);
    ensure_equals("gap remains numeric", style.gap.fixedPixels(), 7.f);
    ensure("numeric gap is not automatic", !style.gap.isAuto());
}

template<> template<> void rduistyle_object::test<3>() {
    rdui::StyleSheet theme;
    theme.loadRadia("button { border-width: 1px; &:focus { opacity: .8; } &:focus-visible { border-width: 3px; } }");
    const uint8_t focused = static_cast<uint8_t>(rdui::WidgetState::Focused);
    const uint8_t focus_visible = rdui::WidgetState::Focused | rdui::WidgetState::FocusVisible;
    ensure_equals(":focus remains supported", theme.resolve("button", "", {}, focused).opacity, .8f);
    ensure_equals(":focus alone does not match :focus-visible", theme.resolve("button", "", {}, focused).border_width.top, 1.f);
    ensure_equals(":focus-visible selector matches", theme.resolve("button", "", {}, focus_visible).border_width.top, 3.f);
    ensure_equals("orphan focus-visible state does not match",
                  theme.resolve("button", "", {}, static_cast<uint8_t>(rdui::WidgetState::FocusVisible)).border_width.top, 1.f);
}

template<> template<> void rduistyle_object::test<4>() {
    rdui::StyleSheet theme;
    ensure("initial stylesheet loads", theme.loadRadia("button { width: 12px; }").ok());
    const rdui::StyleSheetLoadResult failed = theme.loadRadia("button { width: 99px; unknown-property: 1; }", "candidate.radia");
    ensure("unknown property rejects candidate", !failed.ok());
    ensure_equals("failed candidate leaves live stylesheet unchanged", theme.resolve("button", "", {}, 0).width.pixels(), 12.f);
    ensure_equals("property diagnostic code", failed.errors.front().code, "stylesheet.property.unknown");
    ensure_equals("property diagnostic source", failed.errors.front().source, "candidate.radia");
}

template<> template<> void rduistyle_object::test<5>() {
    rdui::StyleSheet stylesheet;
    const rdui::StyleSheetLoadResult malformed = stylesheet.loadRadia(":root { --bad: nonsense; }", "tokens.radia");
    ensure("malformed token rejects candidate", !malformed.ok());
    ensure_equals("token diagnostic code", malformed.errors.front().code, "stylesheet.token.value_invalid");

    const rdui::StyleSheetLoadResult missing = stylesheet.loadRadia("button { width: var(--missing); }", "missing-token.radia");
    ensure("unknown token reference rejects candidate", !missing.ok());
    ensure_equals("property diagnostic source", missing.errors.front().source, "missing-token.radia");
}

template<> template<> void rduistyle_object::test<6>() {
    rdui::StyleSheet stylesheet;
    ensure("child-owner stylesheet loads",
           stylesheet.loadRadia("button.primary > icon { width: 10px; } button.primary:hover > icon { width: 18px; }").ok());
    rdui::Button button;
    button.addClass("primary");
    rdui::Icon& icon = button.setIcon("search");

    ensure_equals("child uses owner class", rdui::resolveWidgetStyle(stylesheet, icon).width.pixels(), 10.f);
    rdui::detail::WidgetCompilerAccess::setState(button, rdui::WidgetState::Hovered, true);
    ensure_equals("child uses owner state", rdui::resolveWidgetStyle(stylesheet, icon).width.pixels(), 18.f);
}

template<> template<> void rduistyle_object::test<7>() {
    rdui::StyleSheet stylesheet;
    ensure("interactive-part stylesheet loads",
           stylesheet.loadRadia("floater { &::header { &::close { width: 10px; &:hover { width: 18px; } } } }").ok());
    rdui::Floater floater;

    rdui::detail::WidgetCompilerAccess::setState(floater, rdui::WidgetState::Hovered, true);
    ensure_equals("owner hover does not hover interactive part", rdui::resolveWidgetStyle(stylesheet, *floater.closeButton()).width.pixels(), 10.f);
    rdui::detail::WidgetCompilerAccess::setState(*floater.closeButton(), rdui::WidgetState::Hovered, true);
    ensure_equals("interactive part uses its own hover state", rdui::resolveWidgetStyle(stylesheet, *floater.closeButton()).width.pixels(), 18.f);
}

template<> template<> void rduistyle_object::test<8>() {
    rdui::StyleSheet stylesheet;
    ensure("cursor properties compile",
           stylesheet
               .loadRadia("button { cursor: pointer; } #horizontal { cursor: e-resize; } #diagonal { cursor: sw-resize; }"
                          "#grab { cursor: grab; } #grabbing { cursor: grabbing; }")
               .ok());
    ensure_equals("pointer cursor is typed", static_cast<int>(stylesheet.resolve("button", "", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::Pointer));
    ensure_equals("directional horizontal alias collapses to axis", static_cast<int>(stylesheet.resolve("panel", "horizontal", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::EastWestResize));
    ensure_equals("directional diagonal alias collapses to axis", static_cast<int>(stylesheet.resolve("panel", "diagonal", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::NortheastSouthwestResize));
    ensure_equals("grab keeps its logical cursor identity", static_cast<int>(stylesheet.resolve("panel", "grab", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::Grab));
    ensure_equals("grabbing keeps its logical cursor identity", static_cast<int>(stylesheet.resolve("panel", "grabbing", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::Grabbing));
    ensure("grab and grabbing remain distinct",
           stylesheet.resolve("panel", "grab", {}, 0).cursor != stylesheet.resolve("panel", "grabbing", {}, 0).cursor);

    const rdui::StyleSheetLoadResult invalid = stylesheet.loadRadia("button { cursor: teleport; }", "cursor.radia");
    ensure("unknown cursor rejects candidate", !invalid.ok());
    ensure_equals("failed cursor candidate preserves stylesheet", static_cast<int>(stylesheet.resolve("button", "", {}, 0).cursor),
                  static_cast<int>(rdui::CursorStyle::Pointer));
}

template<> template<> void rduistyle_object::test<9>() {
    rdui::StyleSheet stylesheet;
    const rdui::StyleSheetLoadResult valid = stylesheet.loadRadia("switch { padding: 4px; &:checked::thumb { background-color: #ffffffff; } }"
                                                                  "switch::thumb { border-radius: 10px; } label { font-size: 13px; }"
                                                                  "panel { flow: row; font-size: 14px; } switch { flow: row; }"
                                                                  "label { gap: 2px; align-items: center; } icon { font-size: 13px; }"
                                                                  "button > label { stroke-width: 2px; } .copy { font-size: 13px; order: 1; }");
    ensure("valid RSL compiles independently of target relevance", valid.ok());
    ensure("declared target-specific states do not warn", valid.warnings.empty());

    auto rejects = [&](const std::string& css, const std::string& code) {
        const rdui::StyleSheetLoadResult result = stylesheet.loadRadia(css, "contract.radia");
        ensure("invalid RSL rejects stylesheet", !result.ok());
        ensure_equals("RSL diagnostic", result.errors.front().code, code);
    };
    rejects("button::label { stroke-width: 2px; }", "stylesheet.selector.part_unknown");
    rejects("switch::missing { width: 10px; }", "stylesheet.selector.part_unknown");
    rejects("panel { align-items: sideways; }", "stylesheet.property.value_invalid");
    rejects("button { align-self: sideways; }", "stylesheet.property.value_invalid");
    rejects("label { text-align: middle; }", "stylesheet.property.value_invalid");
    rejects("panel { vertical-align: center; }", "stylesheet.property.value_invalid");
    rejects("mystery { width: 10px; }", "stylesheet.selector.element_unknown");
    rejects("label:cheked { opacity: .5; }", "stylesheet.selector.state_unknown");
    rejects("button { --local: 2px; }", "stylesheet.token.root_required");
    rejects("label { order: 1.5; }", "stylesheet.property.value_invalid");
    rejects("label { order: 1px; }", "stylesheet.property.value_invalid");

    const rdui::StyleSheetLoadResult dead_states =
        stylesheet.loadRadia("switch::thumb:checked { width: 10px; } label:checked { opacity: .5; }", "contract.radia");
    ensure("known target-specific states remain valid RSL", dead_states.ok());
    ensure_equals("dead target-specific selectors produce warnings", dead_states.warnings.size(), 2U);
    ensure_equals("dead target-specific selector diagnostic is stable", dead_states.warnings.front().code, "stylesheet.selector.state_never_matches");
}

template<> template<> void rduistyle_object::test<10>() {
    rdui::StyleSheet stylesheet;
    ensure("inherited property stylesheet compiles",
           stylesheet
               .loadRadia("panel { font-family: sans; font-size: 19px; font-weight: bold; font-style: italic;"
                          " line-height: 23px; text-color: #204060ff; text-align: center; vertical-align: bottom; cursor: grab;"
                          " opacity: .5; pointer-events: none; background-color: #ffffffff; }"
                          "label#override { font-size: 11px; cursor: default; }")
               .ok());

    auto parent = std::make_unique<rdui::Panel>();
    auto inherited = std::make_unique<rdui::Label>("Inherited");
    rdui::Label* inherited_label = inherited.get();
    parent->addChild(std::move(inherited));
    auto overridden = std::make_unique<rdui::Label>("Overridden");
    rdui::Label* overridden_label = overridden.get();
    overridden->setId("override");
    parent->addChild(std::move(overridden));

    const rdui::Style inherited_style = rdui::resolveWidgetStyle(stylesheet, *inherited_label);
    ensure_equals("font family inherits", static_cast<int>(inherited_style.font_family), static_cast<int>(rdui::FontFamily::Sans));
    ensure_equals("font size inherits", inherited_style.font_size, 19.f);
    ensure_equals("font weight inherits", inherited_style.font_weight, static_cast<U16>(700));
    ensure("font style inherits", inherited_style.font_italic);
    ensure("line height inherits as a set length", inherited_style.line_height.has_value());
    ensure_equals("line height inherits", inherited_style.line_height->pixels, 23.f);
    ensure_approximately_equals("text color inherits", inherited_style.text_color.b, 96.f / 255.f, 6);
    ensure_equals("text alignment inherits", static_cast<int>(inherited_style.text_align), static_cast<int>(rdui::TextAlign::Center));
    ensure_equals("container vertical alignment does not inherit", static_cast<int>(inherited_style.vertical_align),
                  static_cast<int>(rdui::VerticalAlign::Top));
    ensure_equals("cursor inherits", static_cast<int>(inherited_style.cursor), static_cast<int>(rdui::CursorStyle::Grab));
    ensure_equals("opacity remains local for paint composition", inherited_style.opacity, 1.f);
    ensure_equals("pointer event policy does not inherit", static_cast<int>(inherited_style.pointer_events),
                  static_cast<int>(rdui::PointerEvents::Default));
    ensure_equals("background does not inherit", inherited_style.background_color.a, 0.f);

    const rdui::Style overridden_style = rdui::resolveWidgetStyle(stylesheet, *overridden_label);
    ensure_equals("explicit child font size overrides inheritance", overridden_style.font_size, 11.f);
    ensure_equals("explicit child cursor overrides inheritance", static_cast<int>(overridden_style.cursor),
                  static_cast<int>(rdui::CursorStyle::Default));
}

template<> template<> void rduistyle_object::test<11>() {
    rdui::StyleSheet stylesheet;
    ensure("overflow stylesheet compiles",
           stylesheet.loadRadia("panel { overflow: hidden visible; } #visible { overflow: visible; overflow-y: hidden; }").ok());
    const rdui::Style split = stylesheet.resolve("panel", "", {}, 0);
    ensure_equals("overflow shorthand sets horizontal axis", static_cast<int>(split.overflow_x), static_cast<int>(rdui::Overflow::Hidden));
    ensure_equals("overflow shorthand accepts a distinct vertical axis", static_cast<int>(split.overflow_y),
                  static_cast<int>(rdui::Overflow::Visible));
    const rdui::Style overridden = stylesheet.resolve("panel", "visible", {}, 0);
    ensure_equals("overflow longhand overrides shorthand horizontal value", static_cast<int>(overridden.overflow_x),
                  static_cast<int>(rdui::Overflow::Visible));
    ensure_equals("overflow longhand overrides shorthand vertical value", static_cast<int>(overridden.overflow_y),
                  static_cast<int>(rdui::Overflow::Hidden));

    const rdui::StyleSheetLoadResult invalid = stylesheet.loadRadia("panel { overflow: scroll; }", "overflow.radia");
    ensure("unsupported overflow rejects candidate", !invalid.ok());
    ensure_equals("overflow diagnostic code", invalid.errors.front().code, "stylesheet.property.value_invalid");
}

template<> template<> void rduistyle_object::test<12>() {
    rdui::StyleSheet stylesheet;
    ensure("minimized floater state compiles",
           stylesheet
               .loadRadia("floater { &::header { border-width: 0px 0px 1px; }"
                          " &:minimized::header { border-width: 0px; } }")
               .ok());
    rdui::Floater floater;
    floater.setTitle("title").setCanMinimize(true);

    ensure_equals("expanded header retains content separator", rdui::resolveWidgetStyle(stylesheet, *floater.header()).border_width.bottom, 1.f);
    floater.setMinimized(true);
    ensure("minimize projects widget state", floater.hasState(rdui::WidgetState::Minimized));
    ensure_equals("minimized owner state removes header separator", rdui::resolveWidgetStyle(stylesheet, *floater.header()).border_width.bottom, 0.f);
    floater.setMinimized(false);
    ensure("restore clears widget state", !floater.hasState(rdui::WidgetState::Minimized));
    ensure_equals("restored header regains content separator", rdui::resolveWidgetStyle(stylesheet, *floater.header()).border_width.bottom, 1.f);
}

template<> template<> void rduistyle_object::test<13>() {
    const rdui::Style initial;
    ensure("initial width is explicitly auto", initial.width.isAuto());
    ensure("initial height is explicitly auto", initial.height.isAuto());
    ensure("initial minimum is absent", !initial.min_width.has_value());
    ensure("initial position is absent", !initial.left.has_value());
    ensure("initial line height is absent", !initial.line_height.has_value());
    ensure("initial icon stroke width is absent", !initial.svg_stroke_width.has_value());
    ensure("initial effect list is empty", initial.effects.empty());

    rdui::StyleSheet stylesheet;
    ensure("typed lengths compile", stylesheet.loadRadia("panel { width: 40px; min-width: 20px; left: -8px; line-height: 18px; } ").ok());
    const rdui::Style style = stylesheet.resolve("panel", "", {}, 0);
    ensure("explicit width is not auto", !style.width.isAuto());
    ensure_equals("explicit dimension stores pixels", style.width.pixels(), 40.f);
    ensure_equals("minimum stores a length", style.min_width->pixels, 20.f);
    ensure_equals("negative offset remains a specified length", style.left->pixels, -8.f);
    ensure_equals("line height stores a length", style.line_height->pixels, 18.f);

    ensure("auto dimensions compile",
           stylesheet
               .loadRadia("panel { width: 40px; height: 20px; width: auto; height: auto; }"
                          "button { size: auto; } icon { size: auto 16px; }")
               .ok());
    const rdui::Style automatic = stylesheet.resolve("panel", "", {}, 0);
    ensure("width can be reset to auto", automatic.width.isAuto());
    ensure("height can be reset to auto", automatic.height.isAuto());
    const rdui::Style automatic_size = stylesheet.resolve("button", "", {}, 0);
    ensure("one-value auto size resets width", automatic_size.width.isAuto());
    ensure("one-value auto size resets height", automatic_size.height.isAuto());
    const rdui::Style mixed_size = stylesheet.resolve("icon", "", {}, 0);
    ensure("two-value size accepts auto height", mixed_size.height.isAuto());
    ensure_equals("two-value size retains fixed width", mixed_size.width.pixels(), 16.f);

    ensure("automatic gap compiles", stylesheet.loadRadia("panel { gap: auto; }").ok());
    const rdui::Style automatic_gap = stylesheet.resolve("panel", "", {}, 0);
    ensure("gap retains its automatic state", automatic_gap.gap.isAuto());
    ensure_equals("automatic gap has no intrinsic pixels", automatic_gap.gap.fixedPixels(), 0.f);
}

template<> template<> void rduistyle_object::test<14>() {
    rdui::StyleSheet stylesheet;
    ensure("structural selectors compile",
           stylesheet
               .loadRadia("* { opacity: .8; }"
                          "panel.root > label { width: 10px; }"
                          "panel.root label { height: 11px; }"
                          "panel.root { > label.direct { min-width: 20%; } & > label.direct { right: 5%; }"
                          " label.nested { min-height: 25%; } & label.nested { bottom: 10%; } }")
               .ok());

    rdui::Panel root;
    root.addClass("root");
    auto direct = std::make_unique<rdui::Label>("direct");
    direct->addClass("direct");
    rdui::Label* direct_label = direct.get();
    root.addChild(std::move(direct));

    auto container = std::make_unique<rdui::Panel>();
    auto nested = std::make_unique<rdui::Label>("nested");
    nested->addClass("nested");
    rdui::Label* nested_label = nested.get();
    container->addChild(std::move(nested));
    root.addChild(std::move(container));

    const rdui::Style direct_style = rdui::resolveWidgetStyle(stylesheet, *direct_label);
    ensure_equals("universal selector matches", direct_style.opacity, .8f);
    ensure_equals("unnested child combinator matches direct child", direct_style.width.pixels(), 10.f);
    ensure_equals("unnested descendant combinator matches", direct_style.height.pixels(), 11.f);
    ensure("nested leading child combinator retains percentage", direct_style.min_width->isPercentage());
    ensure_approximately_equals("percentage is stored as a ratio", direct_style.min_width->percent, .2f, 6);
    ensure_approximately_equals("explicit parent child combinator expands", direct_style.right->percent, .05f, 6);

    const rdui::Style nested_style = rdui::resolveWidgetStyle(stylesheet, *nested_label);
    ensure("child combinator does not match a deeper descendant", nested_style.width.isAuto());
    ensure_equals("descendant combinator reaches nested child", nested_style.height.pixels(), 11.f);
    ensure_approximately_equals("bare nested selector becomes descendant", nested_style.min_height->percent, .25f, 6);
    ensure_approximately_equals("explicit parent descendant combinator expands", nested_style.bottom->percent, .1f, 6);
}

template<> template<> void rduistyle_object::test<15>() {
    rdui::StyleSheet stylesheet;
    ensure("box effects compile",
           stylesheet
               .loadRadia("panel { background-color: linear-gradient(to right, #ff0000ff, rgb(0, 255, 0, 50%) 75%, #0000ffff);"
                          " shadow: 1px 2px #11223344, 3px 4px 5px 6px rgb(10, 20, 30, 40%) inset;"
                          " outline: 2px 3px #abcdef88; } label { outline: 1px dashed #ffffffff; }")
               .ok());
    const rdui::Style style = stylesheet.resolve("panel", "", {}, 0);
    ensure("linear gradient is typed", style.background_gradient.has_value());
    ensure("linear gradient kind is typed", style.background_gradient->kind == rdui::GradientKind::Linear);
    ensure_equals("gradient direction is typed", style.background_gradient->angle_degrees, 90.f);
    ensure_equals("gradient preserves all stops", style.background_gradient->stops.size(), 3U);
    ensure_approximately_equals("explicit gradient stop is normalized", style.background_gradient->stops[1].position, .75f, 6);
    ensure_approximately_equals("gradient stop preserves alpha", style.background_gradient->stops[1].color.a, .5f, 6);
    ensure_equals("comma-separated shadows are retained", style.shadows.size(), 2U);
    ensure_equals("short shadow defaults blur", style.shadows[0].blur, 0.f);
    ensure("shadow defaults to outset", !style.shadows[0].inset);
    ensure_equals("long shadow parses spread", style.shadows[1].spread, 6.f);
    ensure("inset modifier is typed", style.shadows[1].inset);
    ensure_equals("outline width is typed", style.outline.width, 2.f);
    ensure_equals("outline offset is typed", style.outline.offset, 3.f);
    ensure_approximately_equals("outline color is typed", style.outline.color.r, 171.f / 255.f, 6);
    ensure_equals("omitted outline offset defaults to zero", stylesheet.resolve("label", "", {}, 0).outline.offset, 0.f);
    ensure("dashed outline style is typed", stylesheet.resolve("label", "", {}, 0).outline.style == rdui::OutlineStyle::Dashed);

    ensure("composed blur effects compile",
           stylesheet
               .loadRadia("panel { effect: background-blur(to bottom, 0px 25%, 16px 75%), layer-blur(4px); }"
                          "button { effect: layer-blur(to right, 0px 50%, 4px 50%); }"
                          "label { effect: none; }")
               .ok());
    const rdui::Style effects = stylesheet.resolve("panel", "", {}, 0);
    ensure_equals("effect list preserves both functions", effects.effects.size(), 2U);
    ensure("first effect targets background", effects.effects[0].kind == rdui::EffectKind::BackgroundBlur);
    ensure_equals("progressive blur stores start radius", effects.effects[0].start_radius, 0.f);
    ensure_equals("progressive blur stores end radius", effects.effects[0].end_radius, 16.f);
    ensure_approximately_equals("progressive blur stores start position", effects.effects[0].start_position, .25f, 6);
    ensure_approximately_equals("progressive blur stores end position", effects.effects[0].end_position, .75f, 6);
    ensure_equals("progressive blur stores direction", effects.effects[0].angle_degrees, 180.f);
    ensure("second effect targets layer", effects.effects[1].kind == rdui::EffectKind::LayerBlur);
    ensure_equals("uniform blur has matching radii", effects.effects[1].end_radius, 4.f);
    ensure("equal progressive blur positions compile", stylesheet.resolve("button", "", {}, 0).effects[0].progressive());
    ensure("effect none clears the list", stylesheet.resolve("label", "", {}, 0).effects.empty());

    ensure("all box gradient kinds compile",
           stylesheet
               .loadRadia("panel { background-color: radial-gradient(circle at 25% 75%, #ffffffff, #00000000 80%);"
                          " border-width: 3px; border-color: conic-gradient(from 45deg at top left, #ff0000ff 0deg 90deg, #0000ffff 100%); "
                          "}"
                          "button { border: 2px repeating-linear-gradient(90deg, #ffffffff 0%, #000000ff 20%); }"
                          "switch { background-color: repeating-radial-gradient(ellipse at center, #ffffffff 0%, #000000ff 25%); }"
                          "floater { background-color: repeating-conic-gradient(from .25turn, #ffffffff 0deg 30deg, #000000ff 60deg); }"
                          "panel.solid { background-color: #112233ff; border-color: #445566ff; }")
               .ok());
    const rdui::Style radial = stylesheet.resolve("panel", "", {}, 0);
    ensure("radial background is typed", radial.background_gradient && radial.background_gradient->kind == rdui::GradientKind::Radial);
    ensure("radial shape is typed", radial.background_gradient->radial_shape == rdui::RadialGradientShape::Circle);
    ensure_approximately_equals("radial x center is normalized", radial.background_gradient->center.x, .25f, 6);
    ensure_approximately_equals("CSS y center maps to paint coordinates", radial.background_gradient->center.y, .25f, 6);
    ensure("conic border-color is typed", radial.border_gradient && radial.border_gradient->kind == rdui::GradientKind::Conic);
    ensure_equals("two-position conic stop expands", radial.border_gradient->stops.size(), 3U);
    ensure_equals("border width remains independent of paint", radial.border_width.top, 3.f);
    const rdui::Style repeating_border = stylesheet.resolve("button", "", {}, 0);
    ensure("border shorthand accepts gradients", repeating_border.border_gradient.has_value());
    ensure("border shorthand retains repeating mode", repeating_border.border_gradient->repeating);
    ensure_equals("border shorthand sets width", repeating_border.border_width.left, 2.f);
    const rdui::Style repeating_radial = stylesheet.resolve("switch", "", {}, 0);
    ensure("repeating radial gradient is typed",
           repeating_radial.background_gradient
               && repeating_radial.background_gradient->kind == rdui::GradientKind::Radial
               && repeating_radial.background_gradient->repeating);
    const rdui::Style repeating_conic = stylesheet.resolve("floater", "", {}, 0);
    ensure("repeating conic gradient is typed",
           repeating_conic.background_gradient
               && repeating_conic.background_gradient->kind == rdui::GradientKind::Conic
               && repeating_conic.background_gradient->repeating);
    ensure_equals("conic from angle is normalized", repeating_conic.background_gradient->angle_degrees, 90.f);
    const rdui::Style solid_override = stylesheet.resolve("panel", "", {"solid"}, 0);
    ensure("solid background replaces gradient paint", !solid_override.background_gradient);
    ensure("solid border replaces gradient paint", !solid_override.border_gradient);
    ensure_approximately_equals("solid border color is retained", solid_override.border_color.g, 85.f / 255.f, 6);

    auto rejects = [&](const std::string& css) { ensure("invalid box effect rejects stylesheet", !stylesheet.loadRadia(css, "effects.radia").ok()); };
    rejects("panel { background-color: linear-gradient(#fff); }");
    rejects("panel { background-color: radial-gradient(square, #fff, #000); }");
    rejects("panel { border-color: conic-gradient(from nowhere, #fff, #000); }");
    rejects("panel { border: 1px repeating-linear-gradient(#fff 20%, #000 20%); }");
    rejects("panel { background: #ffffffff; }");
    rejects("panel { shadow: 0 0 -1px #000; }");
    rejects("panel { outline: 10% #000; }");
    rejects("panel { outline: 1px dashed solid #000; }");
    rejects("panel { effect: blur(4px); }");
    rejects("panel { effect: layer-blur(to bottom, 4px); }");
    rejects("panel { effect: background-blur(to nowhere, 0px 0%, 4px 100%); }");
    rejects("panel { effect: background-blur(to bottom, 0px, 4px 100%); }");
    rejects("panel { effect: background-blur(to bottom, 0px 75%, 4px 25%); }");
    rejects("panel { effect: layer-blur(-1px); }");
    rejects("panel { effect: layer-blur(2px), background-blur(4px); }");
    rejects("panel { effect: background-blur(2px) layer-blur(4px); }");
}

template<> template<> void rduistyle_object::test<16>() {
    rdui::StyleSheet stylesheet;
    ensure("typed min-size declarations compile",
           stylesheet
               .loadRadia("panel.one { min-size: 24px; }"
                          "panel.two { min-size: 30% 80px; }"
                          "panel.longhand-after { min-size: 10px 20px; min-width: 40px; }"
                          "panel.shorthand-after { min-height: 5px; min-size: 12px 18px; }")
               .ok());

    const rdui::Style one = stylesheet.resolve("panel", "", {"one"}, 0);
    ensure("one-value min-size sets minimum height", one.min_height.has_value());
    ensure("one-value min-size sets minimum width", one.min_width.has_value());
    ensure_equals("one-value minimum height", one.min_height->pixels, 24.f);
    ensure_equals("one-value minimum width", one.min_width->pixels, 24.f);

    const rdui::Style two = stylesheet.resolve("panel", "", {"two"}, 0);
    ensure_approximately_equals("first min-size value is height", two.min_height->percent, .3f, 6);
    ensure_equals("second min-size value is width", two.min_width->pixels, 80.f);

    const rdui::Style longhand_after = stylesheet.resolve("panel", "", {"longhand-after"}, 0);
    ensure_equals("later longhand overrides expanded width", longhand_after.min_width->pixels, 40.f);
    ensure_equals("later longhand preserves expanded height", longhand_after.min_height->pixels, 10.f);

    const rdui::Style shorthand_after = stylesheet.resolve("panel", "", {"shorthand-after"}, 0);
    ensure_equals("later shorthand overrides height", shorthand_after.min_height->pixels, 12.f);
    ensure_equals("later shorthand supplies width", shorthand_after.min_width->pixels, 18.f);

    auto rejects = [&](const std::string& value) {
        const rdui::StyleSheetLoadResult result = stylesheet.loadRadia("panel { min-size: " + value + "; }", "min-size.radia");
        ensure("invalid min-size rejects the candidate", !result.ok());
        ensure_equals("min-size value diagnostic is stable", result.errors.front().code, "stylesheet.property.value_invalid");
    };
    rejects("auto");
    rejects("-1px");
    rejects("1px 2px 3px");

    const rdui::Style preserved = stylesheet.resolve("panel", "", {"one"}, 0);
    ensure_equals("failed candidate preserves the prior stylesheet", preserved.min_width->pixels, 24.f);
}

template<> template<> void rduistyle_object::test<17>() {
    rdui::StyleSheet original;
    ensure("original stylesheet loads", original.loadRadia("panel { width: 10px; }", "original.radia").ok());
    const std::uint64_t copied_generation = original.generation();

    rdui::StyleSheet copy = original;
    ensure("replacement stylesheet loads", original.loadRadia("panel { width: 20px; }", "replacement.radia").ok());
    ensure_equals("copy preserves compiled rules", copy.resolve("panel", "", {}, 0).width.pixels(), 10.f);
    ensure_equals("copy preserves generation", copy.generation(), copied_generation);
    ensure_equals("original changes independently", original.resolve("panel", "", {}, 0).width.pixels(), 20.f);

    rdui::StyleSheet assigned;
    assigned = copy;
    ensure_equals("copy assignment preserves compiled rules", assigned.resolve("panel", "", {}, 0).width.pixels(), 10.f);

    rdui::StyleSheet moved = std::move(assigned);
    ensure_equals("move construction preserves compiled rules", moved.resolve("panel", "", {}, 0).width.pixels(), 10.f);
}

template<> template<> void rduistyle_object::test<18>() {
    rdui::StyleSheet stylesheet;
    ensure("style layers compile together",
           stylesheet
               .loadRadiaLayers({
                   {"base/skin.radia", "panel { width: 10px; height: 30px; }"},
                   {"derived/skin.radia", "panel { width: 20px; }"},
               })
               .ok());
    const rdui::Style resolved = stylesheet.resolve("panel", "", {}, 0);
    ensure_equals("derived source order wins", resolved.width.pixels(), 20.f);
    ensure_equals("base declarations remain", resolved.height.pixels(), 30.f);

    const auto malformed = stylesheet.loadRadiaLayers({
        {"base/skin.radia", "panel { width: 10px; }"},
        {"derived/skin.radia", "not a rule"},
    });
    ensure("malformed derived layer rejects all layers", !malformed.ok());
    ensure_equals("diagnostic identifies derived layer", malformed.errors.front().source, std::string("derived/skin.radia"));
    ensure_equals("failed layered candidate preserves live stylesheet", stylesheet.resolve("panel", "", {}, 0).width.pixels(), 20.f);
}

template<> template<> void rduistyle_object::test<19>() {
    rdui::StyleSheet stylesheet;
    rdui::ResourceLayer layer{
        "theme/main.radia",
        "@import \"components/panel.radia\";\n"
        ":root { --panel-width: 12px; }\n"
        "panel { width: var(--panel-width); }\n"
        "panel { width: 30px; }",
    };
    layer.entrypoint = "main.radia";
    layer.modules = {
        {"foundation/sizes.radia", ":root { --panel-height: 18px; }"},
        {"components/panel.radia",
         "@import \"../foundation/sizes.radia\";\n"
         "panel { width: var(--panel-width); height: var(--panel-height); }"},
    };

    ensure("recursive imports compile", stylesheet.loadRadiaLayers({layer}).ok());
    const rdui::Style resolved = stylesheet.resolve("panel", "", {}, 0);
    ensure_equals("later entrypoint rule retains source-order precedence", resolved.width.pixels(), 30.f);
    ensure_equals("nested imported token is available", resolved.height.pixels(), 18.f);

    const auto& dependencies = stylesheet.dependencies();
    ensure("entrypoint dependencies recorded", dependencies.contains("theme/main.radia"));
    ensure("entrypoint records component module", dependencies.at("theme/main.radia").contains("theme/components/panel.radia"));
    ensure("nested dependency recorded", dependencies.contains("theme/components/panel.radia"));
    ensure("nested dependency resolves relative path", dependencies.at("theme/components/panel.radia").contains("theme/foundation/sizes.radia"));
}

template<> template<> void rduistyle_object::test<20>() {
    rdui::StyleSheet stylesheet;
    ensure("baseline compiles", stylesheet.loadRadia("panel { width: 44px; }").ok());

    auto layer = [](std::string source) {
        rdui::ResourceLayer result{"theme/main.radia", std::move(source)};
        result.entrypoint = "main.radia";
        return result;
    };

    auto missing = layer("\n@import \"missing.radia\";");
    const auto missing_result = stylesheet.loadRadiaLayers({missing});
    ensure("missing import rejects candidate", !missing_result.ok());
    ensure_equals("missing diagnostic code", missing_result.errors.front().code, std::string("stylesheet.import.missing"));
    ensure_equals("missing diagnostic line", missing_result.errors.front().line, std::size_t(2));

    auto cycle = layer("@import \"cycle.radia\";");
    cycle.modules["cycle.radia"] = "@import \"main.radia\";";
    const auto cycle_result = stylesheet.loadRadiaLayers({cycle});
    ensure("import cycle rejects candidate", !cycle_result.ok());
    ensure_equals("cycle diagnostic code", cycle_result.errors.front().code, std::string("stylesheet.import.cycle"));

    const auto traversal_result = stylesheet.loadRadiaLayers({layer("@import \"../outside.radia\";")});
    ensure("escaping import rejects candidate", !traversal_result.ok());
    ensure_equals("path diagnostic code", traversal_result.errors.front().code, std::string("stylesheet.import.path_invalid"));

    auto malformed = layer("@import \"broken.radia\";");
    malformed.modules["broken.radia"] = "panel { width: ; }";
    const auto malformed_result = stylesheet.loadRadiaLayers({malformed});
    ensure("malformed imported module rejects candidate", !malformed_result.ok());
    ensure_equals("imported diagnostic source", malformed_result.errors.front().source, std::string("theme/broken.radia"));
    ensure("imported diagnostic includes dependency chain",
           malformed_result.errors.front().message.find("main.radia -> broken.radia") != std::string::npos);

    auto late = layer("panel { width: 1px; } @import \"late.radia\";");
    late.modules["late.radia"] = "panel { height: 2px; }";
    const auto late_result = stylesheet.loadRadiaLayers({late});
    ensure("late import rejects candidate", !late_result.ok());
    ensure_equals("late import diagnostic", late_result.errors.front().code, std::string("stylesheet.import.order"));

    ensure_equals("failed imports preserve live stylesheet", stylesheet.resolve("panel", "", {}, 0).width.pixels(), 44.f);
}

template<> template<> void rduistyle_object::test<21>() {
    rdui::StyleSheet stylesheet;
    ensure("RSL type selector lookup is ASCII case-insensitive", stylesheet.loadRadia("BuTtOn { width: 23px; }").ok());
    ensure_equals("RSL stores the canonical Widget spelling for matching", stylesheet.resolve("button", "", {}, 0).width.pixels(), 23.f);

    const auto invalid_id = stylesheet.loadRadia("button#bad_id { width: 1px; }");
    ensure("RSL rejects non-kebab Widget IDs", !invalid_id.ok());
    ensure_equals("RSL invalid identifier diagnostic is stable", invalid_id.errors.front().code,
                  std::string("stylesheet.selector.identifier_invalid"));

    const auto invalid_part = stylesheet.loadRadia("floater::Header { width: 1px; }");
    ensure("RSL rejects non-kebab Part names", !invalid_part.ok());
    ensure_equals("RSL invalid Part diagnostic is stable", invalid_part.errors.front().code, std::string("stylesheet.selector.part_invalid"));
}

template<> template<> void rduistyle_object::test<22>() {
    rdui::StyleSheet stylesheet;
    ensure("Field invalid-state selector compiles", stylesheet.loadRadia("field { opacity: 1; } field:invalid { opacity: .6; }").ok());
    const uint8_t invalid = static_cast<uint8_t>(rdui::WidgetState::Invalid);
    ensure_equals("Field invalid-state selector matches", stylesheet.resolve("field", "", {}, invalid).opacity, .6f);

    const auto unsupported = stylesheet.loadRadia("button:invalid { opacity: .5; }");
    ensure("invalid state remains target-specific", unsupported.ok());
    ensure_equals("unsupported invalid state reports one warning", unsupported.warnings.size(), 1U);
    ensure_equals("unsupported invalid-state diagnostic is stable", unsupported.warnings.front().code,
                  std::string("stylesheet.selector.state_never_matches"));
}

template<> template<> void rduistyle_object::test<23>() {
    rdui::StyleSheet stylesheet;
    ensure("nested Kbd selectors compile",
           stylesheet
               .loadRadia("kbd { padding: 1px; border-radius: 4px; "
                          "  > kbd { padding: 2px; border-radius: 3px; } "
                          "} "
                          "text > kbd { gap: 5px; }")
               .ok());

    rdui::Text owner;
    const rdui::Style chord = stylesheet.resolveInline(owner, "kbd");
    const rdui::Style key = stylesheet.resolveInline(owner, "kbd", {"kbd"});
    ensure_equals("Kbd styles the complete chord", chord.padding.left, 1.f);
    ensure_equals("Text can contextually style its direct Kbd content", chord.gap.fixedPixels(), 5.f);
    ensure_equals("nested Kbd styles each generated key", key.padding.left, 2.f);
    ensure_equals("nested Kbd overrides the shared outer radius", key.border_radius, 3.f);
    ensure("direct Text child selector does not leak through the generated Kbd parent", key.gap.fixedPixels() != 5.f);

    const auto rejected_part = stylesheet.loadRadia("kbd::key { padding: 1px; }");
    ensure("Kbd does not expose a synthetic key Part", !rejected_part.ok());
    ensure_equals("unknown Kbd Part diagnostic is stable", rejected_part.errors.front().code, std::string("stylesheet.selector.part_unknown"));
}
} // namespace tut
