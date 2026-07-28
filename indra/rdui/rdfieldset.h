#ifndef LL_RDUI_FIELDSET_H
#define LL_RDUI_FIELDSET_H

#include "rdtext.h"
#include "rduiwidget.h"

namespace rdui
{
    struct WidgetContract;
    namespace detail
    {
        WidgetContract fieldsetContract();
        WidgetContract legendContract();
    }

    class Fieldset : public Widget
    {
        friend WidgetContract detail::fieldsetContract();

        public:
            static constexpr const char* ELEMENT = "fieldset";

            Fieldset();

            Text* legend() { return mLegend.get(); }
            const Text* legend() const { return mLegend.get(); }

            void paint(PaintContext& context, const Style& style, float scale) const override;

        protected:
            void constrainResolvedStyle(Style& style) const override;
            void onChildrenCleared() override;
            void onArranged(const Style& style) override;
            Rect paintBounds() const override;
            bool hasLayoutGapBetween(const Widget& previous, const Widget& next) const override;
            float layoutOverlapBetween(const Widget& previous, const Widget& next, const Style& style) const override;

        private:
            Text* setLegendContent(TextSource content);
            Rect borderRect() const;

            WidgetRef<Text> mLegend;
    };
}

#endif // LL_RDUI_FIELDSET_H
