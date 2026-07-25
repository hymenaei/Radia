#ifndef LL_RDUI_FIELD_H
#define LL_RDUI_FIELD_H

#include "rdtext.h"
#include "rduivaluecontrol.h"
#include "rduiwidget.h"

namespace rdui
{
    class Label;
    struct WidgetContract;
    namespace detail
    {
        WidgetContract fieldContract();
        WidgetContract hintContract();
        WidgetContract errorContract();
    }

    class Field : public Widget
    {
        friend WidgetContract detail::fieldContract();
        public:
            static constexpr const char* ELEMENT = "field";

            Field();

            Label* label() { return mLabel.get(); }
            const Label* label() const { return mLabel.get(); }
            Widget* control();
            const Widget* control() const;
            Text* hint() { return mHint.get(); }
            const Text* hint() const { return mHint.get(); }
            Text* error() { return mError.get(); }
            const Text* error() const { return mError.get(); }
            bool dirty() const { return mDirty; }
            bool invalid() const { return hasState(WidgetState::Invalid); }

        protected:
            void constrainResolvedStyle(Style& style) const override;
            void onChildAdded(Widget& child) override;
            void onChildrenCleared() override;

        private:
            Widget* setHintContent(InlineContent content);
            Widget* setErrorContent(InlineContent content);
            Widget* createSupportIndent(WidgetRef<Widget>& slot, const char* part, bool collapsed);
            bool controlPrecedesLabel() const;
            void refreshValueState(const ValueControlState& state);

            WidgetRef<Label> mLabel;
            ValueControl* mControl = nullptr;
            WidgetRef<Text> mHint;
            WidgetRef<Text> mError;
            WidgetRef<Widget> mHintIndent;
            WidgetRef<Widget> mErrorIndent;
            InlineContent mAuthoredError;
            ValueBindingSubscription mControlSubscription;
            bool mDirty = false;
    };

}

#endif // LL_RDUI_FIELD_H
