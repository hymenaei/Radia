#include "linden_common.h"
#include "rdlabel.h"
#include "rduilocalization.h"
#include "rduipaintcontext.h"
#include "rduistyle.h"
#include "rduisystem.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Label::Label(std::string text) : Label(ELEMENT, std::move(text)) {}

    Label::Label(const char* element, std::string text) : Widget(element), mText(TextValue::literal(std::move(text))) {}

    Label& Label::setText(std::string text)
    {
        mText = TextValue::literal(std::move(text));
        invalidateMeasure();
        return *this;
    }

    Label& Label::setText(TextValue text)
    {
        mText = std::move(text);
        invalidateMeasure();
        return *this;
    }

    void Label::onLocaleChanged(const System& system)
    {
        if (mText.localized())
        {
            mText.updateLocalizedValue(system.resolveText(mText.localizationKey()));
            invalidateMeasure();
        }
    }

    Vec2 Label::intrinsicSize(const StyleSheet&, const Style& style, const TextMetrics& text_metrics) const
    {
        return text_metrics.measureText(mText.value(), style);
    }

    void Label::paint(PaintContext& context, const Style& style, float) const
    {
        context.paintBox(rect(), style);
        context.paintText(mText.value(), rect(), style);
    }

    WidgetContract detail::labelContract()
    {
        return defineWidget<Label>(Label::ELEMENT)
            .widgetText([](std::string value, Label& label, ViewBuildResult& result,
                           const std::string& source, const ViewBuildContext* context, std::size_t line)
            {
                label.setText(localizedViewText(std::move(value), result, source, context, line));
            })
            .build();
    }
}
