/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <iterator>
#include <optional>
#include <vector>
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "style/stylepass.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "system.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::resizeCursor;
using detail::ResizeEdges;

bool Surface::acceptsPointerEvents(const Element& element, const ComputedStyle& style) {
    const PointerEvents policy = style.pointerEvents;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return element.pointerEvents();
}

void Surface::collectFocusable(Element& node, std::vector<ElementRef<Element>>& result, StylePass& styles) const {
    const ElementVisit observation(node);
    const ComputedStyle style = styles.style(node);
    Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !current->isVisible(style) || current->disabled()) return;
    if (current->focusable()) result.emplace_back(current);
    const auto children = styles.sourceChildren(*current);
    for (const ElementRef<Element>& childRef : *children)
        if (Element* child = childRef.get(); child && child->parentElement() == current) collectFocusable(*child, result, styles);
}

Element* Surface::hitTestNode(Element& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const {
    if (!inheritedClip.contains(point) || !isRootedInSurface(&node)) return nullptr;
    const ElementObservation observation = observe(node);
    const ComputedStyle style = styles.style(node);
    if (!node.isVisible(style)) return nullptr;
    Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return nullptr;
    const bool clipsX = style.overflowX != Overflow::Visible;
    const bool clipsY = style.overflowY != Overflow::Visible;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    const bool clipsChildren = clipAxes != ClipAxes::NoAxes;
    const Vec2 scrollTranslation = scrollContentTranslation(layoutDirection(), {current->scrollLeft(), current->scrollTop()});
    const Vec2 scrollOffset = clipsChildren ? Vec2{-scrollTranslation.x, -scrollTranslation.y} : Vec2{};
    Rect childClip = clipsChildren ? clipToAxes(inheritedClip, ElementInternalAccess::scrollport(*current), clipAxes) : inheritedClip;
    if (clipsChildren) childClip = {childClip.x + scrollOffset.x, childClip.y + scrollOffset.y, childClip.w, childClip.h};
    const Vec2 childPoint = point + scrollOffset;
    const auto children = styles.sourceChildren(*current);
    Element* hitResult = nullptr;
    for (auto child = children->rbegin(); child != children->rend(); ++child)
        if (Element* childElement = child->get())
            if (childElement->parentElement() == current)
                if (Element* hit = hitTestNode(*childElement, childPoint, childClip, styles)) {
                    hitResult = hit;
                    break;
                }
    current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return nullptr;
    if (hitResult) return hitResult;
    if (!current->rect().contains(point) || !acceptsPointerEvents(*current, style)) return nullptr;
    return current;
}

bool Surface::routeEvent(Event& event) {
    if (!event.payloadMatchesType()) return false;
    std::vector<ElementRef<Element>> route;
    for (Element* current = event.target(); current; current = current->parentElement()) {
        route.emplace_back(current);
        if (current->idScopeRoot() || isSurfaceRoot(current)) break;
    }
    const auto routeIsValid = [&]() {
        if (route.empty()) return false;
        for (std::size_t index = 0; index < route.size(); ++index) {
            Element* current = route[index].getMounted();
            if (!current || current->surface() != this) return false;
            if (index + 1 < route.size()) {
                Element* parent = route[index + 1].getMounted();
                if (!parent || current->parentElement() != parent) return false;
            } else if (!current->idScopeRoot() && !isSurfaceRoot(current)) {
                return false;
            }
        }
        return isRootedInSurface(route.front().get());
    };
    if (!routeIsValid()) return false;

    event.setPhase(EventPhase::Capture);
    for (std::size_t index = route.size() - 1; index > 0; --index) {
        Element* target = route[index].get();
        if (!target) break;
        event.setCurrentTarget(target);
        target->dispatchListeners(event, true, routeIsValid);
        if (!routeIsValid()) {
            event.setCurrentTarget(nullptr);
            return event.handled() || event.defaultPrevented();
        }
        if (event.propagationStopped()) {
            event.setCurrentTarget(nullptr);
            return event.handled() || event.defaultPrevented();
        }
    }

    event.setPhase(EventPhase::Target);
    Element* target = route.front().get();
    if (!target) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    event.setCurrentTarget(target);
    target->dispatchListeners(event, true, routeIsValid);
    if (!routeIsValid()) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    if (!event.immediatePropagationStopped()) target->dispatchListeners(event, false, routeIsValid);
    if (!routeIsValid()) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    if (!event.propagationStopped()) {
        event.setPhase(EventPhase::Bubble);
        for (std::size_t index = 1; index < route.size(); ++index) {
            Element* bubbleTarget = route[index].get();
            if (!bubbleTarget) break;
            event.setCurrentTarget(bubbleTarget);
            bubbleTarget->dispatchListeners(event, false, routeIsValid);
            if (!routeIsValid()) {
                event.setCurrentTarget(nullptr);
                return event.handled() || event.defaultPrevented();
            }
            if (event.propagationStopped()) break;
        }
    }
    event.setCurrentTarget(nullptr);
    return event.handled() || event.defaultPrevented();
}

bool Surface::hasActiveModal() const {
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const MountList& modalMounts = mounts(SurfaceLayer::Modal);
    return std::any_of(modalMounts.begin(), modalMounts.end(),
                       [&styles](const MountPtr& mount) { return mount && mount->root && mount->root->isVisible(styles.style(*mount->root)); });
}

Element* Surface::hitTestAt(const Vec2& point) {
    if (!mViewport.contains(point)) return nullptr;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto hitInLayer = [&](SurfaceLayer layer) -> Element* {
        const MountList& layerMounts = mounts(layer);
        for (auto current = layerMounts.rbegin(); current != layerMounts.rend(); ++current)
            if (*current && (*current)->root)
                if (Element* hit = hitTestNode(*(*current)->root, point, mViewport, styles)) return hit;
        return nullptr;
    };
    if (hasActiveModal()) return hitInLayer(SurfaceLayer::Modal);

    for (std::size_t index = static_cast<std::size_t>(SurfaceLayer::Modal); index > static_cast<std::size_t>(SurfaceLayer::Base); --index) {
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index);
        if (layer == SurfaceLayer::Tooltip || layer == SurfaceLayer::Drag || layer == SurfaceLayer::Modal) continue;
        if (Element* hit = hitInLayer(layer)) return hit;
    }
    return hitInLayer(SurfaceLayer::Base);
}

