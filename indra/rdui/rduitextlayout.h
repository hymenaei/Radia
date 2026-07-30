#ifndef LL_RDUI_TEXT_LAYOUT_H
#define LL_RDUI_TEXT_LAYOUT_H

#include "rduistyle.h"
#include "llstring.h"
#include <optional>
#include <string>
#include <vector>

namespace rdui
{
    class TextMetrics;
}

namespace rdui::detail
{
    std::vector<std::size_t> graphemeBoundaries(
        const LLWString& value);

    struct TextRun
    {
        struct Key
        {
            std::string value;
            Style style;
            Vec2 text_size;
            Vec2 box_size;
            Vec2 size;
        };

        std::string value;
        Style style;
        Vec2 size;
        Vec2 box_size;
        std::vector<Key> keys;

        bool keybinding() const { return !keys.empty(); }
    };

    using TextLine = std::vector<TextRun>;

    float interRunSpacing(const TextRun& left,
                          const TextRun& right,
                          const TextMetrics& metrics);

    struct LaidOutTextLine
    {
        TextLine runs;
        Vec2 size;
    };

    struct TextLayout
    {
        std::vector<LaidOutTextLine> lines;
        Vec2 size;
    };

    TextLayout layoutText(std::vector<TextLine> hard_lines,
                          const Style& style,
                          const TextMetrics& metrics,
                          std::optional<float> available_width,
                          bool visual_order,
                          bool apply_overflow);
}

#endif // LL_RDUI_TEXT_LAYOUT_H
