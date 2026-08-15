/**
 * @file componentcontroller.h
 * @brief Defines the viewer ComponentController contract for mounted UI roots.
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

#ifndef RD_COMPONENTCONTROLLER_H
#define RD_COMPONENTCONTROLLER_H

#include <memory>
#include <string>
#include <string_view>
#include "diagnostic.h"
#include "text/source.h"

namespace radia::ui {
class Widget;
class System;
class SettingResolver;
struct EventRegistrationDescriptor;
} // namespace radia::ui

namespace radia::viewer::ui {
using namespace ::radia::ui;
class ComponentManager;
class Widget;
namespace detail { struct ControllerWidgetSlot; }

class ComponentController {
public:
    explicit ComponentController(radia::ui::System& system);
    virtual ~ComponentController();

    virtual void postBuild() {}
    virtual void onOpen() {}
    virtual void onClose() {}
    virtual void onReloadSucceeded() {}
    virtual void onReloadFailed(const radia::ui::DiagnosticResult&) {}

protected:
    Widget& getWidgetById(std::string_view id);
    radia::ui::TextSource localize(std::string id) const;

    template<typename Callback> void event(std::string name, Callback callback);
    template<typename ControllerT, typename... Args> void event(std::string name, void (ControllerT::*method)(Args...));
    template<typename ControllerT, typename... Args> void event(std::string name, void (ControllerT::*method)(Args...) const);

private:
    class PreparedWidgets;
    class PreparedMount;
    struct PreparedMountResult;

    PreparedWidgets prepareWidgets(radia::ui::Widget& root, radia::ui::DiagnosticResult& result);
    void commitWidgets(PreparedWidgets&& prepared);
    PreparedMountResult prepare(radia::ui::Widget& root, radia::ui::SettingResolver& settingResolver);
    bool canCommit(const PreparedMount& prepared, const radia::ui::Widget& root) const;
    void commit(PreparedMount&& prepared);
    void addEventRegistration(radia::ui::EventRegistrationDescriptor registration);

    radia::ui::System& mSystem;
    struct Impl;
    std::unique_ptr<Impl> mImpl;

    friend class ComponentManager;
};
} // namespace radia::viewer::ui
#endif // RD_COMPONENTCONTROLLER_H
