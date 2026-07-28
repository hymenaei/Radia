#ifndef LL_RDUI_BUTTON_H
#define LL_RDUI_BUTTON_H

#include "rduilocalization.h"
#include "rduiwidget.h"

namespace rdui
{
    class Icon;
    class Label;
    struct WidgetContract;
    namespace detail { WidgetContract buttonContract(); }

    class Button : public Widget
    {
        friend WidgetContract detail::buttonContract();
        public:
            static constexpr const char* ELEMENT = "button";

            Button();

            Icon& setIcon(std::string name);
            Label& setLabel(std::string text);
            Icon* icon() { return mIcon.get(); }
            const Icon* icon() const { return mIcon.get(); }
            Label* label() { return mLabel.get(); }
            const Label* label() const { return mLabel.get(); }

            bool defaultPointerEvents() const override { return true; }
            bool focusable() const override { return true; }

        protected:
            void constrainResolvedStyle(Style& style) const override;
            void onChildAdded(Widget& child) override;
            void onChildrenCleared() override;

        private:
            WidgetRef<Icon> mIcon;
            WidgetRef<Label> mLabel;
    };
}

#endif // LL_RDUI_BUTTON_H
