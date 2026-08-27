/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

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
#include "elements/element.h"
#include "elements/input.h"

namespace radia::ui {
class Binder;

class Binding {
    friend class Binder;

public:
    Binding() = default;
    ~Binding();
    Binding(const Binding&) = delete;
    Binding& operator=(const Binding&) = delete;
    Binding(Binding&& other) noexcept;
    Binding& operator=(Binding&& other) noexcept;

    explicit operator bool() const { return mCommitted; }

private:
    struct EventAttachment {
        Element* element = nullptr;
        std::weak_ptr<char> lifetime;
        std::string type;
        EventHandler handler;
        bool capture = false;
    };

    void clearEventListeners() noexcept;

    std::vector<EventAttachment> mEventAttachments;
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
        Element* element = nullptr;
        std::string type;
        const EventCall* call = nullptr;
    };

    explicit Binder(Element& root, SettingResolver* settingResolver = nullptr) : mRoot(&root), mSettingResolver(settingResolver) {}

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
        EventRegistrationDescriptor::Invoke invoke;
        std::function<const char*(const EventCall&)> argumentError;
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

    void validate(Element& root, DiagnosticResult& result);
    void commit(Element& root, Binding& binding);

    Element* mRoot = nullptr;
    SettingResolver* mSettingResolver = nullptr;
    std::vector<PendingEventHandler> mPendingEventHandlers;
    std::vector<PendingValueRequirement> mPendingValueRequirements;
    std::vector<InputElement*> mBoundInputs;
    std::map<std::string, std::vector<EventDeclaration>> mEventDeclarations;
    bool mFinished = false;

    friend class PreparedBinding;
};
} // namespace radia::ui
