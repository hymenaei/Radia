/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "binding/binder.h"
#include <map>
#include <set>
#include "dom/elementinternal.h"
#include "eventcall.h"
#include "surface/surface.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::MountEpoch;

Binding::~Binding() {
    deactivate();
}

Binding::Binding(Binding&& other) noexcept
    : mEventAttachments(std::move(other.mEventAttachments)), mValueAttachments(std::move(other.mValueAttachments)),
      mValueSubscriptions(std::move(other.mValueSubscriptions)), mRoot(other.mRoot), mRootParent(other.mRootParent),
      mRootLifetime(std::move(other.mRootLifetime)), mActive(std::move(other.mActive)), mAttachedSurface(other.mAttachedSurface),
      mCommitted(other.mCommitted) {
    if (mAttachedSurface) mAttachedSurface->replaceBinding(other, *this);
    other.mRoot = nullptr;
    other.mRootParent = nullptr;
    other.mAttachedSurface = nullptr;
    other.mCommitted = false;
}

Binding& Binding::operator=(Binding&& other) noexcept {
    if (this == &other) return *this;
    deactivate();
    mEventAttachments = std::move(other.mEventAttachments);
    mValueAttachments = std::move(other.mValueAttachments);
    mValueSubscriptions = std::move(other.mValueSubscriptions);
    mRoot = other.mRoot;
    mRootParent = other.mRootParent;
    mRootLifetime = std::move(other.mRootLifetime);
    mActive = std::move(other.mActive);
    mAttachedSurface = other.mAttachedSurface;
    mCommitted = other.mCommitted;
    if (mAttachedSurface) mAttachedSurface->replaceBinding(other, *this);
    other.mRoot = nullptr;
    other.mRootParent = nullptr;
    other.mAttachedSurface = nullptr;
    other.mCommitted = false;
    return *this;
}

void Binding::clearEventListeners() noexcept {
    for (EventAttachment& attachment : mEventAttachments) {
        if (attachment.attached && attachment.element && !attachment.lifetime.expired())
            attachment.element->removeEventListener(attachment.type, attachment.handler, attachment.capture);
        attachment.attached = false;
    }
}

void Binding::attachEventListeners() {
    for (EventAttachment& attachment : mEventAttachments) {
        if (attachment.attached || !attachment.element || attachment.lifetime.expired()) continue;
        attachment.element->addEventListener(attachment.type, attachment.handler, attachment.capture);
        attachment.attached = true;
    }
}

bool Binding::activate() {
    if (!mCommitted || !mRoot || mRootLifetime.expired() || mRoot->parentNode() != mRootParent) {
        deactivate();
        return false;
    }

    const auto isInRoot = [this](const Element& element) {
        for (const Node* current = &element; current; current = current->parentNode())
            if (current == mRoot) return true;
        return false;
    };

    for (const ValueAttachment& attachment : mValueAttachments) {
        if (!attachment.element || attachment.lifetime.expired() || !isInRoot(*attachment.element)) {
            deactivate();
            return false;
        }
    }

    for (const EventAttachment& attachment : mEventAttachments) {
        if (!attachment.element || attachment.lifetime.expired() || !isInRoot(*attachment.element)) {
            deactivate();
            return false;
        }
    }

    Surface* surface = mRoot->surface();
    if (mAttachedSurface && mAttachedSurface != surface) {
        deactivate();
        return false;
    }

    if (!mActive) mActive = std::make_shared<bool>(false);
    attachEventListeners();
    if (surface && !surface->attachBinding(*mRoot, *this)) {
        deactivate();
        return false;
    }
    *mActive = true;
    for (const ValueAttachment& attachment : mValueAttachments) {
        ElementRef<HTMLInputElement> input(attachment.element);
        if (input) input->synchronizeValueBinding();
    }
    return true;
}

void Binding::deactivate() noexcept {
    if (mAttachedSurface) mAttachedSurface->detachBinding(*this);
    if (mActive) *mActive = false;
    clearEventListeners();
}

PreparedBinding::PreparedBinding() = default;
PreparedBinding::~PreparedBinding() = default;
PreparedBinding::PreparedBinding(PreparedBinding&&) noexcept = default;
PreparedBinding& PreparedBinding::operator=(PreparedBinding&&) noexcept = default;

PreparedBinding::operator bool() const {
    return mBinder && mBinder->validForCommit();
}

