/**
 * @file icon.cpp
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
#include "widgets/icon.h"
#include "render/paintcontext.h"
#include "widgets/widgetcontract.h"

namespace rdui {
Icon::Icon(std::string name) : Widget(ELEMENT), mName(std::move(name)) {}

Icon& Icon::setName(std::string name) {
    mName = std::move(name);
    invalidatePaint();
    return *this;
}

void Icon::paint(PaintContext& context, const Style& style, float scale) const {
    context.paintBox(rect(), style);
    context.paintIcon(mName, rect(), style, scale);
}

WidgetContract detail::iconContract() {
    return defineWidget<Icon>(Icon::ELEMENT).attributes({stringAttribute("src", &Icon::setName)}).build();
}
} // namespace rdui
