#include "llviewerprecompiledheaders.h"
#include "rdfloaterdemo.h"
#include "llviewercontrol.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdlabel.h"
#include "rdswitch.h"
#include "rduiruntime.h"
#include "rduisystem.h"
#include <algorithm>
#include <iterator>
#include <utility>

namespace rdui::viewer
{
    void registerFloaterDemo(Runtime& runtime)
    {
        runtime.registerFloater("floater-demo", [&runtime](System& system)
        {
            return std::make_unique<FloaterDemo>(
                system,
                [&runtime] { runtime.requestReload(); },
                [] { return gSavedSettings.getBOOL("RduiAuthoringMode"); },
                [](bool enabled) { gSavedSettings.setBOOL("RduiAuthoringMode", enabled); });
        });
    }

    FloaterDemo::FloaterDemo(System& system,
                             std::function<void()> reload_handler,
                             std::function<bool()> authoring_mode_getter,
                             std::function<void(bool)> authoring_mode_setter)
               : mSystem(system),
                 mReloadHandler(std::move(reload_handler)),
                 mAuthoringModeGetter(std::move(authoring_mode_getter)),
                 mAuthoringModeSetter(std::move(authoring_mode_setter)) {}

    PreparedBindingResult FloaterDemo::prepareBindings(Floater& floater)
    {
        Binder binder(floater);
        binder.require("status", mStatus);
        binder.require("active-language", mActiveLanguage);
        binder.require("previous-language", mPreviousLanguage);
        binder.require("next-language", mNextLanguage);
        binder.require("authoring-mode", mAuthoringMode);
        binder.onClick("press", [this]
        {
            mStatus->setText(mSystem.localized("floater_demo.clicked"));
        });
        binder.onChange("switch-changed", [this](const ChangeActionEvent& event)
        {
            mStatus->setText(mSystem.localized(event.checked ? "floater_demo.switch_on" : "floater_demo.switch_off"));
        });
        binder.onClick("previous-language", [this] { selectRelativeLanguage(-1); });
        binder.onClick("next-language", [this] { selectRelativeLanguage(1); });
        binder.onClick("reload-resources", [this]
        {
            if (mReloadHandler) mReloadHandler();
        });
        binder.onChange("authoring-mode-changed", [this](const ChangeActionEvent& event)
        {
            if (mAuthoringModeSetter) mAuthoringModeSetter(event.checked);
        });

        return binder.prepare();
    }

    void FloaterDemo::commitBindings(PreparedBinding&& binding)
    {
        mBinding = binding.commit();
        refreshLanguageControls();
        refreshAuthoringModeControl();
    }

    void FloaterDemo::selectRelativeLanguage(int direction)
    {
        const std::vector<LanguageInfo>& languages = mSystem.languages();
        if (languages.size() <= 1 || direction == 0) return;
        const auto current = std::find_if(languages.begin(), languages.end(), [this](const LanguageInfo& language)
        {
            return language.id == mSystem.activeLocale();
        });
        const std::size_t index = current == languages.end() ? 0
                                                             : static_cast<std::size_t>(std::distance(languages.begin(), current));
        const std::size_t next = direction < 0 ? (index + languages.size() - 1) % languages.size()
                                               : (index + 1) % languages.size();
        if (!mSystem.setLocale(languages[next].id)) return;
        refreshLanguageControls();
    }

    void FloaterDemo::refreshLanguageControls()
    {
        if (mActiveLanguage)
        {
            const LanguageInfo* active = mSystem.activeLanguage();
            mActiveLanguage->setText(active ? active->name : std::string());
        }
        const bool disabled = mSystem.languages().size() <= 1;
        if (mPreviousLanguage) mPreviousLanguage->setDisabled(disabled);
        if (mNextLanguage) mNextLanguage->setDisabled(disabled);
    }

    void FloaterDemo::refreshAuthoringModeControl()
    {
        if (mAuthoringMode && mAuthoringModeGetter)
            mAuthoringMode->setChecked(mAuthoringModeGetter());
    }

    void FloaterDemo::reportReloadSucceeded()
    {
        if (mStatus) mStatus->setText(mSystem.localized("floater_demo.reload_succeeded"));
    }

    void FloaterDemo::reportReloadFailed(const DiagnosticResult& diagnostics)
    {
        if (!mStatus) return;
        std::string message = mSystem.resolveText("floater_demo.reload_failed");
        if (!diagnostics.errors.empty())
        {
            const Diagnostic& error = diagnostics.errors.front();
            message += ": " + error.code + ": " + error.formatted();
            if (diagnostics.errors.size() > 1)
                message += " (+" + std::to_string(diagnostics.errors.size() - 1) + ")";
        }
        mStatus->setText(std::move(message));
    }
}
