/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/floater.h"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "layout/engine.h"
#include "style/style.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "system.h"

namespace radia::ui {
namespace {
constexpr char kElementName[] = "floater";

class MinimizeButtonElement final : public ButtonElement {
public:
    MinimizeButtonElement() : ButtonElement("minimize") {}
};

class CloseButtonElement final : public ButtonElement {
public:
    CloseButtonElement() : ButtonElement("close") {}
};

void findAuthoredHeadElements(Element& root, Element*& title, ButtonElement*& minimize, ButtonElement*& close) {
    for (Element* child : root.children()) {
        if (child->elementName() == "title") {
            if (!title) title = child;
        } else if (child->elementName() == "minimize") {
            if (!minimize) minimize = dynamic_cast<ButtonElement*>(child);
        } else if (child->elementName() == "close") {
            if (!close) close = dynamic_cast<ButtonElement*>(child);
        }

        findAuthoredHeadElements(*child, title, minimize, close);
    }
}

std::size_t countAuthoredElements(const Element& root, const char* elementName) {
    std::size_t count = 0;
    for (const Element* child : root.children()) {
        if (child->elementName() == elementName) ++count;
        count += countAuthoredElements(*child, elementName);
    }
    return count;
}
} // namespace

FloaterElement::FloaterElement() : Element(kElementName) {}

std::string FloaterElement::title() const {
    return mTitleElement ? mTitleElement->textContent() : std::string();
}

FloaterElement& FloaterElement::setResizeable(bool value) {
    mResizeable = value;
    return *this;
}

void FloaterElement::setMovementBounds(const Rect& bounds) {
    mMovementBounds = bounds;
}

void FloaterElement::refreshAuthoredStructure() {
    mHead = nullptr;
    mBody = nullptr;
    mTitleElement = nullptr;
    mCloseButton = nullptr;
    mMinimizeButton = nullptr;

    for (Element* child : children()) {
        if (child->elementName() == "head") {
            if (!mHead) mHead = child;
            continue;
        }
        if (child->elementName() == "body") {
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

ElementDefinition detail::ElementDefinitionFactory::floater() {
    return defineElement<FloaterElement>(kElementName)
        .attributes({booleanAttribute("resizeable", &FloaterElement::setResizeable)})
        .composition([](const ElementBuildInput& input, FloaterElement& floater, const ElementScopeContext&, LayoutBuildResult& result) {
            const std::size_t headCount = countAuthoredElements(floater, "head");
            const std::size_t bodyCount = countAuthoredElements(floater, "body");
            std::size_t directHeadCount = 0;
            std::size_t directBodyCount = 0;
            for (Element* child : floater.children()) {
                if (child->elementName() == "head")
                    ++directHeadCount;
                else if (child->elementName() == "body")
                    ++directBodyCount;
                else
                    result.error("layout.floater.child_invalid", "A floater may contain only one <head> and one <body>.", input.sourceName,
                                 input.source.begin.line, input.source.begin.column);
            }
            if (directHeadCount == 0)
                result.error("layout.floater.head_required", "A floater requires one direct <head> child.", input.sourceName,
                             input.source.begin.line, input.source.begin.column);
            else if (headCount > 1)
                result.error("layout.floater.head_duplicate", "A floater may contain only one <head>.", input.sourceName, input.source.begin.line,
                             input.source.begin.column);
            if (directBodyCount == 0)
                result.error("layout.floater.body_required", "A floater requires one direct <body> child.", input.sourceName,
                             input.source.begin.line, input.source.begin.column);
            else if (bodyCount > 1)
                result.error("layout.floater.body_duplicate", "A floater may contain only one <body>.", input.sourceName, input.source.begin.line,
                             input.source.begin.column);

            if (!floater.head()) return;
            const std::size_t titleCount = countAuthoredElements(*floater.head(), "title");
            const std::size_t minimizeCount = countAuthoredElements(*floater.head(), "minimize");
            const std::size_t closeCount = countAuthoredElements(*floater.head(), "close");
            if (titleCount > 1)
                result.error("layout.floater.title_duplicate", "A floater may contain only one <title>.", input.sourceName, input.source.begin.line,
                             input.source.begin.column);
            if (minimizeCount > 1)
                result.error("layout.floater.minimize_duplicate", "A floater may contain only one <minimize>.", input.sourceName,
                             input.source.begin.line, input.source.begin.column);
            if (closeCount > 1)
                result.error("layout.floater.close_duplicate", "A floater may contain only one <close>.", input.sourceName, input.source.begin.line,
                             input.source.begin.column);
            if (floater.body()) {
                if (countAuthoredElements(*floater.body(), "title") > 0)
                    result.error("layout.floater.head_only", "<title> must be inside a floater <head>.", input.sourceName, input.source.begin.line,
                                 input.source.begin.column);
                if (countAuthoredElements(*floater.body(), "minimize") > 0)
                    result.error("layout.floater.head_only", "<minimize> must be inside a floater <head>.", input.sourceName,
                                 input.source.begin.line, input.source.begin.column);
                if (countAuthoredElements(*floater.body(), "close") > 0)
                    result.error("layout.floater.head_only", "<close> must be inside a floater <head>.", input.sourceName, input.source.begin.line,
                                 input.source.begin.column);
            }
            if (floater.minimizable() && floater.title().empty())
                result.error("layout.floater.title_required", "A minimizable floater requires a non-empty <title>.", input.sourceName,
                             input.source.begin.line, input.source.begin.column);
        })
        .state(ElementState::Minimized)
        .build();
}

ElementDefinition detail::ElementDefinitionFactory::minimize() {
    return defineElement<MinimizeButtonElement>("minimize").build();
}

ElementDefinition detail::ElementDefinitionFactory::close() {
    return defineElement<CloseButtonElement>("close").build();
}

void FloaterElement::onChildAdded(Element&) {
    refreshAuthoredStructure();
}

void FloaterElement::onLocaleChanged(const System& system) {
    Element::onLocaleChanged(system);
}

FloaterElement& FloaterElement::setLifecycleCallbacks(std::function<void()> onOpen, std::function<void()> onClose) {
    mOnOpen = std::move(onOpen);
    mOnClose = std::move(onClose);
    return *this;
}

void FloaterElement::open() {
    const bool wasClosed = mClosed;
    mClosed = false;
    setVisibility(Visibility::Visible);
    setDisplayNone(false);
    if (wasClosed && mOnOpen) mOnOpen();
}

void FloaterElement::close() {
    if (mClosed || !mClosable) return;
    mClosed = true;
    mInteraction = FloaterInteraction::Idle;
    setVisibility(Visibility::Hidden);
    setDisplayNone(true);
    if (Surface* surface = this->surface()) surface->floaterClosed(*this);
    if (mOnClose) mOnClose();
}

void FloaterElement::setMinimized(bool minimized) {
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

void FloaterElement::toggleMinimized() {
    setMinimized(!mMinimized);
}

void FloaterElement::clampToMovementBounds() {
    const Vec2 position = clampedPosition({rect().x, rect().y});
    const Vec2 delta = position - Vec2{rect().x, rect().y};
    if (delta.x == 0.f && delta.y == 0.f) return;
    translate(delta);
    if (mMinimized) {
        mExpandedRect.x += delta.x;
        mExpandedRect.y += delta.y;
    }
}

bool FloaterElement::overChromeButton(const Vec2& point) const {
    const StyleSheet* styleSheet = this->styleSheet();
    const auto visible = [styleSheet](const Element* element) {
        return element && (styleSheet ? element->isVisible(resolveElementStyle(*styleSheet, *element)) : element->isVisible(Style{}));
    };
    return (visible(mCloseButton) && mCloseButton->rect().contains(point))
        || (visible(mMinimizeButton) && mMinimizeButton->rect().contains(point));
}

Vec2 FloaterElement::clampedPosition(const Vec2& position) const {
    if (mMovementBounds.w <= 0.f || mMovementBounds.h <= 0.f) return position;
    return {std::clamp(position.x, mMovementBounds.left(), std::max(mMovementBounds.left(), mMovementBounds.right() - rect().w)),
            std::clamp(position.y, mMovementBounds.bottom(), std::max(mMovementBounds.bottom(), mMovementBounds.top() - rect().h))};
}

void FloaterElement::setAuthoredSize(const Vec2& size, const Vec2& contentSize) {
    mAuthoredSize = {std::max(0.f, size.x), std::max(0.f, size.y)};
    mAuthoredContentSize = {std::max(0.f, contentSize.x), std::max(0.f, contentSize.y)};
    mAuthoredSizeCaptured = true;
}

Vec2 FloaterElement::authoredSize() const {
    return mAuthoredSizeCaptured ? mAuthoredSize : Vec2{rect().w, rect().h};
}

bool FloaterElement::beginResizeInteraction(const PointerEvent& event, std::uint8_t edges, const Vec2& minimum,
                                            const std::optional<Rect>& bounds) {
    if (event.button != PointerButton::Left || !mResizeable || mMinimized || edges == 0) return false;
    mInteraction = FloaterInteraction::Resize;
    mResizeInteraction = {edges, event.position, rect(), minimum, bounds};
    return true;
}

bool FloaterElement::beginPointerInteraction(const PointerEvent& event) {
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

bool FloaterElement::updatePointerInteraction(const PointerEvent& event) {
    if (mInteraction == FloaterInteraction::Resize) {
        const Rect resized =
            detail::resizedRect(mResizeInteraction.initialRect, mResizeInteraction.initialPointer, event.position,
                                static_cast<detail::ResizeEdges>(mResizeInteraction.edges), {mResizeInteraction.minimum, mResizeInteraction.bounds});
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

bool FloaterElement::endPointerInteraction(const PointerEvent&) {
    const FloaterInteraction interaction = mInteraction;
    const bool handled = interaction != FloaterInteraction::Idle;
    mInteraction = FloaterInteraction::Idle;
    if (Surface* surface = this->surface()) {
        if (interaction == FloaterInteraction::Move) surface->floaterMoveEnded(*this);
        else if (interaction == FloaterInteraction::Resize) surface->floaterResizeEnded(*this);
    }
    return handled;
}

void FloaterElement::onChildrenCleared() {
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
