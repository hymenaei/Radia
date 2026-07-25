#include "linden_common.h"
#include "../test/lltut.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdfield.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rduilayout.h"
#include "rduistylesheet.h"
#include "rduitextmetrics.h"
#include "rduiviewcontract.h"

namespace tut
{
    struct rduilayout_data { rdui::FixedTextMetrics text; };
    typedef test_group<rduilayout_data> rduilayout_test;
    typedef rduilayout_test::object rduilayout_object;
    rduilayout_test rduilayout_testcase("rduilayout");

    template<> template<>
    void rduilayout_object::test<1>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button { left: 10px; top: 10px; padding: 7px; gap: 6px; flow: row; font-size: 13px; line-height: 18px; }"
                      "button > icon { size: 14px; }");
        rdui::Panel root;
        root.setRect({0.f, 0.f, 300.f, 200.f});
        auto button = std::make_unique<rdui::Button>();
        button->setIcon("search");
        button->setLabel("Apply");
        root.addChild(std::move(button));
        rdui::layoutTree(root, theme, text);
        const rdui::Widget& result = *root.children().front();
        ensure_equals("button auto width", result.rect().w, 72.f);
        ensure_equals("button auto height", result.rect().h, 32.f);
        ensure_equals("button child gap", result.children()[1]->rect().x - result.children()[0]->rect().right(), 6.f);
    }

    template<> template<>
    void rduilayout_object::test<2>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button { width: 128px; height: 32px; padding: 7px; gap: 6px; flow: row; justify-content: center; line-height: 18px; } button > icon { size: 14px; }");
        rdui::Panel root;
        root.setRect({0.f, 0.f, 300.f, 200.f});
        auto button = std::make_unique<rdui::Button>();
        button->setIcon("search");
        button->setLabel("Apply");
        root.addChild(std::move(button));
        rdui::layoutTree(root, theme, text);
        const rdui::Widget& result = *root.children().front();
        const float content_width = result.children()[1]->rect().right() - result.children()[0]->rect().left();
        ensure_equals("row content centered", result.children()[0]->rect().x - result.rect().x, (result.rect().w - content_width) * 0.5f);
    }

    template<> template<>
    void rduilayout_object::test<3>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { padding: 10px; flow: column; gap: 5px; } label { height: 20px; }");
        rdui::Panel root;
        root.setRect({0.f, 0.f, 100.f, 100.f});
        root.addChild(std::make_unique<rdui::Label>("one"));
        root.addChild(std::make_unique<rdui::Label>("two"));
        rdui::layoutTree(root, theme, text);
        ensure_equals("column fills content width", root.children()[0]->rect().w, 80.f);
        ensure_equals("column gap", root.children()[0]->rect().bottom() - root.children()[1]->rect().top(), 5.f);
    }

    template<> template<>
    void rduilayout_object::test<4>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { width: 40px; height: 30px; right: 5px; bottom: 7px; }");
        rdui::Panel root;
        root.setRect({0.f, 0.f, 100.f, 100.f});
        root.addChild(std::make_unique<rdui::Panel>());
        rdui::layoutTree(root, theme, text);
        ensure_equals("free-flow right positioning", root.children()[0]->rect().right(), 95.f);
        ensure_equals("free-flow bottom positioning", root.children()[0]->rect().bottom(), 7.f);
    }

    template<> template<>
    void rduilayout_object::test<5>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("floater { padding: 10px; flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; }"
                      "floater::header::title { left: 5px; top: 5px; height: 15px; } label { height: 20px; }");
        rdui::Floater floater;
        floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
        floater.addChild(std::make_unique<rdui::Label>("content"));
        rdui::layoutTree(floater, theme, text);
        ensure_equals("header respects floater padding", floater.header()->rect().top(), 90.f);
        ensure_equals("content starts below header and padding", floater.children()[1]->rect().top(), 60.f);
    }

    template<> template<>
    void rduilayout_object::test<6>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; } label { height: 20px; }");
        rdui::Floater floater;
        floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
        floater.addChild(std::make_unique<rdui::Label>("content"));
        floater.header()->setVisibility(rdui::Visibility::Collapsed);
        rdui::layoutTree(floater, theme, text);
        ensure_equals("hidden header does not reserve space", floater.children()[1]->rect().top(), 100.f);
    }

    template<> template<>
    void rduilayout_object::test<7>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { flow: row; } label { width: 10px; height: 10px; } #first { margin: 0px auto 0px 0px; }");
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 20.f});
        auto first = std::make_unique<rdui::Label>("first");
        first->setId("first");
        panel.addChild(std::move(first));
        panel.addChild(std::make_unique<rdui::Label>("second"));
        rdui::layoutTree(panel, theme, text);
        ensure_equals("auto right margin keeps first left", panel.children()[0]->rect().x, 0.f);
        ensure_equals("auto right margin pushes following sibling", panel.children()[1]->rect().x, 90.f);
    }

    template<> template<>
    void rduilayout_object::test<8>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { flow: column; } label { width: 20px; height: 10px; margin: 0px auto; }");
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 40.f});
        panel.addChild(std::make_unique<rdui::Label>("center"));
        rdui::layoutTree(panel, theme, text);
        ensure_equals("paired auto margins center column child", panel.children()[0]->rect().x, 40.f);
    }

    template<> template<>
    void rduilayout_object::test<9>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("button { size: 24px; padding: 20px; flow: row; justify-content: center; } button > icon { size: 16px; }");
        rdui::Button button;
        button.setRect({0.f, 0.f, 24.f, 24.f});
        button.setIcon("search");
        rdui::layoutTree(button, theme, text);
        ensure_equals("oversized padding keeps icon centered horizontally", button.icon()->rect().x, 4.f);
        ensure_equals("oversized padding keeps icon centered vertically", button.icon()->rect().y, 4.f);
    }

    template<> template<>
    void rduilayout_object::test<10>()
    {
        rdui::StyleSheet theme;
        theme.loadRadia("panel { flow: column; justify-content: end; } label { height: 10px; }");
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 40.f});
        panel.addChild(std::make_unique<rdui::Label>("bottom"));
        rdui::layoutTree(panel, theme, text);
        ensure_equals("column end alignment", panel.children().front()->rect().bottom(), 0.f);
    }

    template<> template<>
    void rduilayout_object::test<11>()
    {
        rdui::StyleSheet theme;
        ensure("flex stylesheet compiles", theme.loadRadia(
            "panel { flow: row; } label { width: 10px; height: 10px; flex: 1; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 20.f});
        panel.addChild(std::make_unique<rdui::Label>("first"));
        panel.addChild(std::make_unique<rdui::Label>("second"));
        rdui::layoutTree(panel, theme, text);
        ensure_equals("flex children share remaining width", panel.children()[0]->rect().w, 50.f);
        ensure_equals("second flex child fills row", panel.children()[1]->rect().right(), 100.f);
    }

    template<> template<>
    void rduilayout_object::test<12>()
    {
        rdui::StyleSheet theme;
        ensure("measure stylesheet compiles", theme.loadRadia(
            "panel { flow: row; gap: 3px; padding: 2px; } label { width: 10px; height: 8px; }").ok());
        rdui::Panel panel;
        panel.addChild(std::make_unique<rdui::Label>("first"));
        panel.addChild(std::make_unique<rdui::Label>("second"));

        rdui::measureTree(panel, theme, text);
        ensure_equals("measure computes row width", panel.desiredSize().x, 27.f);
        ensure_equals("measure computes row height", panel.desiredSize().y, 12.f);

        panel.setRect({0.f, 0.f, 40.f, 20.f});
        rdui::arrangeTree(panel, theme, text);
        ensure_equals("arrange consumes measured child width", panel.children()[1]->rect().right(), 25.f);
    }

    template<> template<>
    void rduilayout_object::test<13>()
    {
        rdui::StyleSheet theme;
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 100.f});
        auto button = std::make_unique<rdui::Button>();
        button->setRect({10.f, 10.f, 20.f, 20.f});
        panel.addChild(std::move(button));

        rdui::layoutTree(panel, theme, text);
        const rdui::Rect& rect = panel.children().front()->rect();
        ensure_equals("free layout preserves explicit x", rect.x, 10.f);
        ensure_equals("free layout preserves explicit y", rect.y, 10.f);
        ensure_equals("free layout preserves explicit width", rect.w, 20.f);
        ensure_equals("free layout preserves explicit height", rect.h, 20.f);
    }

    template<> template<>
    void rduilayout_object::test<14>()
    {
        rdui::StyleSheet theme;
        const rdui::StyleSheetLoadResult intrinsic = theme.loadRadia(
            "switch { flow: column; justify-content: center; }", "switch.radia");
        ensure("switch accepts valid authored layout properties", intrinsic.ok());
        ensure("switch part layout compiles", theme.loadRadia(
            "switch { width: 64px; height: 32px; padding: 3px 5px; flow: column; justify-content: center; } "
            "switch::thumb { border-radius: 7px; }").ok());
        rdui::Switch control;
        control.setRect({10.f, 20.f, 64.f, 32.f});

        rdui::layoutTree(control, theme, text);
        ensure_equals("widget fixes switch flow to row",
                      static_cast<int>(rdui::resolveWidgetStyle(theme, control).flow),
                      static_cast<int>(rdui::Flow::Row));
        ensure_equals("widget start alignment positions thumb", control.thumb()->rect().left(), 15.f);
        ensure_equals("widget row flow positions thumb", control.thumb()->rect().bottom(), 23.f);
        ensure_equals("unsized thumb fills content height", control.thumb()->rect().h, 26.f);
        ensure_equals("unsized thumb automatically stays square", control.thumb()->rect().w, 26.f);
        ensure_equals("thumb radius comes from part style",
                      rdui::resolveWidgetStyle(theme, *control.thumb()).border_radius, 7.f);

        control.setChecked(true);
        rdui::layoutTree(control, theme, text);
        ensure_equals("checked widget fixes alignment to end",
                      static_cast<int>(rdui::resolveWidgetStyle(theme, control).justify_content),
                      static_cast<int>(rdui::JustifyContent::End));
        ensure_equals("checked widget moves thumb to end", control.thumb()->rect().right(), 69.f);

        control.clearChildren();
        ensure("clearing a Switch recreates its required thumb", control.thumb());
        ensure_equals("required thumb keeps its declared part", control.thumb()->part(), "thumb");
        ensure("required thumb is remounted under its owner", control.thumb()->parent() == &control);
    }

    template<> template<>
    void rduilayout_object::test<15>()
    {
        class ExactTextMetrics final : public rdui::TextMetrics
        {
            public:
                rdui::Vec2 measureText(const std::string&, const rdui::Style&) const override
                {
                    return {47.f, 19.f};
                }
        } exact;

        rdui::StyleSheet theme;
        rdui::Label label("adapter-owned");
        rdui::measureTree(label, theme, exact);
        ensure_equals("layout uses injected text width", label.desiredSize().x, 47.f);
        ensure_equals("layout uses injected text height", label.desiredSize().y, 19.f);
    }

    template<> template<>
    void rduilayout_object::test<16>()
    {
        rdui::StyleSheet theme;
        ensure("direction stylesheet compiles", theme.loadRadia(
            "panel { flow: row; justify-content: start; gap: 5px; } "
            "label { width: 10px; height: 10px; } #physical { margin: 0px 7px 0px 0px; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 20.f});
        panel.addChild(std::make_unique<rdui::Label>("first"));
        auto physical = std::make_unique<rdui::Label>("second");
        physical->setId("physical");
        panel.addChild(std::move(physical));

        rdui::layoutTree(panel, theme, text, rdui::LayoutDirection::RightToLeft);
        ensure_equals("rtl start begins at physical right", panel.children()[0]->rect().right(), 100.f);
        ensure_equals("rtl row preserves document order from the right", panel.children()[1]->rect().right(), 78.f);
    }

    template<> template<>
    void rduilayout_object::test<17>()
    {
        rdui::StyleSheet theme;
        ensure("negative position stylesheet compiles", theme.loadRadia(
            "label { width: 10px; height: 10px; left: -8px; bottom: -3px; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 100.f});
        panel.addChild(std::make_unique<rdui::Label>("offset"));

        rdui::layoutTree(panel, theme, text);
        ensure_equals("negative left remains an explicit offset", panel.children()[0]->rect().left(), -8.f);
        ensure_equals("negative bottom remains an explicit offset", panel.children()[0]->rect().bottom(), -3.f);
    }

    template<> template<>
    void rduilayout_object::test<18>()
    {
        rdui::StyleSheet theme;
        ensure("field container stylesheet compiles", theme.loadRadia(
            "field { flow: column; } "
            "switch { width: 20px; height: 10px; } "
            "label { width: 30px; height: 10px; }").ok());

        rdui::Field field;
        field.setRect({0.f, 0.f, 50.f, 10.f});
        field.addChild(std::make_unique<rdui::Label>("Label"));
        field.addChild(std::make_unique<rdui::Switch>());
        rdui::layoutTree(field, theme, text);
        ensure_equals("field keeps its default row flow", field.children()[1]->rect().left(), 30.f);
    }

    template<> template<>
    void rduilayout_object::test<19>()
    {
        rdui::StyleSheet theme;
        ensure("percentage geometry compiles", theme.loadRadia(
            "label { width: 50%; height: 25%; left: 10%; top: 20%; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 200.f, 100.f});
        panel.addChild(std::make_unique<rdui::Label>("percentage"));

        rdui::layoutTree(panel, theme, text);
        const rdui::Rect& rect = panel.children().front()->rect();
        ensure_approximately_equals("percentage width resolves against containing width", rect.w, 100.f, 6);
        ensure_approximately_equals("percentage height resolves against containing height", rect.h, 25.f, 6);
        ensure_approximately_equals("percentage left resolves against containing width", rect.left(), 20.f, 6);
        ensure_approximately_equals("percentage top resolves against containing height", rect.top(), 80.f, 6);
    }

    template<> template<>
    void rduilayout_object::test<20>()
    {
        rdui::StyleSheet theme;
        ensure("automatic gap stylesheet compiles", theme.loadRadia(
            "panel { flow: row; gap: auto; } label { width: 10px; height: 10px; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 20.f});
        panel.addChild(std::make_unique<rdui::Label>("first"));
        panel.addChild(std::make_unique<rdui::Label>("second"));
        panel.addChild(std::make_unique<rdui::Label>("third"));

        rdui::layoutTree(panel, theme, text);
        ensure_equals("automatic row gap distributes remaining space", panel.children()[1]->rect().left(), 45.f);
        ensure_equals("automatic row gap places final child at the edge", panel.children()[2]->rect().right(), 100.f);
        ensure_equals("automatic gap contributes zero to intrinsic size", panel.desiredSize().x, 30.f);

        ensure("automatic column gap stylesheet compiles", theme.loadRadia(
            "panel { flow: column; gap: auto; } label { width: 10px; height: 10px; }").ok());
        panel.setRect({0.f, 0.f, 20.f, 100.f});
        rdui::layoutTree(panel, theme, text);
        ensure_equals("automatic column gap distributes remaining space", panel.children()[1]->rect().top(), 55.f);
        ensure_equals("automatic column gap places final child at the edge", panel.children()[2]->rect().bottom(), 0.f);
    }

    template<> template<>
    void rduilayout_object::test<21>()
    {
        rdui::StyleSheet theme;
        ensure("automatic floater size stylesheet compiles", theme.loadRadia(
            "floater { size: auto 100px; flow: column; } "
            "floater::header { height: 30px; } floater::content { flow: column; gap: 5px; } "
            "label { height: 20px; }").ok());
        rdui::Floater floater;
        floater.setCanClose(false);
        floater.addChild(std::make_unique<rdui::Label>("first"));
        floater.addChild(std::make_unique<rdui::Label>("second"));

        const rdui::Vec2 measured = rdui::measureWidget(floater, theme, text);
        ensure_equals("fixed floater width remains fixed", measured.x, 100.f);
        ensure_equals("automatic floater height fits header and content", measured.y, 75.f);
    }

    template<> template<>
    void rduilayout_object::test<22>()
    {
        rdui::StyleSheet theme;
        ensure("row alignment stylesheet compiles", theme.loadRadia(
            "panel { flow: row; align-items: start; } label { width: 10px; height: 10px; } "
            "label#center { align-self: center; } label#end { align-self: end; } "
            "label#stretch { height: auto; align-self: stretch; }").ok());
        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 40.f});
        for (const char* id : {"start", "center", "end", "stretch"})
        {
            auto label = std::make_unique<rdui::Label>(id);
            label->setId(id);
            panel.addChild(std::move(label));
        }

        rdui::layoutTree(panel, theme, text);
        ensure_equals("row start aligns to cross-start", panel.children()[0]->rect().top(), 40.f);
        ensure_equals("row align-self center overrides its container", panel.children()[1]->rect().bottom(), 15.f);
        ensure_equals("row align-self end reaches cross-end", panel.children()[2]->rect().bottom(), 0.f);
        ensure_equals("row stretch fills an automatic cross size", panel.children()[3]->rect().h, 40.f);

        ensure("column alignment stylesheet compiles", theme.loadRadia(
            "panel { flow: column; align-items: start; } label { width: 10px; height: 10px; } "
            "label#center { align-self: center; } label#end { align-self: end; } "
            "label#stretch { width: auto; align-self: stretch; }").ok());
        panel.setRect({0.f, 0.f, 100.f, 40.f});
        rdui::layoutTree(panel, theme, text);
        ensure_equals("column start aligns to logical left in LTR", panel.children()[0]->rect().left(), 0.f);
        ensure_equals("column align-self center overrides its container", panel.children()[1]->rect().left(), 45.f);
        ensure_equals("column align-self end reaches logical right in LTR", panel.children()[2]->rect().right(), 100.f);
        ensure_equals("column stretch fills an automatic cross size", panel.children()[3]->rect().w, 100.f);

        rdui::layoutTree(panel, theme, text, rdui::LayoutDirection::RightToLeft);
        ensure_equals("column logical start follows RTL", panel.children()[0]->rect().right(), 100.f);
        ensure_equals("column logical end follows RTL", panel.children()[2]->rect().left(), 0.f);
    }

    template<> template<>
    void rduilayout_object::test<23>()
    {
        rdui::StyleSheet theme;
        ensure("visibility layout stylesheet compiles", theme.loadRadia(
            "panel { flow: row; } label { width: 10px; height: 10px; }").ok());
        rdui::Panel panel;
        panel.addChild(std::make_unique<rdui::Label>("visible"));
        auto hidden = std::make_unique<rdui::Label>("hidden");
        hidden->setVisibility(rdui::Visibility::Hidden);
        panel.addChild(std::move(hidden));
        auto collapsed = std::make_unique<rdui::Label>("collapsed");
        collapsed->setVisibility(rdui::Visibility::Collapsed);
        panel.addChild(std::move(collapsed));

        rdui::layoutTree(panel, theme, text);
        ensure_equals("Hidden participates in measurement", panel.desiredSize().x, 20.f);
        ensure_equals("Hidden receives an arranged layout slot", panel.children()[1]->rect().left(), 10.f);
        ensure_equals("Collapsed has zero measured size",
                      rdui::measureWidget(*panel.children()[2], theme, text).x, 0.f);

        panel.children()[1]->setVisibility(rdui::Visibility::Collapsed);
        rdui::layoutTree(panel, theme, text);
        ensure_equals("collapsing Hidden invalidates parent measurement", panel.desiredSize().x, 10.f);

        panel.children()[2]->setVisibility(rdui::Visibility::Visible);
        rdui::layoutTree(panel, theme, text);
        ensure_equals("showing Collapsed invalidates parent measurement", panel.desiredSize().x, 20.f);
        ensure_equals("newly Visible child takes the second layout slot", panel.children()[2]->rect().left(), 10.f);
    }

    template<> template<>
    void rduilayout_object::test<24>()
    {
        rdui::StyleSheet theme;
        ensure("vertical container alignment stylesheet compiles", theme.loadRadia(
            "panel { size: 40px 100px; flow: row; } label { size: 10px; } "
            "panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } "
            "panel.column { flow: column; vertical-align: bottom; } "
            "panel.free-bottom { flow: free; vertical-align: bottom; } "
            "button { size: 40px 100px; } button > * { size: 10px; } "
            "button.top { vertical-align: top; }").ok());

        auto add_label = [](rdui::Widget& container)
        {
            container.addChild(std::make_unique<rdui::Label>("child"));
        };

        rdui::Panel top;
        top.setRect({0.f, 0.f, 100.f, 40.f});
        add_label(top);
        rdui::layoutTree(top, theme, text);
        ensure_equals("row content defaults to the top", top.children()[0]->rect().top(), 40.f);

        rdui::Panel middle;
        middle.setRect({0.f, 0.f, 100.f, 40.f});
        middle.addClass("middle");
        add_label(middle);
        rdui::layoutTree(middle, theme, text);
        ensure_equals("row content supports middle alignment", middle.children()[0]->rect().bottom(), 15.f);

        rdui::Panel bottom;
        bottom.setRect({0.f, 0.f, 100.f, 40.f});
        bottom.addClass("bottom");
        add_label(bottom);
        rdui::layoutTree(bottom, theme, text);
        ensure_equals("row content supports bottom alignment", bottom.children()[0]->rect().bottom(), 0.f);

        rdui::Panel column;
        column.setRect({0.f, 0.f, 100.f, 40.f});
        column.addClass("column");
        add_label(column);
        add_label(column);
        rdui::layoutTree(column, theme, text);
        ensure_equals("column vertical alignment moves the complete stack", column.children()[0]->rect().top(), 20.f);
        ensure_equals("column stack reaches the bottom", column.children()[1]->rect().bottom(), 0.f);

        rdui::Panel free_bottom;
        free_bottom.setRect({0.f, 0.f, 100.f, 40.f});
        free_bottom.addClass("free-bottom");
        add_label(free_bottom);
        rdui::layoutTree(free_bottom, theme, text);
        ensure_equals("free-flow unpositioned content follows vertical alignment",
                      free_bottom.children()[0]->rect().bottom(), 0.f);

        rdui::Button button;
        button.setRect({0.f, 0.f, 100.f, 40.f});
        button.setLabel("button");
        rdui::layoutTree(button, theme, text);
        ensure_equals("Button content intrinsically defaults to middle",
                      button.children()[0]->rect().bottom(), 15.f);
        ensure_equals("Button content is horizontally centered by its intrinsic defaults",
                      button.children()[0]->rect().left(), 45.f);

        rdui::Button top_button;
        top_button.setRect({0.f, 0.f, 100.f, 40.f});
        top_button.addClass("top");
        top_button.setLabel("button");
        rdui::layoutTree(top_button, theme, text);
        ensure_equals("authored Button vertical alignment overrides its intrinsic default",
                      top_button.children()[0]->rect().top(), 40.f);
    }

    template<> template<>
    void rduilayout_object::test<25>()
    {
        rdui::StyleSheet theme;
        ensure("row Flow Break stylesheet compiles", theme.loadRadia(
            "panel { flow: row; gap: 2px; } label { size: 10px; }").ok());

        rdui::Panel panel;
        auto first = std::make_unique<rdui::Label>("first");
        auto second = std::make_unique<rdui::Label>("second");
        auto third = std::make_unique<rdui::Label>("third");
        rdui::detail::WidgetCompilerAccess::setFlowBreakBefore(*second, true);
        panel.addChild(std::move(first));
        panel.addChild(std::move(second));
        panel.addChild(std::move(third));

        rdui::measureTree(panel, theme, text);
        ensure_equals("Flow Break measures the widest explicit row", panel.desiredSize().x, 22.f);
        ensure_equals("Flow Break measures both rows and their gap", panel.desiredSize().y, 22.f);

        panel.setRect({0.f, 0.f, 40.f, 40.f});
        rdui::layoutTree(panel, theme, text);
        ensure_equals("first explicit row starts at the container top", panel.children()[0]->rect().top(), 40.f);
        ensure_equals("Flow Break starts its child on the next row", panel.children()[1]->rect().top(), 28.f);
        ensure_equals("following children remain in the new row", panel.children()[2]->rect().left(), 12.f);
    }

    template<> template<>
    void rduilayout_object::test<26>()
    {
        rdui::StyleSheet theme;
        ensure("flex basis and shrink stylesheet compiles", theme.loadRadia(
            "panel { flow: row; } label { height: 10px; flex: 0 1 80px; } label.second { flex-basis: 40px; }").ok());

        rdui::Panel intrinsic;
        intrinsic.addChild(std::make_unique<rdui::Label>());
        auto intrinsic_second = std::make_unique<rdui::Label>();
        intrinsic_second->addClass("second");
        intrinsic.addChild(std::move(intrinsic_second));
        rdui::measureTree(intrinsic, theme, text);
        ensure_equals("flex basis contributes to intrinsic width", intrinsic.desiredSize().x, 120.f);

        rdui::StyleSheet percentage_theme;
        ensure("percentage flex basis stylesheet compiles", percentage_theme.loadRadia(
            "panel { flow: row; } label { width: 30px; flex-basis: 50%; }").ok());
        rdui::Panel indefinite;
        indefinite.addChild(std::make_unique<rdui::Label>());
        rdui::measureTree(indefinite, percentage_theme, text);
        ensure_equals("percentage flex basis is auto in an indefinite container", indefinite.desiredSize().x, 30.f);

        rdui::Panel panel;
        panel.setRect({0.f, 0.f, 100.f, 20.f});
        panel.addChild(std::make_unique<rdui::Label>());
        auto second = std::make_unique<rdui::Label>();
        second->addClass("second");
        panel.addChild(std::move(second));
        rdui::layoutTree(panel, theme, text);
        ensure_approximately_equals("scaled flex shrink reduces the larger basis proportionally",
                                    panel.children()[0]->rect().w, 200.f / 3.f, 5);
        ensure_approximately_equals("scaled flex shrink reduces the smaller basis proportionally",
                                    panel.children()[1]->rect().w, 100.f / 3.f, 5);
        ensure_approximately_equals("shrunk items fill the row", panel.children()[1]->rect().right(), 100.f, 5);
    }

    template<> template<>
    void rduilayout_object::test<27>()
    {
        rdui::StyleSheet theme;
        ensure("floater header stylesheet compiles", theme.loadRadia(
            "floater::header { height: 48px; flow: row; padding: 12px; } "
            "floater::header::icon { size: 28px; } "
            "floater::header::title { line-height: 18px; } "
            "floater::header::custom { height: 24px; flex-grow: 1; } "
            "floater::header::close { size: 24px; }").ok());

        rdui::Floater floater;
        floater.setIcon("search").setTitle("title");
        floater.header()->setRect({0.f, 0.f, 200.f, 48.f});
        rdui::layoutTree(*floater.header(), theme, text);

        const float header_center = floater.header()->rect().y + floater.header()->rect().h * .5f;
        for (const auto& child : floater.header()->children())
        {
            if (child->visibility() != rdui::Visibility::Visible) continue;
            const float child_center = child->rect().y + child->rect().h * .5f;
            ensure_equals("an oversized header icon does not pull the row below center",
                          child_center, header_center);
        }
    }
}
