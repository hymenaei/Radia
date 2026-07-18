#ifndef LL_RDUI_PANEL_H
#define LL_RDUI_PANEL_H

#include "rduiwidget.h"

namespace rdui
{

    class Panel : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "panel";

            Panel();
    };
}

#endif // LL_RDUI_PANEL_H
