/**
 * @file rduibinder.cpp
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

#include "linden_common.h"
#include "rduibinder.h"
#include <map>
#include "rduischema.h"
#include "rduivaluecontrol.h"

namespace rdui {
PreparedBinding::PreparedBinding() = default;
PreparedBinding::~PreparedBinding() = default;
PreparedBinding::PreparedBinding(PreparedBinding&&) noexcept = default;
PreparedBinding& PreparedBinding::operator=(PreparedBinding&&) noexcept = default;

Binding PreparedBinding::commit() {
    Binding binding;
    if (!mBinder || !mBinder->mRoot) return binding;
    mBinder->commit(*mBinder->mRoot, binding);
    mBinder.reset();
    return binding;
}

namespace {
Widget* findInScope(Widget& widget, const std::string& id) {
    if (widget.id() == id) return &widget;
    for (const auto& child : widget.children()) {
        if (child->id() == id) return child.get();
        if (child->idScopeRoot()) continue;
        if (Widget* found = findInScope(*child, id)) return found;
    }
    return nullptr;
}

void collectActions(Widget& widget, std::map<std::string, ActionEventKind>& kinds, std::vector<std::pair<Widget*, ActionEventKind>>& declarations,
                    DiagnosticResult& result) {
    for (ActionEventKind kind : {ActionEventKind::Click, ActionEventKind::DoubleClick, ActionEventKind::Change, ActionEventKind::MouseDown,
                                 ActionEventKind::MouseUp, ActionEventKind::MouseMove, ActionEventKind::LongClick, ActionEventKind::ContextMenu}) {
        const std::string& name = widget.action(kind);
        if (name.empty()) continue;
        const auto inserted = kinds.emplace(name, kind);
        if (!inserted.second && inserted.first->second != kind)
            result.error("binding.action.kind_mismatch", "Action " + name + " is declared for multiple event kinds.");
        declarations.emplace_back(&widget, kind);
    }
    for (const auto& child : widget.children())
        if (!child->idScopeRoot()) collectActions(*child, kinds, declarations, result);
}

void collectValueControls(Widget& widget, std::vector<ValueControl*>& controls) {
    if (auto* control = dynamic_cast<ValueControl*>(&widget); control && !control->bindingId().empty()) controls.push_back(control);
    for (const auto& child : widget.children())
        if (!child->idScopeRoot()) collectValueControls(*child, controls);
}
} // namespace

void Binder::validate(Widget& root, DiagnosticResult& result) {
    mValueControls.clear();
    collectValueControls(root, mValueControls);
    for (ValueControl* control : mValueControls) control->prepareValueBinding(*this);

    std::map<std::string, ActionEventKind> declared_kinds;
    std::vector<std::pair<Widget*, ActionEventKind>> declarations;
    collectActions(root, declared_kinds, declarations, result);

    std::map<std::string, ActionEventKind> registered_actions;
    for (PendingAction& pending : mPendingActions) {
        const auto inserted = registered_actions.emplace(pending.name, pending.kind);
        if (!inserted.second) result.error("binding.action.duplicate", "Controller action is registered more than once: " + pending.name + ".");
        const auto declared = declared_kinds.find(pending.name);
        if (declared != declared_kinds.end() && declared->second != pending.kind)
            result.error("binding.action.kind_mismatch", "Controller action kind does not match layout action: " + pending.name + ".");
    }

    for (const auto& declaration : declared_kinds) {
        const std::string& name = declaration.first;
        if (registered_actions.find(name) == registered_actions.end())
            result.warning("binding.action.unhandled", "Layout action has no controller handler: " + name + ".");
    }

    for (Pending& pending : mPending) {
        pending.resolved = findInScope(root, pending.id);
        if (pending.resolved && !pending.accepts(pending.resolved))
            result.error("binding.type.mismatch",
                         "Widget " + pending.id + " must be <" + pending.expected_type + ">, found <" + pending.resolved->element() + ">.");
    }

    std::map<std::string, PendingValueProvider*> value_providers;
    for (PendingValueProvider& pending : mPendingValueProviders) {
        if (!isLocalIdentifier(pending.id)) {
            result.error("binding.value.name_invalid", "Value binding name must be lowercase kebab-case: " + pending.id + ".");
            continue;
        }
        if (!pending.binding) {
            result.error("binding.value.null", "Value binding provider is empty: " + pending.id + ".");
            continue;
        }
        if (!value_providers.emplace(pending.id, &pending).second)
            result.error("binding.value.duplicate", "Value binding is provided more than once: " + pending.id + ".");
    }

    for (PendingValueRequirement& pending : mPendingValueRequirements) {
        if (!isLocalIdentifier(pending.id)) {
            result.error("binding.value.name_invalid", "Value binding name must be lowercase kebab-case: " + pending.id + ".");
            continue;
        }
        const auto found = value_providers.find(pending.id);
        if (found == value_providers.end()) {
            result.error("binding.value.missing", "Required value binding is missing: " + pending.id + ".");
            continue;
        }
        PendingValueProvider& provider = *found->second;
        if (provider.type != pending.type) {
            result.error("binding.value.type_mismatch",
                         "Value binding " + pending.id + " must be " + pending.type_name + ", found " + provider.type_name + ".");
            continue;
        }
        pending.resolved = provider.binding;
    }

    for (PendingScope& pending : mPendingScopes) {
        pending.resolved = findInScope(root, pending.id);
        if (!pending.resolved) {
            result.error("binding.scope.missing", "Included resource scope is missing: " + pending.id + ".");
            continue;
        }
        if (!pending.resolved->idScopeRoot()) {
            result.error("binding.scope.not_root", "Widget " + pending.id + " is not an included resource scope root.");
            continue;
        }
        pending.binder->validate(*pending.resolved, result);
    }
}

void Binder::commit(Widget& root, Binding& binding) {
    binding.mCommitted = true;
    std::map<std::string, ActionEventKind> declared_kinds;
    std::vector<std::pair<Widget*, ActionEventKind>> declarations;
    BindingResult unused;
    collectActions(root, declared_kinds, declarations, unused);

    for (Pending& pending : mPending) pending.commit(pending.resolved);
    for (PendingValueRequirement& pending : mPendingValueRequirements) pending.commit(pending.resolved);
    for (ValueControl* control : mValueControls) {
        ValueBindingSubscription subscription = control->commitValueBinding();
        if (subscription) binding.mValueSubscriptions.push_back(std::move(subscription));
    }
    for (PendingAction& pending : mPendingActions) {
        for (const auto& declaration : declarations) {
            Widget* widget = declaration.first;
            const ActionEventKind kind = declaration.second;
            if (kind == pending.kind && widget->action(kind) == pending.name) widget->bindAction(kind, pending.handler);
        }
        binding.mHandlers.push_back(std::move(pending.handler));
    }
    for (PendingScope& pending : mPendingScopes) pending.binder->commit(*pending.resolved, binding);
}

BindingResult Binder::finish() {
    PreparedBindingResult prepared = prepare();
    BindingResult result;
    result.warnings = std::move(prepared.warnings);
    result.errors = std::move(prepared.errors);
    if (prepared.ok()) result.binding = prepared.binding.commit();
    return result;
}

PreparedBindingResult Binder::prepare() {
    PreparedBindingResult result;
    if (!mRoot) {
        result.error("binding.scope.finish", "Nested Binder scopes commit through their root Binder.");
        return result;
    }
    if (mFinished) {
        result.error("binding.already_finished", "Binder transaction was already finished.");
        return result;
    }
    mFinished = true;

    validate(*mRoot, result);
    if (!result.hasErrors()) {
        auto binder = std::unique_ptr<Binder>(new Binder(std::move(*this)));
        result.binding.mBinder = std::move(binder);
    }
    return result;
}
} // namespace rdui
