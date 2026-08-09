/**
 * @file valuecontrol.h
 * @brief Defines the Widget contract for controls backed by typed value bindings.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef RD_BINDING_VALUECONTROL_H
#define RD_BINDING_VALUECONTROL_H

#include <functional>
#include <optional>
#include <string>
#include "binding/valuebinding.h"
#include "widgets/widget.h"

namespace rdui {
class Binder;

struct ValueControlState {
    bool dirty = false;
    ValueValidationStatus validation = ValueValidationStatus::Valid;
    std::optional<TextSource> message;
};

class ValueControl : public Widget {
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
} // namespace rdui
#endif // RD_BINDING_VALUECONTROL_H
