/**
 * @file button.h
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

#ifndef RD_WIDGETS_BUTTON_H
#define RD_WIDGETS_BUTTON_H

#include "localization/localization.h"
#include "widgets/widget.h"

namespace radia::ui {
class Icon;
class Label;
struct WidgetContract;

namespace detail { WidgetContract buttonContract(); }

class Button : public Widget {
    friend WidgetContract detail::buttonContract();

public:
    static constexpr const char* sElement = "button";

    Button();
    Icon& setIcon(std::string name);
    Label& setLabel(std::string text);
    Icon* icon() { return mIcon.get(); }
    const Icon* icon() const { return mIcon.get(); }
    Label* label() { return mLabel.get(); }
    const Label* label() const { return mLabel.get(); }
    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onChildAdded(Widget& child) override;
    void onChildrenCleared() override;

private:
    WidgetRef<Icon> mIcon;
    WidgetRef<Label> mLabel;
};
} // namespace radia::ui
#endif // RD_WIDGETS_BUTTON_H
