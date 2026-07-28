#include "linden_common.h"
#include "rdswitch.h"
#include "rduibinder.h"
#include "rduischema.h"
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

    Switch::Switch() : ValueControl(ELEMENT)
    {
        detail::instantiateCompositeParts(*this, detail::switchContract());
    }

    void Switch::onChildrenCleared()
    {
        mThumb.set(nullptr);
        detail::instantiateCompositeParts(*this, detail::switchContract());
    }

    Switch& Switch::setChecked(bool checked)
    {
        const bool changed = checked != this->checked();
        setState(WidgetState::Checked, checked);
        mValueState = {checked, checked, std::nullopt};
        if (changed) notifyValueState();
        return *this;
    }

    Switch& Switch::setBindingId(std::string id)
    {
        mBindingId = std::move(id);
        return *this;
    }

    ValueControlState Switch::valueControlState() const
    {
        ValueControlState result;
        result.dirty = mValueState.dirty();
        result.validation = mValueState.validationStatus();
        if (const TextSource* message = mValueState.validationMessage())
            result.message = *message;
        return result;
    }

    ValueBindingSubscription Switch::observeValueControlState(Observer observer)
    {
        const std::size_t id = mNextValueObserver++;
        mValueObservers.emplace(id, std::move(observer));
        std::weak_ptr<char> lifetime = mValueObserverLifetime;
        return ValueBindingSubscription([this, lifetime, id]
        {
            if (!lifetime.expired()) mValueObservers.erase(id);
        });
    }

    void Switch::notifyValueState()
    {
        const ValueControlState state = valueControlState();
        const auto observers = mValueObservers;
        for (const auto& [id, observer] : observers) if (mValueObservers.find(id) != mValueObservers.end()) observer(state);
    }

    void Switch::applyValueState(ValueState<bool> state)
    {
        mValueState = std::move(state);
        setState(WidgetState::Checked, mValueState.value);
        notifyValueState();
    }

    void Switch::prepareValueBinding(Binder& binder)
    {
        binder.requireValue(mBindingId, mBinding);
    }

    ValueBindingSubscription Switch::commitValueBinding()
    {
        if (!mBinding) return {};
        applyValueState(mBinding->state());
        std::weak_ptr<char> lifetime = mValueObserverLifetime;
        std::shared_ptr<ValueBinding<bool>> provider = mBinding.shared();
        auto provider_subscription = std::make_shared<ValueBindingSubscription>(provider->observe([this, lifetime](const ValueState<bool>& state)
            {
                if (!lifetime.expired()) applyValueState(state);
            }));
        return ValueBindingSubscription([this, lifetime, provider = std::move(provider), provider_subscription]
        {
            provider_subscription->reset();
            if (!lifetime.expired() && mBinding.shared() == provider) mBinding.reset();
        });
    }

    Switch& Switch::setOnCheckedChanged(std::function<void(bool)> callback)
    {
        mOnCheckedChanged = std::move(callback);
        return *this;
    }

    void Switch::onActivate()
    {
        const bool previous = checked();
        if (mBinding)
        {
            mBinding->write(!previous);
            applyValueState(mBinding->state());
        }
        else
        {
            mValueState.value = !previous;
            setState(WidgetState::Checked, mValueState.value);
            notifyValueState();
        }
        if (checked() == previous) return;
        if (mOnCheckedChanged) mOnCheckedChanged(checked());
        emitAction(ChangeActionEvent(*this, checked()));
    }

    void Switch::constrainResolvedStyle(Style& style) const
    {
        style.flow = Flow::Row;
        style.justify_content = checked() ? JustifyContent::End : JustifyContent::Start;
    }

    WidgetContract detail::switchContract()
    {
        return defineWidget<Switch>(Switch::ELEMENT)
            .attributes({booleanAttribute("checked", &Switch::setChecked), stringAttribute("bind", &Switch::setBindingId)})
            .validate([](const LayoutElement& element, Switch&, ViewBuildResult& result, const std::string& source, const ViewBuildContext*)
            {
                const LayoutAttribute* bind = element.attribute("bind");
                if (bind && !isLocalIdentifier(bind->value))
                    result.error("view.value.bind_invalid",
                                 "Value Control bind must be a lowercase kebab-case identifier.",
                                 source, bind->source.begin.line, bind->source.begin.column);
                if (bind && element.attribute("checked"))
                    result.error("view.value.multiple_sources",
                                 "Switch cannot declare both bind and checked.",
                                 source, bind->source.begin.line, bind->source.begin.column);
            })
            .actions({ActionEventKind::Change, ActionEventKind::DoubleClick, ActionEventKind::MouseDown,
                      ActionEventKind::MouseUp, ActionEventKind::MouseMove, ActionEventKind::LongClick,
                      ActionEventKind::ContextMenu})
            .labelable()
            .state(WidgetState::Checked)
            .part<SwitchThumb>("thumb", &Switch::mThumb)
            .build();
    }
}
