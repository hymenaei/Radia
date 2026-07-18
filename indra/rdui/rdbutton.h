#ifndef LL_RDUI_BUTTON_H
#define LL_RDUI_BUTTON_H

#include "rduilocalization.h"
#include "rduiwidget.h"

namespace rdui
{
    class Icon;
    class Label;

    class Button : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "button";

            Button();

            Icon& setIcon(std::string name);
            Label& setLabel(std::string text);
            Label& setLabel(TextValue text);
            Icon* icon() { return mIcon.get(); }
            const Icon* icon() const { return mIcon.get(); }
            Label* label() { return mLabel.get(); }
            const Label* label() const { return mLabel.get(); }

            bool defaultPointerEvents() const override { return true; }
            bool focusable() const override { return true; }

        protected:
            void onChildAdded(Widget& child) override;
            void onChildrenCleared() override;

        private:
            WidgetRef<Icon> mIcon;
            WidgetRef<Label> mLabel;
    };
}

#endif // LL_RDUI_BUTTON_H
