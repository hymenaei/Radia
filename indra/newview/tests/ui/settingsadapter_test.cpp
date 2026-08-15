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
#include <memory>
#include "../test/lltut.h"
#include "binding/valuebinding.h"
#include "llcontrol.h"
#include "settingsadapter.h"

namespace tut {
struct settingsAdapterData {};
using settingsAdapterTest = test_group<settingsAdapterData>;
using settingsAdapterObject = settingsAdapterTest::object;
settingsAdapterTest settingsAdapterTestCase("UISettingsAdapter");

template<> template<> void settingsAdapterObject::test<1>() {
    set_test_name("SettingsAdapter resolves and writes through an injected control group");
    LLControlGroup settings("ui-settings-adapter");
    settings.declareBOOL("adapter-enabled", false, "test setting");
    radia::viewer::ui::SettingsAdapter adapter(settings);

    const radia::ui::SettingResolution resolved = adapter.resolve("adapter-enabled", typeid(bool));
    auto binding = std::dynamic_pointer_cast<radia::ui::ValueBinding<bool>>(resolved.binding);
    ensure("adapter returns a typed binding", resolved.status == radia::ui::SettingResolution::ResolutionStatus::Found && binding);
    ensure("adapter reads the control group", binding && !binding->state().value);

    settings.setBOOL("adapter-enabled", true);
    ensure("adapter observes external writes", binding && binding->state().value);
    binding->write(false);
    ensure("adapter writes the injected control group", !settings.getBOOL("adapter-enabled"));
}

template<> template<> void settingsAdapterObject::test<2>() {
    set_test_name("SettingsAdapter reports missing and incompatible controls");
    LLControlGroup settings("ui-settings-adapter-errors");
    settings.declareF32("adapter-number", 1.5f, "test setting");
    radia::viewer::ui::SettingsAdapter adapter(settings);

    const radia::ui::SettingResolution missing = adapter.resolve("missing", typeid(bool));
    ensure("missing setting is reported", missing.status == radia::ui::SettingResolution::ResolutionStatus::Missing);

    const radia::ui::SettingResolution wrongType = adapter.resolve("adapter-number", typeid(bool));
    ensure("incompatible setting type is reported", wrongType.status == radia::ui::SettingResolution::ResolutionStatus::TypeMismatch);

    const radia::ui::SettingResolution compatible = adapter.resolve("adapter-number", typeid(double));
    auto binding = std::dynamic_pointer_cast<radia::ui::ValueBinding<double>>(compatible.binding);
    ensure("F32 setting can provide a double binding", compatible.status == radia::ui::SettingResolution::ResolutionStatus::Found && binding);
    ensure_approximately_equals("numeric setting is converted", binding ? binding->state().value : 0.0, 1.5, 6);
}
} // namespace tut
