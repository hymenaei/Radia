#include "linden_common.h"
#include "../test/lltut.h"

#include "rdbutton.h"
#include "rdfield.h"
#include "rdfloater.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdtext.h"
#include "rduilayout.h"
#include "rduistylesheet.h"
#include "rduiviewcontract.h"

namespace tut
{
    struct rduistyle_data {};
    typedef test_group<rduistyle_data> rduistyleproperty_test;
    typedef rduistyleproperty_test::object rduistyleproperty_object;
    rduistyleproperty_test rduistyleproperty_testcase("rduistyleproperty");

    template<> template<>
    void rduistyleproperty_object::test<1>()
    {
        rdui::StyleSheet stylesheet;
        ensure("selector stylesheet loads", stylesheet.loadRadia("button.primary:hover > icon { width: 17px; }").ok());
        rdui::Button button;
        button.addClass("primary");
        rdui::detail::WidgetCompilerAccess::setState(button, rdui::WidgetState::Hovered, true);
        rdui::Icon& icon = button.setIcon("search");
        ensure_equals("compiled selector matches element, class, state, and child",
                      rdui::resolveWidgetStyle(stylesheet, icon).width.pixels(), 17.f);
    }

    template<> template<>
    void rduistyleproperty_object::test<2>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia(":root { --accent: #204060ff; --space: 12px; } button { background-color: var(--accent); padding: var(--space); border-radius: 5px; }");
        const rdui::Style style = theme.resolve("button", "", {}, 0);
        ensure_approximately_equals("color token", style.background_color.b, 96.f / 255.f, 6);
        ensure_equals("number token", style.padding.left, 12.f);
        ensure_equals("radius", style.border_radius, 5.f);
    }

    template<> template<>
    void rduistyleproperty_object::test<3>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button { width: 10px; size: 20px 30px; width: 40px; }");
        const rdui::Style style = theme.resolve("button", "", {}, 0);
        ensure_equals("later declaration wins", style.width.pixels(), 40.f);
        ensure_equals("size height retained", style.height.pixels(), 20.f);
    }

    template<> template<>
    void rduistyleproperty_object::test<4>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button.primary { width: 30px; } button { width: 10px; } #save { width: 50px; }");
        const std::set<std::string> classes{"primary"};
        ensure_equals("specificity sorted at load", theme.resolve("button", "save", classes, 0).width.pixels(), 50.f);
        ensure_equals("class beats later element", theme.resolve("button", "", classes, 0).width.pixels(), 30.f);
    }

    template<> template<>
    void rduistyleproperty_object::test<5>()
    {
        rdui::StyleSheet theme;
        ensure("nested stylesheet loads", theme.loadRadia(
            "button { background-color: #101010ff; &:hover { background-color: #202020ff; } "
            "> icon { size: 16px; } &:hover > icon { stroke-width: 3px; } }").ok());
        const uint8_t hover = rdui::WidgetState::Hovered | rdui::WidgetState::Default;
        ensure_equals("nested state", theme.resolve("button", "", {}, hover).background_color.r, 32.f / 255.f);
        rdui::Button button;
        rdui::detail::WidgetCompilerAccess::setState(button, rdui::WidgetState::Hovered, true);
        const rdui::Style icon = rdui::resolveWidgetStyle(theme, button.setIcon("search"));
        ensure_equals("nested child width", icon.width.pixels(), 16.f);
        ensure("nested child stroke width is set", icon.svg_stroke_width.has_value());
        ensure_equals("nested owner state", icon.svg_stroke_width->pixels, 3.f);
    }

    template<> template<>
    void rduistyleproperty_object::test<6>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { flow: row; vertical-align: middle; pointer-events: none; } label { text-align: right; pointer-events: auto; }");
        const rdui::Style panel = theme.resolve("panel", "", {}, 0);
        ensure_equals("row enum", static_cast<int>(panel.flow), static_cast<int>(rdui::Flow::Row));
        ensure_equals("pointer enum", static_cast<int>(panel.pointer_events), static_cast<int>(rdui::PointerEvents::PassThrough));
        const rdui::Style label = theme.resolve("label", "", {}, 0);
        ensure_equals("horizontal enum", static_cast<int>(label.text_align), static_cast<int>(rdui::TextAlign::Right));
        ensure_equals("container vertical enum", static_cast<int>(panel.vertical_align), static_cast<int>(rdui::VerticalAlign::Middle));
        ensure_equals("vertical alignment remains local", static_cast<int>(label.vertical_align), static_cast<int>(rdui::VerticalAlign::Top));
        ensure_equals("auto pointer enum", static_cast<int>(label.pointer_events), static_cast<int>(rdui::PointerEvents::Auto));
        ensure("logical text alignment compiles", theme.loadRadia("label { text-align: start; }").ok());
        ensure_equals("logical text alignment remains distinct", static_cast<int>(theme.resolve("label", "", {}, 0).text_align),
                      static_cast<int>(rdui::TextAlign::Start));
        ensure("cross-axis alignment compiles", theme.loadRadia(
            "panel { align-items: end; } panel.normal { align-items: normal; } "
            "button { align-self: start; } button.auto { align-self: auto; }").ok());
        ensure_equals("container cross-axis alignment is typed",
                      static_cast<int>(theme.resolve("panel", "", {}, 0).align_items),
                      static_cast<int>(rdui::AlignItems::End));
        ensure_equals("flow-item cross-axis override is typed",
                      static_cast<int>(theme.resolve("button", "", {}, 0).align_self),
                      static_cast<int>(rdui::AlignSelf::Start));
        ensure_equals("normal container alignment can be authored",
                      static_cast<int>(theme.resolve("panel", "", {"normal"}, 0).align_items),
                      static_cast<int>(rdui::AlignItems::Normal));
        ensure_equals("auto flow-item alignment can be authored",
                      static_cast<int>(theme.resolve("button", "", {"auto"}, 0).align_self),
                      static_cast<int>(rdui::AlignSelf::Auto));
    }

    template<> template<>
    void rduistyleproperty_object::test<7>()
    {
        rdui::StyleSheet theme;
        ensure("independent typography properties compile", theme.loadRadia(
            "label#a { font-family: sans; font-size: 19px; font-weight: bold; font-style: italic; }").ok());
        const rdui::Style a = theme.resolve("label", "a", {}, 0);
        ensure_equals("font family", static_cast<int>(a.font_family), static_cast<int>(rdui::FontFamily::Sans));
        ensure_equals("font size", a.font_size, 19.f);
        ensure_equals("bold", a.font_weight, static_cast<U16>(700));
        ensure("italic", a.font_italic);

        rdui::StyleSheet variable_weight;
        ensure("numeric variable font weights compile",
               variable_weight.loadRadia("label { font-weight: 525; }").ok());
        ensure_equals("numeric weight is preserved",
                      variable_weight.resolve("label", "", {}, 0).font_weight,
                      static_cast<U16>(525));

        rdui::StyleSheet pseudo_family;
        ensure("weight is not accepted as a pseudo font family",
               !pseudo_family.loadRadia("label { font-family: sans-bold; }").ok());
        ensure("fractional CSS weights are rejected",
               !pseudo_family.loadRadia("label { font-weight: 525.5; }").ok());
        ensure("CSS weights are unitless",
               !pseudo_family.loadRadia("label { font-weight: 700px; }").ok());
    }

    template<> template<>
    void rduistyleproperty_object::test<8>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button { border: 1px #112233ff; border-width: 2px 3px; border-color: #ffffffff; } button > icon { stroke: 4px #abcdef88; stroke-linecap: square; }");
        const rdui::Style style = theme.resolve("button", "", {}, 0);
        ensure_equals("ordered border width top", style.border_width.top, 2.f);
        ensure_equals("ordered border width right", style.border_width.right, 3.f);
        ensure_equals("border color override", style.border_color.r, 1.f);
        rdui::Button button;
        rdui::Icon& button_icon = button.setIcon("search");
        const rdui::Style icon = rdui::resolveWidgetStyle(theme, button_icon);
        ensure("svg width is set", icon.svg_stroke_width.has_value());
        ensure_equals("svg width", icon.svg_stroke_width->pixels, 4.f);
        ensure_equals("cap", static_cast<int>(icon.svg_stroke_cap), static_cast<int>(rdui::StrokeCap::Square));
    }

    template<> template<>
    void rduistyleproperty_object::test<9>()
    {
        rdui::StyleSheet theme;
        const rdui::StyleSheetLoadResult result = theme.loadRadia("panel { flow: grid; } panel#bad { flow: sideways; }", "test.radia");
        ensure_equals("grid falls back", static_cast<int>(theme.resolve("panel", "", {}, 0).flow), static_cast<int>(rdui::Flow::Free));
        ensure_equals("unknown falls back", static_cast<int>(theme.resolve("panel", "bad", {}, 0).flow), static_cast<int>(rdui::Flow::Free));
        ensure("unknown flow prevents commit", !result.ok());
        ensure_equals("unsupported flow warns", result.warnings.size(), 1U);
        ensure_equals("unknown flow errors", result.errors.size(), 1U);
        ensure_equals("flow diagnostic identifies source", result.errors.front().source, "test.radia");
    }

    template<> template<>
    void rduistyleproperty_object::test<10>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button, switch { height: 32px; } button > icon { width: 14px; } button:disabled { opacity: .5; }");
        ensure_equals("selector list button", theme.resolve("button", "", {}, 0).height.pixels(), 32.f);
        ensure_equals("selector list switch", theme.resolve("switch", "", {}, 0).height.pixels(), 32.f);
        rdui::Button button;
        ensure_equals("direct icon child", rdui::resolveWidgetStyle(theme, button.setIcon("search")).width.pixels(), 14.f);
        const uint8_t disabled = static_cast<uint8_t>(rdui::WidgetState::Disabled);
        ensure_equals("state selector", theme.resolve("button", "", {}, disabled).opacity, .5f);
    }

    template<> template<>
    void rduistyleproperty_object::test<11>()
    {
        rdui::StyleSheet theme;
        ensure("flow item stylesheet compiles", theme.loadRadia(
            "panel { padding: 1px 2px 3px 4px; min-width: 20px; min-height: 10px; gap: 7px; flex: 2 3 40%; order: -2; }"
            "panel.auto { flex: auto; } panel.none { flex: none; } panel.one { flex: 4; } panel.two { flex: 5 6; }"
            "panel.basis { flex: 10px; }").ok());
        const rdui::Style style = theme.resolve("panel", "", {}, 0);
        ensure_equals("padding top", style.padding.top, 1.f);
        ensure_equals("padding right", style.padding.right, 2.f);
        ensure_equals("padding bottom", style.padding.bottom, 3.f);
        ensure_equals("padding left", style.padding.left, 4.f);
        ensure("min width is set", style.min_width.has_value());
        ensure_equals("min width", style.min_width->pixels, 20.f);
        ensure_equals("gap", style.gap.fixedPixels(), 7.f);
        ensure_equals("flex grow", style.flex_grow, 2.f);
        ensure_equals("flex shrink", style.flex_shrink, 3.f);
        ensure_approximately_equals("flex basis percentage", style.flex_basis.resolve(0.f, 200.f), 80.f, 6);
        ensure_equals("order", style.order, -2);
        const rdui::Style automatic = theme.resolve("panel", "", {"auto"}, 0);
        ensure_equals("flex auto grows", automatic.flex_grow, 1.f);
        ensure_equals("flex auto shrinks", automatic.flex_shrink, 1.f);
        ensure("flex auto basis remains automatic", automatic.flex_basis.isAuto());
        const rdui::Style none = theme.resolve("panel", "", {"none"}, 0);
        ensure_equals("flex none does not grow", none.flex_grow, 0.f);
        ensure_equals("flex none does not shrink", none.flex_shrink, 0.f);
        ensure("flex none basis remains automatic", none.flex_basis.isAuto());
        ensure_equals("one-number flex sets grow", theme.resolve("panel", "", {"one"}, 0).flex_grow, 4.f);
        ensure_equals("one-number flex defaults basis to zero", theme.resolve("panel", "", {"one"}, 0).flex_basis.resolve(1.f), 0.f);
        const rdui::Style two = theme.resolve("panel", "", {"two"}, 0);
        ensure_equals("two-number flex sets grow", two.flex_grow, 5.f);
        ensure_equals("two-number flex sets shrink", two.flex_shrink, 6.f);
        const rdui::Style basis = theme.resolve("panel", "", {"basis"}, 0);
        ensure_equals("one-length flex defaults grow", basis.flex_grow, 1.f);
        ensure_equals("one-length flex sets basis", basis.flex_basis.resolve(0.f), 10.f);
        rdui::StyleSheet legacy;
        ensure("legacy grow property is rejected", !legacy.loadRadia("panel { grow: 1; }").ok());
        ensure("flex grow remains unitless", !legacy.loadRadia("panel { flex-grow: 1px; }").ok());
    }

    template<> template<>
    void rduistyleproperty_object::test<12>()
    {
        const rdui::Style style;
        ensure_equals("default flow", static_cast<int>(style.flow), static_cast<int>(rdui::Flow::Free));
        ensure_equals("default justification", static_cast<int>(style.justify_content), static_cast<int>(rdui::JustifyContent::Start));
        ensure_equals("default container alignment", static_cast<int>(style.align_items), static_cast<int>(rdui::AlignItems::Normal));
        ensure_equals("default flow-item alignment", static_cast<int>(style.align_self), static_cast<int>(rdui::AlignSelf::Auto));
        ensure_equals("default flex grow", style.flex_grow, 0.f);
        ensure_equals("default flex shrink", style.flex_shrink, 1.f);
        ensure("default flex basis is automatic", style.flex_basis.isAuto());
        ensure_equals("default flow-item order", style.order, 0);
        ensure("default gap is fixed", !style.gap.isAuto());
        ensure_equals("default gap is zero", style.gap.fixedPixels(), 0.f);
        ensure_equals("default pointer behavior", static_cast<int>(style.pointer_events), static_cast<int>(rdui::PointerEvents::Default));
        ensure_equals("default font", static_cast<int>(style.font_family), static_cast<int>(rdui::FontFamily::Sans));
        ensure_equals("default container vertical alignment is top", static_cast<int>(style.vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Top));
        rdui::StyleSheet button_theme;
        rdui::Button button;
        const rdui::Style button_defaults = rdui::resolveWidgetStyle(button_theme, button);
        ensure_equals("Button intrinsically uses row flow", static_cast<int>(button_defaults.flow),
                      static_cast<int>(rdui::Flow::Row));
        ensure_equals("Button intrinsically centers content horizontally",
                      static_cast<int>(button_defaults.justify_content),
                      static_cast<int>(rdui::JustifyContent::Center));
        ensure_equals("Button intrinsically aligns content to the middle",
                      static_cast<int>(button_defaults.vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Middle));
        ensure("explicit Button layout compiles",
               button_theme.loadRadia(
                   "button { flow: column; justify-content: end; vertical-align: top; }").ok());
        const rdui::Style authored_button = rdui::resolveWidgetStyle(button_theme, button);
        ensure_equals("authored Button flow overrides its intrinsic default",
                      static_cast<int>(authored_button.flow), static_cast<int>(rdui::Flow::Column));
        ensure_equals("authored Button justification overrides its intrinsic default",
                      static_cast<int>(authored_button.justify_content),
                      static_cast<int>(rdui::JustifyContent::End));
        ensure_equals("authored Button vertical alignment overrides its intrinsic default",
                      static_cast<int>(authored_button.vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Top));

        rdui::StyleSheet floater_theme;
        rdui::Floater floater;
        rdui::Panel* custom_header = nullptr;
        for (const auto& child : floater.header()->children())
        {
            if (child->part() == "header::custom") custom_header = static_cast<rdui::Panel*>(child.get());
        }
        ensure("Floater custom-header Part exists", custom_header != nullptr);
        ensure_equals("Floater header intrinsically aligns content to the middle",
                      static_cast<int>(rdui::resolveWidgetStyle(floater_theme, *floater.header()).vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Middle));
        ensure_equals("Floater custom header intrinsically aligns content to the middle",
                      static_cast<int>(rdui::resolveWidgetStyle(floater_theme, *custom_header).vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Middle));

        rdui::StyleSheet field_theme;
        rdui::Field field;
        ensure_equals("Field intrinsically aligns content to the middle",
                      static_cast<int>(rdui::resolveWidgetStyle(field_theme, field).vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Middle));
        ensure("explicit Field vertical alignment compiles",
               field_theme.loadRadia("field { vertical-align: bottom; }").ok());
        ensure_equals("authored Field vertical alignment overrides its intrinsic default",
                      static_cast<int>(rdui::resolveWidgetStyle(field_theme, field).vertical_align),
                      static_cast<int>(rdui::VerticalAlign::Bottom));
        ensure_equals("initial box fill is presentation-neutral", style.background_color.a, 0.f);
    }

}
