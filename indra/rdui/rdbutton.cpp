#include "linden_common.h"
#include "rdbutton.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rduilocalization.h"
#include "rduistyle.h"
#include "rduiviewcontract.h"

namespace rdui
{
    namespace
    {
        class ButtonCaption final : public Label
        {
            public:
                explicit ButtonCaption(TextValue text) : Label("button-caption", {})
                {
                    setText(std::move(text));
                }
        };
    }

    Button::Button() : Widget(ELEMENT) {}

    void Button::constrainResolvedStyle(Style& style) const
    {
        if (!style.flow_set) style.flow = Flow::Row;
        if (!style.justify_content_set) style.justify_content = JustifyContent::Center;
        if (!style.vertical_align_set) style.vertical_align = VerticalAlign::Middle;
    }

    void Button::onChildAdded(Widget& child)
    {
        if (auto* icon = dynamic_cast<Icon*>(&child); icon && !mIcon) mIcon.set(icon);
        else if (auto* label = dynamic_cast<Label*>(&child); label && !mLabel) mLabel.set(label);
    }

    Icon& Button::setIcon(std::string name)
    {
        if (!mIcon)
        {
            addChild(std::make_unique<Icon>());
        }
        return mIcon->setName(std::move(name));
    }

    Label& Button::setLabel(std::string text)
    {
        if (!mLabel)
        {
            addChild(std::make_unique<ButtonCaption>(TextValue::literal(std::string())));
        }
        return mLabel->setText(std::move(text));
    }

    Label& Button::setLabel(TextValue text)
    {
        if (!mLabel)
        {
            addChild(std::make_unique<ButtonCaption>(TextValue::literal(std::string())));
        }
        return mLabel->setText(std::move(text));
    }

    void Button::onChildrenCleared()
    {
        mIcon.set(nullptr);
        mLabel.set(nullptr);
    }

    WidgetContract detail::buttonContract()
    {
        return defineWidget<Button>(Button::ELEMENT)
            .actions({
                ActionEventKind::Click,
                ActionEventKind::DoubleClick,
                ActionEventKind::MouseDown,
                ActionEventKind::MouseUp,
                ActionEventKind::MouseMove,
                ActionEventKind::LongClick,
                ActionEventKind::ContextMenu})
            .textChildren([](TextValue text) { return std::make_unique<ButtonCaption>(std::move(text)); })
            .build();
    }

}
