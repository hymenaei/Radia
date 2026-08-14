/**
 * @file componentcontrollerinternal.h
 * @brief Private transaction types used by the viewer ComponentController implementation.
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

#ifndef RD_COMPONENTCONTROLLER_INTERNAL_H
#define RD_COMPONENTCONTROLLER_INTERNAL_H

#include <map>
#include <memory>
#include <vector>
#include "binding/binder.h"
#include "componentcontroller.h"

namespace rdui::viewer {
class ComponentController::PreparedWidgets final {
public:
    PreparedWidgets() = default;
    ~PreparedWidgets() = default;
    PreparedWidgets(PreparedWidgets&&) noexcept = default;
    PreparedWidgets& operator=(PreparedWidgets&&) noexcept = default;
    PreparedWidgets(const PreparedWidgets&) = delete;
    PreparedWidgets& operator=(const PreparedWidgets&) = delete;

    explicit operator bool() const { return mController != nullptr; }

private:
    friend class ComponentController;
    friend class ComponentManager;

    struct Target {
        std::shared_ptr<detail::ControllerWidgetSlot> slot;
        rdui::Widget* widget = nullptr;
    };

    ComponentController* mController = nullptr;
    rdui::Widget* mRoot = nullptr;
    std::map<std::string, rdui::Widget*> index;
    std::vector<Target> mTargets;
};

class ComponentController::PreparedMount final {
public:
    PreparedMount();
    ~PreparedMount();
    PreparedMount(PreparedMount&&) noexcept;
    PreparedMount& operator=(PreparedMount&&) noexcept;
    PreparedMount(const PreparedMount&) = delete;
    PreparedMount& operator=(const PreparedMount&) = delete;

    explicit operator bool() const;

private:
    friend class ComponentController;
    friend class ComponentManager;

    struct State;
    std::unique_ptr<State> mState;
};

struct ComponentController::PreparedMountResult : DiagnosticResult {
    bool ok() const;
    PreparedMount mount;
};
} // namespace rdui::viewer

#endif // RD_COMPONENTCONTROLLER_INTERNAL_H
