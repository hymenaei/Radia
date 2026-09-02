/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/floater.h"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <string_view>
#include "html/button.h"
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "layout/engine.h"
#include "resource/elementdefinition.h"
#include "style/style.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "system.h"

namespace radia::ui {
using detail::resizedRect;
using detail::ResizeEdges;

HTMLMinimizeButtonElement::HTMLMinimizeButtonElement() : HTMLButtonElement(kMinimizeTag.localName) {}

HTMLCloseButtonElement::HTMLCloseButtonElement() : HTMLButtonElement(kCloseTag.localName) {}

namespace {
void findAuthoredHeadElements(Element& root, Element*& title, HTMLButtonElement*& minimize, HTMLButtonElement*& close) {
    for (Element* child : root.children()) {
        if (child->elementName() == kTitleTag.localName) {
            if (!title) title = child;
        } else if (child->elementName() == kMinimizeTag.localName) {
            if (!minimize) minimize = dynamic_cast<HTMLButtonElement*>(child);
        } else if (child->elementName() == kCloseTag.localName) {
            if (!close) close = dynamic_cast<HTMLButtonElement*>(child);
        }

        findAuthoredHeadElements(*child, title, minimize, close);
    }
}

std::size_t countAuthoredElements(const Element& root, std::string_view elementName) {
    std::size_t count = 0;
    for (const Element* child : root.children()) {
        if (child->elementName() == elementName) ++count;
        count += countAuthoredElements(*child, elementName);
    }
    return count;
}

void clearAuthoredCallbacks(Element& root) {
    if (root.elementName() == kCloseTag.localName || root.elementName() == kMinimizeTag.localName) root.setOnActivate({});
    for (Element* child : root.children()) clearAuthoredCallbacks(*child);
}
} // namespace

HTMLFloaterElement::HTMLFloaterElement() : HTMLElement(kFloaterTag.localName) {}

std::string HTMLFloaterElement::title() const {
    return mTitleElement ? mTitleElement->textContent() : std::string();
}

HTMLFloaterElement& HTMLFloaterElement::setResizeable(bool value) {
    mResizeable = value;
    if (value) setAttribute("resizeable");
    else removeAttribute("resizeable");
    return *this;
}

void HTMLFloaterElement::setMovementBounds(const Rect& bounds) {
    mMovementBounds = bounds;
}

void HTMLFloaterElement::refreshAuthoredStructure() {
    mHead = nullptr;
    mBody = nullptr;
    mTitleElement = nullptr;
    mCloseButton = nullptr;
    mMinimizeButton = nullptr;

    for (Element* child : children()) {
        if (child->elementName() == kHeadTag.localName) {
            if (!mHead) mHead = child;
            continue;
        }
        if (child->elementName() == kBodyTag.localName) {
            if (!mBody) mBody = child;
            continue;
        }
    }

    if (mHead) findAuthoredHeadElements(*mHead, mTitleElement, mMinimizeButton, mCloseButton);

    mClosable = mCloseButton != nullptr;
    mMinimizable = mMinimizeButton != nullptr;
    if (mCloseButton) mCloseButton->setOnActivate([this](Element&) { close(); });
    if (mMinimizeButton) mMinimizeButton->setOnActivate([this](Element&) { toggleMinimized(); });
}

ResourceElementDefinition detail::ElementDefinitions::floater() {
    return defineElement<HTMLFloaterElement>(kFloaterTag.localName)
        .attributes({booleanAttribute("resizeable", &HTMLFloaterElement::setResizeable)})
        .composition([](const ElementBuildInput& input, HTMLFloaterElement& floater, const ElementScopeContext&, ElementBuildContext& context) {
            const std::size_t headCount = countAuthoredElements(floater, kHeadTag.localName);
            const std::size_t bodyCount = countAuthoredElements(floater, kBodyTag.localName);
            std::size_t directHeadCount = 0;
            std::size_t directBodyCount = 0;
            for (Element* child : floater.children())
                if (child->elementName() == kHeadTag.localName) ++directHeadCount;
                else if (child->elementName() == kBodyTag.localName) ++directBodyCount;
                else
                    context.error("layout.floater.child_invalid", "A floater may contain only one <head> and one <body>.", input.sourceName,
                                  input.source.begin.line, input.source.begin.column);
            if (directHeadCount == 0)
                context.error("layout.floater.head_required", "A floater requires one direct <head> child.", input.sourceName,
                              input.source.begin.line, input.source.begin.column);
            else if (headCount > 1)
                context.error("layout.floater.head_duplicate", "A floater may contain only one <head>.", input.sourceName, input.source.begin.line,
                              input.source.begin.column);
            if (directBodyCount == 0)
                context.error("layout.floater.body_required", "A floater requires one direct <body> child.", input.sourceName,
                              input.source.begin.line, input.source.begin.column);
            else if (bodyCount > 1)
                context.error("layout.floater.body_duplicate", "A floater may contain only one <body>.", input.sourceName, input.source.begin.line,
                              input.source.begin.column);

            if (!floater.head()) return;
            const std::size_t titleCount = countAuthoredElements(*floater.head(), kTitleTag.localName);
            const std::size_t minimizeCount = countAuthoredElements(*floater.head(), kMinimizeTag.localName);
            const std::size_t closeCount = countAuthoredElements(*floater.head(), kCloseTag.localName);
            if (titleCount > 1)
                context.error("layout.floater.title_duplicate", "A floater may contain only one <title>.", input.sourceName, input.source.begin.line,
                              input.source.begin.column);
            if (minimizeCount > 1)
                context.error("layout.floater.minimize_duplicate", "A floater may contain only one <minimize>.", input.sourceName,
                              input.source.begin.line, input.source.begin.column);
            if (closeCount > 1)
                context.error("layout.floater.close_duplicate", "A floater may contain only one <close>.", input.sourceName, input.source.begin.line,
                              input.source.begin.column);
            if (floater.body()) {
                if (countAuthoredElements(*floater.body(), kTitleTag.localName) > 0)
                    context.error("layout.floater.head_only", "<title> must be inside a floater <head>.", input.sourceName, input.source.begin.line,
                                  input.source.begin.column);
                if (countAuthoredElements(*floater.body(), kMinimizeTag.localName) > 0)
                    context.error("layout.floater.head_only", "<minimize> must be inside a floater <head>.", input.sourceName,
                                  input.source.begin.line, input.source.begin.column);
                if (countAuthoredElements(*floater.body(), kCloseTag.localName) > 0)
                    context.error("layout.floater.head_only", "<close> must be inside a floater <head>.", input.sourceName, input.source.begin.line,
                                  input.source.begin.column);
            }
            if (floater.minimizable() && floater.title().empty())
                context.error("layout.floater.title_required", "A minimizable floater requires a non-empty <title>.", input.sourceName,
                              input.source.begin.line, input.source.begin.column);
        })
        .state(ElementState::Minimized)
        .build();
}

ResourceElementDefinition detail::ElementDefinitions::minimize() {
    return defineElement<HTMLMinimizeButtonElement>(kMinimizeTag.localName).build();
}

ResourceElementDefinition detail::ElementDefinitions::close() {
    return defineElement<HTMLCloseButtonElement>(kCloseTag.localName).build();
}

void HTMLFloaterElement::onChildAdded(Element&) {
    refreshAuthoredStructure();
}

void HTMLFloaterElement::onChildRemoved(Element& child) {
    clearAuthoredCallbacks(child);
    refreshAuthoredStructure();
}

void HTMLFloaterElement::onDescendantAdded(Element&) {
    refreshAuthoredStructure();
}

void HTMLFloaterElement::onDescendantRemoved(Element& child) {
    clearAuthoredCallbacks(child);
    refreshAuthoredStructure();
}

void HTMLFloaterElement::onLocaleChanged(const System& system) {
    Element::onLocaleChanged(system);
}

HTMLFloaterElement& HTMLFloaterElement::setLifecycleCallbacks(std::function<void()> onOpen, std::function<void()> onClose) {
    mOnOpen = std::move(onOpen);
    mOnClose = std::move(onClose);
    return *this;
}

void HTMLFloaterElement::open() {
    const bool wasClosed = mClosed;
    mClosed = false;
    setVisibility(Visibility::Visible);
    setDisplayNone(false);
    if (wasClosed && mOnOpen) mOnOpen();
}

void HTMLFloaterElement::close() {
    if (mClosed || !mClosable) return;
    mClosed = true;
    mInteraction = FloaterInteraction::Idle;
    setVisibility(Visibility::Hidden);
    setDisplayNone(true);
    if (Surface* surface = this->surface()) surface->floaterClosed(*this);
    if (mOnClose) mOnClose();
}

void HTMLFloaterElement::setMinimized(bool minimized) {
    if ((minimized && !mMinimizable) || minimized == mMinimized || !mHead) return;

    if (minimized) {
        mExpandedRect = rect();
        mMinimized = true;
        setState(ElementState::Minimized, true);

        float width = rect().w;
        float height = mHead->rect().h;
        if (const StyleSheet* styleSheet = this->styleSheet()) {
            const Vec2 headSize = measureElement(*mHead, *styleSheet, textMetrics());
            const Style floaterStyle = resolveElementStyle(*styleSheet, *this);
            const Style headStyle = resolveElementStyle(*styleSheet, *mHead);
            width = headSize.x + headStyle.margin.horizontal() + floaterStyle.padding.horizontal();
            height = headSize.y + headStyle.margin.vertical() + floaterStyle.padding.vertical();
        }
        if (mMovementBounds.w > 0.f) width = std::min(width, mMovementBounds.w);
        setRect({rect().x, rect().top() - height, width, height});
    } else {
        mMinimized = false;
        setState(ElementState::Minimized, false);
        setRect(mExpandedRect);
        clampToMovementBounds();
    }
    if (Surface* surface = this->surface()) surface->floaterMinimizedChanged(*this);
}

void HTMLFloaterElement::toggleMinimized() {
    setMinimized(!mMinimized);
}

void HTMLFloaterElement::clampToMovementBounds() {
    const Vec2 position = clampedPosition({rect().x, rect().y});
    const Vec2 delta = position - Vec2{rect().x, rect().y};
    if (delta.x == 0.f && delta.y == 0.f) return;
    translate(delta);
    if (mMinimized) {
        mExpandedRect.x += delta.x;
        mExpandedRect.y += delta.y;
    }
}

bool HTMLFloaterElement::overChromeButton(const Vec2& point) const {
    const StyleSheet* styleSheet = this->styleSheet();
    const auto visible = [styleSheet](const Element* element) {
        return element && (styleSheet ? element->isVisible(resolveElementStyle(*styleSheet, *element)) : element->isVisible(Style{}));
    };
    return (visible(mCloseButton) && mCloseButton->rect().contains(point)) || (visible(mMinimizeButton) && mMinimizeButton->rect().contains(point));
}

Vec2 HTMLFloaterElement::clampedPosition(const Vec2& position) const {
    if (mMovementBounds.w <= 0.f || mMovementBounds.h <= 0.f) return position;
    return {std::clamp(position.x, mMovementBounds.left(), std::max(mMovementBounds.left(), mMovementBounds.right() - rect().w)),
            std::clamp(position.y, mMovementBounds.bottom(), std::max(mMovementBounds.bottom(), mMovementBounds.top() - rect().h))};
}

void HTMLFloaterElement::setAuthoredSize(const Vec2& size, const Vec2& contentSize) {
    mAuthoredSize = {std::max(0.f, size.x), std::max(0.f, size.y)};
    mAuthoredContentSize = {std::max(0.f, contentSize.x), std::max(0.f, contentSize.y)};
    mAuthoredSizeCaptured = true;
}

Vec2 HTMLFloaterElement::authoredSize() const {
    return mAuthoredSizeCaptured ? mAuthoredSize : Vec2{rect().w, rect().h};
}

bool HTMLFloaterElement::beginResizeInteraction(const PointerEvent& event, std::uint8_t edges, const Vec2& minimum,
                                                const std::optional<Rect>& bounds) {
    if (event.button != PointerButton::Left || !mResizeable || mMinimized || edges == 0) return false;
    mInteraction = FloaterInteraction::Resize;
    mResizeInteraction = {edges, event.position, rect(), minimum, bounds};
    return true;
}

bool HTMLFloaterElement::beginPointerInteraction(const PointerEvent& event) {
    if (event.button != PointerButton::Left || !mHead || !mHead->rect().contains(event.position) || overChromeButton(event.position)) return false;
    if (event.clickCount >= 2 && mMinimizable) {
        mInteraction = FloaterInteraction::Idle;
        toggleMinimized();
        return true;
    }
    mInteraction = FloaterInteraction::Move;
    mDragOffset = event.position - Vec2{rect().x, rect().y};
    return true;
}

bool HTMLFloaterElement::updatePointerInteraction(const PointerEvent& event) {
    if (mInteraction == FloaterInteraction::Resize) {
        const Rect resized = resizedRect(mResizeInteraction.initialRect, mResizeInteraction.initialPointer, event.position,
                                         static_cast<ResizeEdges>(mResizeInteraction.edges), {mResizeInteraction.minimum, mResizeInteraction.bounds});
        if (resized.x != rect().x || resized.y != rect().y || resized.w != rect().w || resized.h != rect().h) {
            setRect(resized);
            if (Surface* surface = this->surface()) surface->requestLayout();
        }
        return true;
    }
    if (mInteraction != FloaterInteraction::Move) return false;
    const Vec2 position = clampedPosition(event.position - mDragOffset);
    const Vec2 delta = position - Vec2{rect().x, rect().y};
    if (delta.x != 0.f || delta.y != 0.f) {
        translate(delta);
        if (mMinimized) {
            mExpandedRect.x += delta.x;
            mExpandedRect.y += delta.y;
        }
    }
    return true;
}

bool HTMLFloaterElement::endPointerInteraction(const PointerEvent&) {
    const FloaterInteraction interaction = mInteraction;
    const bool handled = interaction != FloaterInteraction::Idle;
    mInteraction = FloaterInteraction::Idle;
    if (Surface* surface = this->surface()) {
        if (interaction == FloaterInteraction::Move) surface->floaterMoveEnded(*this);
        else if (interaction == FloaterInteraction::Resize) surface->floaterResizeEnded(*this);
    }
    return handled;
}

void HTMLFloaterElement::onChildrenCleared() {
    mHead = nullptr;
    mBody = nullptr;
    mTitleElement = nullptr;
    mCloseButton = nullptr;
    mMinimizeButton = nullptr;
    mClosable = false;
    mMinimizable = false;
    mInteraction = FloaterInteraction::Idle;
    mMinimized = false;
    setState(ElementState::Minimized, false);
}
} // namespace radia::ui
