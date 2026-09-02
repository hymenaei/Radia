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

namespace {
constexpr float kScrollbarLineStep = 40.f;
constexpr float kScrollbarButtonRepeatDelay = .4f;
constexpr float kScrollbarButtonRepeatInterval = .05f;

float scrollbarArrowDelta(ScrollbarPart part) {
    if (part == ScrollbarPart::StartArrow) return -kScrollbarLineStep;
    if (part == ScrollbarPart::EndArrow) return kScrollbarLineStep;
    return 0.f;
}

Vec2 defaultWheelDelta(const WheelEvent& event, LayoutDirection direction) {
    const bool shiftToHorizontal = (event.modifiers & kModifierShift) && event.dx == 0.f;
    const float horizontal = shiftToHorizontal ? event.dy : event.dx;
    const float vertical = shiftToHorizontal ? 0.f : event.dy;
    return {direction == LayoutDirection::RightToLeft ? -horizontal : horizontal, vertical};
}

bool acceptsPointerEvents(const Element& element, const Style& style) {
    const PointerEvents policy = style.pointerEvents;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return element.pointerEvents();
}

Rect offsetRect(const Rect& rect, const Vec2& offset) {
    return {rect.x + offset.x, rect.y + offset.y, rect.w, rect.h};
}

bool acceptsWheelScrolling(Overflow overflow) {
    return overflow == Overflow::Auto || overflow == Overflow::Scroll;
}

Vec2 consumeWheelDelta(Element& element, const Style& style, const Vec2& delta) {
    const float currentLeft = element.scrollLeft();
    const float currentTop = element.scrollTop();
    const float nextLeft =
        acceptsWheelScrolling(style.overflowX) ? std::clamp(currentLeft + delta.x, 0.f, element.scrollMetrics().maxScrollLeft) : currentLeft;
    const float nextTop =
        acceptsWheelScrolling(style.overflowY) ? std::clamp(currentTop + delta.y, 0.f, element.scrollMetrics().maxScrollTop) : currentTop;
    if (nextLeft != currentLeft || nextTop != currentTop) element.scrollTo(nextLeft, nextTop);
    return {nextLeft - currentLeft, nextTop - currentTop};
}

void collectFocusable(Element& node, std::vector<ElementRef<Element>>& result, StylePass& styles) {
    const ElementVisit observation(node);
    const Style style = styles.style(node);
    Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !current->isVisible(style) || current->disabled()) return;
    if (current->focusable()) result.emplace_back(current);
    const StylePass::ChildSnapshot children = styles.sourceChildren(*current);
    for (const ElementRef<Element>& childRef : *children)
        if (Element* child = childRef.get(); child && child->parentElement() == current) collectFocusable(*child, result, styles);
}
} // namespace

