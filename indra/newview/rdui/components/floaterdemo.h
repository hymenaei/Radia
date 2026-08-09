/**
 * @file floaterdemo.h
 * @brief Provides the viewer demo Floater controller for UI bindings, localization, and reloads.
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

#ifndef RD_COMPONENTS_FLOATERDEMO_H
#define RD_COMPONENTS_FLOATERDEMO_H

#include <functional>
#include "binding/binder.h"
#include "floatercontroller.h"

namespace rdui {
class Button;
class Floater;
class Switch;
class System;
class Text;

namespace viewer {
class Runtime;

class FloaterDemo final : public FloaterController {
public:
    static constexpr const char* RESOURCE_ID = "floater_demo.xml";
    explicit FloaterDemo(System& system, std::function<void()> reload_handler = {}, std::function<bool()> authoring_mode_getter = {},
                         std::function<void(bool)> authoring_mode_setter = {});

    std::string resourceId() const override { return RESOURCE_ID; }
    PreparedBindingResult prepareBindings(Floater& floater) override;
    void commitBindings(PreparedBinding&& binding) override;
    void idle() override { refreshAuthoringModeControl(); }
    void refreshLanguageControls();
    void refreshAuthoringModeControl();
    void reportReloadSucceeded() override;
    void reportReloadFailed(const DiagnosticResult& diagnostics) override;

private:
    void selectRelativeLanguage(int direction);

    System& mSystem;
    WidgetRef<Text> mStatus;
    WidgetRef<Text> mActiveLanguage;
    WidgetRef<Button> mPreviousLanguage;
    WidgetRef<Button> mNextLanguage;
    WidgetRef<Switch> mAuthoringMode;
    std::shared_ptr<ValueBinding<bool>> mDemoSwitchBinding;
    Binding mBinding;
    std::function<void()> mReloadHandler;
    std::function<bool()> mAuthoringModeGetter;
    std::function<void(bool)> mAuthoringModeSetter;
};

void registerFloaterDemo(Runtime& runtime);
} // namespace viewer
} // namespace rdui
#endif // RD_COMPONENTS_FLOATERDEMO_H
