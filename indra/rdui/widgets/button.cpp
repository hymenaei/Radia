/**
 * @file button.cpp
 * @brief Defines the semantic Button Widget and its contract.
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
#include "widgets/button.h"
#include "localization/localization.h"
#include "style/style.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/widgetcontractbuilder.h"

namespace rdui {
namespace {
class ButtonCaption final : public Label {
public:
    explicit ButtonCaption(TextSource content) : Label("button-caption", {}) { setContent(std::move(content)); }
};
} // namespace

Button::Button() : Widget(ELEMENT) {}

void Button::constrainResolvedStyle(Style& style) const {
    if (!style.flowSet) style.flow = Flow::Row;
    if (!style.justifyContentSet) style.justifyContent = JustifyContent::Center;
    if (!style.verticalAlignSet) style.verticalAlign = VerticalAlign::Middle;
}

void Button::onChildAdded(Widget& child) {
    if (auto* icon = dynamic_cast<Icon*>(&child); icon && !mIcon) mIcon.set(icon);
    else if (auto* label = dynamic_cast<Label*>(&child); label && !mLabel) mLabel.set(label);
}

Icon& Button::setIcon(std::string name) {
    if (!mIcon) addChild(std::make_unique<Icon>());
    return mIcon->setName(std::move(name));
}

Label& Button::setLabel(std::string text) {
    if (!mLabel) addChild(std::make_unique<ButtonCaption>(TextSource{}));
    return mLabel->setText(std::move(text));
}

void Button::onChildrenCleared() {
    mIcon.set(nullptr);
    mLabel.set(nullptr);
}

WidgetContract detail::buttonContract() {
    return defineWidget<Button>(Button::ELEMENT)
        .events({WidgetEventKind::Click, WidgetEventKind::DoubleClick, WidgetEventKind::MouseDown, WidgetEventKind::MouseUp,
                 WidgetEventKind::MouseMove, WidgetEventKind::LongClick, WidgetEventKind::ContextMenu})
        .textChildren([](TextSource content) { return std::make_unique<ButtonCaption>(std::move(content)); })
        .build();
}
} // namespace rdui
