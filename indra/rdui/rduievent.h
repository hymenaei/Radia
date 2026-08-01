/**
 * @file rduievent.h
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

#ifndef LL_RDUI_EVENT_H
#define LL_RDUI_EVENT_H

#include <cstdint>
#include "rduitypes.h"

namespace rdui {
class Surface;
class Widget;

enum class EventPhase : uint8_t { Capture, Target, Bubble };

enum class EventKind : uint8_t { PointerMove, PointerDown, PointerUp, Scroll, KeyDown, KeyUp, CharacterInput };

enum class PointerButton : uint8_t { NoButton, Left, Right, Middle, Auxiliary1, Auxiliary2 };

struct PointerEvent {
    Vec2 position;
    PointerButton button = PointerButton::NoButton;
    uint32_t modifiers = 0;
    uint8_t clickCount = 1;
    Vec2 delta;
};

using MouseEvent = PointerEvent;

struct ScrollEvent {
    Vec2 position;
    float dx = 0.f;
    float dy = 0.f;
    uint32_t modifiers = 0;
};

struct KeyEvent {
    int key = 0;
    uint32_t modifiers = 0;
    bool repeated = false;
};

class RoutedEvent {
    friend class Surface;

public:
    virtual ~RoutedEvent() = default;

    EventKind kind() const { return mKind; }
    EventPhase phase() const { return mPhase; }
    Widget& target() const { return mTarget; }
    Widget* currentTarget() const { return mCurrentTarget; }
    bool handled() const { return mHandled; }
    bool defaultPrevented() const { return mDefaultPrevented; }
    bool propagationStopped() const { return mPropagationStopped; }

    void markHandled() { mHandled = true; }
    void preventDefault() { mDefaultPrevented = true; }
    void stopPropagation() { mPropagationStopped = true; }

protected:
    RoutedEvent(EventKind kind, Widget& target) : mKind(kind), mTarget(target) {}

private:
    EventKind mKind;
    EventPhase mPhase = EventPhase::Target;
    Widget& mTarget;
    Widget* mCurrentTarget = nullptr;
    bool mHandled = false;
    bool mDefaultPrevented = false;
    bool mPropagationStopped = false;
};

class RoutedPointerEvent final : public RoutedEvent {
public:
    RoutedPointerEvent(EventKind kind, Widget& target, PointerEvent pointer) : RoutedEvent(kind, target), pointer(pointer) {}

    PointerEvent pointer;
};

class RoutedScrollEvent final : public RoutedEvent {
public:
    RoutedScrollEvent(Widget& target, ScrollEvent scroll) : RoutedEvent(EventKind::Scroll, target), scroll(scroll) {}

    ScrollEvent scroll;
};

class RoutedKeyEvent final : public RoutedEvent {
public:
    RoutedKeyEvent(EventKind kind, Widget& target, KeyEvent key) : RoutedEvent(kind, target), key(key) {}

    KeyEvent key;
};

class RoutedCharacterEvent final : public RoutedEvent {
public:
    RoutedCharacterEvent(Widget& target, unsigned int codepoint) : RoutedEvent(EventKind::CharacterInput, target), codepoint(codepoint) {}

    unsigned int codepoint;
};

constexpr int KEY_RETURN = 13;
constexpr int KEY_SPACE = 32;
constexpr int KEY_TAB = 9;

constexpr uint32_t MODIFIER_SHIFT = 1 << 0;
constexpr uint32_t MODIFIER_CONTROL = 1 << 1;
constexpr uint32_t MODIFIER_ALT = 1 << 2;
constexpr uint32_t MODIFIER_PLATFORM_CONTROL = 1 << 3;

inline bool isActivationKey(int key) {
    return key == KEY_RETURN || key == KEY_SPACE;
}
} // namespace rdui
#endif // LL_RDUI_EVENT_H