void Surface::clearInteractionState() {
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    Element* hovered = mHovered;
    Element* pressed = mPressed;
    Element* focused = mFocused;
    Element* captured = mCaptured;
    mHovered = nullptr;
    mPressed = nullptr;
    mFocused = nullptr;
    mCaptured = nullptr;
    if (hovered) hovered->setState(ElementState::Hovered, false);
    if (pressed) pressed->setState(ElementState::Active, false);
    clearKeyboardPress();
    if (focused) {
        focused->setState(ElementState::Focused, false);
        focused->setState(ElementState::FocusVisible, false);
    }
    if (captured) captured->endPointerInteraction({mPointerPosition});
    if (surfaceLifetime.expired()) return;
    mResizeCursor = CursorStyle::Auto;
    mScrollbarHover.reset();
    mScrollbarCapture.reset();
    mPressedClickCount = 0;
    mTabKeyHandled = false;
}

CursorStyle Surface::cursor() const {
    return cursorValue().style;
}

CursorValue Surface::cursorValue() const {
    const ScrollbarTarget* scrollbar = mScrollbarCapture ? &*mScrollbarCapture : (mScrollbarHover ? &*mScrollbarHover : nullptr);
    if (mResizeCursor != CursorStyle::Auto) return {mResizeCursor, {}};
    if (scrollbar) return {CursorStyle::Default, {}};
    const Element* element = mCaptured ? mCaptured : mHovered;
    if (!element) return {CursorStyle::Default, {}};
    const ConstElementObservation observation = observe(*element);
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const ComputedStyle style = styles.style(*element);
    const Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return {CursorStyle::Default, {}};
    const CursorStyle cursor = style.cursor;
    return {cursor == CursorStyle::Auto ? CursorStyle::Default : cursor, style.cursorImages};
}

std::optional<CursorStyle> Surface::pointerCursor() const {
    const std::optional<CursorValue> value = pointerCursorValue();
    return value ? std::optional<CursorStyle>(value->style) : std::nullopt;
}

std::optional<CursorValue> Surface::pointerCursorValue() const {
    if (!mHovered && !mPressed && !mCaptured && !mScrollbarHover && !mScrollbarCapture && mResizeCursor == CursorStyle::Auto) return std::nullopt;
    return cursorValue();
}

