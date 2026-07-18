#include "linden_common.h"
#include "rdswitch.h"
#include "rduistyle.h"
#include "rduiviewcontract.h"

namespace rdui
{
    namespace
    {
        class SwitchThumb final : public Widget
        {
            public:
                SwitchThumb() : Widget("switch-thumb")
                {
                    setPointerEvents(false);
                }

                void constrainResolvedStyle(Style& style) const override
                {
                    style.align_self = AlignSelf::Stretch;
                    style.aspect_ratio = 1.f;
                }
        };
    }

    Switch::Switch() : Widget(ELEMENT)
    {
        detail::instantiateCompositeParts(*this, detail::WidgetContractRegistry::toggleSwitch());
    }

    void Switch::onChildrenCleared()
    {
        mThumb.set(nullptr);
        detail::instantiateCompositeParts(*this, detail::WidgetContractRegistry::toggleSwitch());
    }

    Switch& Switch::setChecked(bool checked)
    {
        if (checked == this->checked()) return *this;
        setState(WidgetState::Checked, checked);
        return *this;
    }

    Switch& Switch::setOnCheckedChanged(std::function<void(bool)> callback)
    {
        mOnCheckedChanged = std::move(callback);
        return *this;
    }

    void Switch::onActivate()
    {
        setChecked(!checked());
        if (mOnCheckedChanged) mOnCheckedChanged(checked());
        emitAction(ChangeActionEvent(*this, checked()));
    }

    void Switch::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Row;
        style.justify_content = checked() ? JustifyContent::End : JustifyContent::Start;
    }

    WidgetContract detail::WidgetContractRegistry::toggleSwitch()
    {
        return defineWidget<Switch>(Switch::ELEMENT)
            .attributes({booleanAttribute("checked", &Switch::setChecked)})
            .actions({ActionEventKind::Change, ActionEventKind::DoubleClick, ActionEventKind::MouseDown,
                      ActionEventKind::MouseUp, ActionEventKind::MouseMove, ActionEventKind::LongClick,
                      ActionEventKind::ContextMenu})
            .state(WidgetState::Checked)
            .part<SwitchThumb>("thumb", &Switch::mThumb)
            .build();
    }
}
