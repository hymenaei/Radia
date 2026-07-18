#include "linden_common.h"
#include "rdpanel.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Panel::Panel() : Widget(ELEMENT) {}

    WidgetContract detail::panelContract()
    {
        return defineWidget<Panel>(Panel::ELEMENT)
            .attributes({allowedAttribute("filename")})
            .build();
    }
}
