#ifndef LL_RDUI_SWITCH_H
#define LL_RDUI_SWITCH_H

#include "rduiwidget.h"

namespace rdui
{

    class Switch : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "switch";

            Switch();

            Switch& setChecked(bool checked);
            Switch& setOnCheckedChanged(std::function<void(bool)> callback);
            bool checked() const { return hasState(WidgetState::Checked); }
            Widget* thumb() { return mThumb.get(); }
            const Widget* thumb() const { return mThumb.get(); }

            bool defaultPointerEvents() const override { return true; }
            bool focusable() const override { return true; }

        protected:
            void constrainResolvedStyle(Style& style) const override;
            void onActivate() override;
            void onChildrenCleared() override;

        private:
            WidgetRef<Widget> mThumb;
            std::function<void(bool)> mOnCheckedChanged;
    };
}

#endif // LL_RDUI_SWITCH_H
