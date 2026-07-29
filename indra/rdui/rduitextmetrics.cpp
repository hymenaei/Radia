#include "linden_common.h"
#include "rduitextmetrics.h"
#include "llstring.h"
#include "rduistyle.h"
#include <algorithm>
#include <cmath>

namespace rdui
{
    Vec2 FixedTextMetrics::measureText(const std::string& text, const Style& style) const
    {
        const float line_height = std::ceil(style.line_height ? style.line_height->pixels : style.font_size);
        if (text.empty()) return {0.f, line_height};
        const std::size_t characters = utf8str_to_wstring(text).size();
        const float weight_mix = std::clamp((static_cast<float>(style.font_weight) - 400.f) / 300.f, 0.f, 1.f);
        const float factor = mRegularWidthFactor + (mBoldWidthFactor - mRegularWidthFactor) * weight_mix;
        return {std::ceil(static_cast<float>(characters) * style.font_size * factor), line_height};
    }

    const TextMetrics& fixedTextMetrics()
    {
        static const FixedTextMetrics metrics;
        return metrics;
    }
}
