#ifndef LL_RDUI_TEXT_H
#define LL_RDUI_TEXT_H

#include "rduitexthost.h"
#include "rduiwidget.h"

namespace rdui
{
    struct WidgetContract;
    namespace detail { WidgetContract textContract(); }

    class Text : public Widget
    {
        friend WidgetContract detail::textContract();
        public:
            static constexpr const char* ELEMENT = "text";

            explicit Text(std::string text = {});

            Text& setText(std::string text);
            Text& setText(TextValue text);
            Text& setContent(InlineContent content);
            const std::string& text() const { return mText.plainText(); }
            const InlineContent& content() const { return mText.content(); }

            Vec2 intrinsicSize(const StyleSheet& theme, const Style& style,
                               const TextMetrics& text_metrics) const override;
            void paint(PaintContext& context, const Style& style, float scale) const override;

        protected:
            struct ElementTag {};
            Text(const char* element, ElementTag);

        private:
            void onLocaleChanged(const System& system) override;
            bool onKeybindingsChanged(const System& system) override;

            TextHost mText;
    };
}

#endif // LL_RDUI_TEXT_H
