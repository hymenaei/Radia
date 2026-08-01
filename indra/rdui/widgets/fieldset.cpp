/**
 * @file fieldset.cpp
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
#include "widgets/fieldset.h"
#include <algorithm>
#include <limits>
#include "render/paintcontext.h"
#include "style/style.h"
#include "widgets/field.h"
#include "widgets/widgetcontract.h"

namespace rdui {
namespace {
class FieldsetLegend final : public Text {
public:
    FieldsetLegend() : Text("legend", ElementTag{}) {}

protected:
    void constrainResolvedStyle(Style& style) const override {
        if (style.align_self == AlignSelf::Auto) style.align_self = AlignSelf::Start;
        style.order = std::numeric_limits<int>::lowest();
    }
};
} // namespace

Fieldset::Fieldset() : Widget(ELEMENT) {}

void Fieldset::constrainResolvedStyle(Style& style) const {
    style.flow = Flow::Column;
}

void Fieldset::onChildrenCleared() {
    mLegend.set(nullptr);
}

void Fieldset::onArranged(const Style& style) {
    if (!mLegend || mLegend->visibility() == Visibility::Collapsed) return;

    translateChild(*mLegend, {0.f, style.padding.top});

    Widget* first_content = nullptr;
    for (const auto& child : children()) {
        if (child.get() == mLegend.get() || child->visibility() == Visibility::Collapsed) continue;
        if (!first_content || child->rect().top() > first_content->rect().top()) first_content = child.get();
    }
    if (!first_content) return;

    const float content_top = borderRect().top() - style.border_width.top - style.padding.top;
    const float offset = content_top - first_content->rect().top();
    for (const auto& child : children())
        if (child.get() != mLegend.get()) translateChild(*child, {0.f, offset});
}

bool Fieldset::hasLayoutGapBetween(const Widget& previous, const Widget&) const {
    return &previous != mLegend.get();
}

float Fieldset::layoutOverlapBetween(const Widget& previous, const Widget&, const Style& style) const {
    if (&previous != mLegend.get()) return 0.f;
    return std::max(0.f, mLegend->desiredSize().y * .5f - style.border_width.top);
}

Text* Fieldset::setLegendContent(TextSource content) {
    if (!mLegend) {
        auto legend = std::make_unique<FieldsetLegend>();
        mLegend.set(legend.get());
        Widget::addChild(std::move(legend));
    }
    mLegend->setContent(std::move(content));
    return mLegend.get();
}

Rect Fieldset::borderRect() const {
    if (!mLegend || mLegend->visibility() == Visibility::Collapsed || mLegend->rect().h <= 0.f) return rect();

    const float border_top = mLegend->rect().y + mLegend->rect().h * .5f;
    return {rect().x, rect().y, rect().w, std::max(0.f, border_top - rect().y)};
}

Rect Fieldset::paintBounds() const {
    return borderRect();
}

void Fieldset::paint(PaintContext& context, const Style& style, float) const {
    const Rect box = borderRect();
    if (!mLegend || box.top() == rect().top()) {
        context.paintBox(box, style);
        return;
    }

    constexpr float legend_clearance = 2.f;
    const TopBorderGap gap{
        std::max(box.left(), mLegend->rect().left() - legend_clearance),
        std::min(box.right(), mLegend->rect().right() + legend_clearance),
    };
    context.paintBox(box, style, gap);
}

WidgetContract detail::fieldsetContract() {
    return defineWidget<Fieldset>(Fieldset::ELEMENT)
        .composition(
            [](const LayoutElement& element, Fieldset& fieldset, const ViewScopeContext&, ViewBuildResult& result, const std::string& source) {
                std::size_t field_count = 0;
                bool has_flow_break = false;
                for (const auto& child : fieldset.children()) {
                    has_flow_break = has_flow_break || child->flowBreakBefore();
                    if (child.get() == fieldset.legend() || !child->part().empty()) continue;
                    if (dynamic_cast<Field*>(child.get())) ++field_count;
                    else
                        result.error("view.fieldset.child_unsupported", "Fieldset accepts only one Legend and direct Fields.", source,
                                     element.source().begin.line, element.source().begin.column);
                }
                if (!fieldset.legend())
                    result.error("view.fieldset.legend_required", "Fieldset requires exactly one direct Legend.", source, element.source().begin.line,
                                 element.source().begin.column);
                if (field_count == 0)
                    result.error("view.fieldset.field_required", "Fieldset requires one or more direct Fields.", source, element.source().begin.line,
                                 element.source().begin.column);
                if (has_flow_break)
                    result.error("view.fieldset.flow_break_unsupported", "Fieldset does not accept Flow Break directives.", source,
                                 element.source().begin.line, element.source().begin.column);
            })
        .scopedInlineContent("legend",
                             {InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                             [](TextSource content, Fieldset& fieldset, ViewBuildResult& result, const std::string& source, std::size_t line,
                                std::size_t column) -> Widget* {
                                 if (fieldset.legend()) {
                                     result.error("view.fieldset.legend_duplicate", "Fieldset accepts only one Legend.", source, line, column);
                                     return fieldset.legend();
                                 }
                                 return fieldset.setLegendContent(std::move(content));
                             })
        .build();
}

WidgetContract detail::legendContract() {
    return defineWidget<FieldsetLegend>("legend")
        .scopedOnly()
        .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                       [](TextSource content, FieldsetLegend& legend) { legend.setContent(std::move(content)); })
        .build();
}
} // namespace rdui