bool Surface::pointerMove(const PointerEvent& event) {
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    if (mScrollbarCapture) return updateScrollbarInteraction(event.position);
    ElementRef<Element> capturedRef(mCaptured);
    if (Element* captured = capturedRef.get()) {
        const Element* capturedParent = captured->parentElement();
        Event routed(kPointerMoveEvent, *captured, event);
        const bool routedHandled = routeEvent(routed);
        captured = capturedRef.get();
        if (!captured || captured->parentElement() != capturedParent || !isRootedInSurface(captured) || !isEnabledInTree(captured))
            return routedHandled;
        const bool handled = !routed.defaultPrevented() && captured->updatePointerInteraction(event);
        return routedHandled || handled;
    }
    updateResizeCursor(event.position);
    refreshHover();
    ElementRef<Element> hoveredRef(mHovered);
    if (Element* hovered = hoveredRef.get()) {
        if (isEnabledInTree(hovered)) {
            Event routed(kPointerMoveEvent, *hovered, event);
            routeEvent(routed);
        }
    }
    return mHovered != nullptr || mPressed != nullptr || mScrollbarHover.has_value() || mResizeCursor != CursorStyle::Auto;
}

void Surface::pointerLeave() {
    mPointerPositionKnown = false;
    if (!mCaptured && !mScrollbarCapture) {
        mResizeCursor = CursorStyle::Auto;
        setScrollbarHover(std::nullopt);
        setHovered(nullptr);
    }
    updatePressedState();
}

bool Surface::pointerDown(const PointerEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    std::uint8_t resizeEdges = 0;
    HTMLFloaterElement* resizeFloater = event.button == PointerButton::Left ? resizeFloaterAt(event.position, resizeEdges) : nullptr;
    if (resizeFloater) {
        ElementRef<HTMLFloaterElement> resizeRef(resizeFloater);
        const std::optional<SurfaceLayer> resolvedLayer = layerOf(resizeFloater);
        if (!resolvedLayer) return false;
        const SurfaceLayer layer = *resolvedLayer;
        raiseWithinLayer(*resizeFloater, layer);
        const Surface* resizeSurface = resizeFloater->surface();
        const Element* resizeParent = resizeFloater->parentElement();
        const auto isResizeFloaterStillAttached = [&]() {
            HTMLFloaterElement* current = resizeRef.get();
            return current && current->surface() == resizeSurface && current->parentElement() == resizeParent && isRootedInSurface(current);
        };
        mPressedClickCount = 0;
        clearKeyboardPress();
        if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
        mPressed = nullptr;
        resizeFloater = resizeRef.get();
        if (!isResizeFloaterStillAttached()) return false;
        const std::optional<Rect> bounds = mViewport;
        const Vec2 minimum = minimumFloaterSize(*resizeFloater);
        resizeFloater = resizeRef.get();
        if (!isResizeFloaterStillAttached()) return false;
        const bool began = resizeFloater->beginResizeInteraction(event, resizeEdges, minimum, bounds);
        resizeFloater = resizeRef.get();
        if (began && isResizeFloaterStillAttached()) {
            mCaptured = resizeFloater;
            setHovered(resizeFloater);
            setFocused(nullptr, false);
            mResizeCursor = resizeCursor(static_cast<ResizeEdges>(resizeEdges));
            return true;
        }
    }
    if (event.button == PointerButton::Left) {
        if (std::optional<ScrollbarTarget> scrollbar = hitTestScrollbarAt(event.position)) {
            if (const std::optional<SurfaceLayer> layer = layerOf(scrollbar->element); layer && *layer == SurfaceLayer::Floater)
                raiseWithinLayer(*scrollbar->element, *layer);
            if (beginScrollbarInteraction(*scrollbar, event.position)) return true;
        }
    }
    ElementRef<Element> hitRef(hitTestAt(event.position));
    Element* hit = hitRef.get();
    if (!hit && hasActiveModal()) {
        clearKeyboardPress();
        if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
        mPressed = nullptr;
        setFocused(nullptr, false);
        setHovered(nullptr);
        return true;
    }
    if (hit) raiseWithinLayer(*hit, SurfaceLayer::Floater);
    setHovered(hit);
    bool defaultPrevented = false;
    if (isEnabledInTree(hit)) {
        Event routed(kPointerDownEvent, *hit, event);
        routeEvent(routed);
        defaultPrevented = routed.defaultPrevented();
        hit = hitRef.get();
    }
    if (event.button != PointerButton::Left) return hitRef.get() && isRootedInSurface(hitRef.get());
    mPressedClickCount = 0;
    clearKeyboardPress();
    if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
    mPressed = nullptr;
    hit = hitRef.get();
    const bool hitEnabledBeforeInteraction = isEnabledInTree(hit);
    for (Element* candidate = hitEnabledBeforeInteraction && !defaultPrevented ? hit : nullptr; candidate;) {
        const ElementObservation candidateObservation = observe(*candidate);
        ElementRef<Element> parentRef(candidate->parentElement());
        if (!isEnabledInTree(candidate)) {
            candidate = parentRef.get();
            continue;
        }
        if (!candidate->beginPointerInteraction(event)) {
            candidate = parentRef.get();
            continue;
        }
        candidate = candidateObservation.get();
        if (!candidate
            || !candidateObservation.layoutValid()
            || !candidateObservation.styleValid()
            || candidate->parentElement() != parentRef.get()
            || !isEnabledInTree(candidate)
            || !isRootedInSurface(candidate))
            return true;
        mCaptured = candidate;
        setFocused(nullptr, false);
        return true;
    }
    hit = hitRef.get();
    const bool hitEnabledAfterInteraction = isEnabledInTree(hit);
    const ElementObservation hitObservation = hit ? observe(*hit) : ElementObservation{};
    const bool focusable = hitEnabledAfterInteraction && !defaultPrevented && hit->focusable();
    hit = hitObservation.get();
    if (hit && (!hitObservation.layoutValid() || !hitObservation.styleValid() || !isRootedInSurface(hit))) return true;
    setFocused(focusable ? hit : nullptr, false);
    mPressed = hitEnabledAfterInteraction && !defaultPrevented ? hit : nullptr;
    mPressedClickCount = mPressed ? event.clickCount : 0;
    updatePressedState();
    return hitRef.get() && isRootedInSurface(hitRef.get());
}

