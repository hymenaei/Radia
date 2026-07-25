#ifndef LL_RDUI_VALUE_CONTROL_H
#define LL_RDUI_VALUE_CONTROL_H

#include "rduivaluebinding.h"
#include "rduiwidget.h"
#include <functional>
#include <optional>
#include <string>

namespace rdui
{
    class Binder;

    struct ValueControlState
    {
        bool dirty = false;
        ValueValidationStatus validation = ValueValidationStatus::Valid;
        std::optional<TextValue> message;
    };

    class ValueControl : public Widget
    {
        friend class Binder;

        public:
            using Observer = std::function<void(const ValueControlState&)>;
            virtual ~ValueControl() = default;

            virtual const std::string& bindingId() const = 0;
            virtual ValueControlState valueControlState() const = 0;
            virtual ValueBindingSubscription observeValueControlState(Observer observer) = 0;

        protected:
            explicit ValueControl(const char* element) : Widget(element) {}

        private:
            virtual void prepareValueBinding(Binder& binder) = 0;
            virtual ValueBindingSubscription commitValueBinding() = 0;
    };
}

#endif // LL_RDUI_VALUE_CONTROL_H
