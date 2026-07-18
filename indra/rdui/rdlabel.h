#ifndef LL_RDUI_LABEL_H
#define LL_RDUI_LABEL_H

#include "rduilocalization.h"
#include "rduiwidget.h"

namespace rdui
{
    struct WidgetContract;
    namespace detail { WidgetContract labelContract(); }

    class Label : public Widget
    {
        friend WidgetContract detail::labelContract();
        public:
            static constexpr const char* ELEMENT = "label";

            explicit Label(std::string text = {});

            Label& setText(std::string text);
            Label& setText(TextValue text);
            const std::string& text() const { return mText.value(); }

            Vec2 intrinsicSize(const StyleSheet& theme, const Style& style,
                               const TextMetrics& text_metrics) const override;
            void paint(PaintContext& context, const Style& style, float scale) const override;

        protected:
            Label(const char* element, std::string text);

        private:
            void onLocaleChanged(const System& system) override;

            TextValue mText;
    };
}

#endif // LL_RDUI_LABEL_H