bool Surface::pointerUp(const PointerEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    if (event.button != PointerButton::Left) {
        ElementRef<Element> hit(hitTestAt(event.position));
        const bool hadHit = !!hit;
        if (isEnabledInTree(hit.get())) {
            Event routed(kPointerUpEvent, *hit, event);
            routeEvent(routed);
            if (hit && event.button == PointerButton::Right && isRootedInSurface(hit.get()) && isEnabledInTree(hit.get())) {
                Event contextMenu(kContextMenuEvent, *hit, event);
                routeEvent(contextMenu);
            }
        }
        return hadHit || hasActiveModal();
    }
    if (mScrollbarCapture) {
        mScrollbarCapture.reset();
        refreshHover();
        return true;
    }
    ElementRef<Element> capturedRef(mCaptured);
    if (Element* captured = capturedRef.get()) {
        const Element* capturedParent = captured->parentElement();
        mPressedClickCount = 0;
        mCaptured = nullptr;
        Event routed(kPointerUpEvent, *captured, event);
        const bool routedHandled = routeEvent(routed);
        captured = capturedRef.get();
        if (!captured || captured->parentElement() != capturedParent || !isRootedInSurface(captured) || !isEnabledInTree(captured)) {
            refreshHover();
            return routedHandled;
        }
        const bool handled = !routed.defaultPrevented() && captured->endPointerInteraction(event);
        refreshHover();
        return routedHandled || handled;
    }
    ElementRef<Element> released(mPressed);
    ElementRef<Element> hit(hitTestAt(event.position));
    bool defaultPrevented = false;
    if (Element* target = released ? released.get() : hit.get()) {
        Event routed(kPointerUpEvent, *target, event);
        routeEvent(routed);
        defaultPrevented = routed.defaultPrevented();
    }
    const uint8_t clickCount = mPressedClickCount;
    if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
    mPressed = nullptr;
    setHovered(hit.get());
    mPressedClickCount = 0;
    const bool clicked =
        released && released.get() == hit.get() && !defaultPrevented && isEnabledInTree(released.get()) && isRootedInSurface(released.get());
    if (clicked) {
        released->activate();
        if (Element* activated = released.get(); activated && clickCount >= 2 && isEnabledInTree(activated) && isRootedInSurface(activated)) {
            PointerEvent doubleClick = event;
            doubleClick.clickCount = clickCount;
            Event doubleClickEvent(kDoubleClickEvent, *activated, doubleClick);
            routeEvent(doubleClickEvent);
        }
        refreshHover();
        return true;
    }
    return released || hit || hasActiveModal();
}

