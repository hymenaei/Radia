/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <variant>
#include "types.h"

namespace radia::ui {
class Element;
class Surface;
class Event;

enum class EventPhase : uint8_t { Capture, Target, Bubble };

enum class PointerButton : uint8_t { NoButton, Left, Right, Middle, Auxiliary1, Auxiliary2 };

struct PointerEvent {
    Vec2 position;
    PointerButton button = PointerButton::NoButton;
    uint32_t modifiers = 0;
    uint8_t clickCount = 1;
    Vec2 delta;
};

struct WheelEvent {
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

inline constexpr std::string_view kClickEvent = "click";
inline constexpr std::string_view kDoubleClickEvent = "dblclick";
inline constexpr std::string_view kInputEvent = "input";
inline constexpr std::string_view kChangeEvent = "change";
inline constexpr std::string_view kPointerDownEvent = "pointerdown";
inline constexpr std::string_view kPointerUpEvent = "pointerup";
inline constexpr std::string_view kPointerMoveEvent = "pointermove";
inline constexpr std::string_view kContextMenuEvent = "contextmenu";
inline constexpr std::string_view kWheelEvent = "wheel";
inline constexpr std::string_view kScrollEvent = "scroll";
inline constexpr std::string_view kKeyDownEvent = "keydown";
inline constexpr std::string_view kKeyUpEvent = "keyup";
inline constexpr std::string_view kCharacterInputEvent = "characterinput";

class EventHandler final {
public:
    using Callback = std::function<void(Event&)>;

    EventHandler() = default;

    template<typename Callable, typename = std::enable_if_t<!std::is_same_v<std::decay_t<Callable>, EventHandler>>> EventHandler(Callable&& callback)
        : mState(std::make_shared<State>(Callback(std::forward<Callable>(callback)))) {}

    void operator()(Event& event) const {
        if (mState) mState->callback(event);
    }

    explicit operator bool() const noexcept { return static_cast<bool>(mState); }

    friend bool operator==(const EventHandler& left, const EventHandler& right) noexcept { return left.mState == right.mState; }
    friend bool operator!=(const EventHandler& left, const EventHandler& right) noexcept { return !(left == right); }

private:
    struct State {
        explicit State(Callback callback) : callback(std::move(callback)) {}

        Callback callback;
    };

    std::shared_ptr<State> mState;
};

class Event final {
    friend class Element;
    friend class Surface;

    using Payload = std::variant<std::monostate, PointerEvent, WheelEvent, KeyEvent, bool, unsigned int>;

public:
    std::string_view type() const noexcept { return mType; }
    EventPhase phase() const noexcept { return mPhase; }
    Element& target() const noexcept { return mTarget; }
    Element* currentTarget() const noexcept { return mCurrentTarget; }
    bool handled() const noexcept { return mHandled; }
    bool cancelable() const noexcept { return mCancelable; }
    bool defaultPrevented() const noexcept { return mDefaultPrevented; }
    bool propagationStopped() const noexcept { return mPropagationStopped; }
    bool immediatePropagationStopped() const noexcept { return mImmediatePropagationStopped; }

    const PointerEvent* pointer() const noexcept { return std::get_if<PointerEvent>(&mPayload); }
    const WheelEvent* wheel() const noexcept { return std::get_if<WheelEvent>(&mPayload); }
    const KeyEvent* key() const noexcept { return std::get_if<KeyEvent>(&mPayload); }
    bool checked() const { return std::get<bool>(mPayload); }
    std::optional<unsigned int> character() const noexcept {
        if (const unsigned int* value = std::get_if<unsigned int>(&mPayload)) return *value;
        return std::nullopt;
    }

    void markHandled() noexcept { mHandled = true; }
    void preventDefault() noexcept {
        if (mCancelable) mDefaultPrevented = true;
    }
    void stopPropagation() noexcept { mPropagationStopped = true; }
    void stopImmediatePropagation() noexcept {
        mImmediatePropagationStopped = true;
        mPropagationStopped = true;
    }

    Event(std::string_view type, Element& target) : mType(type), mTarget(target) {}
    Event(std::string_view type, Element& target, PointerEvent payload) : Event(type, target, Payload(std::move(payload))) {}
    Event(std::string_view type, Element& target, WheelEvent payload) : Event(type, target, Payload(std::move(payload))) {}
    Event(std::string_view type, Element& target, KeyEvent payload) : Event(type, target, Payload(std::move(payload))) {}
    Event(std::string_view type, Element& target, bool payload) : Event(type, target, Payload(payload)) {}
    Event(std::string_view type, Element& target, unsigned int payload) : Event(type, target, Payload(payload)) {}

private:
    Event(std::string_view type, Element& target, Payload payload) : mType(type), mTarget(target), mPayload(std::move(payload)) {}

    void setPhase(EventPhase phase) noexcept { mPhase = phase; }
    void setCurrentTarget(Element* target) noexcept { mCurrentTarget = target; }
    void setCancelable(bool cancelable) noexcept { mCancelable = cancelable; }

    std::string mType;
    Element& mTarget;
    Element* mCurrentTarget = nullptr;
    Payload mPayload;
    EventPhase mPhase = EventPhase::Target;
    bool mHandled = false;
    bool mCancelable = true;
    bool mDefaultPrevented = false;
    bool mPropagationStopped = false;
    bool mImmediatePropagationStopped = false;
};

constexpr int kKeyReturn = 13;
constexpr int kKeySpace = 32;
constexpr int kKeyTab = 9;
constexpr int kKeyLeft = 0x82;
constexpr int kKeyRight = 0x83;
constexpr int kKeyUp = 0x84;
constexpr int kKeyDown = 0x85;
constexpr int kKeyHome = 0x8C;
constexpr int kKeyEnd = 0x8D;
constexpr int kKeyPageUp = 0x8E;
constexpr int kKeyPageDown = 0x8F;

constexpr uint32_t kModifierShift = 1 << 0;
constexpr uint32_t kModifierControl = 1 << 1;
constexpr uint32_t kModifierAlt = 1 << 2;
constexpr uint32_t kModifierPlatformControl = 1 << 3;

inline bool isActivationKey(int key) {
    return key == kKeyReturn || key == kKeySpace;
}
} // namespace radia::ui
