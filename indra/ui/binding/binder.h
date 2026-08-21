/**
 * @file binder.h
 * @brief Validates and commits controller bindings to UI Widget trees.
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

#ifndef RD_BINDING_BINDER_H
#define RD_BINDING_BINDER_H

#include <functional>
#include <limits>
#include <map>
#include <memory>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <typeindex>
#include <utility>
#include <vector>
#include "binding/eventregistration.h"
#include "binding/settingresolver.h"
#include "binding/valuebinding.h"
#include "diagnostic.h"
#include "widgets/widget.h"

namespace radia::ui {
class Binder;
class ValueControl;

class Binding {
    friend class Binder;

public:
    Binding() = default;
    Binding(const Binding&) = delete;
    Binding& operator=(const Binding&) = delete;
    Binding(Binding&&) noexcept = default;
    Binding& operator=(Binding&&) noexcept = default;

    explicit operator bool() const { return mCommitted; }

private:
    std::vector<std::shared_ptr<detail::EventHandler>> mHandlers;
    std::vector<ValueBindingSubscription> mValueSubscriptions;
    bool mCommitted = false;
};

class PreparedBinding {
    friend class Binder;

public:
    PreparedBinding();
    ~PreparedBinding();
    PreparedBinding(PreparedBinding&&) noexcept;
    PreparedBinding& operator=(PreparedBinding&&) noexcept;

    PreparedBinding(const PreparedBinding&) = delete;
    PreparedBinding& operator=(const PreparedBinding&) = delete;

    explicit operator bool() const { return mBinder != nullptr; }
    Binding commit();

private:
    std::unique_ptr<Binder> mBinder;
};

struct PreparedBindingResult : DiagnosticResult {
    bool ok() const { return !hasErrors() && static_cast<bool>(binding); }
    PreparedBinding binding;
};

class Binder {
public:
    struct EventDeclaration {
        Widget* widget = nullptr;
        WidgetEventKind kind = WidgetEventKind::Click;
        const EventCall* call = nullptr;
    };

    explicit Binder(Widget& root, SettingResolver* settingResolver = nullptr) : mRoot(&root), mSettingResolver(settingResolver) {}

    Binder(const Binder&) = delete;
    Binder& operator=(const Binder&) = delete;

    template<typename T> void requireValueBinding(ValueBindingRequest request, ValueBindingRef<T>& output) {
        PendingValueRequirement pending;
        pending.settingName = std::move(request.settingName);
        pending.type = typeid(T);
        pending.typeName = detail::valueTypeName<T>();
        pending.matches = [](const std::shared_ptr<ValueBindingBase>& binding) {
            return static_cast<bool>(std::dynamic_pointer_cast<ValueBinding<T>>(binding));
        };
        pending.commit = [&output](const std::shared_ptr<ValueBindingBase>& binding) {
            output.set(std::dynamic_pointer_cast<ValueBinding<T>>(binding));
        };
        mPendingValueRequirements.push_back(std::move(pending));
    }

    PreparedBindingResult prepare();

    void event(EventHandlerRegistration&& registration);
    void event(const EventHandlerRegistration& registration);

private:
    struct PendingEventHandler {
        std::string name;
        std::optional<WidgetEventKind> kind;
        std::shared_ptr<detail::EventHandler> handler;
        std::function<const char*(const EventCall&, WidgetEventKind)> argumentError;
        bool valid = false;
    };

    struct PendingValueRequirement {
        std::string settingName;
        std::type_index type{typeid(void)};
        const char* typeName = nullptr;
        std::function<bool(const std::shared_ptr<ValueBindingBase>&)> matches;
        std::function<void(const std::shared_ptr<ValueBindingBase>&)> commit;
        std::shared_ptr<ValueBindingBase> resolved;
    };

    Binder(Binder&&) noexcept = default;
    Binder& operator=(Binder&&) noexcept = default;

    void validate(Widget& root, DiagnosticResult& result);
    void commit(Widget& root, Binding& binding);

    Widget* mRoot = nullptr;
    SettingResolver* mSettingResolver = nullptr;
    std::vector<PendingEventHandler> mPendingEventHandlers;
    std::vector<PendingValueRequirement> mPendingValueRequirements;
    std::vector<ValueControl*> mValueControls;
    std::map<std::string, std::vector<EventDeclaration>> mEventDeclarations;
    bool mFinished = false;

    friend class PreparedBinding;
};
} // namespace radia::ui
#endif // RD_BINDING_BINDER_H
