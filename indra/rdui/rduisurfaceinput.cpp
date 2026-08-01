/**
 * @file rduisurfaceinput.cpp
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
#include <algorithm>
#include <iterator>
#include <optional>
#include <vector>
#include "rdfloater.h"
#include "rduifloaterresize.h"
#include "rduisurface.h"
#include "rduisystem.h"

namespace rdui {
namespace {
bool acceptsPointerEvents(const Widget& widget, const StyleSheet& style_sheet) {
    const PointerEvents policy = resolveWidgetStyle(style_sheet, widget).pointer_events;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return widget.pointerEvents();
}

Widget* hitTest(Widget& node, const Vec2& point, const StyleSheet& style_sheet, const Rect& inherited_clip) {
    if (node.visibility() != Visibility::Visible || !inherited_clip.contains(point)) return nullptr;
    const Style style = resolveWidgetStyle(style_sheet, node);
    const bool clips_x = style.overflow_x == Overflow::Hidden;
    const bool clips_y = style.overflow_y == Overflow::Hidden;
    const ClipAxes clip_axes = (clips_x ? ClipAxes::X : ClipAxes::NoAxes) | (clips_y ? ClipAxes::Y : ClipAxes::NoAxes);
    const Rect child_clip = clip_axes == ClipAxes::NoAxes ? inherited_clip : clipToAxes(inherited_clip, node.rect(), clip_axes);
    for (auto child = node.children().rbegin(); child != node.children().rend(); ++child)
        if (Widget* hit = hitTest(**child, point, style_sheet, child_clip)) return hit;
    return node.rect().contains(point) && acceptsPointerEvents(node, style_sheet) ? &node : nullptr;
}

void collectFocusable(Widget& node, std::vector<Widget*>& result) {
    if (node.visibility() != Visibility::Visible || node.disabled()) return;
    if (node.focusable()) result.push_back(&node);
    for (const auto& child : node.children()) collectFocusable(*child, result);
}
} // namespace

bool Surface::routeEvent(RoutedEvent& event) {
    std::vector<Widget*> route;
    for (Widget* current = &event.target(); current; current = current->parent()) {
        route.push_back(current);
        if (isSurfaceRoot(current)) break;
    }
    if (route.empty() || !isSurfaceRoot(route.back())) return false;

    event.mPhase = EventPhase::Capture;
    for (auto current = route.rbegin(); current != route.rend() - 1; ++current) {
        event.mCurrentTarget = *current;
        (*current)->onEvent(event);
        if (event.propagationStopped()) {
            event.mCurrentTarget = nullptr;
            return event.handled() || event.defaultPrevented();
        }
    }

    event.mPhase = EventPhase::Target;
    event.mCurrentTarget = route.front();
    route.front()->onEvent(event);
    if (!event.propagationStopped()) {
        event.mPhase = EventPhase::Bubble;
        for (auto current = std::next(route.begin()); current != route.end(); ++current) {
            event.mCurrentTarget = *current;
            (*current)->onEvent(event);
            if (event.propagationStopped()) break;
        }
    }
    event.mCurrentTarget = nullptr;
    return event.handled() || event.defaultPrevented();
}

bool Surface::hasActiveModal() const {
    const Widget& modal = layerRoot(SurfaceLayer::Modal);
    return std::any_of(modal.children().begin(), modal.children().end(),
                       [](const auto& child) { return child->visibility() == Visibility::Visible; });
}

Widget* Surface::hitTestAt(const Vec2& point) {
    if (!mViewport.contains(point)) return nullptr;
    if (hasActiveModal()) return hitTest(layerRoot(SurfaceLayer::Modal), point, *mStyleSheet, mViewport);

    for (std::size_t index = static_cast<std::size_t>(SurfaceLayer::Modal); index > static_cast<std::size_t>(SurfaceLayer::Content); --index) {
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index);
        if (layer == SurfaceLayer::Tooltip || layer == SurfaceLayer::Drag || layer == SurfaceLayer::Modal) continue;
        if (Widget* hit = hitTest(layerRoot(layer), point, *mStyleSheet, mViewport)) return hit;
    }
    return hitTest(*mRoot, point, *mStyleSheet, mViewport);
}

void Surface::clearInteractionState() {
    if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, false);
    if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
    clearKeyboardPress();
    if (Widget* focused = mFocused.get()) {
        focused->setState(WidgetState::Focused, false);
        focused->setState(WidgetState::FocusVisible, false);
    }
    if (Widget* captured = mCaptured.get()) captured->endPointerInteraction({mPointerPosition});
    mHovered.set(nullptr);
    mPressed.set(nullptr);
    mFocused.set(nullptr);
    mCaptured.set(nullptr);
    mResizeCursor = CursorStyle::Auto;
    resetLongClick();
    mPressedClickCount = 0;
    mTabKeyHandled = false;
}

CursorStyle Surface::cursor() const {
    if (mResizeCursor != CursorStyle::Auto) return mResizeCursor;
    const Widget* widget = mCaptured ? mCaptured.get() : mHovered.get();
    if (!widget) return CursorStyle::Default;
    const CursorStyle cursor = resolveWidgetStyle(*mStyleSheet, *widget).cursor;
    return cursor == CursorStyle::Auto ? CursorStyle::Default : cursor;
}

bool Surface::pointerMove(const PointerEvent& event) {
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    if (Widget* captured = mCaptured.get()) {
        RoutedPointerEvent routed(EventKind::PointerMove, *captured, event);
        const bool routed_handled = routeEvent(routed);
        const bool handled = !routed.defaultPrevented() && captured->updatePointerInteraction(event);
        captured->dispatchMouseAction(ActionEventKind::MouseMove, event);
        return routed_handled || handled;
    }
    updateResizeCursor(event.position);
    refreshHover();
    if (!mLongClickFired && mLongClickTarget && mHovered.get() != mLongClickTarget.get()) resetLongClick();
    if (Widget* hovered = mHovered.get()) {
        if (isEnabledInTree(hovered)) {
            RoutedPointerEvent routed(EventKind::PointerMove, *hovered, event);
            routeEvent(routed);
            hovered->dispatchMouseAction(ActionEventKind::MouseMove, event);
        }
    }
    return mHovered.get() != nullptr || mPressed.get() != nullptr;
}

void Surface::pointerLeave() {
    mPointerPositionKnown = false;
    if (!mCaptured) mResizeCursor = CursorStyle::Auto;
    setHovered(nullptr);
    updatePressedState();
    if (!mLongClickFired) resetLongClick();
}

bool Surface::pointerDown(const PointerEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    std::uint8_t resize_edges = 0;
    Floater* resize_floater = event.button == PointerButton::Left ? resizeFloaterAt(event.position, resize_edges) : nullptr;
    if (resize_floater) {
        const SurfaceLayer layer = resize_floater->parent() == &layerRoot(SurfaceLayer::Modal) ? SurfaceLayer::Modal : SurfaceLayer::Floater;
        raiseWithinLayer(*resize_floater, layer);
        resetLongClick();
        mPressedClickCount = 0;
        clearKeyboardPress();
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
        mPressed.set(nullptr);
        const bool native = mFloaterDelegate && mFloaterDelegate->beginNativeFloaterResize(*this, *resize_floater);
        const std::optional<Rect> bounds = native ? std::nullopt : std::optional<Rect>(mViewport);
        if (resize_floater->beginResizeInteraction(event, resize_edges, minimumFloaterSize(*resize_floater), bounds)) {
            mCaptured.set(resize_floater);
            setHovered(resize_floater);
            setFocused(nullptr, false);
            mResizeCursor = detail::resizeCursor(static_cast<detail::ResizeEdges>(resize_edges));
            return true;
        }
    }
    Widget* hit = hitTestAt(event.position);
    if (hit) raiseWithinLayer(*hit, SurfaceLayer::Floater);
    setHovered(hit);
    bool default_prevented = false;
    if (isEnabledInTree(hit)) {
        RoutedPointerEvent routed(EventKind::PointerDown, *hit, event);
        routeEvent(routed);
        default_prevented = routed.defaultPrevented();
    }
    if (event.button != PointerButton::Left) {
        if (isEnabledInTree(hit)) hit->dispatchMouseAction(ActionEventKind::MouseDown, event);
        return hit != nullptr;
    }
    resetLongClick();
    mPressedClickCount = 0;
    clearKeyboardPress();
    if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
    mPressed.set(nullptr);
    const bool enabled_control = isEnabledInTree(hit);
    for (Widget* candidate = enabled_control && !default_prevented ? hit : nullptr; candidate; candidate = candidate->parent()) {
        if (!candidate->beginPointerInteraction(event)) continue;
        mCaptured.set(candidate);
        setFocused(nullptr, false);
        candidate->dispatchMouseAction(ActionEventKind::MouseDown, event);
        return true;
    }
    setFocused(enabled_control && !default_prevented && hit->focusable() ? hit : nullptr, false);
    mPressed.set(enabled_control && !default_prevented ? hit : nullptr);
    mPressedClickCount = mPressed ? event.clickCount : 0;
    updatePressedState();
    if (enabled_control && !default_prevented && !hit->action(ActionEventKind::LongClick).empty()) mLongClickTarget.set(hit);
    if (enabled_control) hit->dispatchMouseAction(ActionEventKind::MouseDown, event);
    return hit != nullptr;
}

bool Surface::pointerUp(const PointerEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    if (event.button != PointerButton::Left) {
        WidgetRef<Widget> hit(hitTestAt(event.position));
        const bool had_hit = !!hit;
        if (isEnabledInTree(hit.get())) {
            RoutedPointerEvent routed(EventKind::PointerUp, *hit, event);
            routeEvent(routed);
            hit->dispatchMouseAction(ActionEventKind::MouseUp, event);
            if (hit && event.button == PointerButton::Right) hit->dispatchMouseAction(ActionEventKind::ContextMenu, event);
        }
        return had_hit;
    }
    if (Widget* captured = mCaptured.get()) {
        resetLongClick();
        mPressedClickCount = 0;
        mCaptured.set(nullptr);
        RoutedPointerEvent routed(EventKind::PointerUp, *captured, event);
        const bool routed_handled = routeEvent(routed);
        const bool handled = !routed.defaultPrevented() && captured->endPointerInteraction(event);
        captured->dispatchMouseAction(ActionEventKind::MouseUp, event);
        refreshHover();
        return routed_handled || handled;
    }
    WidgetRef<Widget> released(mPressed.get());
    WidgetRef<Widget> hit(hitTestAt(event.position));
    bool default_prevented = false;
    if (Widget* target = released ? released.get() : hit.get()) {
        RoutedPointerEvent routed(EventKind::PointerUp, *target, event);
        routeEvent(routed);
        default_prevented = routed.defaultPrevented();
    }
    const bool suppress_click = mLongClickFired;
    const uint8_t click_count = mPressedClickCount;
    if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
    mPressed.set(nullptr);
    setHovered(hit.get());
    if (released && isEnabledInTree(released.get())) released->dispatchMouseAction(ActionEventKind::MouseUp, event);
    resetLongClick();
    mPressedClickCount = 0;
    const bool clicked = !suppress_click && released && released.get() == hit.get() && !default_prevented;
    if (clicked) {
        released->activate();
        if (released && click_count >= 2) {
            PointerEvent double_click = event;
            double_click.clickCount = click_count;
            released->dispatchMouseAction(ActionEventKind::DoubleClick, double_click);
        }
        refreshHover();
        return true;
    }
    return released || hit;
}

bool Surface::scroll(const ScrollEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    Widget* hit = hitTestAt(event.position);
    bool routed_handled = false;
    bool default_prevented = false;
    if (isEnabledInTree(hit)) {
        RoutedScrollEvent routed(*hit, event);
        routed_handled = routeEvent(routed);
        default_prevented = routed.defaultPrevented();
    }
    for (Widget* candidate = default_prevented ? nullptr : hit; candidate; candidate = candidate->parent())
        if (candidate->defaultScroll(event)) return true;
    return routed_handled || hit != nullptr;
}

bool Surface::keyDown(const KeyEvent& event) {
    validateFocus();
    if (event.key == KEY_TAB && (event.modifiers & ~MODIFIER_SHIFT) == 0) {
        mTabKeyHandled = moveFocus((event.modifiers & MODIFIER_SHIFT) != 0);
        return mTabKeyHandled;
    }
    Widget* focused = mFocused.get();
    if (!focused) return false;
    RoutedKeyEvent routed(EventKind::KeyDown, *focused, event);
    const bool routed_handled = routeEvent(routed);
    if (routed.defaultPrevented()) return routed_handled;
    if (isActivationKey(event.key)) {
        if (mKeyPressed.get() && (mKeyPressed.get() != focused || mPressedKey != event.key)) clearKeyboardPress();
        if (!focused->defaultKeyDown(event)) return routed_handled;
        mKeyPressed.set(focused);
        mPressedKey = event.key;
        return true;
    }
    return focused->defaultKeyDown(event) || routed_handled;
}

bool Surface::keyUp(const KeyEvent& event) {
    if (event.key == KEY_TAB) {
        const bool handled = mTabKeyHandled;
        mTabKeyHandled = false;
        return handled;
    }
    validateFocus();
    Widget* focused = mFocused.get();
    if (!focused) return false;
    RoutedKeyEvent routed(EventKind::KeyUp, *focused, event);
    const bool routed_handled = routeEvent(routed);
    if (routed.defaultPrevented()) {
        if (isActivationKey(event.key)) clearKeyboardPress();
        return routed_handled;
    }
    if (isActivationKey(event.key)) {
        if (mKeyPressed.get() != focused || mPressedKey != event.key) return false;
        mKeyPressed.set(nullptr);
        mPressedKey = 0;
    }
    const bool handled = focused->defaultKeyUp(event);
    if (handled) refreshHover();
    return handled || routed_handled;
}

bool Surface::charInput(unsigned int codepoint) {
    validateFocus();
    Widget* focused = mFocused.get();
    if (!focused) return false;
    RoutedCharacterEvent routed(*focused, codepoint);
    const bool routed_handled = routeEvent(routed);
    return routed_handled || (!routed.defaultPrevented() && focused->defaultCharacterInput(codepoint));
}

void Surface::refreshHover() {
    updateLayout();
    if (!mPointerPositionKnown || !mRoot) return;
    updateResizeCursor(mPointerPosition);
    setHovered(hitTestAt(mPointerPosition));
    updatePressedState();
}

void Surface::update(std::chrono::milliseconds elapsed) {
    if (elapsed.count() <= 0 || mLongClickFired) return;
    WidgetRef<Widget> target(mLongClickTarget.get());
    if (!target || mPressed.get() != target.get() || mHovered.get() != target.get() || !isEnabledInTree(target.get())) {
        resetLongClick();
        return;
    }
    mLongClickElapsed += elapsed;
    const std::chrono::milliseconds delay = target->longClickDelay().value_or(defaultLongClickDelay());
    if (mLongClickElapsed < delay) return;
    mLongClickFired = true;
    target->dispatchLongClickAction(mLongClickElapsed);
}

void Surface::setHovered(Widget* node) {
    if (mHovered.get() == node) return;
    if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, false);
    mHovered.set(node);
    if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, true);
}

void Surface::setFocused(Widget* node, bool focus_visible) {
    if (mFocused.get() == node) {
        if (node) node->setState(WidgetState::FocusVisible, focus_visible);
        return;
    }
    if (Widget* focused = mFocused.get()) {
        clearKeyboardPress();
        focused->setState(WidgetState::Focused, false);
        focused->setState(WidgetState::FocusVisible, false);
    }
    mFocused.set(node);
    if (Widget* focused = mFocused.get()) {
        focused->setState(WidgetState::Focused, true);
        focused->setState(WidgetState::FocusVisible, focus_visible);
    }
}

bool Surface::isEnabledInTree(const Widget* node) const {
    if (!node) return false;
    for (const Widget* current = node; current; current = current->parent()) {
        if (current->visibility() != Visibility::Visible || current->disabled()) return false;
        if (isSurfaceRoot(current)) return true;
    }
    return false;
}

bool Surface::isFocusableInTree(const Widget* node) const {
    return node && node->focusable() && isEnabledInTree(node);
}

void Surface::validateFocus() {
    Widget* focused = mFocused.get();
    if (focused && hasActiveModal()) {
        const Widget* current = focused;
        const Widget* modal_root = &layerRoot(SurfaceLayer::Modal);
        while (current && current != modal_root) current = current->parent();
        if (current != modal_root) {
            setFocused(nullptr, false);
            return;
        }
    }
    if (focused && !isFocusableInTree(focused)) setFocused(nullptr, false);
}

void Surface::clearKeyboardPress() {
    if (Widget* pressed = mKeyPressed.get()) pressed->setState(WidgetState::Active, false);
    mKeyPressed.set(nullptr);
    mPressedKey = 0;
}

void Surface::updatePressedState() {
    if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, mHovered.get() == pressed);
}

void Surface::widgetBecameUnavailable(Widget&) {
    if (Widget* captured = mCaptured.get(); captured && !isEnabledInTree(captured)) {
        mCaptured.set(nullptr);
        captured->endPointerInteraction({mPointerPosition});
        mResizeCursor = CursorStyle::Auto;
    }
    if (Widget* pressed = mPressed.get(); pressed && !isEnabledInTree(pressed)) {
        pressed->setState(WidgetState::Active, false);
        mPressed.set(nullptr);
    }
    if (Widget* hovered = mHovered.get(); hovered && !isEnabledInTree(hovered)) setHovered(nullptr);
    if (Widget* key_pressed = mKeyPressed.get(); key_pressed && !isEnabledInTree(key_pressed)) clearKeyboardPress();
    if (Widget* target = mLongClickTarget.get(); target && !isEnabledInTree(target)) resetLongClick();
    validateFocus();
}

void Surface::resetLongClick() {
    mLongClickTarget.set(nullptr);
    mLongClickElapsed = std::chrono::milliseconds(0);
    mLongClickFired = false;
}

std::chrono::milliseconds Surface::defaultLongClickDelay() const {
    return mSystem ? mSystem->longClickDelay() : std::chrono::milliseconds(500);
}

bool Surface::moveFocus(bool backwards) {
    if (!mRoot) return false;
    std::vector<Widget*> focusable;
    if (hasActiveModal()) collectFocusable(layerRoot(SurfaceLayer::Modal), focusable);
    else {
        collectFocusable(*mRoot, focusable);
        collectFocusable(layerRoot(SurfaceLayer::Floater), focusable);
        collectFocusable(layerRoot(SurfaceLayer::Popup), focusable);
    }
    if (focusable.empty()) return false;

    const auto current = std::find(focusable.begin(), focusable.end(), mFocused.get());
    Widget* next = nullptr;
    if (current == focusable.end()) next = backwards ? focusable.back() : focusable.front();
    else if (backwards) next = current == focusable.begin() ? focusable.back() : *(current - 1);
    else next = std::next(current) == focusable.end() ? focusable.front() : *std::next(current);
    setFocused(next, true);
    return true;
}
} // namespace rdui
