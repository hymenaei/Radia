/**
 * @file input.cpp
 * @brief Routes surface pointer, keyboard, scroll, focus, and interaction events.
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
#include "style/stylepass.h"
#include "surface/floaterresize.h"
#include "surface/ordering.h"
#include "surface/surface.h"
#include "system.h"
#include "widgets/floater.h"

namespace rdui {
namespace {
bool acceptsPointerEvents(const Widget& widget, const Style& style) {
    const PointerEvents policy = style.pointerEvents;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return widget.pointerEvents();
}

void collectFocusable(Widget& node, std::vector<WidgetRef<Widget>>& result, StylePass& styles) {
    if (node.visibility() != Visibility::Visible || node.disabled()) return;
    const WidgetRef<Widget> lifetime(&node);
    const Widget* parent = node.parent();
    const bool focusable = node.focusable();
    Widget* current = lifetime.get();
    if (!current || current->parent() != parent || current->visibility() != Visibility::Visible || current->disabled()) return;
    if (focusable) result.emplace_back(current);
    const StylePass::ChildSnapshot children = sourceChildren(*current, styles);
    for (const WidgetRef<Widget>& child_ref : *children)
        if (Widget* child = child_ref.get(); child && child->parent() == current) collectFocusable(*child, result, styles);
}
} // namespace

Widget* Surface::hitTestNode(Widget& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const {
    if (node.visibility() != Visibility::Visible || !inheritedClip.contains(point) || !isRootedInSurface(&node)) return nullptr;
    const WidgetRef<Widget> lifetime(&node);
    const Widget* originalParent = node.parent();
    const std::uint64_t originalStyleRevision = node.mStyleRevision;
    const std::uint64_t originalLayoutRevision = node.mLayoutInvalidationRevision;
    const Style& style = styles.style(node);
    Widget* styled_node = lifetime.get();
    if (!styled_node
        || !isRootedInSurface(styled_node)
        || styled_node->visibility() != Visibility::Visible
        || styled_node->parent() != originalParent
        || styled_node->mStyleRevision != originalStyleRevision
        || styled_node->mLayoutInvalidationRevision != originalLayoutRevision)
        return nullptr;
    const bool clips_x = style.overflowX == Overflow::Hidden;
    const bool clips_y = style.overflowY == Overflow::Hidden;
    const ClipAxes clip_axes = (clips_x ? ClipAxes::X : ClipAxes::NoAxes) | (clips_y ? ClipAxes::Y : ClipAxes::NoAxes);
    const Rect child_clip = clip_axes == ClipAxes::NoAxes ? inheritedClip : clipToAxes(inheritedClip, node.rect(), clip_axes);
    const StylePass::ChildSnapshot children = sourceChildren(node, styles);
    Widget* hit_result = nullptr;
    for (auto child = children->rbegin(); child != children->rend(); ++child)
        if (Widget* child_widget = child->get())
            if (child_widget->parent() == &node)
                if (Widget* hit = hitTestNode(*child_widget, point, child_clip, styles)) {
                    hit_result = hit;
                    break;
                }
    Widget* current = lifetime.get();
    if (!current
        || !isRootedInSurface(current)
        || current->visibility() != Visibility::Visible
        || current->parent() != originalParent
        || current->mStyleRevision != originalStyleRevision
        || current->mLayoutInvalidationRevision != originalLayoutRevision)
        return nullptr;
    if (hit_result) return hit_result;
    if (!current->rect().contains(point) || !acceptsPointerEvents(*current, style)) return nullptr;
    current = lifetime.get();
    if (!current
        || !isRootedInSurface(current)
        || current->visibility() != Visibility::Visible
        || current->parent() != originalParent
        || current->mStyleRevision != originalStyleRevision
        || current->mLayoutInvalidationRevision != originalLayoutRevision)
        return nullptr;
    return current;
}

bool Surface::routeEvent(RoutedEvent& event) {
    std::vector<WidgetRef<Widget>> route;
    std::vector<const Widget*> route_parents;
    for (Widget* current = &event.target(); current; current = current->parent()) {
        route.emplace_back(current);
        route_parents.push_back(current->parent());
        if (isSurfaceRoot(current)) break;
    }
    if (route.empty() || !route.back() || !isSurfaceRoot(route.back().get()) || !isRootedInSurface(route.front().get())) return false;

    event.mPhase = EventPhase::Capture;
    for (std::size_t index = route.size() - 1; index > 0; --index) {
        Widget* target = route[index].get();
        Widget* child = route[index - 1].get();
        if (!target || !child || target->parent() != route_parents[index] || child->parent() != target || !isRootedInSurface(target)) break;
        event.mCurrentTarget = target;
        target->onEvent(event);
        if (event.propagationStopped()) {
            event.mCurrentTarget = nullptr;
            return event.handled() || event.defaultPrevented();
        }
    }

    event.mPhase = EventPhase::Target;
    Widget* target = route.front().get();
    if (!target || !isRootedInSurface(target)) {
        event.mCurrentTarget = nullptr;
        return event.handled() || event.defaultPrevented();
    }
    if (target->parent() != route_parents.front() || (route.size() > 1 && target->parent() != route[1].get())) {
        event.mCurrentTarget = nullptr;
        return event.handled() || event.defaultPrevented();
    }
    event.mCurrentTarget = target;
    target->onEvent(event);
    if (!event.propagationStopped()) {
        event.mPhase = EventPhase::Bubble;
        for (std::size_t index = 1; index < route.size(); ++index) {
            Widget* bubble_target = route[index].get();
            Widget* child = route[index - 1].get();
            if (!bubble_target
                || !child
                || bubble_target->parent() != route_parents[index]
                || child->parent() != bubble_target
                || !isRootedInSurface(bubble_target))
                break;
            event.mCurrentTarget = bubble_target;
            bubble_target->onEvent(event);
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
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    if (hasActiveModal()) return hitTestNode(layerRoot(SurfaceLayer::Modal), point, mViewport, styles);

    for (std::size_t index = static_cast<std::size_t>(SurfaceLayer::Modal); index > static_cast<std::size_t>(SurfaceLayer::Content); --index) {
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index);
        if (layer == SurfaceLayer::Tooltip || layer == SurfaceLayer::Drag || layer == SurfaceLayer::Modal) continue;
        if (Widget* hit = hitTestNode(layerRoot(layer), point, mViewport, styles)) return hit;
    }
    return hitTestNode(*mRoot, point, mViewport, styles);
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
    const WidgetRef<const Widget> lifetime(widget);
    const Widget* originalParent = widget->parent();
    const std::uint64_t originalStyleRevision = widget->mStyleRevision;
    const std::uint64_t originalLayoutRevision = widget->mLayoutInvalidationRevision;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style& style = styles.style(*widget);
    const Widget* current = lifetime.get();
    if (!current
        || !isRootedInSurface(current)
        || current->parent() != originalParent
        || current->mStyleRevision != originalStyleRevision
        || current->mLayoutInvalidationRevision != originalLayoutRevision)
        return CursorStyle::Default;
    const CursorStyle cursor = style.cursor;
    return cursor == CursorStyle::Auto ? CursorStyle::Default : cursor;
}

bool Surface::pointerMove(const PointerEvent& event) {
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    WidgetRef<Widget> captured_ref(mCaptured.get());
    if (Widget* captured = captured_ref.get()) {
        const Widget* captured_parent = captured->parent();
        RoutedPointerEvent routed(EventKind::PointerMove, *captured, event);
        const bool routed_handled = routeEvent(routed);
        captured = captured_ref.get();
        if (!captured || captured->parent() != captured_parent || !isRootedInSurface(captured) || !isEnabledInTree(captured)) return routed_handled;
        const bool handled = !routed.defaultPrevented() && captured->updatePointerInteraction(event);
        if (captured = captured_ref.get(); captured && isRootedInSurface(captured) && isEnabledInTree(captured))
            captured->dispatchMouseEvent(WidgetEventKind::MouseMove, event);
        return routed_handled || handled;
    }
    updateResizeCursor(event.position);
    refreshHover();
    if (!mLongClickFired && mLongClickTarget && mHovered.get() != mLongClickTarget.get()) resetLongClick();
    WidgetRef<Widget> hovered_ref(mHovered.get());
    if (Widget* hovered = hovered_ref.get()) {
        if (isEnabledInTree(hovered)) {
            RoutedPointerEvent routed(EventKind::PointerMove, *hovered, event);
            routeEvent(routed);
            if (hovered = hovered_ref.get(); hovered && isRootedInSurface(hovered) && isEnabledInTree(hovered))
                hovered->dispatchMouseEvent(WidgetEventKind::MouseMove, event);
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
        WidgetRef<Floater> resize_ref(resize_floater);
        const SurfaceLayer layer = resize_floater->parent() == &layerRoot(SurfaceLayer::Modal) ? SurfaceLayer::Modal : SurfaceLayer::Floater;
        raiseWithinLayer(*resize_floater, layer);
        const Surface* resize_surface = resize_floater->attachedSurface();
        const Widget* resize_parent = resize_floater->parent();
        const auto isResizeFloaterStillAttached = [&]() {
            Floater* current = resize_ref.get();
            return current && current->attachedSurface() == resize_surface && current->parent() == resize_parent && isRootedInSurface(current);
        };
        resetLongClick();
        mPressedClickCount = 0;
        clearKeyboardPress();
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
        mPressed.set(nullptr);
        const bool native = mFloaterDelegate && mFloaterDelegate->beginNativeFloaterResize(*this, *resize_floater);
        resize_floater = resize_ref.get();
        if (!isResizeFloaterStillAttached()) return false;
        const std::optional<Rect> bounds = native ? std::nullopt : std::optional<Rect>(mViewport);
        const Vec2 minimum = minimumFloaterSize(*resize_floater);
        resize_floater = resize_ref.get();
        if (!isResizeFloaterStillAttached()) return false;
        const bool began = resize_floater->beginResizeInteraction(event, resize_edges, minimum, bounds);
        resize_floater = resize_ref.get();
        if (began && isResizeFloaterStillAttached()) {
            mCaptured.set(resize_floater);
            setHovered(resize_floater);
            setFocused(nullptr, false);
            mResizeCursor = detail::resizeCursor(static_cast<detail::ResizeEdges>(resize_edges));
            return true;
        }
    }
    WidgetRef<Widget> hit_ref(hitTestAt(event.position));
    Widget* hit = hit_ref.get();
    if (hit) raiseWithinLayer(*hit, SurfaceLayer::Floater);
    setHovered(hit);
    bool default_prevented = false;
    if (isEnabledInTree(hit)) {
        RoutedPointerEvent routed(EventKind::PointerDown, *hit, event);
        routeEvent(routed);
        default_prevented = routed.defaultPrevented();
        hit = hit_ref.get();
    }
    if (event.button != PointerButton::Left) {
        if (hit && isRootedInSurface(hit) && isEnabledInTree(hit)) hit->dispatchMouseEvent(WidgetEventKind::MouseDown, event);
        return hit_ref.get() && isRootedInSurface(hit_ref.get());
    }
    resetLongClick();
    mPressedClickCount = 0;
    clearKeyboardPress();
    if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
    mPressed.set(nullptr);
    hit = hit_ref.get();
    const bool hitEnabledBeforeInteraction = isEnabledInTree(hit);
    for (Widget* candidate = hitEnabledBeforeInteraction && !default_prevented ? hit : nullptr; candidate;) {
        const WidgetSnapshot candidate_state = snapshot(*candidate);
        WidgetRef<Widget> parent_ref(candidate->parent());
        if (!isEnabledInTree(candidate)) {
            candidate = parent_ref.get();
            continue;
        }
        if (!candidate->beginPointerInteraction(event)) {
            candidate = parent_ref.get();
            continue;
        }
        candidate = candidate_state.lifetime.get();
        if (!candidate
            || !snapshotValid(candidate_state)
            || candidate->parent() != parent_ref.get()
            || !isEnabledInTree(candidate)
            || !isRootedInSurface(candidate))
            return true;
        mCaptured.set(candidate);
        setFocused(nullptr, false);
        candidate->dispatchMouseEvent(WidgetEventKind::MouseDown, event);
        return true;
    }
    hit = hit_ref.get();
    const bool hitEnabledAfterInteraction = isEnabledInTree(hit);
    const WidgetSnapshot hit_state = hit ? snapshot(*hit) : WidgetSnapshot{};
    const bool focusable = hitEnabledAfterInteraction && !default_prevented && hit->focusable();
    hit = hit_state.lifetime.get();
    if (hit && (!snapshotValid(hit_state) || !isRootedInSurface(hit))) return true;
    setFocused(focusable ? hit : nullptr, false);
    mPressed.set(hitEnabledAfterInteraction && !default_prevented ? hit : nullptr);
    mPressedClickCount = mPressed ? event.clickCount : 0;
    updatePressedState();
    if (hitEnabledAfterInteraction && !default_prevented && hit && hit->eventCall(WidgetEventKind::LongClick)) mLongClickTarget.set(hit);
    if (hitEnabledAfterInteraction && hit && isRootedInSurface(hit)) hit->dispatchMouseEvent(WidgetEventKind::MouseDown, event);
    return hit_ref.get() && isRootedInSurface(hit_ref.get());
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
            if (hit && isRootedInSurface(hit.get()) && isEnabledInTree(hit.get())) hit->dispatchMouseEvent(WidgetEventKind::MouseUp, event);
            if (hit && event.button == PointerButton::Right && isRootedInSurface(hit.get()) && isEnabledInTree(hit.get()))
                hit->dispatchMouseEvent(WidgetEventKind::ContextMenu, event);
        }
        return had_hit;
    }
    WidgetRef<Widget> captured_ref(mCaptured.get());
    if (Widget* captured = captured_ref.get()) {
        const Widget* captured_parent = captured->parent();
        resetLongClick();
        mPressedClickCount = 0;
        mCaptured.set(nullptr);
        RoutedPointerEvent routed(EventKind::PointerUp, *captured, event);
        const bool routed_handled = routeEvent(routed);
        captured = captured_ref.get();
        if (!captured || captured->parent() != captured_parent || !isRootedInSurface(captured) || !isEnabledInTree(captured)) {
            refreshHover();
            return routed_handled;
        }
        const bool handled = !routed.defaultPrevented() && captured->endPointerInteraction(event);
        if (captured = captured_ref.get(); captured && isRootedInSurface(captured) && isEnabledInTree(captured))
            captured->dispatchMouseEvent(WidgetEventKind::MouseUp, event);
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
    if (released && isRootedInSurface(released.get()) && isEnabledInTree(released.get()))
        released->dispatchMouseEvent(WidgetEventKind::MouseUp, event);
    resetLongClick();
    mPressedClickCount = 0;
    const bool clicked = !suppress_click
        && released
        && released.get() == hit.get()
        && !default_prevented
        && isEnabledInTree(released.get())
        && isRootedInSurface(released.get());
    if (clicked) {
        released->activate();
        if (Widget* activated = released.get(); activated && click_count >= 2 && isEnabledInTree(activated) && isRootedInSurface(activated)) {
            PointerEvent double_click = event;
            double_click.clickCount = click_count;
            activated->dispatchMouseEvent(WidgetEventKind::DoubleClick, double_click);
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
    WidgetRef<Widget> hit_ref(hitTestAt(event.position));
    Widget* hit = hit_ref.get();
    bool routed_handled = false;
    bool default_prevented = false;
    if (isEnabledInTree(hit)) {
        RoutedScrollEvent routed(*hit, event);
        routed_handled = routeEvent(routed);
        default_prevented = routed.defaultPrevented();
        hit = hit_ref.get();
    }
    for (Widget* candidate = default_prevented ? nullptr : hit; candidate;) {
        WidgetRef<Widget> candidate_ref(candidate);
        WidgetRef<Widget> parent_ref(candidate ? candidate->parent() : nullptr);
        const WidgetSnapshot candidate_state = candidate ? snapshot(*candidate) : WidgetSnapshot{};
        if (!candidate_ref || !isEnabledInTree(candidate) || !isRootedInSurface(candidate)) break;
        if (candidate->defaultScroll(event)) return true;
        candidate = candidate_ref.get();
        if (!candidate || !snapshotValid(candidate_state) || !isEnabledInTree(candidate) || !isRootedInSurface(candidate)) break;
        candidate = parent_ref.get();
    }
    return routed_handled || hit_ref.get() != nullptr;
}

bool Surface::keyDown(const KeyEvent& event) {
    validateFocus();
    if (event.key == KEY_TAB && (event.modifiers & ~MODIFIER_SHIFT) == 0) {
        mTabKeyHandled = moveFocus((event.modifiers & MODIFIER_SHIFT) != 0);
        return mTabKeyHandled;
    }
    WidgetRef<Widget> focused_ref(mFocused.get());
    Widget* focused = focused_ref.get();
    if (!focused) return false;
    const Widget* focused_parent = focused->parent();
    RoutedKeyEvent routed(EventKind::KeyDown, *focused, event);
    const bool routed_handled = routeEvent(routed);
    focused = focused_ref.get();
    if (!focused || focused->parent() != focused_parent || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routed_handled;
    if (routed.defaultPrevented()) return routed_handled;
    if (isActivationKey(event.key)) {
        if (mKeyPressed.get() && (mKeyPressed.get() != focused || mPressedKey != event.key)) clearKeyboardPress();
        if (!focused->defaultKeyDown(event)) return routed_handled;
        focused = focused_ref.get();
        if (!focused || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return true;
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
    WidgetRef<Widget> focused_ref(mFocused.get());
    Widget* focused = focused_ref.get();
    if (!focused) return false;
    const Widget* focused_parent = focused->parent();
    RoutedKeyEvent routed(EventKind::KeyUp, *focused, event);
    const bool routed_handled = routeEvent(routed);
    focused = focused_ref.get();
    if (!focused || focused->parent() != focused_parent || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routed_handled;
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
    WidgetRef<Widget> focused_ref(mFocused.get());
    Widget* focused = focused_ref.get();
    if (!focused) return false;
    RoutedCharacterEvent routed(*focused, codepoint);
    const bool routed_handled = routeEvent(routed);
    focused = focused_ref.get();
    if (!focused || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routed_handled;
    return routed_handled || (!routed.defaultPrevented() && focused->defaultCharacterInput(codepoint));
}

void Surface::refreshHover() {
    updateLayoutIfNeeded();
    refreshHoverState();
}

void Surface::refreshHoverState() {
    if (!mPointerPositionKnown || !mRoot) return;
    const bool refresh_was_requested = mHitTestDirty;
    mHitTestDirty = false;
    updateResizeCursor(mPointerPosition);
    Widget* hit = hitTestAt(mPointerPosition);
    setHovered(hit && isEnabledInTree(hit) ? hit : nullptr);
    updatePressedState();
    if (refresh_was_requested) mHitTestDirty = false;
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
    target->dispatchLongClickEvent(mLongClickElapsed);
}

void Surface::setHovered(Widget* node) {
    if (mHovered.get() == node) return;
    if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, false);
    mHovered.set(node);
    if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, true);
}

void Surface::setFocused(Widget* node, bool focusVisible) {
    if (mFocused.get() == node) {
        if (node) node->setState(WidgetState::FocusVisible, focusVisible);
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
        focused->setState(WidgetState::FocusVisible, focusVisible);
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

bool Surface::isRootedInSurface(const Widget* node) const {
    for (const Widget* current = node; current; current = current->parent())
        if (isSurfaceRoot(current)) return true;
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
    return mSystem ? mSystem->longClickDelay() : System::defaultLongClickDelay();
}

bool Surface::moveFocus(bool backwards) {
    if (!mRoot) return false;
    std::vector<WidgetRef<Widget>> focusable;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    if (hasActiveModal()) collectFocusable(layerRoot(SurfaceLayer::Modal), focusable, styles);
    else {
        collectFocusable(*mRoot, focusable, styles);
        collectFocusable(layerRoot(SurfaceLayer::Floater), focusable, styles);
        collectFocusable(layerRoot(SurfaceLayer::Popup), focusable, styles);
    }
    if (focusable.empty()) return false;

    focusable.erase(
        std::remove_if(focusable.begin(), focusable.end(), [this](const WidgetRef<Widget>& ref) { return !ref || !isFocusableInTree(ref.get()); }),
        focusable.end());
    if (focusable.empty()) return false;

    const auto current =
        std::find_if(focusable.begin(), focusable.end(), [this](const WidgetRef<Widget>& ref) { return ref.get() == mFocused.get(); });
    WidgetRef<Widget> next;
    if (current == focusable.end()) next = backwards ? focusable.back() : focusable.front();
    else if (backwards) next = current == focusable.begin() ? focusable.back() : *(current - 1);
    else next = std::next(current) == focusable.end() ? focusable.front() : *std::next(current);
    if (Widget* focused = next.get()) setFocused(focused, true);
    return true;
}
} // namespace rdui
