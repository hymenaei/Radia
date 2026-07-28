#ifndef LL_RDUI_TEXT_HOST_H
#define LL_RDUI_TEXT_HOST_H

#include "rduitextsource.h"
#include "rduitypes.h"

namespace rdui
{
    class StyleSheet;
    class Widget;

    class PaintContext;
    struct Style;
    class TextMetrics;

    class TextHost
    {
        public:
            TextHost() = default;
            explicit TextHost(TextSource content) { setContent(std::move(content)); }
            explicit TextHost(InlineContent content)
            {
                setContent(TextSource::literal(std::move(content)));
            }

            void setContent(TextSource content);
            void setContent(InlineContent content)
            {
                setContent(TextSource::literal(std::move(content)));
            }
            const InlineContent& content() const { return mContent; }
            const std::string& plainText() const { return mPlainText; }
            void resolveLocalized(const std::function<InlineContent(const LocalizationRequest&)>& resolve);
            bool resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve);

            Vec2 measure(const TextMetrics& metrics, const Style& style, const StyleSheet& theme, const Widget& owner) const;
            void paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const;

        private:
            void updatePlainText();

            TextSource mSource;
            InlineContent mContent;
            std::string mPlainText;
            bool mHasKeybindings = false;
    };
}

#endif // LL_RDUI_TEXT_HOST_H
