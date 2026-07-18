#ifndef LL_RDUI_PANEL_H
#define LL_RDUI_PANEL_H

#include "rduiwidget.h"

namespace rdui
{
    struct WidgetContract;
    namespace detail { WidgetContract panelContract(); }

    class Panel : public Widget
    {
        friend WidgetContract detail::panelContract();
        public:
            static constexpr const char* ELEMENT = "panel";

            Panel();
    };
}

#endif // LL_RDUI_PANEL_H
