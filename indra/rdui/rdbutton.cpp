#include "linden_common.h"
#include "rdbutton.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rduilocalization.h"
#include "rduistyle.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Button::Button() : Widget(ELEMENT) {}

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
            addChild(std::make_unique<Label>());
        }
        return mLabel->setText(std::move(text));
    }

    Label& Button::setLabel(TextValue text)
    {
        if (!mLabel)
        {
            addChild(std::make_unique<Label>());
        }
        return mLabel->setText(std::move(text));
    }

    void Button::onChildrenCleared()
    {
        mIcon.set(nullptr);
        mLabel.set(nullptr);
    }

    WidgetContract detail::WidgetContractRegistry::button()
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
            .textChildren()
            .build();
    }

}
