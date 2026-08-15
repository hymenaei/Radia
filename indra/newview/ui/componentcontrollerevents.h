/**
 * @file componentcontrollerevents.h
 * @brief Defines the viewer ComponentController Event Handler facades.
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

#ifndef RD_COMPONENTCONTROLLER_EVENTS_H
#define RD_COMPONENTCONTROLLER_EVENTS_H

#include <chrono>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include "binding/eventdescriptor.h"
#include "eventcall.h"
#include "text/source.h"
#include "widgetevent.h"

namespace radia::viewer::ui {
using namespace ::radia::ui;
namespace detail {
struct ControllerWidgetSlot;
template<typename T> struct ControllerEventArgumentAdapter;
} // namespace detail

class ComponentController;

using ComponentControllerEventRegistration = radia::ui::EventRegistrationDescriptor;

class Widget final {
public:
    Widget() = default;

    explicit operator bool() const noexcept;
    std::string_view id() const noexcept;

    Widget& setContent(radia::ui::TextSource content);
    Widget& setDisabled(bool disabled);
    Widget& setHidden(bool hidden);
    Widget& setChecked(bool checked);
    bool isDisabled() const noexcept;
    bool isChecked() const;

private:
    friend class ComponentController;
    friend class Event;
    friend struct detail::ControllerEventArgumentAdapter<Widget>;
    explicit Widget(std::shared_ptr<detail::ControllerWidgetSlot> slot) : mSlot(std::move(slot)) {}
    static Widget fromEventSource(radia::ui::Widget& source);
    radia::ui::Widget* runtimeWidget() const noexcept;

    std::shared_ptr<detail::ControllerWidgetSlot> mSlot;
};

class Event {
public:
    Widget source() const noexcept { return mSource; }
    radia::ui::WidgetEventKind kind() const noexcept { return mKind; }

protected:
    explicit Event(const radia::ui::WidgetEvent& event) : mSource(Widget::fromEventSource(event.source)), mKind(event.kind) {}

private:
    friend struct detail::ControllerEventArgumentAdapter<Event>;
    Widget mSource;
    radia::ui::WidgetEventKind mKind = radia::ui::WidgetEventKind::Click;
};

class ClickEvent final : public Event {
private:
    friend struct detail::ControllerEventArgumentAdapter<ClickEvent>;
    explicit ClickEvent(const radia::ui::WidgetEvent& event) : Event(event) {}
};

class ChangeEvent final : public Event {
public:
    bool checked = false;

private:
    friend struct detail::ControllerEventArgumentAdapter<ChangeEvent>;
    explicit ChangeEvent(const radia::ui::ChangeEvent& event) : Event(event), checked(event.checked) {}
};

class MouseWidgetEvent final : public Event {
public:
    const radia::ui::MouseEvent& mouse() const noexcept { return mMouse; }

private:
    friend struct detail::ControllerEventArgumentAdapter<MouseWidgetEvent>;
    explicit MouseWidgetEvent(const radia::ui::MouseWidgetEvent& event) : Event(event), mMouse(event.mouse) {}
    radia::ui::MouseEvent mMouse;
};

class LongClickEvent final : public Event {
public:
    std::chrono::milliseconds heldFor() const noexcept { return mHeldFor; }

private:
    friend struct detail::ControllerEventArgumentAdapter<LongClickEvent>;
    explicit LongClickEvent(const radia::ui::LongClickEvent& event) : Event(event), mHeldFor(event.heldFor) {}
    std::chrono::milliseconds mHeldFor{0};
};
} // namespace radia::viewer::ui
#endif // RD_COMPONENTCONTROLLER_EVENTS_H