Binding PreparedBinding::commit() {
    if (!mBinder) return {};

    std::unique_ptr<Binder> binder = std::move(mBinder);
    if (!binder->validForCommit()) return {};

    Binding binding;
    binder->commit(*binder->mRoot, binding);
    return binding;
}

Binder::Binder(Element& root, SettingResolver* settingResolver)
    : mRoot(&root), mRootLifetime(ElementInternalAccess::lifetime(root)), mSettingResolver(settingResolver) {}

void Binder::event(EventHandlerRegistration&& registration) {
    const bool valid = registration.valid();
    std::string name = std::move(registration).takeName();
    EventRegistrationDescriptor::Invoke invoke = std::move(registration).takeInvoke();
    EventHandlerRegistration::ArgumentError argumentError = std::move(registration).takeArgumentError();
    mPendingEventHandlers.push_back({std::move(name), std::move(invoke), std::move(argumentError), valid});
}

void Binder::event(const EventHandlerRegistration& registration) {
    event(registration.copy());
}

namespace {
bool sameEventArgument(const EventArgument& left, const EventArgument& right) {
    if (left.index() != right.index()) return false;
    return std::visit(
        [](const auto& leftValue, const auto& rightValue) {
            using Left = std::decay_t<decltype(leftValue)>;
            using Right = std::decay_t<decltype(rightValue)>;
            if constexpr (!std::is_same_v<Left, Right>) return false;
            else if constexpr (std::is_same_v<Left, SourceElementArgument> || std::is_same_v<Left, CurrentEventArgument>) return true;
            else return leftValue == rightValue;
        },
        left, right);
}

bool sameEventCall(const EventCall& left, const EventCall& right) {
    if (left.name() != right.name() || left.arguments().size() != right.arguments().size()) return false;
    for (std::size_t index = 0; index < left.arguments().size(); ++index)
        if (!sameEventArgument(left.arguments()[index], right.arguments()[index])) return false;
    return true;
}

void collectEventCalls(Element& element, std::map<std::string, std::vector<Binder::EventDeclaration>>& declarations) {
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        const std::string type(descriptor.type);
        const EventCall* eventCall = authoredEventCall(element, type);
        if (!eventCall) continue;
        const std::string& name = eventCall->name();
        declarations[name].push_back({&element, ElementInternalAccess::lifetime(element), element.parentNode(), type, *eventCall,
                                      ElementInternalAccess::mountEpoch(element),
                                      element.parentElement() ? ElementInternalAccess::topologyEpoch(*element.parentElement()) : 0});
    }
    for (Element* child : element.children())
        if (!child->idScopeRoot()) collectEventCalls(*child, declarations);
}

void collectBoundInputs(Element& element, std::vector<HTMLInputElement*>& inputs) {
    if (auto* input = dynamic_cast<HTMLInputElement*>(&element); input && input->valueBindingRequest()) inputs.push_back(input);
    for (Element* child : element.children())
        if (!child->idScopeRoot()) collectBoundInputs(*child, inputs);
}
} // namespace

