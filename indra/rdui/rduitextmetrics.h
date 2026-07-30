#ifndef LL_RDUI_TEXT_METRICS_H
#define LL_RDUI_TEXT_METRICS_H

#include "rduitypes.h"
#include <string>

namespace rdui
{
    struct Style;

    class TextMetrics
    {
        public:
            virtual ~TextMetrics() = default;
            virtual Vec2 measureText(const std::string& text, const Style& style) const = 0;
            virtual float usedLetterSpacing(const Style& style) const;
    };

    class FixedTextMetrics final : public TextMetrics
    {
        public:
            explicit FixedTextMetrics(float regular_width_factor = .58f, float bold_width_factor = .62f)
                   : mRegularWidthFactor(regular_width_factor), mBoldWidthFactor(bold_width_factor) {}

            Vec2 measureText(const std::string& text, const Style& style) const override;
            float usedLetterSpacing(const Style& style) const override;

        private:
            float mRegularWidthFactor;
            float mBoldWidthFactor;
    };

    const TextMetrics& fixedTextMetrics();
}

#endif // LL_RDUI_TEXT_METRICS_H
