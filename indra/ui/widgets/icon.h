/**
 * @file icon.h
 * @brief Defines the asset-backed Icon Widget.
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

#ifndef RD_WIDGETS_ICON_H
#define RD_WIDGETS_ICON_H

#include "widgets/widget.h"

namespace radia::ui {
struct WidgetContract;

namespace detail { WidgetContract iconContract(); }

class Icon : public Widget {
    friend WidgetContract detail::iconContract();

public:
    static constexpr const char* sElement = "icon";

    explicit Icon(std::string name = {});
    Icon& setName(std::string name);
    const std::string& name() const { return mName; }
    void paint(PaintContext& context, const Style& style, float scale) const override;

private:
    std::string mName;
};
} // namespace radia::ui
#endif // RD_WIDGETS_ICON_H
