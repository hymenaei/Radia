/**
 * @file floatercontroller.h
 * @brief Defines the viewer controller contract for binding and updating UI Floaters.
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

#ifndef RD_FLOATERCONTROLLER_H
#define RD_FLOATERCONTROLLER_H

#include <string>
#include "binding/binder.h"
#include "diagnostic.h"

namespace rdui {
class Floater;

namespace viewer {
class FloaterController {
public:
    virtual ~FloaterController() = default;

    virtual std::string resourceId() const = 0;
    virtual PreparedBindingResult prepareBindings(Floater& floater) = 0;
    virtual void commitBindings(PreparedBinding&& binding) = 0;
    virtual void idle() {}
    virtual void reportReloadSucceeded() {}
    virtual void reportReloadFailed(const DiagnosticResult&) {}
};
} // namespace viewer
} // namespace rdui
#endif // RD_FLOATERCONTROLLER_H
