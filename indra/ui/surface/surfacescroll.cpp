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

bool acceptsWheelScrolling(Overflow overflow) {
    return overflow == Overflow::Auto || overflow == Overflow::Scroll;
}

Vec2 consumeWheelDelta(Element& element, const ComputedStyle& style, const Vec2& delta) {
    const float currentLeft = element.scrollLeft();
    const float currentTop = element.scrollTop();
    const float nextLeft =
        acceptsWheelScrolling(style.overflowX) ? std::clamp(currentLeft + delta.x, 0.f, element.scrollMetrics().maxScrollLeft) : currentLeft;
    const float nextTop =
        acceptsWheelScrolling(style.overflowY) ? std::clamp(currentTop + delta.y, 0.f, element.scrollMetrics().maxScrollTop) : currentTop;
    if (nextLeft != currentLeft || nextTop != currentTop) element.scrollTo(nextLeft, nextTop);
    return {nextLeft - currentLeft, nextTop - currentTop};
}
} // namespace

std::optional<Surface::ScrollbarTarget> Surface::hitTestScrollbarNode(Element& node, const Vec2& point, const Rect& inheritedClip,
                                                                      StylePass& styles) const {
    if (!inheritedClip.contains(point) || !isRootedInSurface(&node)) return std::nullopt;
    const ElementObservation observation = observe(node);
    const ComputedStyle style = styles.style(node);
    if (!node.isVisible(style)) return std::nullopt;
    Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid() || !isRootedInSurface(current) || !current->isVisible(style))
        return std::nullopt;

    if (acceptsPointerEvents(*current, style)) {
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
    if (clipsChildren) childClip = {childClip.x + scrollOffset.x, childClip.y + scrollOffset.y, childClip.w, childClip.h};
    const Vec2 childPoint = point + scrollOffset;
    const auto children = styles.sourceChildren(*current);
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
        const MountList& layerMounts = mounts(layer);
        for (auto current = layerMounts.rbegin(); current != layerMounts.rend(); ++current)
            if (*current && (*current)->root)
                if (std::optional<ScrollbarTarget> hit = hitTestScrollbarNode(*(*current)->root, point, mViewport, styles)) return hit;
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
        const ComputedStyle& style = styles.style(*element);
        if (!acceptsPointerEvents(*element, style)) return false;
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
        const ComputedStyle& style = styles.style(*element);
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
        const ComputedStyle& style = styles.style(*element);
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
        const auto tryAxis = [&](Element& element, const ComputedStyle& style, ScrollbarAxis axis) {
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
            const ComputedStyle& style = styles.style(*candidate);
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

            const ComputedStyle style = styles.style(*candidate);
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
} // namespace radia::ui
