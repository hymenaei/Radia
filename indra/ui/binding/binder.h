/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <functional>
#include <limits>
#include <map>
#include <memory>
#include <optional>
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
#include "dom/element.h"
#include "html/input.h"

namespace radia::ui {
class Binder;
class Surface;

class Binding {
    friend class Binder;
    friend class Surface;

public:
    Binding() = default;
    ~Binding();
    Binding(const Binding&) = delete;
    Binding& operator=(const Binding&) = delete;
    Binding(Binding&& other) noexcept;
    Binding& operator=(Binding&& other) noexcept;

    explicit operator bool() const { return mCommitted; }

    [[nodiscard]] bool activate();
    void deactivate() noexcept;

private:
    struct ValueAttachment {
        HTMLInputElement* element = nullptr;
        std::weak_ptr<char> lifetime;
    };

    struct EventAttachment {
        Element* element = nullptr;
        std::weak_ptr<char> lifetime;
        std::string type;
        EventHandler handler;
        bool capture = false;
        bool attached = false;
    };

    void clearEventListeners() noexcept;
    void attachEventListeners();

    std::vector<EventAttachment> mEventAttachments;
    std::vector<ValueAttachment> mValueAttachments;
    std::vector<ValueBindingSubscription> mValueSubscriptions;
    Element* mRoot = nullptr;
    Node* mRootParent = nullptr;
    std::weak_ptr<char> mRootLifetime;
    std::shared_ptr<bool> mActive = std::make_shared<bool>(false);
    Surface* mAttachedSurface = nullptr;
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

    explicit operator bool() const;
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
        std::weak_ptr<char> lifetime;
        Node* parent = nullptr;
        std::string type;
        EventCall call;
        detail::MountEpoch mountEpoch;
        std::uint64_t parentTopologyEpoch = 0;
    };

    explicit Binder(Element& root, SettingResolver* settingResolver = nullptr);

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
    using MountEpoch = detail::MountEpoch;

    struct BoundInput {
        HTMLInputElement* element = nullptr;
        std::weak_ptr<char> lifetime;
        Node* parent = nullptr;
        std::string settingName;
        detail::MountEpoch mountEpoch;
        std::uint64_t parentTopologyEpoch = 0;
    };

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

    struct TopologyObservation {
        Element* element = nullptr;
        std::weak_ptr<char> lifetime;
        std::uint64_t epoch = 0;
    };

    Binder(Binder&&) noexcept = default;
    Binder& operator=(Binder&&) noexcept = default;

    bool validForCommit() const;
    void validate(Element& root, DiagnosticResult& result);
    void commit(Element& root, Binding& binding);

    Element* mRoot = nullptr;
    std::weak_ptr<char> mRootLifetime;
    bool mRootWasMounted = false;
    MountEpoch mRootMountEpoch;
    std::uint64_t mRootTopologyEpoch = 0;
    Node* mRootParent = nullptr;
    SettingResolver* mSettingResolver = nullptr;
    std::vector<PendingEventHandler> mPendingEventHandlers;
    std::vector<PendingValueRequirement> mPendingValueRequirements;
    std::vector<BoundInput> mBoundInputs;
    std::vector<TopologyObservation> mTopologyObservations;
    std::map<std::string, std::vector<EventDeclaration>> mEventDeclarations;
    bool mFinished = false;

    friend class PreparedBinding;
};
} // namespace radia::ui
