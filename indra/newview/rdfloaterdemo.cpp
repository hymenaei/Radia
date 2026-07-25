#include "llviewerprecompiledheaders.h"
#include "rdfloaterdemo.h"
#include "llviewercontrol.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdswitch.h"
#include "rdtext.h"
#include "rduiruntime.h"
#include "rduisystem.h"
#include <algorithm>
#include <iterator>
#include <map>
#include <utility>

namespace rdui::viewer
{
    namespace
    {
        class DemoSwitchBinding final : public ValueBinding<bool>
        {
            public:
                ValueState<bool> state() const override { return mState; }

                void write(bool value) override
                {
                    mState.value = value;
                    mState.validation = value ? ValueValidation::valid() : ValueValidation::invalid();
                    const auto observers = mObservers;
                    for (const auto& [id, observer] : observers)
                        if (mObservers.find(id) != mObservers.end()) observer(mState);
                }

                ValueBindingSubscription observe(Observer observer) override
                {
                    const std::size_t id = mNextObserver++;
                    mObservers.emplace(id, std::move(observer));
                    std::weak_ptr<char> lifetime = mLifetime;
                    return ValueBindingSubscription([this, lifetime, id]
                    {
                        if (!lifetime.expired()) mObservers.erase(id);
                    });
                }

            private:
                ValueState<bool> mState{true, true, ValueValidation::valid()};
                std::map<std::size_t, Observer> mObservers;
                std::size_t mNextObserver = 1;
                std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
        };
    }

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
                 mDemoSwitchBinding(std::make_shared<DemoSwitchBinding>()),
                 mReloadHandler(std::move(reload_handler)),
                 mAuthoringModeGetter(std::move(authoring_mode_getter)),
                 mAuthoringModeSetter(std::move(authoring_mode_setter)) {}

    PreparedBindingResult FloaterDemo::prepareBindings(Floater& floater)
    {
        Binder binder(floater);
        binder.bind("status", mStatus);
        binder.bind("active-language", mActiveLanguage);
        binder.bind("previous-language", mPreviousLanguage);
        binder.bind("next-language", mNextLanguage);
        binder.bind("authoring-mode", mAuthoringMode);
        binder.provideValue("demo-switch-enabled", mDemoSwitchBinding);
        binder.onClick("press", [this]
        {
            if (mStatus) mStatus->setText(mSystem.localized("floater_demo.clicked"));
        });
        binder.onChange("switch-changed", [this](const ChangeActionEvent& event)
        {
            if (mStatus)
                mStatus->setText(mSystem.localized(
                    event.checked ? "floater_demo.switch_on" : "floater_demo.switch_off"));
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
