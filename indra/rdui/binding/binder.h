/**
 * @file binder.h
 * @brief
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
#include <map>
#include <memory>
#include <string>
#include <type_traits>
#include <typeindex>
#include <utility>
#include <vector>
#include "binding/valuebinding.h"
#include "diagnostic.h"
#include "widgets/widget.h"

namespace rdui {
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
    void reset() {
        mValueSubscriptions.clear();
        mHandlers.clear();
        mCommitted = false;
    }

private:
    std::vector<std::shared_ptr<detail::ActionHandler>> mHandlers;
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

struct BindingResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }
    Binding binding;
};

struct PreparedBindingResult : DiagnosticResult {
    bool ok() const { return !hasErrors() && static_cast<bool>(binding); }
    PreparedBinding binding;
};

class Binder {
public:
    explicit Binder(Widget& root) : mRoot(&root) {}

    Binder(const Binder&) = delete;
    Binder& operator=(const Binder&) = delete;

    template<typename WidgetT> void bind(std::string id, WidgetRef<WidgetT>& output) {
        Pending pending;
        pending.id = std::move(id);
        pending.expected_type = WidgetT::ELEMENT;
        pending.accepts = [](Widget* widget) { return dynamic_cast<WidgetT*>(widget) != nullptr; };
        pending.commit = [&output](Widget* widget) { output.set(static_cast<WidgetT*>(widget)); };
        mPending.push_back(std::move(pending));
    }

    template<typename BindingT> void provideValue(std::string id, std::shared_ptr<BindingT> binding) {
        using T = typename BindingT::ValueType;
        static_assert(std::is_base_of_v<ValueBinding<T>, BindingT>, "Value provider must derive from ValueBinding<T>.");

        PendingValueProvider pending;
        pending.id = std::move(id);
        pending.type = typeid(T);
        pending.type_name = detail::valueTypeName<T>();
        pending.binding = std::static_pointer_cast<ValueBinding<T>>(binding);
        mPendingValueProviders.push_back(std::move(pending));
    }

    template<typename T> void requireValue(std::string id, ValueBindingRef<T>& output) {
        PendingValueRequirement pending;
        pending.id = std::move(id);
        pending.type = typeid(T);
        pending.type_name = detail::valueTypeName<T>();
        pending.commit = [&output](const std::shared_ptr<ValueBindingBase>& binding) {
            output.set(std::static_pointer_cast<ValueBinding<T>>(binding));
        };
        mPendingValueRequirements.push_back(std::move(pending));
    }

    template<typename Callback> void onClick(std::string action, Callback callback) {
        on<ClickActionEvent>(ActionEventKind::Click, std::move(action), std::move(callback));
    }

    template<typename Callback> void onDoubleClick(std::string action, Callback callback) {
        on<MouseActionEvent>(ActionEventKind::DoubleClick, std::move(action), std::move(callback));
    }

    template<typename Callback> void onChange(std::string action, Callback callback) {
        on<ChangeActionEvent>(ActionEventKind::Change, std::move(action), std::move(callback));
    }

    template<typename Callback> void onMouseDown(std::string action, Callback callback) {
        on<MouseActionEvent>(ActionEventKind::MouseDown, std::move(action), std::move(callback));
    }

    template<typename Callback> void onMouseUp(std::string action, Callback callback) {
        on<MouseActionEvent>(ActionEventKind::MouseUp, std::move(action), std::move(callback));
    }

    template<typename Callback> void onMouseMove(std::string action, Callback callback) {
        on<MouseActionEvent>(ActionEventKind::MouseMove, std::move(action), std::move(callback));
    }

    template<typename Callback> void onLongClick(std::string action, Callback callback) {
        on<LongClickActionEvent>(ActionEventKind::LongClick, std::move(action), std::move(callback));
    }

    template<typename Callback> void onContextMenu(std::string action, Callback callback) {
        on<MouseActionEvent>(ActionEventKind::ContextMenu, std::move(action), std::move(callback));
    }

    template<typename Callback> void scope(std::string id, Callback callback) {
        static_assert(std::is_invocable_v<Callback, Binder&>, "Scope callback must accept a Binder reference.");

        auto binder = std::unique_ptr<Binder>(new Binder());
        callback(*binder);
        mPendingScopes.push_back({std::move(id), std::move(binder)});
    }

    BindingResult finish();
    PreparedBindingResult prepare();

private:
    struct Pending {
        std::string id;
        const char* expected_type = nullptr;
        std::function<bool(Widget*)> accepts;
        std::function<void(Widget*)> commit;
        Widget* resolved = nullptr;
    };

    struct PendingAction {
        std::string name;
        ActionEventKind kind;
        std::shared_ptr<detail::ActionHandler> handler;
    };

    struct PendingScope {
        std::string id;
        std::unique_ptr<Binder> binder;
        Widget* resolved = nullptr;
    };

    struct PendingValueProvider {
        std::string id;
        std::type_index type{typeid(void)};
        const char* type_name = nullptr;
        std::shared_ptr<ValueBindingBase> binding;
    };

    struct PendingValueRequirement {
        std::string id;
        std::type_index type{typeid(void)};
        const char* type_name = nullptr;
        std::function<void(const std::shared_ptr<ValueBindingBase>&)> commit;
        std::shared_ptr<ValueBindingBase> resolved;
    };

    Binder() = default;
    Binder(Binder&&) noexcept = default;
    Binder& operator=(Binder&&) noexcept = default;

    template<typename EventT, typename Callback> void on(ActionEventKind kind, std::string action, Callback callback) {
        using CallbackT = std::decay_t<Callback>;
        static_assert(std::is_invocable_v<CallbackT, const EventT&> || std::is_invocable_v<CallbackT>,
                      "Action callback must accept its typed event or no arguments.");

        auto handler = std::make_shared<detail::ActionHandler>();
        handler->kind = kind;
        handler->invoke = [callback = CallbackT(std::move(callback))](const ActionEvent& event) mutable {
            if constexpr (std::is_invocable_v<CallbackT, const EventT&>) callback(static_cast<const EventT&>(event));
            else callback();
        };
        mPendingActions.push_back({std::move(action), kind, std::move(handler)});
    }

    void validate(Widget& root, DiagnosticResult& result);
    void commit(Widget& root, Binding& binding);

    Widget* mRoot = nullptr;
    std::vector<Pending> mPending;
    std::vector<PendingAction> mPendingActions;
    std::vector<PendingScope> mPendingScopes;
    std::vector<PendingValueProvider> mPendingValueProviders;
    std::vector<PendingValueRequirement> mPendingValueRequirements;
    std::vector<ValueControl*> mValueControls;
    bool mFinished = false;

    friend class PreparedBinding;
};
} // namespace rdui
#endif // RD_BINDING_BINDER_H
