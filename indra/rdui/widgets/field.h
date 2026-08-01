/**
 * @file field.h
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

#ifndef LL_RDUI_FIELD_H
#define LL_RDUI_FIELD_H

#include "binding/valuecontrol.h"
#include "widgets/text.h"
#include "widgets/widget.h"

namespace rdui {
class Label;
struct WidgetContract;

namespace detail {
WidgetContract fieldContract();
WidgetContract hintContract();
WidgetContract errorContract();
} // namespace detail

class Field : public Widget {
    friend WidgetContract detail::fieldContract();

public:
    static constexpr const char* ELEMENT = "field";

    Field();
    Label* label() { return mLabel.get(); }
    const Label* label() const { return mLabel.get(); }
    Widget* control();
    const Widget* control() const;
    Text* hint() { return mHint.get(); }
    const Text* hint() const { return mHint.get(); }
    Text* error() { return mError.get(); }
    const Text* error() const { return mError.get(); }
    bool dirty() const { return mDirty; }
    bool invalid() const { return hasState(WidgetState::Invalid); }

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onChildAdded(Widget& child) override;
    void onChildrenCleared() override;

private:
    Widget* setHintContent(TextSource content);
    Widget* setErrorContent(TextSource content);
    Widget* createSupportIndent(WidgetRef<Widget>& slot, const char* part, bool collapsed);
    bool controlPrecedesLabel() const;
    void refreshValueState(const ValueControlState& state);

    WidgetRef<Label> mLabel;
    ValueControl* mControl = nullptr;
    WidgetRef<Text> mHint;
    WidgetRef<Text> mError;
    WidgetRef<Widget> mHintIndent;
    WidgetRef<Widget> mErrorIndent;
    TextSource mAuthoredError;
    ValueBindingSubscription mControlSubscription;
    bool mDirty = false;
};
} // namespace rdui
#endif // LL_RDUI_FIELD_H
