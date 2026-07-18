#ifndef LL_RDUI_LAYOUT_H
#define LL_RDUI_LAYOUT_H

#include "rduitypes.h"

namespace rdui
{
    class Widget;
    struct Style;
    class StyleSheet;
    class TextMetrics;

    Style resolveWidgetStyle(const StyleSheet& theme, const Widget& node);
    Vec2 measureWidget(const Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics);
    void measureTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics);
    void arrangeTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction = LayoutDirection::LeftToRight);
    void layoutTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction = LayoutDirection::LeftToRight);
}

#endif // LL_RDUI_LAYOUT_H
