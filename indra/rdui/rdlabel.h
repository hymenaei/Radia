#ifndef LL_RDUI_LABEL_H
#define LL_RDUI_LABEL_H

#include "rduitexthost.h"
#include "rduiwidget.h"

namespace rdui
{
    struct WidgetContract;
    namespace detail { WidgetContract labelContract(); }

    class Label : public Widget
    {
        friend WidgetContract detail::labelContract();
        friend class detail::WidgetCompilerAccess;
        public:
            static constexpr const char* ELEMENT = "label";

            explicit Label(std::string text = {});

            Label& setText(std::string text);
            Label& setText(TextValue text);
            Label& setContent(InlineContent content);
            const std::string& text() const { return mText.plainText(); }
            const InlineContent& content() const { return mText.content(); }

            Vec2 intrinsicSize(const StyleSheet& theme, const Style& style,
                               const TextMetrics& text_metrics) const override;
            void paint(PaintContext& context, const Style& style, float scale) const override;
            bool defaultPointerEvents() const override { return static_cast<bool>(mTarget); }

        protected:
            Label(const char* element, std::string text);

        private:
            Label& setTargetId(std::string id);
            void onActivate() override;
            void onLocaleChanged(const System& system) override;
            bool onKeybindingsChanged(const System& system) override;

            TextHost mText;
            std::string mTargetId;
            WidgetRef<Widget> mTarget;
    };
}

#endif // LL_RDUI_LABEL_H
