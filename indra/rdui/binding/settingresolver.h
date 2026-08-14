/**
 * @file settingresolver.h
 * @brief Defines the internal seam for resolving named setting bindings.
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

#ifndef RD_BINDING_SETTINGRESOLVER_H
#define RD_BINDING_SETTINGRESOLVER_H

#include <memory>
#include <string_view>
#include <typeindex>
#include "binding/valuebinding.h"

namespace rdui {
struct SettingResolution {
    enum class ResolutionStatus { Found, Missing, TypeMismatch, Invalid };

    ResolutionStatus status = ResolutionStatus::Missing;
    std::shared_ptr<ValueBindingBase> binding;
};

class SettingResolver {
public:
    virtual ~SettingResolver() = default;

    virtual SettingResolution resolve(std::string_view settingName, std::type_index requestedType) = 0;
};
} // namespace rdui
#endif // RD_BINDING_SETTINGRESOLVER_H
