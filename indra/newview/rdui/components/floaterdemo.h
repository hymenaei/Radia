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
#include "componentcontroller.h"

namespace rdui {
class System;

namespace viewer {
class ChangeEvent;
class Runtime;

class FloaterDemo final : public ComponentController {
public:
    explicit FloaterDemo(System& system, std::function<void()> reloadHandler = {});

    void postBuild() override;
    void refreshLocaleControls();
    void onReloadSucceeded() override;
    void onReloadFailed(const DiagnosticResult& diagnostics) override;

private:
    void press();
    void switchChanged(const ChangeEvent& event);
    void selectLocale(int step);

    System& mSystem;
    Widget& mStatus = getWidgetById("status");
    Widget& mActiveLocale = getWidgetById("activeLocale");
    Widget& mPreviousLocale = getWidgetById("previousLocale");
    Widget& mNextLocale = getWidgetById("nextLocale");
    std::function<void()> mReloadHandler;
};

void registerFloaterDemo(Runtime& runtime);
} // namespace viewer
} // namespace rdui
#endif // RD_COMPONENTS_FLOATERDEMO_H
