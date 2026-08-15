/**
 * @file floaterdemo.cpp
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

#include "llviewerprecompiledheaders.h"
#include "floaterdemo.h"
#include <algorithm>
#include <iterator>
#include <tuple>
#include <utility>
#include "componentcontrollerregistration.h"
#include "runtime.h"
#include "system.h"

namespace radia::viewer::ui {
using namespace ::radia::ui;
void registerFloaterDemo(Runtime& runtime) {
    runtime.registerFloater("floaterDemo", "floater_demo.xml", [&runtime](System& system) {
        return std::make_unique<FloaterDemo>(system, [&runtime] { runtime.requestSkinReload(); });
    });
}

FloaterDemo::FloaterDemo(System& system, std::function<void()> reloadHandler)
    : ComponentController(system), mSystem(system), mReloadHandler(std::move(reloadHandler)) {
    event("press", &FloaterDemo::press);
    event("switchChanged", &FloaterDemo::switchChanged);
    event("selectLocale", &FloaterDemo::selectLocale);
    event("requestSkinReload", [this] {
        if (mReloadHandler) mReloadHandler();
    });
}

void FloaterDemo::postBuild() {
    refreshLocaleControls();
}

void FloaterDemo::press() {
    mStatus.setContent(localize("demo.clicked"));
}

void FloaterDemo::switchChanged(const ChangeEvent& event) {
    mStatus.setContent(localize(event.checked ? "demo.switchOn" : "demo.switchOff"));
}

void FloaterDemo::selectLocale(int step) {
    std::vector<LocaleInfo> locales = mSystem.locales();
    std::sort(locales.begin(), locales.end(),
              [](const LocaleInfo& left, const LocaleInfo& right) { return std::tie(left.name, left.id) < std::tie(right.name, right.id); });
    if (locales.size() <= 1 || step == 0) return;
    const auto current =
        std::find_if(locales.begin(), locales.end(), [this](const LocaleInfo& locale) { return locale.id == mSystem.activeLocale(); });
    const std::size_t index = current == locales.end() ? 0 : static_cast<std::size_t>(std::distance(locales.begin(), current));
    const std::size_t next = step < 0 ? (index + locales.size() - 1) % locales.size() : (index + 1) % locales.size();
    if (!mSystem.setLocale(locales[next].id)) return;
    refreshLocaleControls();
}

void FloaterDemo::refreshLocaleControls() {
    const LocaleInfo* active = mSystem.activeLocaleInfo();
    mActiveLocale.setContent(TextSource::text(active ? active->name : std::string()));
    const bool disabled = mSystem.locales().size() <= 1;
    mPreviousLocale.setDisabled(disabled);
    mNextLocale.setDisabled(disabled);
}

void FloaterDemo::onReloadSucceeded() {
    mStatus.setContent(localize("demo.reloadSucceeded"));
}

void FloaterDemo::onReloadFailed(const DiagnosticResult& diagnostics) {
    std::string message = mSystem.resolveText("demo.reloadFailed");
    if (!diagnostics.errors.empty()) {
        const Diagnostic& error = diagnostics.errors.front();
        message += ": " + error.code + ": " + error.formatted();
        if (diagnostics.errors.size() > 1) message += " (+" + std::to_string(diagnostics.errors.size() - 1) + ")";
    }
    mStatus.setContent(TextSource::text(std::move(message)));
}
} // namespace radia::viewer::ui
