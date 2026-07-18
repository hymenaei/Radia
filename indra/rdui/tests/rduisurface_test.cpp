#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rduibinder.h"
#include "rduirecordingpaintcontext.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"
#include <algorithm>

namespace tut
{
    class InputProbe final : public rdui::Widget
    {
        public:
            InputProbe() : Widget("input_probe") {}

            bool defaultPointerEvents() const override { return true; }
            bool focusable() const override { return true; }
            bool beginPointerInteraction(const rdui::PointerEvent& event) override
            {
                lastClickCount = event.clickCount;
                return false;
            }
            bool defaultCharacterInput(unsigned int codepoint) override
            {
                lastCodepoint = codepoint;
                return true;
            }
            bool defaultScroll(const rdui::ScrollEvent& event) override
            {
                lastScrollX = event.dx;
                lastScrollY = event.dy;
                return true;
            }

            uint8_t lastClickCount = 0;
            unsigned int lastCodepoint = 0;
            float lastScrollX = 0.f;
            float lastScrollY = 0.f;
    };

    class CaptureProbe final : public rdui::Widget
    {
        public:
            CaptureProbe() : Widget("capture_probe") {}

            bool defaultPointerEvents() const override { return true; }
            bool beginPointerInteraction(const rdui::PointerEvent&) override { return true; }
            bool endPointerInteraction(const rdui::PointerEvent&) override
            {
                ++ends;
                return true;
            }

            int ends = 0;
    };

    class PaintProbe final : public rdui::Widget
    {
        public:
            PaintProbe() : Widget("paint_probe") {}

            bool defaultPointerEvents() const override { return true; }
            bool focusable() const override { return true; }
            void paint(rdui::PaintContext&, const rdui::Style&, float) const override { ++paints; }

            mutable int paints = 0;
    };

    class RoutedProbe final : public rdui::Widget
    {
        public:
            RoutedProbe(std::string name, std::vector<std::string>& log)
                : Widget("routed_probe"), mName(std::move(name)), mLog(log) {}

            bool defaultPointerEvents() const override { return true; }
            bool beginPointerInteraction(const rdui::PointerEvent&) override
            {
                ++begins;
                return true;
            }

            bool preventDefault = false;
            int begins = 0;

        protected:
            void onEvent(rdui::RoutedEvent& event) override
            {
                if (event.kind() != rdui::EventKind::PointerDown) return;
                const char* phase = event.phase() == rdui::EventPhase::Capture ? "capture"
                    : event.phase() == rdui::EventPhase::Target ? "target" : "bubble";
                mLog.push_back(mName + ":" + phase);
                if (preventDefault && event.phase() == rdui::EventPhase::Target) event.preventDefault();
            }

        private:
            std::string mName;
            std::vector<std::string>& mLog;
    };

    class FloaterDelegateProbe final : public rdui::SurfaceFloaterDelegate
    {
        public:
            bool canDetachFloater(const rdui::Surface&, const rdui::Floater&) const override { return allowDetach; }
            void floaterClosed(rdui::Surface&, rdui::Floater&) override { ++closes; }
            void floaterMinimizedChanged(rdui::Surface&, rdui::Floater&, bool) override { ++minimizeChanges; }
            void floaterMoved(rdui::Surface&, rdui::Floater&) override { ++moves; }
            bool beginNativeFloaterResize(rdui::Surface&, rdui::Floater&) override
            {
                ++resizeStarts;
                return nativeResize;
            }
            void floaterResized(rdui::Surface&, rdui::Floater&, bool complete) override
            {
                ++resizeChanges;
                if (complete) ++resizeCompletions;
            }
            void floaterDetachRequested(rdui::Surface&, rdui::Floater&, const rdui::Vec2& desired,
                                        const rdui::Vec2&) override
            {
                ++detachRequests;
                requested = desired;
            }

            bool allowDetach = true;
            int closes = 0;
            int minimizeChanges = 0;
            int moves = 0;
            int detachRequests = 0;
            int resizeStarts = 0;
            int resizeChanges = 0;
            int resizeCompletions = 0;
            bool nativeResize = false;
            rdui::Vec2 requested;
    };

    struct rduisurface_data {};
    typedef test_group<rduisurface_data> rduisurface_test;
    typedef rduisurface_test::object rduisurface_object;
    rduisurface_test rduisurface_testcase("rduisurface");

