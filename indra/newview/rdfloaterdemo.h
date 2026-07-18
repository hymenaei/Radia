#ifndef LL_RDUI_FLOATER_DEMO_H
#define LL_RDUI_FLOATER_DEMO_H

#include "rduibinder.h"
#include "rduifloatercontroller.h"
#include <functional>

namespace rdui
{
    class Button;
    class Floater;
    class Label;
    class Switch;
    class System;

    namespace viewer
    {
        class Runtime;

        class FloaterDemo final : public FloaterController
        {
            public:
                static constexpr const char* RESOURCE_ID = "floater_demo.xml";
                explicit FloaterDemo(System& system,
                                     std::function<void()> reload_handler = {},
                                     std::function<bool()> authoring_mode_getter = {},
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
                WidgetRef<Label> mStatus;
                WidgetRef<Label> mActiveLanguage;
                WidgetRef<Button> mPreviousLanguage;
                WidgetRef<Button> mNextLanguage;
                WidgetRef<Switch> mAuthoringMode;
                Binding mBinding;
                std::function<void()> mReloadHandler;
                std::function<bool()> mAuthoringModeGetter;
                std::function<void(bool)> mAuthoringModeSetter;
        };

        void registerFloaterDemo(Runtime& runtime);
    }
}

#endif // LL_RDUI_FLOATER_DEMO_H
