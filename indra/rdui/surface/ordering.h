/**
 * @file ordering.h
 * @brief Provides one order-aware child traversal for layout-facing surface work.
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
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 * $/LicenseInfo$
 */

#ifndef RD_SURFACE_ORDERING_H
#define RD_SURFACE_ORDERING_H

#include <vector>
#include "style/stylepass.h"
#include "widgets/widget.h"

namespace rdui {
inline StylePass::ChildSnapshot orderedChildren(const Widget& parent, StylePass& styles) {
    return styles.orderedChildren(parent);
}

inline StylePass::ChildSnapshot sourceChildren(const Widget& parent, StylePass& styles) {
    return styles.sourceChildren(parent);
}
} // namespace rdui
#endif // RD_SURFACE_ORDERING_H
