#ifndef LL_RDUI_PAINT_CONTEXT_H
#define LL_RDUI_PAINT_CONTEXT_H

#include "rduistyle.h"
#include "rduitextmetrics.h"
#include <string>

namespace rdui
{
    class PaintContext : public TextMetrics
    {
        public:
            virtual ~PaintContext() = default;

            virtual void beginFrame() {}
            virtual void endFrame() {}
            virtual void pushClip(const Rect& rect, float scale) = 0;
            virtual void popClip() = 0;
            virtual void beginEffects(const Rect& rect, const Style& style, float scale) = 0;
            virtual void endEffects() = 0;
            virtual void paintBox(const Rect& rect, const Style& style) = 0;
            virtual void paintText(const std::string& text, const Rect& rect, const Style& style) = 0;
            virtual void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) = 0;
    };
}

#endif // LL_RDUI_PAINT_CONTEXT_H
