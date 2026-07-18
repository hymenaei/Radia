#include "linden_common.h"
#include "rdicon.h"
#include "rduipaintcontext.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Icon::Icon(std::string name) : Widget(ELEMENT), mName(std::move(name)) {}

    Icon& Icon::setName(std::string name)
    {
        mName = std::move(name);
        invalidatePaint();
        return *this;
    }

    void Icon::paint(PaintContext& context, const Style& style, float scale) const
    {
        context.paintBox(rect(), style);
        context.paintIcon(mName, rect(), style, scale);
    }

    WidgetContract detail::WidgetContractRegistry::icon()
    {
        return defineWidget<Icon>(Icon::ELEMENT)
            .attributes({stringAttribute({"icon", "name"}, &Icon::setName)})
            .build();
    }
}
