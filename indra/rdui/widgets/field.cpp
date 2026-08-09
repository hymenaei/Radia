/**
 * @file field.cpp
 * @brief Implements the Field composite that relates labels and value controls.
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
#include "widgets/field.h"
#include "layout/engine.h"
#include "style/style.h"
#include "system.h"
#include "widgets/label.h"
#include "widgets/widgetcontractbuilder.h"

namespace rdui {
namespace {
class FieldHint final : public Text {
public:
    FieldHint() : Text("hint", ElementTag{}) {}
};

class FieldError final : public Text {
public:
    FieldError() : Text("error", ElementTag{}) { setVisibility(Visibility::Collapsed); }
};

class FieldSupportIndent final : public Widget {
public:
    FieldSupportIndent() : Widget("field-support-indent") { setPointerEvents(false); }

    void setControl(Widget* control) { mControl.set(control); }

    Vec2 intrinsicSize(const StyleSheet& theme, const Style&, const TextMetrics&, const IntrinsicSizeConstraints&) const override {
        if (!mControl) return {};
        const Style control_style = resolveWidgetStyle(theme, *mControl);
        return {mControl->desiredSize().x + control_style.margin.horizontal(), 0.f};
    }

private:
    WidgetRef<Widget> mControl;
};
} // namespace

Field::Field() : Widget(ELEMENT) {}

void Field::constrainResolvedStyle(Style& style) const {
    style.flow = Flow::Row;
    if (!style.vertical_align_set) style.vertical_align = VerticalAlign::Middle;
}

Widget* Field::control() {
    return mControl;
}

const Widget* Field::control() const {
    return mControl;
}

void Field::onChildAdded(Widget& child) {
    if (!child.part().empty()) return;
    if (auto* label = dynamic_cast<Label*>(&child); label && !mLabel) mLabel.set(label);
    if (auto* control = dynamic_cast<ValueControl*>(&child); control && !mControl) {
        mControl = control;
        refreshValueState(control->valueControlState());
        mControlSubscription = control->observeValueControlState([this](const ValueControlState& state) { refreshValueState(state); });
    }
}

void Field::onChildrenCleared() {
    mControlSubscription.reset();
    mLabel.set(nullptr);
    mControl = nullptr;
    mHint.set(nullptr);
    mError.set(nullptr);
    mHintIndent.set(nullptr);
    mErrorIndent.set(nullptr);
    mAuthoredError = {};
    mDirty = false;
    setState(WidgetState::Invalid, false);
}

bool Field::controlPrecedesLabel() const {
    for (const auto& child : children()) {
        if (child.get() == control()) return true;
        if (child.get() == label()) return false;
    }
    return false;
}

Widget* Field::createSupportIndent(WidgetRef<Widget>& slot, const char* part, bool collapsed) {
    auto indent = std::make_unique<FieldSupportIndent>();
    indent->setControl(control());
    if (collapsed) indent->setVisibility(Visibility::Collapsed);
    detail::WidgetCompilerAccess::setStyleIdentity(*indent, styleElement(), part);
    Widget* result = indent.get();
    Widget::addChild(std::move(indent));
    slot.set(result);
    return result;
}

Widget* Field::setHintContent(TextSource content) {
    if (!mHint) {
        if (controlPrecedesLabel()) createSupportIndent(mHintIndent, "hint-indent", false);
        auto hint = std::make_unique<FieldHint>();
        mHint.set(hint.get());
        Widget::addChild(std::move(hint));
    }
    mHint->setContent(std::move(content));
    return mHintIndent ? mHintIndent.get() : mHint.get();
}

Widget* Field::setErrorContent(TextSource content) {
    if (!mError) {
        if (controlPrecedesLabel()) createSupportIndent(mErrorIndent, "error-indent", true);
        auto error = std::make_unique<FieldError>();
        mError.set(error.get());
        Widget::addChild(std::move(error));
    }
    mAuthoredError = std::move(content);
    mError->setContent(mAuthoredError);
    refreshValueState(mControl ? mControl->valueControlState() : ValueControlState{});
    return mErrorIndent ? mErrorIndent.get() : mError.get();
}

void Field::refreshValueState(const ValueControlState& state) {
    mDirty = state.dirty;
    const bool invalid = state.validation == ValueValidationStatus::Invalid;
    setState(WidgetState::Invalid, invalid);
    if (!mError) return;
    if (!invalid) {
        if (mErrorIndent) mErrorIndent->setVisibility(Visibility::Collapsed);
        mError->setVisibility(Visibility::Collapsed);
        return;
    }
    TextSource content = state.message ? *state.message : mAuthoredError;
    mError->setContent(std::move(content));
    if (mErrorIndent) mErrorIndent->setVisibility(Visibility::Visible);
    mError->setVisibility(Visibility::Visible);
}

WidgetContract detail::fieldContract() {
    return defineWidget<Field>(Field::ELEMENT)
        .state(WidgetState::Invalid)
        .composition([](const LayoutElement& element, Field& field, const ViewScopeContext&, ViewBuildResult& result, const std::string& source) {
            std::vector<Label*> labels;
            std::vector<ValueControl*> controls;
            for (const auto& child : field.children()) {
                if (!child->part().empty()) continue;
                if (child.get() == field.hint() || child.get() == field.error()) continue;
                if (auto* label = dynamic_cast<Label*>(child.get())) labels.push_back(label);
                else if (auto* control = dynamic_cast<ValueControl*>(child.get())) controls.push_back(control);
                else
                    result.error("view.field.child_unsupported", "Field accepts only Label, one Value Control, Hint, Error, and Flow Break.", source,
                                 element.source().begin.line, element.source().begin.column);
            }
            if (labels.size() != 1)
                result.error("view.field.label_required", "Field requires exactly one direct Label.", source, element.source().begin.line,
                             element.source().begin.column);
            if (controls.size() != 1)
                result.error("view.field.control_required", "Field requires exactly one direct Value Control.", source, element.source().begin.line,
                             element.source().begin.column);
            if (labels.size() == 1 && controls.size() == 1) {
                const std::string& target_id = detail::WidgetCompilerAccess::labelTargetId(*labels.front());
                if (!target_id.empty() && target_id != controls.front()->id())
                    result.error("view.field.label_target_mismatch", "Field Label must target its direct Value Control.", source,
                                 element.source().begin.line, element.source().begin.column);
            }
        })
        .scopedInlineContent("hint",
                             {InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                             [](TextSource content, Field& field, ViewBuildResult& result, const std::string& source, std::size_t line,
                                std::size_t column) -> Widget* {
                                 if (field.hint()) {
                                     result.error("view.field.hint_duplicate", "Field accepts only one hint.", source, line, column);
                                     return field.hint();
                                 }
                                 return field.setHintContent(std::move(content));
                             })
        .scopedInlineContent("error",
                             {InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                             [](TextSource content, Field& field, ViewBuildResult& result, const std::string& source, std::size_t line,
                                std::size_t column) -> Widget* {
                                 if (field.error()) {
                                     result.error("view.field.error_duplicate", "Field accepts only one error.", source, line, column);
                                     return field.error();
                                 }
                                 return field.setErrorContent(std::move(content));
                             })
        .build();
}

WidgetContract detail::hintContract() {
    return defineWidget<FieldHint>("hint")
        .scopedOnly()
        .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                       [](TextSource content, FieldHint& hint) { hint.setContent(std::move(content)); })
        .build();
}

WidgetContract detail::errorContract() {
    return defineWidget<FieldError>("error")
        .scopedOnly()
        .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                       [](TextSource content, FieldError& error) { error.setContent(std::move(content)); })
        .build();
}
} // namespace rdui
