#include "linden_common.h"
#include "rdpanel.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Panel::Panel() : Widget(ELEMENT) {}

    WidgetContract detail::WidgetContractRegistry::panel()
    {
        return defineWidget<Panel>(Panel::ELEMENT)
            .attributes({allowedAttribute("filename")})
            .build();
    }
}