Element* Surface::hitTestNode(Element& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const {
    if (!inheritedClip.contains(point) || !isRootedInSurface(&node)) return nullptr;
    const ElementObservation observation = observe(node);
    const Style style = styles.style(node);
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
    if (clipsChildren) childClip = offsetRect(childClip, scrollOffset);
    const Vec2 childPoint = point + scrollOffset;
    const StylePass::ChildSnapshot children = styles.sourceChildren(*current);
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
    std::vector<ElementRef<Element>> route;
    std::vector<const Element*> routeParents;
    for (Element* current = event.target(); current; current = current->parentElement()) {
        route.emplace_back(current);
        routeParents.push_back(current->parentElement());
        if (isSurfaceRoot(current)) break;
    }
    if (route.empty() || !route.back() || !isSurfaceRoot(route.back().get()) || !isRootedInSurface(route.front().get())) return false;

    std::vector<Element::EventListenerSnapshot> listenerSnapshots;
    listenerSnapshots.reserve(route.size());
    for (const ElementRef<Element>& elementRef : route) {
        Element* element = elementRef.get();
        if (!element) return false;
        listenerSnapshots.push_back(element->eventListenerSnapshot());
    }

    event.setPhase(EventPhase::Capture);
    for (std::size_t index = route.size() - 1; index > 0; --index) {
        Element* target = route[index].get();
        Element* child = route[index - 1].get();
        if (!target || !child || target->parentElement() != routeParents[index] || child->parentElement() != target || !isRootedInSurface(target))
            break;
        event.setCurrentTarget(target);
        target->dispatchListeners(event, true, listenerSnapshots[index]);
        if (!route[index]) {
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
    if (!target || !isRootedInSurface(target)) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    if (target->parentElement() != routeParents.front() || (route.size() > 1 && target->parentElement() != route[1].get())) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    event.setCurrentTarget(target);
    target->dispatchListeners(event, true, listenerSnapshots.front());
    if (!route.front()) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    if (!event.immediatePropagationStopped()) target->dispatchListeners(event, false, listenerSnapshots.front());
    if (!route.front()) {
        event.setCurrentTarget(nullptr);
        return event.handled() || event.defaultPrevented();
    }
    if (!event.propagationStopped()) {
        event.setPhase(EventPhase::Bubble);
        for (std::size_t index = 1; index < route.size(); ++index) {
            Element* bubbleTarget = route[index].get();
            Element* child = route[index - 1].get();
            if (!bubbleTarget
                || !child
                || bubbleTarget->parentElement() != routeParents[index]
                || child->parentElement() != bubbleTarget
                || !isRootedInSurface(bubbleTarget))
                break;
            event.setCurrentTarget(bubbleTarget);
            bubbleTarget->dispatchListeners(event, false, listenerSnapshots[index]);
            if (!route[index]) {
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
    const RootList& modalRoots = roots(SurfaceLayer::Modal);
    return std::any_of(modalRoots.begin(), modalRoots.end(), [&styles](const auto& root) { return root->isVisible(styles.style(*root)); });
}

Element* Surface::hitTestAt(const Vec2& point) {
    if (!mViewport.contains(point)) return nullptr;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto hitInLayer = [&](SurfaceLayer layer) -> Element* {
        const RootList& layerRoots = roots(layer);
        for (auto current = layerRoots.rbegin(); current != layerRoots.rend(); ++current)
            if (Element* hit = hitTestNode(**current, point, mViewport, styles)) return hit;
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

std::optional<Surface::ScrollbarTarget> Surface::hitTestScrollbarNode(Element& node, const Vec2& point, const Rect& inheritedClip,
                                                                      StylePass& styles) const {
    if (!inheritedClip.contains(point) || !isRootedInSurface(&node)) return std::nullopt;
    const ElementObservation observation = observe(node);
    const Style style = styles.style(node);
    if (!node.isVisible(style)) return std::nullopt;
    Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return std::nullopt;

    if (style.pointerEvents != PointerEvents::PassThrough) {
        const ScrollGeometry geometry = scrollbarGeometry(*current, style);
        const ScrollbarHit hit = hitTestScrollbar(geometry, point);
        if (hit.valid()) return ScrollbarTarget{current, geometry, hit};
    }

    const bool clipsX = style.overflowX != Overflow::Visible;
    const bool clipsY = style.overflowY != Overflow::Visible;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    const bool clipsChildren = clipAxes != ClipAxes::NoAxes;
    const Vec2 scrollTranslation = scrollContentTranslation(layoutDirection(), {current->scrollLeft(), current->scrollTop()});
    const Vec2 scrollOffset = clipsChildren ? Vec2{-scrollTranslation.x, -scrollTranslation.y} : Vec2{};
    Rect childClip = clipsChildren ? clipToAxes(inheritedClip, ElementInternalAccess::scrollport(*current), clipAxes) : inheritedClip;
    if (clipsChildren) childClip = offsetRect(childClip, scrollOffset);
    const Vec2 childPoint = point + scrollOffset;
    const StylePass::ChildSnapshot children = styles.sourceChildren(*current);
    for (auto child = children->rbegin(); child != children->rend(); ++child)
        if (Element* childElement = child->get())
            if (childElement->parentElement() == current)
                if (std::optional<ScrollbarTarget> hit = hitTestScrollbarNode(*childElement, childPoint, childClip, styles)) return hit;

    current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return std::nullopt;
    return std::nullopt;
}

std::optional<Surface::ScrollbarTarget> Surface::hitTestScrollbarAt(const Vec2& point) {
    if (!mViewport.contains(point)) return std::nullopt;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto hitInLayer = [&](SurfaceLayer layer) -> std::optional<ScrollbarTarget> {
        const RootList& layerRoots = roots(layer);
        for (auto current = layerRoots.rbegin(); current != layerRoots.rend(); ++current)
            if (std::optional<ScrollbarTarget> hit = hitTestScrollbarNode(**current, point, mViewport, styles)) return hit;
        return std::nullopt;
    };
    if (hasActiveModal()) return hitInLayer(SurfaceLayer::Modal);

    for (std::size_t index = static_cast<std::size_t>(SurfaceLayer::Modal); index > static_cast<std::size_t>(SurfaceLayer::Base); --index) {
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index);
        if (layer == SurfaceLayer::Tooltip || layer == SurfaceLayer::Drag || layer == SurfaceLayer::Modal) continue;
        if (std::optional<ScrollbarTarget> hit = hitInLayer(layer)) return hit;
    }
    return hitInLayer(SurfaceLayer::Base);
}

namespace {
const ScrollbarAxisGeometry* scrollbarAxisGeometry(const ScrollGeometry& geometry, ScrollbarAxis axis) {
    if (axis == ScrollbarAxis::Horizontal) return &geometry.horizontal;
    if (axis == ScrollbarAxis::Vertical) return &geometry.vertical;
    return nullptr;
}
} // namespace

void Surface::setScrollbarHover(std::optional<ScrollbarTarget> target) {
    const bool unchanged = !mScrollbarHover ? !target
                                            : target
            && mScrollbarHover->element == target->element
            && mScrollbarHover->hit.axis == target->hit.axis
            && mScrollbarHover->hit.part == target->hit.part;
    if (unchanged) return;
    mScrollbarHover = std::move(target);
    requestPaint();
}

bool Surface::beginScrollbarInteraction(const ScrollbarTarget& target, const Vec2& point) {
    Element* element = target.element;
    if (!element || !target.hit.valid() || !isRootedInSurface(element) || !isEnabledInTree(element)) return false;

    ScrollbarInteraction interaction;
    {
        StylePass& styles = stylePass();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        const Style& style = styles.style(*element);
        const ScrollGeometry geometry = scrollbarGeometry(*element, style);
        const ScrollbarAxisGeometry* axis = scrollbarAxisGeometry(geometry, target.hit.axis);
        if (!axis || !axis->visible) return false;

        interaction.element = element;
        interaction.geometry = geometry;
        interaction.hit = target.hit;
        if (target.hit.part == ScrollbarPart::Thumb) {
            if (axis->thumb.empty()) return false;
            const float thumbStart = target.hit.axis == ScrollbarAxis::Horizontal ? axis->thumb.left() : axis->thumb.bottom();
            interaction.grabOffset = scrollbarAxisPosition(target.hit.axis, point) - thumbStart;
        } else {
            float delta = scrollbarArrowDelta(target.hit.part);
            if (target.hit.part == ScrollbarPart::Track) {
                const float thumbStart = target.hit.axis == ScrollbarAxis::Horizontal ? axis->thumb.left() : axis->thumb.bottom();
                bool beforeThumb;
                if (target.hit.axis == ScrollbarAxis::Horizontal) beforeThumb = scrollbarAxisPosition(target.hit.axis, point) < thumbStart;
                else beforeThumb = scrollbarAxisPosition(target.hit.axis, point) > axis->thumb.top();
                if (axis->reversed) beforeThumb = !beforeThumb;
                const float viewport = target.hit.axis == ScrollbarAxis::Horizontal ? element->clientWidth() : element->clientHeight();
                const float page = std::max(1.f, viewport - kScrollbarLineStep);
                delta = beforeThumb ? -page : page;
            }
            if (target.hit.axis == ScrollbarAxis::Horizontal) element->scrollBy(delta, 0.f);
            else element->scrollBy(0.f, delta);
            interaction.geometry = scrollbarGeometry(*element, style);
            if (target.hit.part == ScrollbarPart::Track) {
                const ScrollbarAxisGeometry* updatedAxis = scrollbarAxisGeometry(interaction.geometry, target.hit.axis);
                if (updatedAxis && !updatedAxis->thumb.empty()) {
                    interaction.hit.part = ScrollbarPart::Thumb;
                    const float pointerPosition = scrollbarAxisPosition(target.hit.axis, point);
                    const float thumbStart = target.hit.axis == ScrollbarAxis::Horizontal ? updatedAxis->thumb.left() : updatedAxis->thumb.bottom();
                    const float thumbLength = target.hit.axis == ScrollbarAxis::Horizontal ? updatedAxis->thumb.w : updatedAxis->thumb.h;
                    interaction.grabOffset = updatedAxis->thumb.contains(point) ? pointerPosition - thumbStart : thumbLength * .5f;
                }
            }
        }
    }

    clearKeyboardPress();
    if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
    mPressed = nullptr;
    mPressedClickCount = 0;
    mCaptured = nullptr;
    setHovered(nullptr);
    setFocused(nullptr, false);
    mResizeCursor = CursorStyle::Auto;
    mScrollbarCapture = std::move(interaction);
    setScrollbarHover(ScrollbarTarget{element, mScrollbarCapture->geometry, mScrollbarCapture->hit});
    flushScrollNotifications();
    requestPaint();
    return true;
}

void Surface::advanceScrollbarInteraction(float deltaSeconds) {
    if (!mScrollbarCapture) return;
    ScrollbarInteraction& interaction = *mScrollbarCapture;
    if (interaction.hit.part != ScrollbarPart::StartArrow && interaction.hit.part != ScrollbarPart::EndArrow) return;

    ElementRef<Element> elementRef(interaction.element);
    Element* element = elementRef.get();
    if (!element || !isRootedInSurface(element) || !isEnabledInTree(element)) {
        mScrollbarCapture.reset();
        setScrollbarHover(std::nullopt);
        requestPaint();
        return;
    }
    if (!(deltaSeconds > 0.f)) return;

    interaction.repeatElapsed += std::min(deltaSeconds, 1.f);
    std::size_t repeatCount = 0;
    if (!interaction.repeatStarted) {
        if (interaction.repeatElapsed < kScrollbarButtonRepeatDelay) return;
        interaction.repeatElapsed -= kScrollbarButtonRepeatDelay;
        interaction.repeatStarted = true;
        repeatCount = 1;
    }
    const std::size_t intervalCount = static_cast<std::size_t>(interaction.repeatElapsed / kScrollbarButtonRepeatInterval);
    interaction.repeatElapsed -= intervalCount * kScrollbarButtonRepeatInterval;
    repeatCount += intervalCount;
    if (repeatCount == 0) return;

    const float delta = scrollbarArrowDelta(interaction.hit.part);
    for (std::size_t index = 0; index < repeatCount; ++index)
        if (interaction.hit.axis == ScrollbarAxis::Horizontal) element->scrollBy(delta, 0.f);
        else element->scrollBy(0.f, delta);

    element = elementRef.get();
    if (!element || !isRootedInSurface(element) || !isEnabledInTree(element)) {
        mScrollbarCapture.reset();
        setScrollbarHover(std::nullopt);
        requestPaint();
        return;
    }
    {
        StylePass& styles = stylePass();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        const Style& style = styles.style(*element);
        interaction.geometry = scrollbarGeometry(*element, style);
    }
    setScrollbarHover(ScrollbarTarget{element, interaction.geometry, interaction.hit});
    flushScrollNotifications();
    requestPaint();
}

bool Surface::updateScrollbarInteraction(const Vec2& point) {
    if (!mScrollbarCapture) return false;
    Element* element = mScrollbarCapture->element;
    ElementRef<Element> elementRef(element);
    if (!element || !elementRef || !isRootedInSurface(element) || !isEnabledInTree(element)) {
        mScrollbarCapture.reset();
        setScrollbarHover(std::nullopt);
        requestPaint();
        return true;
    }

    std::optional<ScrollbarTarget> updatedTarget;
    bool unavailable = false;
    {
        StylePass& styles = stylePass();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        const Style& style = styles.style(*element);
        const ScrollGeometry geometry = scrollbarGeometry(*element, style);
        const ScrollbarAxis axis = mScrollbarCapture->hit.axis;
        const ScrollbarAxisGeometry* axisGeometry = scrollbarAxisGeometry(geometry, axis);
        if (!axisGeometry || !axisGeometry->visible) return true;
        if (mScrollbarCapture->hit.part == ScrollbarPart::Thumb) {
            const float offset = scrollOffsetForThumbPosition(*axisGeometry, scrollbarAxisPosition(axis, point), mScrollbarCapture->grabOffset);
            if (axis == ScrollbarAxis::Horizontal) element->scrollTo(offset, element->scrollTop());
            else element->scrollTo(element->scrollLeft(), offset);
        }

        element = elementRef.get();
        if (!element || !isRootedInSurface(element) || !isEnabledInTree(element)) unavailable = true;
        else {
            const ScrollGeometry updatedGeometry = scrollbarGeometry(*element, style);
            mScrollbarCapture->element = element;
            mScrollbarCapture->geometry = updatedGeometry;
            updatedTarget = ScrollbarTarget{element, updatedGeometry, mScrollbarCapture->hit};
        }
    }
    if (unavailable) {
        mScrollbarCapture.reset();
        setScrollbarHover(std::nullopt);
        requestPaint();
        return true;
    }
    setScrollbarHover(std::move(updatedTarget));
    flushScrollNotifications();
    return true;
}

bool Surface::scrollFocusedElement(const KeyEvent& event, Element& focused) {
    if (event.modifiers & (kModifierControl | kModifierAlt | kModifierPlatformControl)) return false;

    enum class ScrollAction { NoneValue, LineBackward, LineForward, PageBackward, PageForward, Home, End };
    ScrollAction action = ScrollAction::NoneValue;
    if (event.key == kKeyUp || event.key == kKeyLeft) action = ScrollAction::LineBackward;
    else if (event.key == kKeyDown || event.key == kKeyRight) action = ScrollAction::LineForward;
    else if (event.key == kKeyPageUp) action = ScrollAction::PageBackward;
    else if (event.key == kKeyPageDown) action = ScrollAction::PageForward;
    else if (event.key == kKeyHome) action = ScrollAction::Home;
    else if (event.key == kKeyEnd) action = ScrollAction::End;
    else if (event.key == kKeySpace) action = (event.modifiers & kModifierShift) ? ScrollAction::PageBackward : ScrollAction::PageForward;
    else return false;

    const bool horizontalKey = event.key == kKeyLeft || event.key == kKeyRight;
    bool handled = false;
    {
        StylePass& styles = stylePass();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        const auto tryAxis = [&](Element& element, const Style& style, ScrollbarAxis axis) {
            const Overflow overflow = axis == ScrollbarAxis::Horizontal ? style.overflowX : style.overflowY;
            if (!acceptsWheelScrolling(overflow)) return false;
            const float current = axis == ScrollbarAxis::Horizontal ? element.scrollLeft() : element.scrollTop();
            const float maximum = axis == ScrollbarAxis::Horizontal ? element.scrollMetrics().maxScrollLeft : element.scrollMetrics().maxScrollTop;
            const float viewport = axis == ScrollbarAxis::Horizontal ? element.clientWidth() : element.clientHeight();
            const float page = std::max(1.f, viewport - kScrollbarLineStep);
            float next = current;
            switch (action) {
                case ScrollAction::LineBackward: next = current - kScrollbarLineStep; break;
                case ScrollAction::LineForward: next = current + kScrollbarLineStep; break;
                case ScrollAction::PageBackward: next = current - page; break;
                case ScrollAction::PageForward: next = current + page; break;
                case ScrollAction::Home: next = 0.f; break;
                case ScrollAction::End: next = maximum; break;
                case ScrollAction::NoneValue: return false;
            }
            next = std::clamp(next, 0.f, maximum);
            if (next == current) return false;
            if (axis == ScrollbarAxis::Horizontal) element.scrollTo(next, element.scrollTop());
            else element.scrollTo(element.scrollLeft(), next);
            return true;
        };

        for (Element* candidate = &focused; candidate;) {
            ElementRef<Element> candidateRef(candidate);
            const Style& style = styles.style(*candidate);
            candidate = candidateRef.get();
            if (!candidate || !isRootedInSurface(candidate) || !isEnabledInTree(candidate)) break;
            if (horizontalKey) {
                if (tryAxis(*candidate, style, ScrollbarAxis::Horizontal)) {
                    handled = true;
                    break;
                }
            } else {
                if (tryAxis(*candidate, style, ScrollbarAxis::Vertical)) {
                    handled = true;
                    break;
                }
                if ((event.key == kKeyHome || event.key == kKeyEnd) && tryAxis(*candidate, style, ScrollbarAxis::Horizontal)) {
                    handled = true;
                    break;
                }
            }
            candidate = candidate->parentElement();
        }
    }
    if (handled) flushScrollNotifications();
    return handled;
}

void Surface::clearInteractionState() {
    if (Element* hovered = mHovered) hovered->setState(ElementState::Hovered, false);
    if (Element* pressed = mPressed) pressed->setState(ElementState::Active, false);
    clearKeyboardPress();
    if (Element* focused = mFocused) {
        focused->setState(ElementState::Focused, false);
        focused->setState(ElementState::FocusVisible, false);
    }
    if (Element* captured = mCaptured) captured->endPointerInteraction({mPointerPosition});
    mHovered = nullptr;
    mPressed = nullptr;
    mFocused = nullptr;
    mCaptured = nullptr;
    mResizeCursor = CursorStyle::Auto;
    mScrollbarHover.reset();
    mScrollbarCapture.reset();
    mPressedClickCount = 0;
    mTabKeyHandled = false;
}

CursorStyle Surface::cursor() const {
    const ScrollbarTarget* scrollbar = mScrollbarCapture ? &*mScrollbarCapture : (mScrollbarHover ? &*mScrollbarHover : nullptr);
    if (mResizeCursor != CursorStyle::Auto) return mResizeCursor;
    if (scrollbar) return CursorStyle::Default;
    const Element* element = mCaptured ? mCaptured : mHovered;
    if (!element) return CursorStyle::Default;
    const ConstElementObservation observation = observe(*element);
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style style = styles.style(*element);
    const Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return CursorStyle::Default;
    const CursorStyle cursor = style.cursor;
    return cursor == CursorStyle::Auto ? CursorStyle::Default : cursor;
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

bool Surface::scroll(const WheelEvent& event) {
    updateLayout();
    mPointerPosition = event.position;
    mPointerPositionKnown = true;
    ElementRef<Element> hitRef(hitTestAt(event.position));
    Element* hit = hitRef.get();
    bool routedHandled = false;
    bool defaultPrevented = false;
    if (isEnabledInTree(hit)) {
        Event routed(kWheelEvent, *hit, event);
        routedHandled = routeEvent(routed);
        defaultPrevented = routed.defaultPrevented();
        hit = hitRef.get();
    }
    bool scrollHandled = false;
    if (!defaultPrevented) {
        StylePass& styles = stylePass();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        Vec2 remaining = defaultWheelDelta(event, layoutDirection());
        for (Element* candidate = hit; candidate;) {
            ElementRef<Element> candidateRef(candidate);
            ElementRef<Element> parentRef(candidate ? candidate->parentElement() : nullptr);
            const ElementObservation candidateObservation = candidate ? observe(*candidate) : ElementObservation{};
            if (!candidateRef || !isEnabledInTree(candidate) || !isRootedInSurface(candidate)) break;

            WheelEvent defaultEvent = event;
            defaultEvent.dx = remaining.x;
            defaultEvent.dy = remaining.y;
            if ((remaining.x != 0.f || remaining.y != 0.f) && candidate->defaultScroll(defaultEvent)) {
                scrollHandled = true;
                break;
            }
            candidate = candidateRef.get();
            if (!candidate
                || !candidateObservation.layoutValid()
                || !candidateObservation.styleValid()
                || !isEnabledInTree(candidate)
                || !isRootedInSurface(candidate))
                break;

            const Style style = styles.style(*candidate);
            const Vec2 consumed = consumeWheelDelta(*candidate, style, remaining);
            remaining = remaining - consumed;
            if (consumed.x != 0.f || consumed.y != 0.f) scrollHandled = true;
            if (remaining.x == 0.f && remaining.y == 0.f) break;
            candidate = parentRef.get();
        }
    }
    flushScrollNotifications();
    return routedHandled || scrollHandled || hitRef.get() != nullptr;
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
    Event routed(kCharacterInputEvent, *focused, Event::Payload(codepoint));
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
    if (mScrollbarCapture && (!mScrollbarCapture->element || !isEnabledInTree(mScrollbarCapture->element))) {
        mScrollbarCapture.reset();
        requestPaint();
    }
    if (mScrollbarHover && (!mScrollbarHover->element || !isEnabledInTree(mScrollbarHover->element))) setScrollbarHover(std::nullopt);
    if (Element* captured = mCaptured; captured && !isEnabledInTree(captured)) {
        mCaptured = nullptr;
        captured->endPointerInteraction({mPointerPosition});
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
        for (const auto& root : roots(layer)) collectFocusable(*root, focusable, styles);
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
