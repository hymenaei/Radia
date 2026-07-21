#include "linden_common.h"
#include "../test/lltut.h"

#include "../rduiinputbridge.h"

#include "indra_constants.h"
#include "llkeyboard.h"

#include <variant>

namespace tut
{
    struct rdui_input_bridge
    {
        rdui::viewer::RduiInputBridge bridge;
    };

    using rdui_input_bridge_group = test_group<rdui_input_bridge>;
    using rdui_input_bridge_object = rdui_input_bridge_group::object;
    rdui_input_bridge_group rdui_input_bridge_tests("RduiInputBridge");

    template<> template<>
    void rdui_input_bridge_object::test<1>()
    {
        set_test_name("pointer input translates once into RDUI coordinates and flags");
        const rdui::viewer::SurfaceInputEvent translated = bridge.translate(
            rdui::viewer::NativePointerInput{
                rdui::viewer::NativePointerPhase::Down, 12.5f, 34.25f,
                rdui::viewer::NativePointerButton::Auxiliary1,
                MASK_SHIFT | MASK_ALT, 2, 3.5f, -4.f});

        const auto* pointer = std::get_if<rdui::viewer::SurfacePointerInput>(&translated);
        ensure("pointer event produced", pointer != nullptr);
        ensure("phase preserved", pointer->phase == rdui::viewer::NativePointerPhase::Down);
        ensure_equals("fractional x remains canonical", pointer->event.position.x, 12.5f);
        ensure_equals("fractional y remains canonical", pointer->event.position.y, 34.25f);
        ensure("button translated", pointer->event.button == rdui::PointerButton::Auxiliary1);
        ensure_equals("modifiers translated", pointer->event.modifiers,
                      rdui::MODIFIER_SHIFT | rdui::MODIFIER_ALT);
        ensure_equals("click count preserved", pointer->event.clickCount, std::uint8_t{2});
        ensure_equals("x delta preserved", pointer->event.delta.x, 3.5f);
        ensure_equals("y delta preserved", pointer->event.delta.y, -4.f);
    }

    template<> template<>
    void rdui_input_bridge_object::test<2>()
    {
        set_test_name("viewer keys and modifiers translate to RDUI meanings");
        const rdui::viewer::SurfaceInputEvent translated = bridge.translate(
            rdui::viewer::NativeKeyInput{
                KEY_PAD_RETURN, MASK_CONTROL | MASK_MAC_CONTROL, false, true});

        const auto* key = std::get_if<rdui::viewer::SurfaceKeyInput>(&translated);
        ensure("key event produced", key != nullptr);
        ensure("key-up preserved", !key->down);
        ensure_equals("key translated", key->event.key, rdui::KEY_RETURN);
        ensure_equals("modifiers translated", key->event.modifiers,
                      rdui::MODIFIER_CONTROL | rdui::MODIFIER_PLATFORM_CONTROL);
        ensure("repeat preserved", key->event.repeated);
    }

    template<> template<>
    void rdui_input_bridge_object::test<3>()
    {
        set_test_name("scroll and interaction loss retain their semantics");
        const auto scroll = bridge.translate(rdui::viewer::NativeScrollInput{
            8, 9, -1.f, 2.f, MASK_CONTROL});
        const auto* event = std::get_if<rdui::ScrollEvent>(&scroll);
        ensure("scroll event produced", event != nullptr);
        ensure_equals("horizontal delta preserved", event->dx, -1.f);
        ensure_equals("vertical delta preserved", event->dy, 2.f);
        ensure_equals("scroll modifiers translated", event->modifiers, rdui::MODIFIER_CONTROL);

        const auto loss = bridge.translate(rdui::viewer::NativeInteractionLoss::Capture);
        const auto* reason = std::get_if<rdui::viewer::NativeInteractionLoss>(&loss);
        ensure("interaction loss produced", reason != nullptr);
        ensure("capture loss preserved", *reason == rdui::viewer::NativeInteractionLoss::Capture);
    }

    template<> template<>
    void rdui_input_bridge_object::test<4>()
    {
        set_test_name("cursor mapping is shared by main and detached windows");
        ensure("pointer cursor", bridge.translateCursor(rdui::CursorStyle::Pointer) == UI_CURSOR_HAND);
        ensure("horizontal resize cursor",
               bridge.translateCursor(rdui::CursorStyle::EastWestResize) == UI_CURSOR_SIZEWE);
        ensure("fallback cursor", bridge.translateCursor(rdui::CursorStyle::Auto) == UI_CURSOR_ARROW);
    }
}
