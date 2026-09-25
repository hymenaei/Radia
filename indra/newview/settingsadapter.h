/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "binding/settingresolver.h"

class LLControlGroup;

namespace radia::viewer::ui {
using radia::ui::SettingResolution;
using radia::ui::SettingResolver;

class SettingsAdapter final : public SettingResolver {
public:
    explicit SettingsAdapter(LLControlGroup& settings) : mSettings(settings) {}

    SettingsAdapter(const SettingsAdapter&) = delete;
    SettingsAdapter& operator=(const SettingsAdapter&) = delete;
    SettingsAdapter(SettingsAdapter&&) = delete;
    SettingsAdapter& operator=(SettingsAdapter&&) = delete;

    SettingResolution resolve(std::string_view settingName, std::type_index requestedType) override;

private:
    LLControlGroup& mSettings;
};
} // namespace radia::viewer::ui
