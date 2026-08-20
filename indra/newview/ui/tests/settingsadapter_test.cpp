/**
 * @file settingsadapter_test.cpp
 * @brief Tests the viewer settings binding adapter.
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
