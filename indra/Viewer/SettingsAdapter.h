/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/SettingResolver.h>
#include <string_view>
#include <typeindex>

class LLControlGroup;

namespace Viewer {
class SettingsAdapter final : public Core::SettingResolver {
public:
    explicit SettingsAdapter(LLControlGroup& settings)
        : mSettings(settings) {}

    SettingsAdapter(const SettingsAdapter&) = delete;
    SettingsAdapter& operator=(const SettingsAdapter&) = delete;
    SettingsAdapter(SettingsAdapter&&) = delete;
    SettingsAdapter& operator=(SettingsAdapter&&) = delete;

    Core::SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override;

private:
    LLControlGroup& mSettings;
};
} // namespace Viewer