void Binder::validate(Element& root, DiagnosticResult& result) {
    std::vector<HTMLInputElement*> inputs;
    collectBoundInputs(root, inputs);
    mBoundInputs.clear();
    mBoundInputs.reserve(inputs.size());
    for (HTMLInputElement* input : inputs) {
        const std::optional<ValueBindingRequest> request = input->valueBindingRequest();
        if (!request) continue;
        input->prepareValueBinding(*this);
        mBoundInputs.push_back({input, ElementInternalAccess::lifetime(*input), input->parentNode(), request->settingName,
                                ElementInternalAccess::mountEpoch(*input),
                                input->parentElement() ? ElementInternalAccess::topologyEpoch(*input->parentElement()) : 0});
    }

    mEventDeclarations.clear();
    collectEventCalls(root, mEventDeclarations);

    std::set<std::string> registeredHandlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) {
        if (!pending.valid)
            result.error("binding.event.registration_invalid", "Controller Event Handler registration is incomplete: " + pending.name + ".");
        if (!isEventHandlerName(pending.name))
            result.error("binding.event.name_invalid", "Controller Event Handler name must use lower-camel-case: " + pending.name + ".");
        const auto inserted = registeredHandlers.emplace(pending.name);
        if (!inserted.second) result.error("binding.event.duplicate", "Controller Event Handler is registered more than once: " + pending.name + ".");
    }

    for (const auto& entry : mEventDeclarations) {
        const std::string& name = entry.first;
        if (registeredHandlers.find(name) == registeredHandlers.end())
            result.warning("binding.event.unhandled", "Event Handler Call has no Controller Handler: " + name + ".");
    }

    for (PendingEventHandler& pending : mPendingEventHandlers) {
        const auto found = mEventDeclarations.find(pending.name);
        if (found == mEventDeclarations.end()) continue;
        for (const EventDeclaration& declaration : found->second) {
            if (pending.argumentError) {
                if (const char* error = pending.argumentError(declaration.call))
                    result.warning(error, "Event Handler Call " + pending.name + " does not match its registered Handler signature.");
            }
        }
    }

    std::set<std::string> settingDiagnostics;
    for (PendingValueRequirement& pending : mPendingValueRequirements) {
        const auto reportSettingError = [&](const char* code, std::string message) {
            std::string key = code;
            key.push_back('\0');
            key += pending.settingName;
            key.push_back('\0');
            key += pending.typeName;
            if (settingDiagnostics.emplace(std::move(key)).second) result.error(code, std::move(message));
        };
        if (!mSettingResolver) {
            reportSettingError("binding.setting.resolver_missing", "No SettingResolver was provided for setting: " + pending.settingName + ".");
            continue;
        }

        const SettingResolution resolution = mSettingResolver->resolve(pending.settingName, pending.type);
        switch (resolution.status) {
            case SettingResolution::ResolutionStatus::Found:
                if (!resolution.binding || !pending.matches || !pending.matches(resolution.binding)) {
                    reportSettingError("binding.setting.type_mismatch",
                                       "Setting " + pending.settingName + " does not provide " + pending.typeName + ".");
                    continue;
                }
                pending.resolved = resolution.binding;
                break;
            case SettingResolution::ResolutionStatus::Missing:
                reportSettingError("binding.setting.missing", "Setting is not available in this viewer: " + pending.settingName + ".");
                break;
            case SettingResolution::ResolutionStatus::TypeMismatch:
                reportSettingError("binding.setting.type_mismatch", "Setting " + pending.settingName + " is not a " + pending.typeName + ".");
                break;
            case SettingResolution::ResolutionStatus::Invalid:
                reportSettingError("binding.setting.name_invalid", "Setting is not a valid setting name: " + pending.settingName + ".");
                break;
        }
    }
}

void Binder::commit(Element&, Binding& binding) {
    binding.mCommitted = true;
    binding.mRoot = mRoot;
    binding.mRootParent = mRootParent;
    binding.mRootLifetime = mRootLifetime;
    if (!binding.mActive) binding.mActive = std::make_shared<bool>(false);
    *binding.mActive = false;

    for (PendingValueRequirement& pending : mPendingValueRequirements) pending.commit(pending.resolved);
    for (const BoundInput& bound : mBoundInputs) {
        if (bound.lifetime.expired()) continue;
        ValueBindingSubscription subscription = bound.element->commitValueBinding(binding.mActive, *mRoot);
        binding.mValueAttachments.push_back({bound.element, bound.lifetime});
        if (subscription) binding.mValueSubscriptions.push_back(std::move(subscription));
    }
    std::map<std::string, PendingEventHandler*> handlers;
    for (PendingEventHandler& pending : mPendingEventHandlers) handlers.emplace(pending.name, &pending);
    for (const auto& [name, declarations] : mEventDeclarations) {
        const auto found = handlers.find(name);
        if (found == handlers.end()) continue;
        PendingEventHandler& pending = *found->second;
        for (const EventDeclaration& declaration : declarations)
            if (!pending.argumentError || !pending.argumentError(declaration.call)) {
                const ElementRef<Element> declarationRef(declaration.element);
                const ElementRef<Element> rootRef(mRoot);
                const std::weak_ptr<bool> bindingActive = binding.mActive;
                EventHandler listener([invoke = pending.invoke, call = declaration.call, type = declaration.type, declarationRef, rootRef,
                                       bindingActive](Event& event) mutable {
                    const std::shared_ptr<bool> active = bindingActive.lock();
                    if (!active || !*active) return;
                    Element* element = declarationRef.get();
                    Element* root = rootRef.get();
                    if (!element || !root) return;
                    bool inRoot = false;
                    for (Node* current = element; current; current = current->parentNode())
                        if (current == root) {
                            inRoot = true;
                            break;
                        }
                    const EventCall* authored = authoredEventCall(*element, type);
                    if (!inRoot || !authored || !sameEventCall(*authored, call)) return;
                    invoke(event, call);
                });
                binding.mEventAttachments.push_back({declaration.element, ElementInternalAccess::lifetime(*declaration.element), declaration.type,
                                                     std::move(listener), false, false});
            }
    }
}

