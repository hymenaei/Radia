#ifndef LL_RDUI_TEXT_HOST_H
#define LL_RDUI_TEXT_HOST_H

#include "rduiinlinecontent.h"
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
            explicit TextHost(InlineContent content) { setContent(std::move(content)); }

            void setContent(InlineContent content);
            const InlineContent& content() const { return mContent; }
            const std::string& plainText() const { return mPlainText; }
            void resolveLocalized(const std::function<std::string(const std::string&)>& resolve);
            bool resolveKeybindings(
                const std::function<KeybindingPresentation(const std::string&)>& resolve);

            Vec2 measure(const TextMetrics& metrics, const Style& style,
                         const StyleSheet& theme, const Widget& owner) const;
            void paint(PaintContext& context, const Rect& rect, const Style& style,
                       const StyleSheet* theme, const Widget& owner) const;

        private:
            void updatePlainText();

            InlineContent mContent;
            std::string mPlainText;
            bool mHasKeybindings = false;
    };
}

#endif // LL_RDUI_TEXT_HOST_H
