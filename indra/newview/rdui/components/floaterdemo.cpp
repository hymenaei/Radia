/**
 * @file floaterdemo.cpp
 * @brief
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
#include <map>
#include <tuple>
#include <utility>
#include "llviewercontrol.h"
#include "runtime.h"
#include "system.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/switch.h"
#include "widgets/text.h"

namespace rdui::viewer {
namespace {
class DemoSwitchBinding final : public ValueBinding<bool> {
public:
    ValueState<bool> state() const override { return mState; }

    void write(bool value) override {
        mState.value = value;
        mState.validation = value ? ValueValidation::valid() : ValueValidation::invalid();
        const auto observers = mObservers;
        for (const auto& [id, observer] : observers)
            if (mObservers.find(id) != mObservers.end()) observer(mState);
    }

    ValueBindingSubscription observe(Observer observer) override {
        const std::size_t id = mNextObserver++;
        mObservers.emplace(id, std::move(observer));
        std::weak_ptr<char> lifetime = mLifetime;
        return ValueBindingSubscription([this, lifetime, id] {
            if (!lifetime.expired()) mObservers.erase(id);
        });
    }

private:
    ValueState<bool> mState{true, true, ValueValidation::valid()};
    std::map<std::size_t, Observer> mObservers;
    std::size_t mNextObserver = 1;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
};
} // namespace

void registerFloaterDemo(Runtime& runtime) {
    runtime.registerFloater("floater-demo", [&runtime](System& system) {
        return std::make_unique<FloaterDemo>(
            system, [&runtime] { runtime.requestReload(); }, [] { return gSavedSettings.getBOOL("RduiAuthoringMode"); },
            [](bool enabled) { gSavedSettings.setBOOL("RduiAuthoringMode", enabled); });
    });
}

FloaterDemo::FloaterDemo(System& system, std::function<void()> reload_handler, std::function<bool()> authoring_mode_getter,
                         std::function<void(bool)> authoring_mode_setter)
    : mSystem(system), mDemoSwitchBinding(std::make_shared<DemoSwitchBinding>()), mReloadHandler(std::move(reload_handler)),
      mAuthoringModeGetter(std::move(authoring_mode_getter)), mAuthoringModeSetter(std::move(authoring_mode_setter)) {}

PreparedBindingResult FloaterDemo::prepareBindings(Floater& floater) {
    Binder binder(floater);
    binder.bind("status", mStatus);
    binder.bind("active-language", mActiveLanguage);
    binder.bind("previous-language", mPreviousLanguage);
    binder.bind("next-language", mNextLanguage);
    binder.bind("authoring-mode", mAuthoringMode);
    binder.provideValue("demo-switch-enabled", mDemoSwitchBinding);
    binder.onClick("press", [this] {
        if (mStatus) mStatus->setContent(mSystem.localized("demo.clicked"));
    });
    binder.onChange("switch-changed", [this](const ChangeActionEvent& event) {
        if (mStatus) mStatus->setContent(mSystem.localized(event.checked ? "demo.switchOn" : "demo.switchOff"));
    });
    binder.onClick("previous-language", [this] { selectRelativeLanguage(-1); });
    binder.onClick("next-language", [this] { selectRelativeLanguage(1); });
    binder.onClick("reload-resources", [this] {
        if (mReloadHandler) mReloadHandler();
    });
    binder.onChange("authoring-mode-changed", [this](const ChangeActionEvent& event) {
        if (mAuthoringModeSetter) mAuthoringModeSetter(event.checked);
    });

    return binder.prepare();
}

void FloaterDemo::commitBindings(PreparedBinding&& binding) {
    mBinding = binding.commit();
    refreshLanguageControls();
    refreshAuthoringModeControl();
}

void FloaterDemo::selectRelativeLanguage(int direction) {
    std::vector<LocaleInfo> locales = mSystem.locales();
    std::sort(locales.begin(), locales.end(),
              [](const LocaleInfo& left, const LocaleInfo& right) { return std::tie(left.name, left.id) < std::tie(right.name, right.id); });
    if (locales.size() <= 1 || direction == 0) return;
    const auto current =
        std::find_if(locales.begin(), locales.end(), [this](const LocaleInfo& locale) { return locale.id == mSystem.activeLocale(); });
    const std::size_t index = current == locales.end() ? 0 : static_cast<std::size_t>(std::distance(locales.begin(), current));
    const std::size_t next = direction < 0 ? (index + locales.size() - 1) % locales.size() : (index + 1) % locales.size();
    if (!mSystem.setLocale(locales[next].id)) return;
    refreshLanguageControls();
}

void FloaterDemo::refreshLanguageControls() {
    if (mActiveLanguage) {
        const LocaleInfo* active = mSystem.activeLocaleInfo();
        mActiveLanguage->setText(active ? active->name : std::string());
    }
    const bool disabled = mSystem.locales().size() <= 1;
    if (mPreviousLanguage) mPreviousLanguage->setDisabled(disabled);
    if (mNextLanguage) mNextLanguage->setDisabled(disabled);
}

void FloaterDemo::refreshAuthoringModeControl() {
    if (mAuthoringMode && mAuthoringModeGetter) mAuthoringMode->setChecked(mAuthoringModeGetter());
}

void FloaterDemo::reportReloadSucceeded() {
    if (mStatus) mStatus->setContent(mSystem.localized("demo.reloadSucceeded"));
}

void FloaterDemo::reportReloadFailed(const DiagnosticResult& diagnostics) {
    if (!mStatus) return;
    std::string message = mSystem.resolveText("demo.reloadFailed");
    if (!diagnostics.errors.empty()) {
        const Diagnostic& error = diagnostics.errors.front();
        message += ": " + error.code + ": " + error.formatted();
        if (diagnostics.errors.size() > 1) message += " (+" + std::to_string(diagnostics.errors.size() - 1) + ")";
    }
    mStatus->setText(std::move(message));
}
} // namespace rdui::viewer
