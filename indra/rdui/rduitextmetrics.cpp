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
        const bool bold = style.font_bold || style.font_family == FontFamily::Bold || style.font_family == FontFamily::SmallBold;
        const std::size_t characters = utf8str_to_wstring(text).size();
        const float factor = bold ? mBoldWidthFactor : mRegularWidthFactor;
        return {std::ceil(static_cast<float>(characters) * style.font_size * factor), line_height};
    }

    const TextMetrics& fixedTextMetrics()
    {
        static const FixedTextMetrics metrics;
        return metrics;
    }
}