bool Surface::keyDown(const KeyEvent& event) {
    validateFocus();
    if (event.key == kKeyTab && (event.modifiers & ~kModifierShift) == 0) {
        mTabKeyHandled = moveFocus((event.modifiers & kModifierShift) != 0);
        return mTabKeyHandled;
    }
    ElementRef<Element> focusedRef(mFocused);
    Element* focused = focusedRef.get();
    if (!focused) return false;
    const Element* focusedParent = focused->parentElement();
    Event routed(kKeyDownEvent, *focused, event);
    const bool routedHandled = routeEvent(routed);
    focused = focusedRef.get();
    if (!focused || focused->parentElement() != focusedParent || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routedHandled;
    if (routed.defaultPrevented()) return routedHandled;
    if (scrollFocusedElement(event, *focused)) return true;
    if (isActivationKey(event.key)) {
        if (mKeyPressed && (mKeyPressed != focused || mPressedKey != event.key)) clearKeyboardPress();
        if (!focused->defaultKeyDown(event)) return routedHandled;
        focused = focusedRef.get();
        if (!focused || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return true;
        mKeyPressed = focused;
        mPressedKey = event.key;
        return true;
    }
    return focused->defaultKeyDown(event) || routedHandled;
}

bool Surface::keyUp(const KeyEvent& event) {
    if (event.key == kKeyTab) {
        const bool handled = mTabKeyHandled;
        mTabKeyHandled = false;
        return handled;
    }
    validateFocus();
    ElementRef<Element> focusedRef(mFocused);
    Element* focused = focusedRef.get();
    if (!focused) return false;
    const Element* focusedParent = focused->parentElement();
    Event routed(kKeyUpEvent, *focused, event);
    const bool routedHandled = routeEvent(routed);
    focused = focusedRef.get();
    if (!focused || focused->parentElement() != focusedParent || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routedHandled;
    if (routed.defaultPrevented()) {
        if (isActivationKey(event.key)) clearKeyboardPress();
        return routedHandled;
    }
    if (isActivationKey(event.key)) {
        if (mKeyPressed != focused || mPressedKey != event.key) return false;
        mKeyPressed = nullptr;
        mPressedKey = 0;
    }
    const bool handled = focused->defaultKeyUp(event);
    if (handled) refreshHover();
    return handled || routedHandled;
}

bool Surface::charInput(unsigned int codepoint) {
    validateFocus();
    ElementRef<Element> focusedRef(mFocused);
    Element* focused = focusedRef.get();
    if (!focused) return false;
    Event routed(kCharacterInputEvent, *focused, codepoint);
    const bool routedHandled = routeEvent(routed);
    focused = focusedRef.get();
    if (!focused || !isRootedInSurface(focused) || !isEnabledInTree(focused)) return routedHandled;
    return routedHandled || (!routed.defaultPrevented() && focused->defaultCharacterInput(codepoint));
}

void Surface::refreshHover() {
    updateLayoutIfNeeded();
    refreshHoverState();
}

void Surface::refreshHoverState() {
    if (!mPointerPositionKnown) return;
    const bool refreshWasRequested = mHitTestDirty;
    mHitTestDirty = false;
    updateResizeCursor(mPointerPosition);
    std::optional<ScrollbarTarget> scrollbar = mScrollbarCapture
        ? std::optional<ScrollbarTarget>(ScrollbarTarget{mScrollbarCapture->element, mScrollbarCapture->geometry, mScrollbarCapture->hit})
        : hitTestScrollbarAt(mPointerPosition);
    setScrollbarHover(std::move(scrollbar));
    Element* hit = mScrollbarHover ? nullptr : hitTestAt(mPointerPosition);
    setHovered(hit && isEnabledInTree(hit) ? hit : nullptr);
    updatePressedState();
    if (refreshWasRequested) mHitTestDirty = false;
}

void Surface::setHovered(Element* node) {
    if (mHovered == node) return;
    if (Element* hovered = mHovered) hovered->setState(ElementState::Hovered, false);
    mHovered = node;
    if (Element* hovered = mHovered) hovered->setState(ElementState::Hovered, true);
}

void Surface::setFocused(Element* node, bool focusVisible) {
    if (mFocused == node) {
        if (node) node->setState(ElementState::FocusVisible, focusVisible);
        return;
    }
    if (Element* focused = mFocused) {
        clearKeyboardPress();
        focused->setState(ElementState::Focused, false);
        focused->setState(ElementState::FocusVisible, false);
    }
    mFocused = node;
    if (Element* focused = mFocused) {
        focused->setState(ElementState::Focused, true);
        focused->setState(ElementState::FocusVisible, focusVisible);
    }
}

bool Surface::isEnabledInTree(const Element* node) const {
    if (!node || node->mSurface != this) return false;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    for (const Element* current = node; current; current = current->parentElement()) {
        if (!current->isVisible(styles.style(*current)) || current->disabled()) return false;
        if (isSurfaceRoot(current)) return true;
    }
    return false;
}

bool Surface::isRootedInSurface(const Element* node) const {
    if (!node || node->mSurface != this) return false;
    const Element* root = mountedRoot(node);
    return root && root->mSurface == this;
}

bool Surface::isFocusableInTree(const Element* node) const {
    return node && node->focusable() && isEnabledInTree(node);
}

void Surface::validateFocus() {
    Element* focused = mFocused;
    if (focused && hasActiveModal() && layerOf(focused) != SurfaceLayer::Modal) {
        setFocused(nullptr, false);
        return;
    }
    if (focused && !isFocusableInTree(focused)) setFocused(nullptr, false);
}

void Surface::clearKeyboardPress() {
    if (Element* pressed = mKeyPressed) pressed->setState(ElementState::Active, false);
    mKeyPressed = nullptr;
    mPressedKey = 0;
}

void Surface::updatePressedState() {
    if (Element* pressed = mPressed) pressed->setState(ElementState::Active, mHovered == pressed);
}

void Surface::elementBecameUnavailable(Element&) {
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    if (mScrollbarCapture && (!mScrollbarCapture->element || !isEnabledInTree(mScrollbarCapture->element))) {
        mScrollbarCapture.reset();
        requestPaint();
    }
    if (mScrollbarHover && (!mScrollbarHover->element || !isEnabledInTree(mScrollbarHover->element))) setScrollbarHover(std::nullopt);
    if (Element* captured = mCaptured; captured && !isEnabledInTree(captured)) {
        mCaptured = nullptr;
        captured->endPointerInteraction({mPointerPosition});
        if (surfaceLifetime.expired()) return;
        mResizeCursor = CursorStyle::Auto;
    }
    if (Element* pressed = mPressed; pressed && !isEnabledInTree(pressed)) {
        pressed->setState(ElementState::Active, false);
        mPressed = nullptr;
    }
    if (Element* hovered = mHovered; hovered && !isEnabledInTree(hovered)) setHovered(nullptr);
    if (Element* keyPressed = mKeyPressed; keyPressed && !isEnabledInTree(keyPressed)) clearKeyboardPress();
    validateFocus();
}

bool Surface::moveFocus(bool backwards) {
    std::vector<ElementRef<Element>> focusable;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto collectLayer = [&](SurfaceLayer layer) {
        for (const MountPtr& mount : mounts(layer))
            if (mount && mount->root) collectFocusable(*mount->root, focusable, styles);
    };
    if (hasActiveModal()) collectLayer(SurfaceLayer::Modal);
    else {
        collectLayer(SurfaceLayer::Base);
        collectLayer(SurfaceLayer::Floater);
        collectLayer(SurfaceLayer::Popup);
    }
    if (focusable.empty()) return false;

    focusable.erase(
        std::remove_if(focusable.begin(), focusable.end(), [this](const ElementRef<Element>& ref) { return !ref || !isFocusableInTree(ref.get()); }),
        focusable.end());
    if (focusable.empty()) return false;

    const auto current = std::find_if(focusable.begin(), focusable.end(), [this](const ElementRef<Element>& ref) { return ref.get() == mFocused; });
    ElementRef<Element> next;
    if (current == focusable.end()) next = backwards ? focusable.back() : focusable.front();
    else if (backwards) next = current == focusable.begin() ? focusable.back() : *(current - 1);
    else next = std::next(current) == focusable.end() ? focusable.front() : *std::next(current);
    if (Element* focused = next.get()) setFocused(focused, true);
    return true;
}
} // namespace radia::ui