    template<> template<>
    void rduisurface_object::test<1>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto button = std::make_unique<rdui::Button>();
        rdui::Button* target = button.get();
        int activations = 0;
        button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true).setOnActivate([&](rdui::Widget&) { ++activations; });
        context.root().addChild(std::move(button));
        ensure("move consumed", context.pointerMove({{15.f, 15.f}}));
        ensure("hover set", target->hasState(rdui::WidgetState::Hovered));
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure("pressed control active", target->hasState(rdui::WidgetState::Active));
        context.pointerMove({{50.f, 50.f}});
        ensure("leaving pressed control clears active", !target->hasState(rdui::WidgetState::Active));
        context.pointerMove({{15.f, 15.f}});
        ensure("re-entering pressed control restores active", target->hasState(rdui::WidgetState::Active));
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("pointer activates", activations, 1);
        ensure("control focused", context.hasFocus());
        ensure("mouse focus is not focus-visible", !target->hasState(rdui::WidgetState::FocusVisible));

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerLeave();
        ensure("mouse leave clears pressed active", !target->hasState(rdui::WidgetState::Active));
        context.pointerUp({{50.f, 50.f}, rdui::PointerButton::Left});
        ensure_equals("release outside does not activate", activations, 1);
    }

    template<> template<>
    void rduisurface_object::test<2>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto control = std::make_unique<rdui::Switch>();
        rdui::Switch* target = control.get();
        int changes = 0;
        control->setOnCheckedChanged([&](bool) { ++changes; });
        control->setRect({10.f, 10.f, 40.f, 20.f}).setPointerEvents(true);
        context.root().addChild(std::move(control));
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure("switch toggles itself", target->checked());
        ensure("unmatched activation key-up is ignored", !context.keyUp({rdui::KEY_SPACE}));
        ensure("unmatched key-up does not toggle switch", target->checked());
        context.keyDown({rdui::KEY_SPACE});
        ensure("keyboard active state set", target->hasState(rdui::WidgetState::Active));
        ensure("mismatched activation key-up is ignored", !context.keyUp({rdui::KEY_RETURN}));
        ensure("mismatched key-up preserves held active state", target->hasState(rdui::WidgetState::Active));
        context.keyUp({rdui::KEY_SPACE});
        ensure("keyboard toggles switch", !target->checked());
        ensure("duplicate activation key-up is ignored", !context.keyUp({rdui::KEY_SPACE}));
        ensure_equals("checked callback follows both activations", changes, 2);
    }

    template<> template<>
    void rduisurface_object::test<3>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto button = std::make_unique<rdui::Button>();
        button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        context.root().addChild(std::move(button));
        context.pointerMove({{15.f, 15.f}});
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure("control focused before mutation", context.hasFocus());
        context.root().clearChildren();
        ensure("tree mutation invalidates interaction references", !context.hasFocus());
        ensure("hover refresh after mutation is safe", !context.pointerMove({{15.f, 15.f}}));
    }

    template<> template<>
    void rduisurface_object::test<5>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto button = std::make_unique<rdui::Button>();
        button->setDisabled(true).setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        context.root().addChild(std::move(button));
        ensure("disabled control still blocks pointer", context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure("disabled control is not focused", !context.hasFocus());
    }

    template<> template<>
    void rduisurface_object::test<4>()
    {
        rdui::StyleSheet style_sheet;
        style_sheet.loadCss("floater { flow: column; } floater::header { height: 30px; } floater::content { grow: 1; } label { height: 20px; }");
        rdui::Surface context(style_sheet);
        context.setViewport(200.f, 200.f);

        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* window = floater.get();
        floater->setTitle("title").setCanClose(false).setCanMinimize(true);
        auto content = std::make_unique<rdui::Label>("content");
        rdui::Label* content_node = content.get();
        floater->addChild(std::move(content));
        floater->setRect({20.f, 20.f, 100.f, 100.f});
        context.mountFloater(std::move(floater));
        context.updateLayout();

        ensure("header starts drag", context.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left}));
        ensure("captured move handled", context.pointerMove({{50.f, 120.f}, rdui::PointerButton::Left}));
        context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
        ensure_equals("drag moves x", window->rect().x, 40.f);
        ensure_equals("drag moves y", window->rect().y, 30.f);

        const float expanded_top = window->rect().top();
        const float expanded_width = window->rect().w;
        window->setMinimized(true);
        ensure("minimized state is style-visible", window->hasState(rdui::WidgetState::Minimized));
        ensure("content box collapsed while minimized", window->content()->visibility() == rdui::Visibility::Collapsed);
        ensure("child visibility is preserved while content box collapses", content_node->visibility() == rdui::Visibility::Visible);
        ensure_equals("minimize preserves top", window->rect().top(), expanded_top);
        ensure_equals("minimize uses header height", window->rect().h, 30.f);
        ensure("minimize shrinks width to header identity and controls", window->rect().w < expanded_width);
        window->setMinimized(false);
        ensure("expanded state clears minimized style", !window->hasState(rdui::WidgetState::Minimized));
        ensure("content box visibility restored", window->content()->visibility() == rdui::Visibility::Visible);
        ensure("child remains visible after expansion", content_node->visibility() == rdui::Visibility::Visible);
        ensure_equals("expanded height restored", window->rect().h, 100.f);
        ensure_equals("expanded width restored", window->rect().w, expanded_width);

        ensure("header double-click is handled", context.pointerDown(
            {{50.f, 120.f}, rdui::PointerButton::Left, 0, 2}));
        ensure("header double-click minimizes", window->minimized());
        context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
        ensure("second header double-click is handled", context.pointerDown(
            {{50.f, 120.f}, rdui::PointerButton::Left, 0, 2}));
        ensure("header double-click restores", !window->minimized());
        context.pointerUp({{50.f, 120.f}, rdui::PointerButton::Left});
    }

    template<> template<>
    void rduisurface_object::test<6>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto button = std::make_unique<rdui::Button>();
        rdui::Button* target = button.get();
        int activations = 0;
        button->setRect({10.f, 10.f, 20.f, 20.f}).setOnActivate([&](rdui::Widget&) { ++activations; });
        context.root().addChild(std::move(button));

        for (rdui::PointerButton pointer_button : {rdui::PointerButton::Right, rdui::PointerButton::Middle,
                                                   rdui::PointerButton::Button4, rdui::PointerButton::Button5})
        {
            ensure("non-left down is consumed over control", context.pointerDown({{15.f, 15.f}, pointer_button}));
            ensure("non-left up is consumed over control", context.pointerUp({{15.f, 15.f}, pointer_button}));
        }
        ensure_equals("non-left buttons do not activate", activations, 0);
        ensure("non-left buttons do not focus", !context.hasFocus());
        ensure("non-left buttons do not set active state", !target->hasState(rdui::WidgetState::Active));

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("left button still activates", activations, 1);
    }

    template<> template<>
    void rduisurface_object::test<7>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto probe = std::make_unique<InputProbe>();
        InputProbe* target = probe.get();
        probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        context.root().addChild(std::move(probe));

        ensure("double click down handled", context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left, 0, 2}));
        ensure_equals("double click count reaches node", target->lastClickCount, static_cast<uint8_t>(2));
        ensure("double click release handled", context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure("double click release clears active", !target->hasState(rdui::WidgetState::Active));
        ensure("probe receives focus", context.hasFocus());
        ensure("character input handled by focus", context.charInput(0x03A9));
        ensure_equals("Unicode codepoint reaches focus", target->lastCodepoint, 0x03A9u);

        ensure("vertical scroll handled", context.scroll({{15.f, 15.f}, 0.f, 3.f}));
        ensure_equals("vertical scroll reaches node", target->lastScrollY, 3.f);
        ensure("horizontal scroll handled", context.scroll({{15.f, 15.f}, -2.f, 0.f}));
        ensure_equals("horizontal scroll reaches node", target->lastScrollX, -2.f);

        context.clearInteractionState();
        ensure("focus loss clears focus", !context.hasFocus());
        ensure("character input ignored without focus", !context.charInput('x'));

        ensure("hover restored", context.pointerMove({{15.f, 15.f}}));
        context.pointerLeave();
        ensure("mouse leave clears hover", !target->hasState(rdui::WidgetState::Hovered));
    }

    template<> template<>
    void rduisurface_object::test<8>()
    {
        rdui::StyleSheet style_sheet;
        style_sheet.loadCss("floater { flow: column; } floater::header { height: 30px; } floater::content { grow: 1; }");
        rdui::Surface context(style_sheet);
        context.setViewport(200.f, 200.f);
        auto floater = std::make_unique<rdui::Floater>();
        floater->setTitle("title").setCanClose(false).setCanMinimize(false);
        floater->setRect({20.f, 20.f, 100.f, 100.f});
        context.mountFloater(std::move(floater));
        context.updateLayout();

        ensure("header starts capture", context.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left}));
        ensure("context owns capture", context.hasPointerCapture());
        ensure("captured move outside viewport handled", context.pointerMove({{-50.f, -50.f}, rdui::PointerButton::Left}));
        ensure("captured release outside viewport handled", context.pointerUp({{-50.f, -50.f}, rdui::PointerButton::Left}));
        ensure("release ends capture", !context.hasPointerCapture());

        ensure("second header press starts capture", context.pointerDown({{10.f, 90.f}, rdui::PointerButton::Left}));
        context.clearInteractionState();
        ensure("capture loss clears capture", !context.hasPointerCapture());
    }

    template<> template<>
    void rduisurface_object::test<9>()
    {
        rdui::Surface context;
        context.setViewport(200.f, 200.f);

        auto first = std::make_unique<rdui::Button>();
        rdui::Button* first_target = first.get();
        first->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        context.root().addChild(std::move(first));

        auto hidden = std::make_unique<rdui::Button>();
        rdui::Button* hidden_target = hidden.get();
        hidden->setVisibility(rdui::Visibility::Hidden).setRect({40.f, 10.f, 20.f, 20.f});
        context.root().addChild(std::move(hidden));

        auto disabled = std::make_unique<rdui::Button>();
        rdui::Button* disabled_target = disabled.get();
        disabled->setDisabled(true).setRect({70.f, 10.f, 20.f, 20.f});
        context.root().addChild(std::move(disabled));

        auto last = std::make_unique<rdui::Switch>();
        rdui::Switch* last_target = last.get();
        last->setRect({100.f, 10.f, 40.f, 20.f});
        context.root().addChild(std::move(last));

        ensure("Tab focuses first control", context.keyDown({rdui::KEY_TAB}));
        ensure("first is focused", first_target->hasState(rdui::WidgetState::Focused));
        ensure("Tab focus is visible", first_target->hasState(rdui::WidgetState::FocusVisible));
        ensure("Tab key-up consumed", context.keyUp({rdui::KEY_TAB}));

        context.keyDown({rdui::KEY_TAB});
        ensure("Tab skips hidden and disabled controls", last_target->hasState(rdui::WidgetState::Focused));
        ensure("hidden control not focused", !hidden_target->hasState(rdui::WidgetState::Focused));
        ensure("disabled control not focused", !disabled_target->hasState(rdui::WidgetState::Focused));

        context.keyDown({rdui::KEY_TAB});
        ensure("forward traversal wraps", first_target->hasState(rdui::WidgetState::Focused));
        context.keyDown({rdui::KEY_TAB, rdui::MODIFIER_SHIFT});
        ensure("Shift+Tab traverses backward and wraps", last_target->hasState(rdui::WidgetState::Focused));

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure("mouse moves focus", first_target->hasState(rdui::WidgetState::Focused));
        ensure("mouse focus clears focus-visible", !first_target->hasState(rdui::WidgetState::FocusVisible));
        first_target->setVisibility(rdui::Visibility::Hidden);
        ensure("hidden focused node rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
        ensure("hidden focused node clears focus", !context.hasFocus());
        first_target->setVisibility(rdui::Visibility::Visible);
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        first_target->setDisabled(true);
        ensure("disabled focused node rejects character input", !context.charInput('x'));
        ensure("disabled focused node clears focus", !context.hasFocus());
        first_target->setDisabled(false);
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.clearInteractionState();
        ensure("focus loss clears focused state", !first_target->hasState(rdui::WidgetState::Focused));
        ensure("focus loss clears focus-visible state", !first_target->hasState(rdui::WidgetState::FocusVisible));
    }

    template<> template<>
    void rduisurface_object::test<10>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto panel = std::make_unique<rdui::Panel>();
        rdui::Panel* parent = panel.get();
        panel->setRect({0.f, 0.f, 100.f, 100.f});
        auto button = std::make_unique<rdui::Button>();
        rdui::Button* target = button.get();
        button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        panel->addChild(std::move(button));
        context.root().addChild(std::move(panel));

        context.pointerMove({{15.f, 15.f}});
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        context.keyDown({rdui::KEY_SPACE});
        ensure("keyboard press active before capture loss", target->hasState(rdui::WidgetState::Active));
        context.clearInteractionState();
        ensure("capture loss clears hover", !target->hasState(rdui::WidgetState::Hovered));
        ensure("capture loss clears keyboard active", !target->hasState(rdui::WidgetState::Active));
        ensure("capture loss clears focus", !context.hasFocus());

        context.pointerMove({{15.f, 15.f}});
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure("pointer press active before capture loss", target->hasState(rdui::WidgetState::Active));
        context.clearInteractionState();
        ensure("capture loss clears pointer active", !target->hasState(rdui::WidgetState::Active));

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        parent->setVisibility(rdui::Visibility::Hidden);
        ensure("hidden ancestor rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
        ensure("hidden ancestor clears descendant focus", !context.hasFocus());
        parent->setVisibility(rdui::Visibility::Visible);
        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        parent->setDisabled(true);
        ensure("disabled ancestor rejects keyboard input", !context.keyDown({rdui::KEY_SPACE}));
        ensure("disabled ancestor clears descendant focus", !context.hasFocus());
    }

    template<> template<>
    void rduisurface_object::test<11>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 100.f);
        auto button = std::make_unique<rdui::Button>();
        button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        button->setAction(rdui::ActionEventKind::MouseDown, "press");
        button->setAction(rdui::ActionEventKind::MouseUp, "release");
        button->setAction(rdui::ActionEventKind::Click, "click");
        button->setAction(rdui::ActionEventKind::DoubleClick, "double_click");
        button->setAction(rdui::ActionEventKind::ContextMenu, "context_menu");
        context.root().addChild(std::move(button));

        std::vector<std::string> events;
        rdui::Binder binder(context.root());
        binder.onMouseDown("press", [&](const rdui::MouseActionEvent& event)
        {
            ensure("mouse context reports button", event.mouse.button != rdui::PointerButton::None);
            events.push_back("down");
        });
        binder.onMouseUp("release", [&] { events.push_back("up"); });
        binder.onClick("click", [&] { events.push_back("click"); });
        binder.onDoubleClick("double_click", [&](const rdui::MouseActionEvent& event)
        {
            ensure_equals("double click preserves native count", event.mouse.clickCount, 2);
            events.push_back("double");
        });
        binder.onContextMenu("context_menu", [&](const rdui::MouseActionEvent& event)
        {
            ensure("context menu reports right button", event.mouse.button == rdui::PointerButton::Right);
            events.push_back("context");
        });
        rdui::BindingResult binding = binder.finish();
        ensure("mouse actions bind", binding.ok());

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("down up click order", events.size(), 3U);
        ensure_equals("down first", events[0], "down");
        ensure_equals("up second", events[1], "up");
        ensure_equals("click last", events[2], "click");

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        context.pointerUp({{50.f, 50.f}, rdui::PointerButton::Left});
        ensure_equals("release outside still emits up without click", events.size(), 5U);
        ensure_equals("outside release ends pair", events.back(), "up");

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Right});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Right});
        ensure_equals("context menu follows right mouse up", events.size(), 8U);
        ensure_equals("context menu is last", events.back(), "context");

        context.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left, 0, 2});
        context.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("second click emits click then double click", events.size(), 12U);
        ensure_equals("second click precedes double click", events[10], "click");
        ensure_equals("double click is last", events[11], "double");
    }

    template<> template<>
    void rduisurface_object::test<12>()
    {
        rdui::Surface context;
        context.setViewport(100.f, 80.f);
        auto panel = std::make_unique<rdui::Panel>();
        rdui::Panel* mounted = panel.get();
        context.mount(std::move(panel));
        ensure("mounted widget unmounts", context.unmount(*mounted) != nullptr);

        ensure_equals("unmount leaves valid root width", context.root().rect().w, 100.f);
        ensure_equals("unmount leaves valid root height", context.root().rect().h, 80.f);
        ensure("input remains safe after unmount", !context.pointerDown({{10.f, 10.f}, rdui::PointerButton::Left}));
    }

    template<> template<>
    void rduisurface_object::test<13>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 100.f);
        auto probe = std::make_unique<CaptureProbe>();
        CaptureProbe* target = probe.get();
        probe->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        surface.root().addChild(std::move(probe));

        ensure("probe captures pointer", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure("surface records capture", surface.hasPointerCapture());
        target->setDisabled(true);
        ensure("disabling widget immediately clears capture", !surface.hasPointerCapture());
        ensure_equals("capture cancellation ends widget interaction", target->ends, 1);
    }

    template<> template<>
    void rduisurface_object::test<14>()
    {
        rdui::System system;
        ensure("global delay accepts positive duration", system.setLongClickDelay(std::chrono::milliseconds(600)));
        std::unique_ptr<rdui::Surface> owned_surface = system.createSurface(rdui::fixedTextMetrics());
        rdui::Surface& surface = *owned_surface;
        surface.setViewport(100.f, 100.f);

        auto button = std::make_unique<rdui::Button>();
        rdui::Button* target = button.get();
        button->setRect({10.f, 10.f, 20.f, 20.f}).setPointerEvents(true);
        button->setAction(rdui::ActionEventKind::LongClick, "hold");
        button->setAction(rdui::ActionEventKind::Click, "tap");
        button->setAction(rdui::ActionEventKind::MouseUp, "release");
        surface.root().addChild(std::move(button));

        int holds = 0;
        int taps = 0;
        int releases = 0;
        std::chrono::milliseconds held_for{0};
        rdui::Binder binder(surface.root());
        binder.onLongClick("hold", [&](const rdui::LongClickActionEvent& event)
        {
            held_for = event.heldFor;
            ++holds;
        });
        binder.onClick("tap", [&] { ++taps; });
        binder.onMouseUp("release", [&] { ++releases; });
        rdui::BindingResult binding = binder.finish();
        ensure("long click actions bind", binding.ok());

        surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        surface.update(std::chrono::milliseconds(599));
        ensure_equals("global threshold not early", holds, 0);
        surface.update(std::chrono::milliseconds(1));
        ensure_equals("global threshold fires once", holds, 1);
        ensure_equals("long click reports held duration", held_for.count(), 600LL);
        surface.update(std::chrono::milliseconds(500));
        ensure_equals("held action does not repeat", holds, 1);
        surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("release still fires after long click", releases, 1);
        ensure_equals("long click suppresses click", taps, 0);

        target->setLongClickDelay(std::chrono::milliseconds(200));
        surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        surface.update(std::chrono::milliseconds(200));
        ensure_equals("widget delay overrides global threshold", holds, 2);
        ensure_equals("override duration reaches typed event", held_for.count(), 200LL);
        surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("override release still fires", releases, 2);
        ensure_equals("override long click also suppresses click", taps, 0);
    }

    template<> template<>
    void rduisurface_object::test<15>()
    {
        rdui::StyleSheet style_sheet;
        ensure("pointer policy stylesheet compiles", style_sheet.loadCss(
            "button { pointer-events: none; } panel { pointer-events: auto; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(100.f, 100.f);

        auto button = std::make_unique<rdui::Button>();
        button->setRect({10.f, 10.f, 20.f, 20.f});
        surface.root().addChild(std::move(button));
        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({40.f, 10.f, 20.f, 20.f});
        surface.root().addChild(std::move(panel));

        ensure("style can disable interactive widget without layout", !surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure("style can enable noninteractive widget without layout", surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));
    }

    template<> template<>
    void rduisurface_object::test<16>()
    {
        rdui::StyleSheet style_sheet;
        ensure("automatic layout stylesheet compiles", style_sheet.loadCss(
            "panel { flow: row; } label { height: 10px; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(100.f, 100.f);

        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({0.f, 0.f, 100.f, 20.f});
        auto label = std::make_unique<rdui::Label>("a");
        rdui::Label* text = label.get();
        panel->addChild(std::move(label));
        surface.root().addChild(std::move(panel));

        surface.updateLayout();
        const float short_width = text->rect().w;
        text->setText("a much longer label");
        surface.updateLayout();
        ensure("intrinsic mutation automatically remeasures Surface", text->rect().w > short_width);
    }

    template<> template<>
    void rduisurface_object::test<17>()
    {
        rdui::StyleSheet style_sheet;
        ensure("initial generated stylesheet compiles", style_sheet.loadCss("label { width: 10px; height: 10px; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(100.f, 100.f);

        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({0.f, 0.f, 100.f, 20.f});
        auto label = std::make_unique<rdui::Label>("text");
        rdui::Label* text = label.get();
        panel->addChild(std::move(label));
        surface.root().addChild(std::move(panel));
        surface.updateLayout();
        ensure_equals("initial stylesheet generation arranged", text->rect().w, 10.f);

        ensure("replacement stylesheet compiles", style_sheet.loadCss("label { width: 30px; height: 10px; }").ok());
        surface.updateLayout();
        ensure_equals("stylesheet generation invalidates cached measurement", text->rect().w, 30.f);
    }

    template<> template<>
    void rduisurface_object::test<18>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 100.f);
        std::vector<std::string> log;

        auto parent = std::make_unique<RoutedProbe>("parent", log);
        parent->setRect({0.f, 0.f, 100.f, 100.f});
        auto child = std::make_unique<RoutedProbe>("child", log);
        child->setRect({10.f, 10.f, 20.f, 20.f});
        parent->addChild(std::move(child));
        surface.root().addChild(std::move(parent));

        ensure("routed press handled", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure_equals("capture, target, bubble each run once", log.size(), std::size_t(3));
        ensure_equals("ancestor captures first", log[0], std::string("parent:capture"));
        ensure_equals("target runs second", log[1], std::string("child:target"));
        ensure_equals("ancestor bubbles last", log[2], std::string("parent:bubble"));
    }

    template<> template<>
    void rduisurface_object::test<19>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 100.f);
        std::vector<std::string> log;
        auto probe = std::make_unique<RoutedProbe>("target", log);
        RoutedProbe* target = probe.get();
        target->preventDefault = true;
        target->setRect({10.f, 10.f, 20.f, 20.f});
        surface.root().addChild(std::move(probe));

        ensure("prevented routed press still consumed", surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left}));
        ensure_equals("preventDefault skips widget default behavior", target->begins, 0);
        ensure("preventDefault skips pointer capture", !surface.hasPointerCapture());
    }

    template<> template<>
    void rduisurface_object::test<20>()
    {
        rdui::StyleSheet style_sheet;
        ensure("cursor stylesheet compiles", style_sheet.loadCss(
            "#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(100.f, 100.f);

        auto parent = std::make_unique<rdui::Panel>();
        parent->setId("parent").setRect({0.f, 0.f, 100.f, 100.f});
        auto child = std::make_unique<rdui::Panel>();
        child->setId("child").setRect({10.f, 10.f, 20.f, 20.f});
        parent->addChild(std::move(child));
        surface.root().addChild(std::move(parent));

        ensure("child receives hover", surface.pointerMove({{15.f, 15.f}}));
        ensure_equals("auto cursor inherits nearest explicit ancestor",
                      static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Grab));

        ensure("explicit auto cursor compiles", style_sheet.loadCss(
            "#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; cursor: auto; }").ok());
        ensure_equals("explicit auto cursor stops inheritance",
                      static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Default));

        ensure("cursor override compiles", style_sheet.loadCss(
            "#parent { pointer-events: auto; cursor: grab; } #child { pointer-events: auto; cursor: text; }").ok());
        ensure_equals("hovered widget overrides inherited cursor",
                      static_cast<int>(surface.cursor()), static_cast<int>(rdui::CursorStyle::Text));
    }

    template<> template<>
    void rduisurface_object::test<21>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 100.f);
        int content_activations = 0;
        int floater_activations = 0;
        int popup_activations = 0;
        int tooltip_activations = 0;
        int drag_activations = 0;
        int modal_activations = 0;

        auto mount_button = [&](rdui::SurfaceLayer layer, int& activations, const rdui::Rect& rect)
        {
            auto button = std::make_unique<rdui::Button>();
            button->setRect(rect).setOnActivate([&activations](rdui::Widget&) { ++activations; });
            surface.mount(std::move(button), layer);
        };
        mount_button(rdui::SurfaceLayer::Content, content_activations, {0.f, 0.f, 100.f, 100.f});
        mount_button(rdui::SurfaceLayer::Floater, floater_activations, {10.f, 10.f, 30.f, 30.f});
        mount_button(rdui::SurfaceLayer::Popup, popup_activations, {10.f, 10.f, 30.f, 30.f});
        mount_button(rdui::SurfaceLayer::Tooltip, tooltip_activations, {10.f, 10.f, 30.f, 30.f});
        mount_button(rdui::SurfaceLayer::Drag, drag_activations, {10.f, 10.f, 30.f, 30.f});

        surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("popup precedes floater and content", popup_activations, 1);
        ensure_equals("tooltip layer is input transparent", tooltip_activations, 0);
        ensure_equals("drag adornment layer is input transparent", drag_activations, 0);

        mount_button(rdui::SurfaceLayer::Modal, modal_activations, {10.f, 10.f, 30.f, 30.f});
        surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("modal precedes every lower layer", modal_activations, 1);

        ensure("modal backdrop consumes outside press",
               surface.pointerDown({{80.f, 80.f}, rdui::PointerButton::Left}));
        surface.pointerUp({{80.f, 80.f}, rdui::PointerButton::Left});
        ensure_equals("modal backdrop blocks content activation", content_activations, 0);

        surface.clearLayer(rdui::SurfaceLayer::Modal);
        surface.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});
        surface.pointerUp({{15.f, 15.f}, rdui::PointerButton::Left});
        ensure_equals("clearing modal restores popup precedence", popup_activations, 2);
    }

    template<> template<>
    void rduisurface_object::test<22>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 100.f);
        int first_activations = 0;
        int second_activations = 0;

        auto first = std::make_unique<rdui::Panel>();
        first->setRect({0.f, 0.f, 50.f, 50.f});
        auto first_button = std::make_unique<rdui::Button>();
        first_button->setRect({0.f, 0.f, 50.f, 50.f}).setOnActivate(
            [&first_activations](rdui::Widget&) { ++first_activations; });
        first->addChild(std::move(first_button));
        surface.mount(std::move(first), rdui::SurfaceLayer::Floater);

        auto second = std::make_unique<rdui::Panel>();
        second->setRect({25.f, 0.f, 50.f, 50.f});
        auto second_button = std::make_unique<rdui::Button>();
        second_button->setRect({25.f, 0.f, 50.f, 50.f}).setOnActivate(
            [&second_activations](rdui::Widget&) { ++second_activations; });
        second->addChild(std::move(second_button));
        surface.mount(std::move(second), rdui::SurfaceLayer::Floater);

        surface.pointerDown({{10.f, 10.f}, rdui::PointerButton::Left});
        surface.pointerUp({{10.f, 10.f}, rdui::PointerButton::Left});
        surface.pointerDown({{30.f, 10.f}, rdui::PointerButton::Left});
        surface.pointerUp({{30.f, 10.f}, rdui::PointerButton::Left});
        ensure_equals("press raises containing floater", first_activations, 2);
        ensure_equals("previously top floater remains behind", second_activations, 0);
    }

    template<> template<>
    void rduisurface_object::test<23>()
    {
        rdui::StyleSheet stylesheet;
        ensure("visible overflow compiles", stylesheet.loadCss(
            "#parent { overflow: visible; pointer-events: none; } #child { pointer-events: auto; }").ok());
        rdui::Surface surface(stylesheet);
        surface.setViewport(100.f, 100.f);
        auto parent = std::make_unique<rdui::Panel>();
        parent->setId("parent").setRect({10.f, 10.f, 20.f, 20.f});
        auto child = std::make_unique<rdui::Panel>();
        child->setId("child").setRect({40.f, 10.f, 10.f, 10.f});
        parent->addChild(std::move(child));
        surface.root().addChild(std::move(parent));

        ensure("visible overflow permits descendant hit outside parent",
               surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));
        surface.pointerUp({{45.f, 15.f}, rdui::PointerButton::Left});

        ensure("hidden overflow compiles", stylesheet.loadCss(
            "#parent { overflow: hidden; pointer-events: none; } #child { pointer-events: auto; }").ok());
        ensure("hidden overflow clips descendant hit outside parent",
               !surface.pointerDown({{45.f, 15.f}, rdui::PointerButton::Left}));

        rdui::RecordingPaintContext recording;
        surface.paint(recording);
        ensure_equals("paint clip stack balances", recording.clipDepth(), 0);
        ensure_equals("surface and overflow clips nest", recording.maxClipDepth(), 2);
        const rdui::PaintCommand* overflow_clip = recording.last(rdui::PaintCommandKind::PushClip);
        ensure("overflow clip recorded", overflow_clip != nullptr);
        ensure_equals("overflow clip uses parent width", overflow_clip->rect.w, 20.f);
    }

    template<> template<>
    void rduisurface_object::test<24>()
    {
        rdui::Surface first;
        rdui::Surface second;
        first.setViewport(100.f, 100.f);
        second.setViewport(80.f, 60.f);

        auto button = std::make_unique<rdui::Button>();
        rdui::Button* transferred = button.get();
        button->setRect({10.f, 10.f, 20.f, 20.f});
        first.mount(std::move(button), rdui::SurfaceLayer::Floater);
        first.pointerDown({{15.f, 15.f}, rdui::PointerButton::Left});

        std::unique_ptr<rdui::Widget> detached = first.unmount(*transferred);
        ensure("mounted root can be transferred", detached && detached.get() == transferred);
        ensure("unmount clears source interaction", !first.hasPointerCapture());
        ensure("unmounted widget leaves source hierarchy", transferred->parent() == nullptr);
        second.mount(std::move(detached), rdui::SurfaceLayer::Floater);
        ensure("transferred widget enters destination hierarchy", transferred->parent() != nullptr);
        ensure("unmount rejects nested or absent widget", !second.unmount(*transferred->parent()));
    }

    template<> template<>
    void rduisurface_object::test<25>()
    {
        rdui::StyleSheet style_sheet;
        ensure("floater drag style compiles", style_sheet.loadCss(
            "floater { flow: column; } floater::header { height: 30px; } floater::content { grow: 1; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(200.f, 200.f);
        FloaterDelegateProbe delegate;
        surface.setFloaterDelegate(&delegate);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanClose(false);
        floater->setRect({20.f, 20.f, 100.f, 100.f});
        surface.mountFloater(std::move(floater));
        surface.updateLayout();

        surface.pointerDown({{30.f, 110.f}, rdui::PointerButton::Left});
        surface.pointerMove({{-99.f, 110.f}, rdui::PointerButton::Left});
        ensure_equals("99 logical pixels beyond the Surface remain attached", delegate.detachRequests, 0);
        surface.pointerMove({{-100.f, 110.f}, rdui::PointerButton::Left});
        ensure_equals("100 logical pixels beyond the Surface request live detachment", delegate.detachRequests, 1);
        ensure_equals("request preserves desired unclamped x", delegate.requested.x, -110.f);
        surface.pointerMove({{-80.f, 110.f}, rdui::PointerButton::Left});
        ensure_equals("one drag emits one detach request", delegate.detachRequests, 1);
        surface.pointerUp({{-80.f, 110.f}, rdui::PointerButton::Left});

        target->setCanDetach(false);
        surface.pointerDown({{10.f, 110.f}, rdui::PointerButton::Left});
        surface.pointerMove({{-80.f, 110.f}, rdui::PointerButton::Left});
        ensure_equals("attached-only Floater resists every overshoot", delegate.detachRequests, 1);
        surface.pointerUp({{-80.f, 110.f}, rdui::PointerButton::Left});
    }

    template<> template<>
    void rduisurface_object::test<26>()
    {
        rdui::StyleSheet style_sheet;
        ensure("minimized restore style compiles", style_sheet.loadCss(
            "floater { flow: column; } floater::header { height: 30px; } floater::content { grow: 1; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(200.f, 200.f);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setTitle("title").setCanClose(false).setCanMinimize(true);
        floater->setRect({20.f, 20.f, 100.f, 100.f});
        surface.mountFloater(std::move(floater));
        surface.updateLayout();

        target->setMinimized(true);
        surface.updateLayout();
        const rdui::Vec2 drag_start{target->rect().left() + 2.f, target->rect().top() - 15.f};
        ensure("minimized header starts drag", surface.pointerDown({drag_start, rdui::PointerButton::Left}));
        surface.pointerMove({{199.f, drag_start.y}, rdui::PointerButton::Left});
        surface.pointerUp({{199.f, drag_start.y}, rdui::PointerButton::Left});
        const float minimized_left = target->rect().left();

        target->setMinimized(false);
        ensure_equals("restored width is preserved", target->rect().w, 100.f);
        ensure_equals("restored Floater is moved inside right bound", target->rect().right(), 200.f);
        ensure("restoring a wider Floater moves it left", target->rect().left() < minimized_left);
    }

    template<> template<>
    void rduisurface_object::test<27>()
    {
        rdui::Surface first;
        rdui::Surface second;
        FloaterDelegateProbe first_delegate;
        FloaterDelegateProbe second_delegate;
        first.setFloaterDelegate(&first_delegate);
        second.setFloaterDelegate(&second_delegate);
        first.setViewport(100.f, 100.f);
        second.setViewport(80.f, 60.f);

        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setTitle("title").setCanMinimize(true);
        floater->setRect({90.f, 90.f, 30.f, 30.f});
        first.mountFloater(std::move(floater));
        ensure_equals("mount clamps to first Surface right edge", target->rect().right(), 100.f);
        ensure_equals("mount clamps to first Surface top edge", target->rect().top(), 100.f);
        ensure_equals("first Surface observes its placement change", first_delegate.moves, 1);

        std::unique_ptr<rdui::Floater> transferred = first.unmountFloater(*target);
        ensure("typed Floater transfer returns ownership", transferred && transferred.get() == target);
        second.mountFloater(std::move(transferred));
        ensure_equals("transfer applies destination right edge", target->rect().right(), 80.f);
        ensure_equals("transfer applies destination top edge", target->rect().top(), 60.f);
        ensure_equals("destination Surface observes transferred placement", second_delegate.moves, 1);

        target->setMinimized(true);
        ensure_equals("destination Surface observes minimization", second_delegate.minimizeChanges, 1);
        ensure_equals("source Surface no longer observes transferred Floater", first_delegate.minimizeChanges, 0);
        target->setMinimized(false);
        target->close();
        ensure_equals("destination Surface observes close", second_delegate.closes, 1);
        ensure_equals("source Surface does not observe close", first_delegate.closes, 0);
    }

    template<> template<>
    void rduisurface_object::test<28>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 40.f);
        int visible_activations = 0;
        int hidden_activations = 0;
        int collapsed_activations = 0;

        auto add = [&](float x, rdui::Visibility visibility, int& activations) -> PaintProbe*
        {
            auto probe = std::make_unique<PaintProbe>();
            PaintProbe* result = probe.get();
            probe->setRect({x, 10.f, 20.f, 20.f})
                 .setVisibility(visibility)
                 .setOnActivate([&activations](rdui::Widget&) { ++activations; });
            surface.mount(std::move(probe));
            return result;
        };

        PaintProbe* visible = add(0.f, rdui::Visibility::Visible, visible_activations);
        PaintProbe* hidden = add(30.f, rdui::Visibility::Hidden, hidden_activations);
        PaintProbe* collapsed = add(60.f, rdui::Visibility::Collapsed, collapsed_activations);
        rdui::RecordingPaintContext recording;
        surface.paint(recording);
        ensure_equals("Visible participates in paint", visible->paints, 1);
        ensure_equals("Hidden does not paint", hidden->paints, 0);
        ensure_equals("Collapsed does not paint", collapsed->paints, 0);

        ensure("Visible participates in hit testing", surface.pointerDown({{10.f, 20.f}, rdui::PointerButton::Left}));
        surface.pointerUp({{10.f, 20.f}, rdui::PointerButton::Left});
        ensure("Hidden is absent from hit testing", !surface.pointerDown({{40.f, 20.f}, rdui::PointerButton::Left}));
        ensure("Collapsed is absent from hit testing", !surface.pointerDown({{70.f, 20.f}, rdui::PointerButton::Left}));
        ensure_equals("only Visible activates", visible_activations, 1);
        ensure_equals("Hidden never activates", hidden_activations, 0);
        ensure_equals("Collapsed never activates", collapsed_activations, 0);

        surface.clearInteractionState();
        ensure("Tab finds a Visible focus target", surface.keyDown({rdui::KEY_TAB}));
        ensure("Visible receives focus", visible->hasState(rdui::WidgetState::Focused));
        ensure("Hidden does not receive focus", !hidden->hasState(rdui::WidgetState::Focused));
        ensure("Collapsed does not receive focus", !collapsed->hasState(rdui::WidgetState::Focused));
    }

    template<> template<>
    void rduisurface_object::test<29>()
    {
        rdui::StyleSheet style_sheet;
        ensure("state layout stylesheet compiles", style_sheet.loadCss(
            "panel { flow: row; } switch { width: 20px; height: 10px; } "
            "switch:checked { width: 40px; } label { width: 10px; height: 10px; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(100.f, 20.f);
        auto panel = std::make_unique<rdui::Panel>();
        panel->setRect({0.f, 0.f, 100.f, 20.f});
        auto control = std::make_unique<rdui::Switch>();
        rdui::Switch* target = control.get();
        panel->addChild(std::move(control));
        auto label = std::make_unique<rdui::Label>("after");
        rdui::Label* after = label.get();
        panel->addChild(std::move(label));
        surface.mount(std::move(panel));

        surface.updateLayout();
        ensure_equals("unchecked RSL state has initial width", after->rect().left(), 20.f);
        target->setChecked(true);
        surface.updateLayout();
        ensure_equals("RSL state change invalidates ancestor layout", after->rect().left(), 40.f);
    }

    template<> template<>
    void rduisurface_object::test<30>()
    {
        rdui::StyleSheet style_sheet;
        ensure("startup Floater stylesheet compiles", style_sheet.loadCss(
            "floater { width: 100px; flow: column; } floater::header { height: 30px; }").ok());
        rdui::Surface surface(style_sheet);
        FloaterDelegateProbe delegate;
        surface.setFloaterDelegate(&delegate);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanClose(false);
        surface.mountFloater(std::move(floater));

        surface.updateLayout();
        ensure_equals("zero-viewport layout does not place managed Floater x", target->rect().x, 0.f);
        ensure_equals("zero-viewport layout does not place managed Floater y", target->rect().y, 0.f);
        surface.setViewport(200.f, 100.f);
        ensure_equals("viewport initialization does not report a synthetic Floater move", delegate.moves, 0);
    }

    template<> template<>
    void rduisurface_object::test<31>()
    {
        rdui::StyleSheet style_sheet;
        ensure("resize stylesheet compiles", style_sheet.loadCss(
            "floater { size: 80px 100px; min-size: 40px 50px; flow: column; } "
            "floater::header { height: 20px; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(200.f, 160.f);
        FloaterDelegateProbe delegate;
        surface.setFloaterDelegate(&delegate);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanResize(true).setCanClose(false);
        surface.mountFloater(std::move(floater));
        surface.placeFloater(*target, surface.prepareFloater(*target, 20.f));
        surface.updateLayout();

        surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 30.f}});
        ensure("right border has intrinsic cursor",
               surface.cursor() == rdui::CursorStyle::EastWestResize);
        ensure("right border starts capture", surface.pointerDown(
            {{target->rect().right() - 1.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left}));
        surface.pointerMove({{250.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left});
        ensure_equals("attached resize remains in Surface", target->rect().right(), 200.f);
        ensure_equals("resize never enters detach path", delegate.detachRequests, 0);
        surface.pointerUp({{250.f, target->rect().bottom() + 30.f}, rdui::PointerButton::Left});
        ensure_equals("resize completes once", delegate.resizeCompletions, 1);
    }

    template<> template<>
    void rduisurface_object::test<32>()
    {
        rdui::Surface surface;
        surface.setViewport(200.f, 160.f);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanResize(false).setRect({20.f, 20.f, 100.f, 80.f});
        surface.mountFloater(std::move(floater));

        surface.pointerMove({{119.f, 60.f}});
        ensure("disabled resizing exposes no intrinsic cursor",
               surface.cursor() == rdui::CursorStyle::Default);
        target->setCanResize(true).setCanMinimize(true).setMinimized(true);
        surface.pointerMove({{target->rect().right() - 1.f, target->rect().bottom() + 2.f}});
        ensure("minimized Floater exposes no resize cursor",
               surface.cursor() == rdui::CursorStyle::Default);
    }

    template<> template<>
    void rduisurface_object::test<33>()
    {
        rdui::StyleSheet style_sheet;
        ensure("percentage minimum compiles", style_sheet.loadCss(
            "floater { size: 80px 100px; min-size: 50%; }").ok());
        rdui::Surface surface(style_sheet);
        surface.setViewport(400.f, 300.f);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanResize(true).setCanClose(false);
        surface.mountFloater(std::move(floater));
        surface.placeFloater(*target, surface.prepareFloater(*target, 20.f));

        const rdui::Vec2 left_edge{target->rect().left() + 1.f, target->rect().bottom() + 30.f};
        surface.pointerDown({left_edge, rdui::PointerButton::Left});
        surface.pointerMove({{target->rect().right() + 500.f, left_edge.y}, rdui::PointerButton::Left});
        surface.pointerUp({{target->rect().right() + 500.f, left_edge.y}, rdui::PointerButton::Left});
        ensure_equals("percentage minimum uses frozen original width", target->rect().w, 50.f);
    }

    template<> template<>
    void rduisurface_object::test<34>()
    {
        rdui::Surface surface;
        surface.setViewport(100.f, 80.f);
        FloaterDelegateProbe delegate;
        delegate.nativeResize = true;
        surface.setFloaterDelegate(&delegate);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanResize(true).setRect({0.f, 0.f, 100.f, 80.f});
        surface.mountFloater(std::move(floater));

        ensure("native-hosted resize starts", surface.pointerDown(
            {{99.f, 40.f}, rdui::PointerButton::Left}));
        surface.pointerMove({{139.f, 40.f}, rdui::PointerButton::Left});
        ensure_equals("native-hosted logical geometry may grow beyond its old viewport", target->rect().w, 140.f);
        surface.pointerUp({{139.f, 40.f}, rdui::PointerButton::Left});
        ensure_equals("native resize seam entered once", delegate.resizeStarts, 1);
        ensure_equals("native resize publishes completion", delegate.resizeCompletions, 1);
    }

    template<> template<>
    void rduisurface_object::test<35>()
    {
        rdui::Surface surface;
        surface.setViewport(200.f, 160.f);
        auto floater = std::make_unique<rdui::Floater>();
        rdui::Floater* target = floater.get();
        floater->setCanResize(true).setRect({20.f, 20.f, 100.f, 80.f});
        surface.mountFloater(std::move(floater), rdui::SurfaceLayer::Modal);

        ensure("modal resize starts before modal child routing", surface.pointerDown(
            {{119.f, 60.f}, rdui::PointerButton::Left}));
        surface.pointerMove({{159.f, 60.f}, rdui::PointerButton::Left});
        surface.pointerUp({{159.f, 60.f}, rdui::PointerButton::Left});
        ensure_equals("expanded modal Floater resizes", target->rect().w, 140.f);
    }
}
