#include "linden_common.h"
#include "rdfield.h"
#include "rduistyle.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Field::Field() : Widget(ELEMENT) {}

    void Field::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Row;
    }

    WidgetContract detail::WidgetContractRegistry::field()
    {
        return defineWidget<Field>(Field::ELEMENT).build();
    }

    Content::Content() : Widget(ELEMENT) {}

    void Content::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Column;
    }

    WidgetContract detail::WidgetContractRegistry::content()
    {
        return defineWidget<Content>(Content::ELEMENT).build();
    }

    Description::Description() : Widget(ELEMENT) {}

    void Description::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Row;
    }

    WidgetContract detail::WidgetContractRegistry::description()
    {
        return defineWidget<Description>(Description::ELEMENT)
            .textChildren()
            .build();
    }

}
