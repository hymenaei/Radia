/**
 * @file engine_test.cpp
 * @brief Tests retained layout measurement, flex allocation, and arrangement.
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
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace tut {
struct engineData {
    rdui::FixedTextMetrics text;
};
using engineTest = test_group<engineData>;
using engineObject = engineTest::object;
engineTest engineTestCase("engine");

template<> template<> void engineObject::test<1>() {
    rdui::StyleSheet theme;
    const char* kButtonLayout =
        "button { left: 10px; top: 10px; padding: 7px; gap: 6px; flow: row; font-size: 13px; line-height: 18px; } button > icon { size: 14px; }";
    theme.loadRadia(kButtonLayout);
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

template<> template<> void engineObject::test<2>() {
    rdui::StyleSheet theme;
    const char* kCenteredButtonLayout =
        "button { width: 128px; height: 32px; padding: 7px; gap: 6px; flow: row; justify-content: center; line-height: 18px; } button > icon { size: 14px; }";
    theme.loadRadia(kCenteredButtonLayout);
    rdui::Panel root;
    root.setRect({0.f, 0.f, 300.f, 200.f});
    auto button = std::make_unique<rdui::Button>();
    button->setIcon("search");
    button->setLabel("Apply");
    root.addChild(std::move(button));
    rdui::layoutTree(root, theme, text);
    const rdui::Widget& result = *root.children().front();
    const float contentWidth = result.children()[1]->rect().right() - result.children()[0]->rect().left();
    ensure_equals("row content centered", result.children()[0]->rect().x - result.rect().x, (result.rect().w - contentWidth) * 0.5f);
}

template<> template<> void engineObject::test<3>() {
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

template<> template<> void engineObject::test<4>() {
    rdui::StyleSheet theme;
    theme.loadRadia("panel { width: 40px; height: 30px; right: 5px; bottom: 7px; }");
    rdui::Panel root;
    root.setRect({0.f, 0.f, 100.f, 100.f});
    root.addChild(std::make_unique<rdui::Panel>());
    rdui::layoutTree(root, theme, text);
    ensure_equals("free-flow right positioning", root.children()[0]->rect().right(), 95.f);
    ensure_equals("free-flow bottom positioning", root.children()[0]->rect().bottom(), 7.f);
}

template<> template<> void engineObject::test<5>() {
    rdui::StyleSheet theme;
    const char* kFloaterLayout =
        "floater { padding: 10px; flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; } floater::header::title { left: 5px; top: 5px; height: 15px; } label { height: 20px; }";
    theme.loadRadia(kFloaterLayout);
    rdui::Floater floater;
    floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
    floater.addChild(std::make_unique<rdui::Label>("content"));
    rdui::layoutTree(floater, theme, text);
    ensure_equals("header respects floater padding", floater.header()->rect().top(), 90.f);
    ensure_equals("content starts below header and padding", floater.children()[1]->rect().top(), 60.f);
}

template<> template<> void engineObject::test<6>() {
    rdui::StyleSheet theme;
    theme.loadRadia("floater { flow: column; } floater::header { height: 30px; } floater::content { flex-grow: 1; } label { height: 20px; }");
    rdui::Floater floater;
    floater.setTitle("title").setRect({0.f, 0.f, 100.f, 100.f});
    floater.addChild(std::make_unique<rdui::Label>("content"));
    floater.header()->setVisibility(rdui::Visibility::Collapsed);
    rdui::layoutTree(floater, theme, text);
    ensure_equals("hidden header does not reserve space", floater.children()[1]->rect().top(), 100.f);
}

template<> template<> void engineObject::test<7>() {
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

template<> template<> void engineObject::test<8>() {
    rdui::StyleSheet theme;
    theme.loadRadia("panel { flow: column; } label { width: 20px; height: 10px; margin: 0px auto; }");
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<rdui::Label>("center"));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("paired auto margins center column child", panel.children()[0]->rect().x, 40.f);
}

template<> template<> void engineObject::test<9>() {
    rdui::StyleSheet theme;
    theme.loadRadia("button { size: 24px; padding: 20px; flow: row; justify-content: center; } button > icon { size: 16px; }");
    rdui::Button button;
    button.setRect({0.f, 0.f, 24.f, 24.f});
    button.setIcon("search");
    rdui::layoutTree(button, theme, text);
    ensure_equals("oversized padding keeps icon centered horizontally", button.icon()->rect().x, 4.f);
    ensure_equals("oversized padding keeps icon centered vertically", button.icon()->rect().y, 4.f);
}

template<> template<> void engineObject::test<10>() {
    rdui::StyleSheet theme;
    theme.loadRadia("panel { flow: column; justify-content: end; } label { height: 10px; }");
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<rdui::Label>("bottom"));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("column end alignment", panel.children().front()->rect().bottom(), 0.f);
}

template<> template<> void engineObject::test<11>() {
    rdui::StyleSheet theme;
    ensure("flex stylesheet compiles", theme.loadRadia("panel { flow: row; } label { width: 10px; height: 10px; flex: 1; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("flex children share remaining width", panel.children()[0]->rect().w, 50.f);
    ensure_equals("second flex child fills row", panel.children()[1]->rect().right(), 100.f);
}

template<> template<> void engineObject::test<12>() {
    rdui::StyleSheet theme;
    ensure("measure stylesheet compiles", theme.loadRadia("panel { flow: row; gap: 3px; padding: 2px; } label { width: 10px; height: 8px; }").ok());
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

template<> template<> void engineObject::test<13>() {
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

template<> template<> void engineObject::test<14>() {
    rdui::StyleSheet theme;
    const rdui::StyleSheetLoadResult intrinsic = theme.loadRadia("switch { flow: column; justify-content: center; }", "switch.radia");
    ensure("switch accepts valid authored layout properties", intrinsic.ok());
    const char* kSwitch =
        "switch { width: 64px; height: 32px; padding: 3px 5px; flow: column; justify-content: center; } switch::thumb { border-radius: 7px; }";
    ensure("switch part layout compiles", theme.loadRadia(kSwitch).ok());
    rdui::Switch control;
    control.setRect({10.f, 20.f, 64.f, 32.f});

    rdui::layoutTree(control, theme, text);
    ensure_equals("widget fixes switch flow to row", static_cast<int>(rdui::resolveWidgetStyle(theme, control).flow),
                  static_cast<int>(rdui::Flow::Row));
    ensure_equals("widget start alignment positions thumb", control.thumb()->rect().left(), 15.f);
    ensure_equals("widget row flow positions thumb", control.thumb()->rect().bottom(), 23.f);
    ensure_equals("unsized thumb fills content height", control.thumb()->rect().h, 26.f);
    ensure_equals("unsized thumb automatically stays square", control.thumb()->rect().w, 26.f);
    ensure_equals("thumb radius comes from part style", rdui::resolveWidgetStyle(theme, *control.thumb()).borderRadius, 7.f);

    control.setChecked(true);
    rdui::layoutTree(control, theme, text);
    ensure_equals("checked widget fixes alignment to end", static_cast<int>(rdui::resolveWidgetStyle(theme, control).justifyContent),
                  static_cast<int>(rdui::JustifyContent::End));
    ensure_equals("checked widget moves thumb to end", control.thumb()->rect().right(), 69.f);

    control.clearChildren();
    ensure("clearing a Switch recreates its required thumb", control.thumb());
    ensure_equals("required thumb keeps its declared part", control.thumb()->part(), "thumb");
    ensure("required thumb is remounted under its owner", control.thumb()->parent() == &control);
}

template<> template<> void engineObject::test<15>() {
    class ExactTextMetrics final : public rdui::TextMetrics {
    public:
        rdui::Vec2 measureText(const std::string&, const rdui::Style&) const override { return {47.f, 19.f}; }
    } exact;

    rdui::StyleSheet theme;
    rdui::Label label("adapter-owned");
    rdui::measureTree(label, theme, exact);
    ensure_equals("layout uses injected text width", label.desiredSize().x, 47.f);
    ensure_equals("layout uses injected text height", label.desiredSize().y, 19.f);
}

template<> template<> void engineObject::test<16>() {
    rdui::StyleSheet theme;
    const char* kDirection =
        "panel { flow: row; justify-content: start; gap: 5px; } label { width: 10px; height: 10px; } #physical { margin: 0px 7px 0px 0px; }";
    ensure("direction stylesheet compiles", theme.loadRadia(kDirection).ok());
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

template<> template<> void engineObject::test<17>() {
    rdui::StyleSheet theme;
    ensure("negative position stylesheet compiles", theme.loadRadia("label { width: 10px; height: 10px; left: -8px; bottom: -3px; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<rdui::Label>("offset"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("negative left remains an explicit offset", panel.children()[0]->rect().left(), -8.f);
    ensure_equals("negative bottom remains an explicit offset", panel.children()[0]->rect().bottom(), -3.f);
}

template<> template<> void engineObject::test<18>() {
    rdui::StyleSheet theme;
    const char* kField = "field { flow: column; } switch { width: 20px; height: 10px; } label { width: 30px; height: 10px; }";
    ensure("field container stylesheet compiles", theme.loadRadia(kField).ok());

    rdui::Field field;
    field.setRect({0.f, 0.f, 50.f, 10.f});
    field.addChild(std::make_unique<rdui::Label>("Label"));
    field.addChild(std::make_unique<rdui::Switch>());
    rdui::layoutTree(field, theme, text);
    ensure_equals("field keeps its default row flow", field.children()[1]->rect().left(), 30.f);
}

template<> template<> void engineObject::test<19>() {
    rdui::StyleSheet theme;
    ensure("percentage geometry compiles", theme.loadRadia("label { width: 50%; height: 25%; left: 10%; top: 20%; }").ok());
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

template<> template<> void engineObject::test<20>() {
    rdui::StyleSheet theme;
    ensure("automatic gap stylesheet compiles", theme.loadRadia("panel { flow: row; gap: auto; } label { width: 10px; height: 10px; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));
    panel.addChild(std::make_unique<rdui::Label>("third"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("automatic row gap distributes remaining space", panel.children()[1]->rect().left(), 45.f);
    ensure_equals("automatic row gap places final child at the edge", panel.children()[2]->rect().right(), 100.f);
    ensure_equals("automatic gap contributes zero to intrinsic size", panel.desiredSize().x, 30.f);

    ensure("automatic column gap stylesheet compiles",
           theme.loadRadia("panel { flow: column; gap: auto; } label { width: 10px; height: 10px; }").ok());
    panel.setRect({0.f, 0.f, 20.f, 100.f});
    rdui::layoutTree(panel, theme, text);
    ensure_equals("automatic column gap distributes remaining space", panel.children()[1]->rect().top(), 55.f);
    ensure_equals("automatic column gap places final child at the edge", panel.children()[2]->rect().bottom(), 0.f);
}

template<> template<> void engineObject::test<21>() {
    rdui::StyleSheet theme;
    const char* kFloater =
        "floater { size: auto 100px; flow: column; } floater::header { height: 30px; } floater::content { flow: column; gap: 5px; } label { height: 20px; }";
    ensure("automatic floater size stylesheet compiles", theme.loadRadia(kFloater).ok());
    rdui::Floater floater;
    floater.setCanClose(false);
    floater.addChild(std::make_unique<rdui::Label>("first"));
    floater.addChild(std::make_unique<rdui::Label>("second"));

    const rdui::Vec2 measured = rdui::measureWidget(floater, theme, text);
    ensure_equals("fixed floater width remains fixed", measured.x, 100.f);
    ensure_equals("automatic floater height fits header and content", measured.y, 75.f);
}

template<> template<> void engineObject::test<22>() {
    rdui::StyleSheet theme;
    const char* kRowAlignment =
        "panel { flow: row; align-items: start; } label { width: 10px; height: 10px; } label#center { align-self: center; } label#end { align-self: end; } label#stretch { height: auto; align-self: stretch; }";
    ensure("row alignment stylesheet compiles", theme.loadRadia(kRowAlignment).ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    for (const char* id : {"start", "center", "end", "stretch"}) {
        auto label = std::make_unique<rdui::Label>(id);
        label->setId(id);
        panel.addChild(std::move(label));
    }

    rdui::layoutTree(panel, theme, text);
    ensure_equals("row start aligns to cross-start", panel.children()[0]->rect().top(), 40.f);
    ensure_equals("row align-self center overrides its container", panel.children()[1]->rect().bottom(), 15.f);
    ensure_equals("row align-self end reaches cross-end", panel.children()[2]->rect().bottom(), 0.f);
    ensure_equals("row stretch fills an automatic cross size", panel.children()[3]->rect().h, 40.f);

    const char* kColumnAlignment =
        "panel { flow: column; align-items: start; } label { width: 10px; height: 10px; } label#center { align-self: center; } label#end { align-self: end; } label#stretch { width: auto; align-self: stretch; }";
    ensure("column alignment stylesheet compiles", theme.loadRadia(kColumnAlignment).ok());
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

template<> template<> void engineObject::test<23>() {
    rdui::StyleSheet theme;
    ensure("visibility layout stylesheet compiles", theme.loadRadia("panel { flow: row; } label { width: 10px; height: 10px; }").ok());
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
    ensure_equals("Collapsed has zero measured size", rdui::measureWidget(*panel.children()[2], theme, text).x, 0.f);

    panel.children()[1]->setVisibility(rdui::Visibility::Collapsed);
    rdui::layoutTree(panel, theme, text);
    ensure_equals("collapsing Hidden invalidates parent measurement", panel.desiredSize().x, 10.f);

    panel.children()[2]->setVisibility(rdui::Visibility::Visible);
    rdui::layoutTree(panel, theme, text);
    ensure_equals("showing Collapsed invalidates parent measurement", panel.desiredSize().x, 20.f);
    ensure_equals("newly Visible child takes the second layout slot", panel.children()[2]->rect().left(), 10.f);
}

template<> template<> void engineObject::test<24>() {
    rdui::StyleSheet theme;
    const char* kVerticalAlignment =
        "panel { size: 40px 100px; flow: row; } label { size: 10px; } panel.middle { vertical-align: middle; } panel.bottom { vertical-align: bottom; } panel.column { flow: column; vertical-align: bottom; } panel.free-bottom { flow: free; vertical-align: bottom; } button { size: 40px 100px; } button > * { size: 10px; } button.top { vertical-align: top; }";
    ensure("vertical container alignment stylesheet compiles", theme.loadRadia(kVerticalAlignment).ok());

    auto addLabel = [](rdui::Widget& container) { container.addChild(std::make_unique<rdui::Label>("child")); };

    rdui::Panel top;
    top.setRect({0.f, 0.f, 100.f, 40.f});
    addLabel(top);
    rdui::layoutTree(top, theme, text);
    ensure_equals("row content defaults to the top", top.children()[0]->rect().top(), 40.f);

    rdui::Panel middle;
    middle.setRect({0.f, 0.f, 100.f, 40.f});
    middle.addClass("middle");
    addLabel(middle);
    rdui::layoutTree(middle, theme, text);
    ensure_equals("row content supports middle alignment", middle.children()[0]->rect().bottom(), 15.f);

    rdui::Panel bottom;
    bottom.setRect({0.f, 0.f, 100.f, 40.f});
    bottom.addClass("bottom");
    addLabel(bottom);
    rdui::layoutTree(bottom, theme, text);
    ensure_equals("row content supports bottom alignment", bottom.children()[0]->rect().bottom(), 0.f);

    rdui::Panel column;
    column.setRect({0.f, 0.f, 100.f, 40.f});
    column.addClass("column");
    addLabel(column);
    addLabel(column);
    rdui::layoutTree(column, theme, text);
    ensure_equals("column vertical alignment moves the complete stack", column.children()[0]->rect().top(), 20.f);
    ensure_equals("column stack reaches the bottom", column.children()[1]->rect().bottom(), 0.f);

    rdui::Panel freeBottom;
    freeBottom.setRect({0.f, 0.f, 100.f, 40.f});
    freeBottom.addClass("free-bottom");
    addLabel(freeBottom);
    rdui::layoutTree(freeBottom, theme, text);
    ensure_equals("free-flow unpositioned content follows vertical alignment", freeBottom.children()[0]->rect().bottom(), 0.f);

    rdui::Button button;
    button.setRect({0.f, 0.f, 100.f, 40.f});
    button.setLabel("button");
    rdui::layoutTree(button, theme, text);
    ensure_equals("Button content intrinsically defaults to middle", button.children()[0]->rect().bottom(), 15.f);
    ensure_equals("Button content is horizontally centered by its intrinsic defaults", button.children()[0]->rect().left(), 45.f);

    rdui::Button topButton;
    topButton.setRect({0.f, 0.f, 100.f, 40.f});
    topButton.addClass("top");
    topButton.setLabel("button");
    rdui::layoutTree(topButton, theme, text);
    ensure_equals("authored Button vertical alignment overrides its intrinsic default", topButton.children()[0]->rect().top(), 40.f);
}

template<> template<> void engineObject::test<25>() {
    rdui::StyleSheet theme;
    ensure("row Flow Break stylesheet compiles", theme.loadRadia("panel { flow: row; gap: 2px; } label { size: 10px; }").ok());

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

template<> template<> void engineObject::test<26>() {
    rdui::StyleSheet theme;
    ensure("flex basis and shrink stylesheet compiles",
           theme.loadRadia("panel { flow: row; } label { height: 10px; flex: 0 1 80px; } label.second { flex-basis: 40px; }").ok());

    rdui::Panel intrinsic;
    intrinsic.addChild(std::make_unique<rdui::Label>());
    auto intrinsicSecond = std::make_unique<rdui::Label>();
    intrinsicSecond->addClass("second");
    intrinsic.addChild(std::move(intrinsicSecond));
    rdui::measureTree(intrinsic, theme, text);
    ensure_equals("flex basis contributes to intrinsic width", intrinsic.desiredSize().x, 120.f);

    rdui::StyleSheet percentageTheme;
    ensure("percentage flex basis stylesheet compiles",
           percentageTheme.loadRadia("panel { flow: row; } label { width: 30px; flex-basis: 50%; }").ok());
    rdui::Panel indefinite;
    indefinite.addChild(std::make_unique<rdui::Label>());
    rdui::measureTree(indefinite, percentageTheme, text);
    ensure_equals("percentage flex basis is auto in an indefinite container", indefinite.desiredSize().x, 30.f);

    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<rdui::Label>());
    auto second = std::make_unique<rdui::Label>();
    second->addClass("second");
    panel.addChild(std::move(second));
    rdui::layoutTree(panel, theme, text);
    ensure_approximately_equals("scaled flex shrink reduces the larger basis proportionally", panel.children()[0]->rect().w, 200.f / 3.f, 5);
    ensure_approximately_equals("scaled flex shrink reduces the smaller basis proportionally", panel.children()[1]->rect().w, 100.f / 3.f, 5);
    ensure_approximately_equals("shrunk items fill the row", panel.children()[1]->rect().right(), 100.f, 5);
}

template<> template<> void engineObject::test<27>() {
    rdui::StyleSheet theme;
    const char* kFloaterHeader =
        "floater::header { height: 48px; flow: row; padding: 12px; } floater::header::icon { size: 28px; } floater::header::title { line-height: 18px; } floater::header::custom { height: 24px; flex-grow: 1; } floater::header::close { size: 24px; }";
    ensure("floater header stylesheet compiles", theme.loadRadia(kFloaterHeader).ok());

    rdui::Floater floater;
    floater.setIcon("search").setTitle("title");
    floater.header()->setRect({0.f, 0.f, 200.f, 48.f});
    rdui::layoutTree(*floater.header(), theme, text);

    const float headerCenter = floater.header()->rect().y + floater.header()->rect().h * .5f;
    for (const auto& child : floater.header()->children()) {
        if (child->visibility() != rdui::Visibility::Visible) continue;
        const float childCenter = child->rect().y + child->rect().h * .5f;
        ensure_equals("an oversized header icon does not pull the row below center", childCenter, headerCenter);
    }
}

template<> template<> void engineObject::test<28>() {
    rdui::StyleSheet theme;
    const char* kWrapping = "panel { flow: column; } text { font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ensure("explicit wrapping stylesheet compiles", theme.loadRadia(kWrapping).ok());

    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 30.f, 100.f});
    panel.addChild(std::make_unique<rdui::Text>("alpha beta"));
    panel.addChild(std::make_unique<rdui::Text>("after"));

    rdui::layoutTree(panel, theme, text);
    const rdui::Widget& wrapped = *panel.children()[0];
    const rdui::Widget& following = *panel.children()[1];
    ensure_equals("stretched auto-width text contributes both lines to layout height", wrapped.rect().h, 20.f);
    ensure_equals("the following sibling starts after the wrapped text", following.rect().top(), wrapped.rect().bottom());
}

template<> template<> void engineObject::test<29>() {
    rdui::StyleSheet theme;
    const char* kRowWrapping =
        "panel { width: 45px; flow: row; align-items: start; } text { min-width: 0px; flex: 1; font-size: 10px; line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; align-self: stretch; }";
    ensure("row wrapping stylesheet compiles", theme.loadRadia(kRowWrapping).ok());

    rdui::Panel panel;
    panel.addChild(std::make_unique<rdui::Text>("alpha beta"));
    panel.addChild(std::make_unique<rdui::Label>("x"));

    const rdui::Vec2 measured = rdui::measureWidget(panel, theme, text);
    ensure_equals("row measurement reflows text after flex shrink", measured.y, 20.f);

    panel.setRect({0.f, 0.f, measured.x, measured.y});
    const rdui::LayoutStatistics layoutStats = rdui::layoutTree(panel, theme, text);
    ensure("row layout remeasures text after width allocation", layoutStats.constrained_remeasures > 0);
    ensure_equals("row arrangement keeps the reflowed text height", panel.children()[0]->rect().h, 20.f);
    ensure_equals("row sibling receives its non-shrinking width", panel.children()[1]->rect().w, 10.f);
    ensure_equals("final row height is reapplied to stretched siblings", panel.children()[1]->rect().h, 20.f);
}

template<> template<> void engineObject::test<30>() {
    rdui::StyleSheet columnTheme;
    const char* kColumnBasis = "panel { width: 30px; flow: column; } text { flex-basis: 40px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
    ensure("column basis stylesheet compiles", columnTheme.loadRadia(kColumnBasis).ok());
    rdui::Panel column;
    column.addChild(std::make_unique<rdui::Text>("alpha beta"));
    ensure_equals("column measurement reapplies flex-basis after text reflow", rdui::measureWidget(column, columnTheme, text).y, 40.f);

    rdui::StyleSheet rowTheme;
    const char* kRowMinimum =
        "panel { width: 80px; flow: row; } text { flex: 0 1 100px; font-size: 10px; line-height: 10px; text-wrap: wrap; } label { width: 10px; flex-shrink: 0; }";
    ensure("row intrinsic minimum stylesheet compiles", rowTheme.loadRadia(kRowMinimum).ok());
    rdui::Panel row;
    row.addChild(std::make_unique<rdui::Text>("alpha beta"));
    row.addChild(std::make_unique<rdui::Label>("x"));
    row.setRect({0.f, 0.f, 80.f, 20.f});
    rdui::layoutTree(row, rowTheme, text);
    ensure_equals("row flex-basis can shrink to its pre-basis intrinsic minimum", row.children()[0]->rect().w, 70.f);
}
template<> template<> void engineObject::test<31>() {
    rdui::StyleSheet theme;
    ensure("cache stylesheet compiles", theme.loadRadia("panel { flow: row; } label { size: 10px; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));

    const rdui::LayoutStatistics first = rdui::layoutTree(panel, theme, text);
    const rdui::LayoutStatistics second = rdui::layoutTree(panel, theme, text);
    ensure("initial layout measures nodes", first.measured_nodes > 0);
    ensure("initial layout arranges nodes", first.arranged_nodes > 0);
    ensure_equals("clean layout does not measure", second.measured_nodes, std::size_t(0));
    ensure_equals("clean layout does not arrange", second.arranged_nodes, std::size_t(0));
    ensure("clean layout records skipped work", second.skipped_nodes >= 2);
}

template<> template<> void engineObject::test<32>() {
    rdui::StyleSheet theme;
    ensure("arrange-only stylesheet compiles", theme.loadRadia("panel { flow: row; } label { size: 10px; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));
    rdui::layoutTree(panel, theme, text);

    panel.setRect({0.f, 0.f, 140.f, 20.f});
    const rdui::LayoutStatistics resized = rdui::layoutTree(panel, theme, text);
    ensure("viewport resize remeasures constrained sizing", resized.measured_nodes > 0);
    ensure("viewport resize arranges the affected tree", resized.arranged_nodes > 0);

    const rdui::LayoutStatistics directionChanged = rdui::layoutTree(panel, theme, text, rdui::LayoutDirection::RightToLeft);
    ensure_equals("direction-only changes do not measure", directionChanged.measured_nodes, std::size_t(0));
    ensure("direction-only changes arrange the tree", directionChanged.arranged_nodes > 0);
}

template<> template<> void engineObject::test<33>() {
    class GenerationTextMetrics final : public rdui::TextMetrics {
    public:
        rdui::Vec2 measureText(const std::string&, const rdui::Style&) const override { return {32.f, 12.f}; }
        std::uint64_t generation() const override { return mGeneration; }
        void advance() { ++mGeneration; }

    private:
        std::uint64_t mGeneration = 1;
    } metrics;

    rdui::StyleSheet theme;
    rdui::Label label("generation");
    const rdui::LayoutStatistics first = rdui::layoutTree(label, theme, metrics);
    metrics.advance();
    const rdui::LayoutStatistics second = rdui::layoutTree(label, theme, metrics);
    ensure("initial metrics pass measures the label", first.measured_nodes > 0);
    ensure("metrics generation invalidates the measurement cache", second.measured_nodes > 0);
}

template<> template<> void engineObject::test<34>() {
    rdui::StyleSheet theme;
    ensure("column growth stylesheet compiles", theme.loadRadia("panel { flow: column; } label { height: 10px; flex-grow: 1; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));

    const rdui::LayoutStatistics stats = rdui::layoutTree(panel, theme, text);
    ensure("column allocation remeasures flex children with height constraints", stats.constrained_remeasures >= 2);
    ensure_equals("first flex child receives half the column", panel.children()[0]->rect().h, 50.f);
    ensure_equals("second flex child receives half the column", panel.children()[1]->rect().h, 50.f);
    ensure_equals("column children consume the allocated height", panel.children()[1]->rect().bottom(), 0.f);
}

template<> template<> void engineObject::test<35>() {
    rdui::StyleSheet theme;
    ensure("column percentage-height stylesheet compiles",
           theme.loadRadia("panel { flow: column; } #quarter { height: 25%; } #half { height: 50%; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto quarter = std::make_unique<rdui::Label>("quarter");
    quarter->setId("quarter");
    auto half = std::make_unique<rdui::Label>("half");
    half->setId("half");
    panel.addChild(std::move(quarter));
    panel.addChild(std::move(half));

    rdui::layoutTree(panel, theme, text);
    ensure_approximately_equals("column percentage height resolves against panel", panel.children()[0]->rect().h, 25.f, 6);
    ensure_approximately_equals("second percentage height resolves independently", panel.children()[1]->rect().h, 50.f, 6);
    ensure_approximately_equals("percentage-height siblings retain their order", panel.children()[0]->rect().bottom(),
                                panel.children()[1]->rect().top(), 6);
}

template<> template<> void engineObject::test<36>() {
    rdui::StyleSheet theme;
    ensure("nested percentage-height stylesheet compiles",
           theme.loadRadia("panel { flow: column; } #child { height: 50%; flow: column; } #grandchild { height: 50%; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    auto child = std::make_unique<rdui::Panel>();
    child->setId("child");
    auto grandchild = std::make_unique<rdui::Label>("nested");
    grandchild->setId("grandchild");
    child->addChild(std::move(grandchild));
    panel.addChild(std::move(child));

    rdui::layoutTree(panel, theme, text);
    const rdui::Widget& nestedPanel = *panel.children().front();
    ensure_approximately_equals("nested panel receives its percentage height", nestedPanel.rect().h, 50.f, 6);
    ensure_approximately_equals("nested percentage resolves against allocated parent height", nestedPanel.children().front()->rect().h, 25.f, 6);
}

template<> template<> void engineObject::test<37>() {
    rdui::StyleSheet theme;
    ensure("column minimum-height stylesheet compiles", theme.loadRadia("panel { flow: column; } label { height: 30px; min-height: 25px; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 40.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("column shrink respects the first minimum height", panel.children()[0]->rect().h, 25.f);
    ensure_equals("column shrink respects the second minimum height", panel.children()[1]->rect().h, 25.f);
}

template<> template<> void engineObject::test<38>() {
    rdui::StyleSheet theme;
    ensure("column height cache stylesheet compiles", theme.loadRadia("panel { flow: column; } label { flex: 1; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<rdui::Label>("first"));
    panel.addChild(std::make_unique<rdui::Label>("second"));

    const rdui::LayoutStatistics first = rdui::layoutTree(panel, theme, text);
    const rdui::LayoutStatistics clean = rdui::layoutTree(panel, theme, text);
    ensure("initial column layout performs constrained measurement", first.constrained_remeasures > 0);
    ensure_equals("clean column layout reuses height-constrained cache", clean.measured_nodes, std::size_t(0));
    ensure_equals("clean column layout does not re-arrange", clean.arranged_nodes, std::size_t(0));

    panel.setRect({0.f, 0.f, 100.f, 140.f});
    const rdui::LayoutStatistics resized = rdui::layoutTree(panel, theme, text);
    ensure("column height change invalidates constrained measurement", resized.constrained_remeasures > 0);
}

template<> template<> void engineObject::test<39>() {
    rdui::StyleSheet narrow;
    rdui::StyleSheet wide;
    ensure("first cache identity stylesheet compiles", narrow.loadRadia("label { width: 10px; height: 10px; }").ok());
    ensure("second cache identity stylesheet compiles", wide.loadRadia("label { width: 30px; height: 10px; }").ok());

    rdui::Label label("identity");
    rdui::layoutTree(label, narrow, text);
    rdui::layoutTree(label, wide, text);
    ensure_equals("different stylesheet objects cannot reuse layout cache", label.desiredSize().x, 30.f);
}

template<> template<> void engineObject::test<40>() {
    class WidthMetrics final : public rdui::TextMetrics {
    public:
        explicit WidthMetrics(float width) : mWidth(width) {}
        rdui::Vec2 measureText(const std::string&, const rdui::Style&) const override { return {mWidth, 12.f}; }

    private:
        float mWidth;
    } narrow(12.f), wide(48.f);

    rdui::StyleSheet theme;
    rdui::Label label("identity");
    rdui::layoutTree(label, theme, narrow);
    rdui::layoutTree(label, theme, wide);
    ensure_equals("different text metric objects cannot reuse layout cache", label.desiredSize().x, 48.f);
}

template<> template<> void engineObject::test<41>() {
    rdui::StyleSheet theme;
    const char* kOrderedFlow = "panel { flow: row; } #late { order: 2; width: 10px; height: 10px; } #early { order: -1; width: 10px; height: 10px; }";
    ensure("ordered-flow stylesheet compiles", theme.loadRadia(kOrderedFlow).ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 20.f});
    auto late = std::make_unique<rdui::Label>("late");
    late->setId("late");
    auto early = std::make_unique<rdui::Label>("early");
    early->setId("early");
    panel.addChild(std::move(late));
    panel.addChild(std::move(early));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("ordered row measures in visual order", panel.children()[1]->rect().x, 0.f);
    ensure_equals("ordered row places the later source child after the early child", panel.children()[0]->rect().x, 10.f);
}

template<> template<> void engineObject::test<42>() {
    rdui::StyleSheet theme;
    ensure("initial assigned stylesheet compiles", theme.loadRadia("label { width: 10px; height: 10px; }").ok());
    rdui::Label label("assigned");
    rdui::layoutTree(label, theme, text);

    rdui::StyleSheet replacement;
    ensure("replacement assigned stylesheet compiles", replacement.loadRadia("label { width: 30px; height: 10px; }").ok());
    theme = replacement;
    rdui::layoutTree(label, theme, text);
    ensure_equals("stylesheet assignment advances the cache generation", label.desiredSize().x, 30.f);
}

template<> template<> void engineObject::test<43>() {
    rdui::StyleSheet theme;
    ensure("free-flow percentage wrapping stylesheet compiles",
           theme.loadRadia("panel { flow: free; } text { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<rdui::Text>("alpha beta"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("free-flow percentage text initially wraps", panel.children().front()->rect().h, 20.f);

    panel.setRect({0.f, 0.f, 200.f, 100.f});
    rdui::layoutTree(panel, theme, text);
    ensure_equals("free-flow percentage text reflows after parent resize", panel.children().front()->rect().h, 10.f);
}

template<> template<> void engineObject::test<44>() {
    rdui::StyleSheet theme;
    ensure("free-flow intrinsic offsets stylesheet compiles",
           theme.loadRadia("panel { flow: free; } label { width: 20px; height: 10px; right: 5px; bottom: 7px; }").ok());
    rdui::Panel panel;
    panel.addChild(std::make_unique<rdui::Label>("positioned"));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("free-flow right offset contributes to intrinsic width", panel.desiredSize().x, 25.f);
    ensure_equals("free-flow bottom offset contributes to intrinsic height", panel.desiredSize().y, 17.f);
}

template<> template<> void engineObject::test<45>() {
    rdui::StyleSheet theme;
    ensure("free-flow explicit geometry stylesheet compiles", theme.loadRadia("panel { flow: free; }").ok());
    rdui::Panel panel;
    auto child = std::make_unique<rdui::Panel>();
    child->setRect({0.f, 0.f, 40.f, 30.f});
    panel.addChild(std::move(child));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("explicit free-flow child width contributes to intrinsic size", panel.desiredSize().x, 40.f);
    ensure_equals("explicit free-flow child height contributes to intrinsic size", panel.desiredSize().y, 30.f);
}

template<> template<> void engineObject::test<46>() {
    rdui::StyleSheet theme;
    ensure("free-flow explicit geometry cache stylesheet compiles", theme.loadRadia("panel { flow: free; }").ok());
    rdui::Panel panel;
    auto child = std::make_unique<rdui::Panel>();
    child->setRect({0.f, 0.f, 20.f, 10.f});
    rdui::Widget* childPtr = child.get();
    panel.addChild(std::move(child));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("initial explicit free-flow width contributes to intrinsic size", panel.desiredSize().x, 20.f);

    childPtr->setRect({0.f, 0.f, 60.f, 10.f});
    rdui::layoutTree(panel, theme, text);
    ensure_equals("changing explicit free-flow geometry invalidates intrinsic size", panel.desiredSize().x, 60.f);
}

template<> template<> void engineObject::test<47>() {
    rdui::StyleSheet theme;
    ensure("free-flow explicit position stylesheet compiles", theme.loadRadia("panel { flow: free; }").ok());
    rdui::Panel panel;
    auto child = std::make_unique<rdui::Panel>();
    child->setRect({10.f, 12.f, 20.f, 8.f});
    panel.addChild(std::move(child));
    rdui::layoutTree(panel, theme, text);
    ensure_equals("explicit free-flow x contributes to intrinsic size", panel.desiredSize().x, 30.f);
    ensure_equals("explicit free-flow y contributes to intrinsic size", panel.desiredSize().y, 20.f);
}

template<> template<> void engineObject::test<48>() {
    rdui::StyleSheet theme;
    ensure("free-flow intrinsic percentage height stylesheet compiles",
           theme.loadRadia("panel { flow: free; width: 100px; } text { width: 50%; font-size: 10px; line-height: 10px; text-wrap: wrap; }").ok());
    rdui::Panel panel;
    panel.addChild(std::make_unique<rdui::Text>("alpha beta"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("auto-height free-flow parent includes wrapped percentage child", panel.desiredSize().y, 20.f);
    ensure_equals("wrapped percentage child keeps intrinsic height", panel.children().front()->rect().h, 20.f);
}

template<> template<> void engineObject::test<49>() {
    rdui::StyleSheet theme;
    ensure("free-flow percentage offsets stylesheet compiles",
           theme.loadRadia("panel { flow: free; } label { width: 20px; height: 10px; left: 50%; top: 20%; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 100.f});
    panel.addChild(std::make_unique<rdui::Label>("positioned"));

    rdui::layoutTree(panel, theme, text);
    ensure_equals("percentage left contributes against parent width", panel.desiredSize().x, 70.f);
    ensure_approximately_equals("percentage top contributes against parent height", panel.desiredSize().y, 30.f, 6);
}

template<> template<> void engineObject::test<50>() {
    rdui::StyleSheet theme;
    ensure("free-flow percentage dimensions use explicit geometry stylesheet compiles",
           theme.loadRadia("panel { flow: free; width: 50%; height: 50%; } label { width: 50%; height: 50%; }").ok());
    rdui::Panel panel;
    panel.setRect({0.f, 0.f, 100.f, 80.f});
    panel.addChild(std::make_unique<rdui::Label>("sized"));

    rdui::layoutTree(panel, theme, text);
    ensure_approximately_equals("free-flow percentage child width uses explicit parent width", panel.children().front()->rect().w, 50.f, 6);
    ensure_approximately_equals("free-flow percentage child height uses explicit parent height", panel.children().front()->rect().h, 40.f, 6);
}

template<> template<> void engineObject::test<51>() {
    rdui::StyleSheet theme;
    const char* kNestedFlexBasis =
        "#outer { flow: row; width: 100px; height: 20px; } #inner { flow: row; width: 50%; } label { flex-basis: 50%; height: 10px; }";
    ensure("nested percentage flex-basis stylesheet compiles", theme.loadRadia(kNestedFlexBasis).ok());
    rdui::Panel outer;
    outer.setId("outer");
    auto inner = std::make_unique<rdui::Panel>();
    inner->setId("inner");
    rdui::Widget* innerPtr = inner.get();
    inner->addChild(std::make_unique<rdui::Label>("basis"));
    rdui::Widget* label = inner->children().front().get();
    outer.addChild(std::move(inner));

    rdui::layoutTree(outer, theme, text);
    ensure_approximately_equals("percentage flex-basis uses the allocated nested width", innerPtr->rect().w, 50.f, 6);
    ensure_approximately_equals("nested percentage flex-basis resolves against its parent", label->rect().w, 25.f, 6);
}
} // namespace tut
