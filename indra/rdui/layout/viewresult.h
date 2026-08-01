/**
 * @file viewresult.h
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

#ifndef LL_RDUI_VIEW_RESULT_H
#define LL_RDUI_VIEW_RESULT_H

#include <memory>
#include "diagnostic.h"
#include "widgets/widget.h"

namespace rdui {
struct ViewBuildResult : DiagnosticResult {
    std::unique_ptr<Widget> root;
    bool ok() const { return !hasErrors() && root != nullptr; }

    template<typename WidgetT> WidgetT* rootAs() { return dynamic_cast<WidgetT*>(root.get()); }

    template<typename WidgetT> const WidgetT* rootAs() const { return dynamic_cast<const WidgetT*>(root.get()); }
};
} // namespace rdui
#endif // LL_RDUI_VIEW_RESULT_H
