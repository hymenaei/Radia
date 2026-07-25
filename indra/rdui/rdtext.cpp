#include "linden_common.h"
#include "rdtext.h"
#include "rduipaintcontext.h"
#include "rduisystem.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Text::Text(std::string text) : Text(ELEMENT, ElementTag{})
    {
        setText(std::move(text));
    }

    Text::Text(const char* element, ElementTag) : Widget(element) {}

    Text& Text::setText(std::string text)
    {
        return setContent(InlineContent::text(std::move(text)));
    }

    Text& Text::setText(TextValue text)
    {
        return setContent(InlineContent::text(std::move(text)));
    }

    Text& Text::setContent(InlineContent content)
    {
        mText.setContent(std::move(content));
        invalidateMeasure();
        return *this;
    }

    void Text::onLocaleChanged(const System& system)
    {
        mText.resolveLocalized([&system](const std::string& key) { return system.resolveText(key); });
        mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
        invalidateMeasure();
    }

    bool Text::onKeybindingsChanged(const System& system)
    {
        const bool changed = mText.resolveKeybindings(
            [&system](const std::string& key) { return system.resolveKeybinding(key); });
        if (changed) invalidateMeasure();
        return changed;
    }

    Vec2 Text::intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& text_metrics) const
    {
        return mText.measure(text_metrics, style, theme, *this);
    }

    void Text::paint(PaintContext& context, const Style& style, float) const
    {
        context.paintBox(rect(), style);
        mText.paint(context, rect(), style, attachedStyleSheet(), *this);
    }

    WidgetContract detail::textContract()
    {
        return defineWidget<Text>(Text::ELEMENT)
            .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S,
                            InlineContentKind::Kbd, InlineContentKind::Br},
                [](InlineContent content, Text& text) { text.setContent(std::move(content)); })
            .build();
    }
}
