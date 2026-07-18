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

    WidgetContract detail::fieldContract()
    {
        return defineWidget<Field>(Field::ELEMENT).build();
    }

    Content::Content() : Widget(ELEMENT) {}

    void Content::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Column;
    }

    WidgetContract detail::contentContract()
    {
        return defineWidget<Content>(Content::ELEMENT).build();
    }

    Description::Description() : Widget(ELEMENT) {}

    void Description::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Row;
    }

    WidgetContract detail::descriptionContract()
    {
        return defineWidget<Description>(Description::ELEMENT)
            .textChildren()
            .build();
    }

}
