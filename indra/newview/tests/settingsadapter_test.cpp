/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <memory>
#include "binding/valuebinding.h"
#include "llcontrol.h"
#include "settingsadapter.h"

namespace {
using radia::ui::SettingResolution;
using radia::ui::ValueBinding;
using radia::viewer::ui::SettingsAdapter;
using ResolutionStatus = SettingResolution::ResolutionStatus;
}

TEST(SettingsAdapterTest, SynchronizesBooleanBindingWithControlGroup) {
    LLControlGroup settings("ui-settings-adapter");
    settings.declareBOOL("adapter-enabled", false, "test setting");
    SettingsAdapter adapter(settings);

    const SettingResolution resolved = adapter.resolve("adapter-enabled", typeid(bool));
    ASSERT_EQ(resolved.status, ResolutionStatus::Found);
    const auto binding = std::dynamic_pointer_cast<ValueBinding<bool>>(resolved.binding);
    ASSERT_NE(binding, nullptr);
    EXPECT_FALSE(binding->state().value);

    settings.setBOOL("adapter-enabled", true);
    EXPECT_TRUE(binding->state().value);
    binding->write(false);
    EXPECT_FALSE(settings.getBOOL("adapter-enabled"));
}

TEST(SettingsAdapterTest, ReportsMissingControl) {
    LLControlGroup settings("ui-settings-adapter-errors");
    SettingsAdapter adapter(settings);

    const SettingResolution missing = adapter.resolve("missing", typeid(bool));
    EXPECT_EQ(missing.status, ResolutionStatus::Missing);
}

TEST(SettingsAdapterTest, RejectsIncompatibleControlType) {
    LLControlGroup settings("ui-settings-adapter-type-errors");
    settings.declareF32("adapter-number", 1.5f, "test setting");
    SettingsAdapter adapter(settings);

    const SettingResolution wrongType = adapter.resolve("adapter-number", typeid(bool));
    EXPECT_EQ(wrongType.status, ResolutionStatus::TypeMismatch);
}

TEST(SettingsAdapterTest, ConvertsF32ControlToDoubleBinding) {
    LLControlGroup settings("ui-settings-adapter-numeric");
    settings.declareF32("adapter-number", 1.5f, "test setting");
    SettingsAdapter adapter(settings);

    const SettingResolution compatible = adapter.resolve("adapter-number", typeid(double));
    ASSERT_EQ(compatible.status, ResolutionStatus::Found);
    const auto binding = std::dynamic_pointer_cast<ValueBinding<double>>(compatible.binding);
    ASSERT_NE(binding, nullptr);
    EXPECT_DOUBLE_EQ(binding->state().value, 1.5);
}