bool Binder::validForCommit() const {
    if (!mRoot || mRootLifetime.expired() || mRoot->parentNode() != mRootParent) return false;
    const MountEpoch currentMountEpoch = ElementInternalAccess::mountEpoch(*mRoot);
    const bool firstMountAfterPrepare = !mRootWasMounted
        && ElementInternalAccess::isMounted(*mRoot)
        && mRootMountEpoch.value < std::numeric_limits<std::uint64_t>::max()
        && currentMountEpoch.value == mRootMountEpoch.value + 1;
    if (currentMountEpoch != mRootMountEpoch && !firstMountAfterPrepare) return false;
    if (ElementInternalAccess::topologyEpoch(*mRoot) != mRootTopologyEpoch) return false;

    const auto mountEpochValid = [this, firstMountAfterPrepare](const Element& element, MountEpoch expected) {
        const MountEpoch current = ElementInternalAccess::mountEpoch(element);
        if (current == expected) return true;
        return firstMountAfterPrepare
            && ElementInternalAccess::isMounted(element)
            && expected.value < std::numeric_limits<std::uint64_t>::max()
            && current.value == expected.value + 1;
    };

    for (const TopologyObservation& observation : mTopologyObservations)
        if (!observation.element || observation.lifetime.expired() || ElementInternalAccess::topologyEpoch(*observation.element) != observation.epoch)
            return false;

    const auto isInRoot = [this](const Element& element) {
        for (const Node* current = &element; current; current = current->parentNode())
            if (current == mRoot) return true;
        return false;
    };

    for (const BoundInput& bound : mBoundInputs) {
        if (!bound.element
            || bound.lifetime.expired()
            || bound.element->parentNode() != bound.parent
            || !isInRoot(*bound.element)
            || !mountEpochValid(*bound.element, bound.mountEpoch)
            || (bound.element->parentElement() ? ElementInternalAccess::topologyEpoch(*bound.element->parentElement()) : 0)
                != bound.parentTopologyEpoch)
            return false;
        const std::optional<ValueBindingRequest> request = bound.element->valueBindingRequest();
        if (!request || request->settingName != bound.settingName) return false;
    }

    for (const auto& [name, declarations] : mEventDeclarations) {
        for (const EventDeclaration& declaration : declarations) {
            if (!declaration.element
                || declaration.lifetime.expired()
                || declaration.element->parentNode() != declaration.parent
                || !isInRoot(*declaration.element)
                || !mountEpochValid(*declaration.element, declaration.mountEpoch)
                || (declaration.element->parentElement() ? ElementInternalAccess::topologyEpoch(*declaration.element->parentElement()) : 0)
                    != declaration.parentTopologyEpoch)
                return false;
            const EventCall* current = authoredEventCall(*declaration.element, declaration.type);
            if (!current || !sameEventCall(*current, declaration.call)) return false;
        }
    }
    return true;
}

PreparedBindingResult Binder::prepare() {
    PreparedBindingResult result;
    if (mFinished) {
        result.error("binding.already_finished", "Binder transaction was already finished.");
        return result;
    }
    mFinished = true;

    if (!mRoot || mRootLifetime.expired()) {
        result.error("binding.root.invalid", "Cannot prepare a binding for a destroyed root Element.");
        return result;
    }
    mRootParent = mRoot->parentNode();
    mRootWasMounted = ElementInternalAccess::isMounted(*mRoot);
    mRootMountEpoch = ElementInternalAccess::mountEpoch(*mRoot);
    mRootTopologyEpoch = ElementInternalAccess::topologyEpoch(*mRoot);
    mTopologyObservations.clear();
    const auto collectTopology = [this](auto&& self, Element& element) -> void {
        mTopologyObservations.push_back({&element, ElementInternalAccess::lifetime(element), ElementInternalAccess::topologyEpoch(element)});
        for (Element* child : element.children()) self(self, *child);
    };
    collectTopology(collectTopology, *mRoot);
    validate(*mRoot, result);
    if (!result.hasErrors()) {
        auto binder = std::unique_ptr<Binder>(new Binder(std::move(*this)));
        result.binding.mBinder = std::move(binder);
    }
    return result;
}
} // namespace radia::ui
