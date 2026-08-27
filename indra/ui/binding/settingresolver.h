/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string_view>
#include <typeindex>
#include "binding/valuebinding.h"

namespace radia::ui {
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
} // namespace radia::ui
