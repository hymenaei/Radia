/**
 * @file fieldset.h
 * @brief Defines grouped Fieldset and Legend composition.
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

#ifndef RD_WIDGETS_FIELDSET_H
#define RD_WIDGETS_FIELDSET_H

#include "widgets/text.h"
#include "widgets/widget.h"

namespace radia::ui {
struct WidgetContract;

namespace detail {
WidgetContract fieldsetContract();
WidgetContract legendContract();
} // namespace detail

class Fieldset : public Widget {
    friend WidgetContract detail::fieldsetContract();

public:
    static constexpr const char* ELEMENT = "fieldset";

    Fieldset();
    Text* legend() { return mLegend.get(); }
    const Text* legend() const { return mLegend.get(); }
    void paint(PaintContext& context, const Style& style, float scale) const override;

protected:
    void constrainResolvedStyle(Style& style) const override;
    void onChildrenCleared() override;
    void onArranged(const Style& style) override;
    Rect paintBounds() const override;
    bool hasLayoutGapBetween(const Widget& previous, const Widget& next) const override;
    float layoutOverlapBetween(const Widget& previous, const Widget& next, const Style& style) const override;

private:
    Text* setLegendContent(TextSource content);
    Rect borderRect() const;

    WidgetRef<Text> mLegend;
};
} // namespace radia::ui
#endif // RD_WIDGETS_FIELDSET_H
