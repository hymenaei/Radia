/**
 * @file floater.cpp
 * @brief Defines the movable, detachable Floater Widget.
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
#include "widgets/floater.h"
#include <algorithm>
#include <cmath>
#include "layout/engine.h"
#include "localization/localization.h"
#include "style/style.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "system.h"
#include "widgets/button.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/widgetcontractbuilder.h"

namespace rdui {
namespace {
class MiddleAlignedPanel final : public Panel {
protected:
    void constrainResolvedStyle(Style& style) const override {
        if (!style.verticalAlignSet) style.verticalAlign = VerticalAlign::Middle;
    }
};
} // namespace

Floater::Floater() : Widget(sElement) {
    detail::instantiateCompositeParts(*this, detail::floaterContract());
    configureCompositeParts();
}

void Floater::configureCompositeParts() {
    mHeaderIcon->setName(mIcon);
    mHeaderTitle->setContent(mTitle);
    mMinimizeButton->setVisibility(mCanMinimize ? Visibility::Visible : Visibility::Collapsed).setOnActivate([this](Widget&) { toggleMinimized(); });
    mMinimizeButtonIcon->setName(mMinimizeIcon);
    mCloseButton->setVisibility(mCanClose ? Visibility::Visible : Visibility::Collapsed).setOnActivate([this](Widget&) { close(); });
    mCloseButtonIcon->setName(mCloseIcon);
    updateHeaderPresentation();
}

WidgetContract detail::floaterContract() {
    return defineWidget<Floater>(Floater::sElement)
        .attributes({
            localizedStringAttribute("title", &Floater::setTitleContent),
            stringAttribute("icon", &Floater::setIcon),
            stringAttribute("closeIcon", &Floater::setCloseIcon),
            stringAttribute("minimizeIcon", &Floater::setMinimizeIcon),
            booleanAttribute("canClose", &Floater::setCanClose),
            booleanAttribute("canMinimize", &Floater::setCanMinimize),
            booleanAttribute("canResize", &Floater::setCanResize),
            booleanAttribute("canDetach", &Floater::setCanDetach),
            booleanAttribute("showHeaderIdentity", &Floater::setShowHeaderIdentity),
        })
        .validate([](const LayoutElement&, Floater& floater, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext*) {
            if (floater.canMinimize() && floater.title().empty())
                result.error("layout.floater.title_required", "A minimizable floater requires a non-empty title.", source);
        })
        .childContainer("header", {},
                        [](const LayoutElement& child, Floater& floater, LayoutBuildResult& result, const std::string& source) -> Widget* {
                            Panel* header = floater.claimCustomHeader();
                            if (!header) result.error("layout.part.duplicate", "A floater may declare only one <header>.", source);
                            else applyCommonWidgetAttributes(child, *header, result, source);
                            return header;
                        })
        .state(WidgetState::Minimized)
        .part<MiddleAlignedPanel>("header", &Floater::mHeader)
        .part("header::icon", &Floater::mHeaderIcon)
        .part("header::title", &Floater::mHeaderTitle)
        .part<MiddleAlignedPanel>("header::custom", &Floater::mCustomHeader)
        .part("header::minimize", &Floater::mMinimizeButton)
        .part("header::minimize::icon", &Floater::mMinimizeButtonIcon)
        .part("header::close", &Floater::mCloseButton)
        .part("header::close::icon", &Floater::mCloseButtonIcon)
        .part("content", &Floater::mContent)
        .build();
}

Floater& Floater::setTitle(std::string localizationKey) {
    const System* system = attachedSystem();
    if (system) return setTitleContent(system->localize(std::move(localizationKey)));
    LocalizationRequest request = LocalizationRequest::text(localizationKey);
    return setTitleContent(TextSource::fromLocalization(std::move(request), InlineContent::text(std::move(localizationKey))));
}

Floater& Floater::setTitleContent(TextSource content) {
    mTitle = std::move(content);
    if (mHeaderTitle) mHeaderTitle->setContent(mTitle);
    updateHeaderPresentation();
    return *this;
}

const std::string& Floater::title() const {
    static const std::string empty;
    return mHeaderTitle ? mHeaderTitle->text() : empty;
}

void Floater::onLocaleChanged(const System&) {
    if (mHeaderTitle) mHeaderTitle->setContent(mTitle);
    updateHeaderPresentation();
}

Floater& Floater::setIcon(std::string icon) {
    mIcon = std::move(icon);
    if (mHeaderIcon) mHeaderIcon->setName(mIcon);
    updateHeaderPresentation();
    return *this;
}

Floater& Floater::setCloseIcon(std::string icon) {
    mCloseIcon = std::move(icon);
    if (mCloseButtonIcon) mCloseButtonIcon->setName(mCloseIcon);
    return *this;
}

Floater& Floater::setMinimizeIcon(std::string icon) {
    mMinimizeIcon = std::move(icon);
    if (mMinimizeButtonIcon) mMinimizeButtonIcon->setName(mMinimizeIcon);
    return *this;
}

Floater& Floater::setShowHeaderIdentity(bool value) {
    mShowHeaderIdentity = value;
    updateHeaderPresentation();
    return *this;
}

Floater& Floater::setCanClose(bool value) {
    mCanClose = value;
    if (mCloseButton) mCloseButton->setVisibility(value ? Visibility::Visible : Visibility::Collapsed);
    return *this;
}

Floater& Floater::setCanMinimize(bool value) {
    if (!value && mMinimized) setMinimized(false);
    mCanMinimize = value;
    if (mMinimizeButton) mMinimizeButton->setVisibility(value ? Visibility::Visible : Visibility::Collapsed);
    return *this;
}
Floater& Floater::setCanResize(bool value) {
    mCanResize = value;
    return *this;
}
Floater& Floater::setCanDetach(bool value) {
    mCanDetach = value;
    return *this;
}
void Floater::setMovementBounds(const Rect& bounds) {
    mMovementBounds = bounds;
}

Floater& Floater::addChild(std::unique_ptr<Widget> child) {
    if (mContent) mContent->addChild(std::move(child));
    else Widget::addChild(std::move(child));
    return *this;
}

Floater& Floater::prependChild(std::unique_ptr<Widget> child) {
    if (mContent) mContent->prependChild(std::move(child));
    else Widget::prependChild(std::move(child));
    return *this;
}

void Floater::clearChildren() {
    if (mContent) mContent->clearChildren();
    if (mCustomHeader) mCustomHeader->clearChildren();
    mCustomHeaderClaimed = false;
}

Panel* Floater::claimCustomHeader() {
    if (mCustomHeaderClaimed) return nullptr;
    mCustomHeaderClaimed = true;
    return mCustomHeader.get();
}

void Floater::updateHeaderPresentation() {
    const bool showIdentity = mMinimized || mShowHeaderIdentity;
    if (mHeaderIcon) mHeaderIcon->setVisibility(showIdentity && !mIcon.empty() ? Visibility::Visible : Visibility::Collapsed);
    if (mHeaderTitle) mHeaderTitle->setVisibility(showIdentity && !title().empty() ? Visibility::Visible : Visibility::Collapsed);
}

Floater& Floater::setLifecycleCallbacks(std::function<void()> onOpen, std::function<void()> onClose) {
    mOnOpen = std::move(onOpen);
    mOnClose = std::move(onClose);
    return *this;
}

void Floater::open() {
    const bool wasClosed = mClosed;
    mClosed = false;
    setVisibility(Visibility::Visible);
    if (wasClosed && mOnOpen) mOnOpen();
}

void Floater::close() {
    if (mClosed || !mCanClose) return;
    mClosed = true;
    mInteraction = FloaterInteraction::Idle;
    setVisibility(Visibility::Collapsed);
    if (Surface* surface = attachedSurface()) surface->floaterClosed(*this);
    if (mOnClose) mOnClose();
}

void Floater::setMinimized(bool minimized) {
    if ((minimized && !mCanMinimize) || minimized == mMinimized || !mHeader) return;

    if (minimized) {
        mExpandedRect = rect();
        if (mContent) mContentVisibility = mContent->visibility();
        if (mCustomHeader) mCustomHeaderVisibility = mCustomHeader->visibility();
        mMinimized = true;
        setState(WidgetState::Minimized, true);
        if (mContent) mContent->setVisibility(Visibility::Collapsed);
        if (mCustomHeader) mCustomHeader->setVisibility(Visibility::Collapsed);
        updateHeaderPresentation();

        float width = rect().w;
        float height = mHeader->rect().h;
        if (const StyleSheet* styleSheet = attachedStyleSheet()) {
            const Vec2 headerSize = measureWidget(*mHeader, *styleSheet, attachedTextMetrics());
            const Style floaterStyle = resolveWidgetStyle(*styleSheet, *this);
            const Style headerStyle = resolveWidgetStyle(*styleSheet, *mHeader);
            width = headerSize.x + headerStyle.margin.horizontal() + floaterStyle.padding.horizontal();
            height = headerSize.y + headerStyle.margin.vertical() + floaterStyle.padding.vertical();
        }
        if (mMovementBounds.w > 0.f) width = std::min(width, mMovementBounds.w);
        setRect({rect().x, rect().top() - height, width, height});
    } else {
        mMinimized = false;
        setState(WidgetState::Minimized, false);
        setRect(mExpandedRect);
        if (mContent) mContent->setVisibility(mContentVisibility);
        if (mCustomHeader) mCustomHeader->setVisibility(mCustomHeaderVisibility);
        updateHeaderPresentation();
        clampToMovementBounds();
    }
    if (Surface* surface = attachedSurface()) surface->floaterMinimizedChanged(*this, minimized);
}

void Floater::toggleMinimized() {
    setMinimized(!mMinimized);
}

void Floater::clampToMovementBounds() {
    const Vec2 position = clampedPosition({rect().x, rect().y});
    const Vec2 delta = position - Vec2{rect().x, rect().y};
    if (delta.x == 0.f && delta.y == 0.f) return;
    translate(delta);
    if (mMinimized) {
        mExpandedRect.x += delta.x;
        mExpandedRect.y += delta.y;
    }
    if (Surface* surface = attachedSurface()) surface->floaterMoved(*this);
}

bool Floater::overChromeButton(const Vec2& point) const {
    return (mCloseButton && mCloseButton->visibility() == Visibility::Visible && mCloseButton->rect().contains(point))
        || (mMinimizeButton && mMinimizeButton->visibility() == Visibility::Visible && mMinimizeButton->rect().contains(point));
}

Vec2 Floater::clampedPosition(const Vec2& position) const {
    if (mMovementBounds.w <= 0.f || mMovementBounds.h <= 0.f) return position;
    return {std::clamp(position.x, mMovementBounds.left(), std::max(mMovementBounds.left(), mMovementBounds.right() - rect().w)),
            std::clamp(position.y, mMovementBounds.bottom(), std::max(mMovementBounds.bottom(), mMovementBounds.top() - rect().h))};
}

void Floater::setAuthoredSize(const Vec2& size, const Vec2& contentSize) {
    mAuthoredSize = {std::max(0.f, size.x), std::max(0.f, size.y)};
    mAuthoredContentSize = {std::max(0.f, contentSize.x), std::max(0.f, contentSize.y)};
    mAuthoredSizeCaptured = true;
}

Vec2 Floater::authoredSize() const {
    return mAuthoredSizeCaptured ? mAuthoredSize : Vec2{rect().w, rect().h};
}

bool Floater::beginResizeInteraction(const PointerEvent& event, std::uint8_t edges, const Vec2& minimum, const std::optional<Rect>& bounds) {
    if (event.button != PointerButton::Left || !mCanResize || mMinimized || edges == 0) return false;
    mInteraction = FloaterInteraction::Resize;
    mDetachRequested = false;
    mResizeInteraction = {edges, event.position, rect(), minimum, bounds};
    return true;
}

bool Floater::beginPointerInteraction(const PointerEvent& event) {
    if (event.button != PointerButton::Left || !mHeader || !mHeader->rect().contains(event.position) || overChromeButton(event.position))
        return false;
    if (event.clickCount >= 2 && mCanMinimize) {
        mInteraction = FloaterInteraction::Idle;
        toggleMinimized();
        return true;
    }
    mInteraction = FloaterInteraction::Move;
    mDetachRequested = false;
    mDragOffset = event.position - Vec2{rect().x, rect().y};
    return true;
}

bool Floater::updatePointerInteraction(const PointerEvent& event) {
    WidgetRef<Floater> self(this);
    if (mInteraction == FloaterInteraction::Resize) {
        const Rect resized =
            detail::resizedRect(mResizeInteraction.initialRect, mResizeInteraction.initialPointer, event.position,
                                static_cast<detail::ResizeEdges>(mResizeInteraction.edges), {mResizeInteraction.minimum, mResizeInteraction.bounds});
        if (resized.x != rect().x || resized.y != rect().y || resized.w != rect().w || resized.h != rect().h) {
            setRect(resized);
            if (Surface* surface = attachedSurface()) surface->floaterResized(*this, false);
        }
        return true;
    }
    if (mInteraction != FloaterInteraction::Move) return false;
    const Vec2 desiredPosition = event.position - mDragOffset;
    const Vec2 position = clampedPosition(desiredPosition);
    constexpr float kBreakawayDistance = 100.f;
    const float pointerOvershoot = std::max({mMovementBounds.left() - event.position.x, event.position.x - mMovementBounds.right(),
                                             mMovementBounds.bottom() - event.position.y, event.position.y - mMovementBounds.top(), 0.f});
    Surface* surface = attachedSurface();
    if (!mDetachRequested && !mMinimized && mCanDetach && surface && surface->canDetachFloater(*this) && pointerOvershoot >= kBreakawayDistance) {
        mDetachRequested = true;
        const Widget* originalParent = parent();
        surface->floaterDetachRequested(*this, desiredPosition, mDragOffset);
        Floater* current = self.get();
        if (!current || current->attachedSurface() != surface || current->parent() != originalParent) return true;
    }
    const Vec2 delta = position - Vec2{rect().x, rect().y};
    if (delta.x != 0.f || delta.y != 0.f) {
        translate(delta);
        if (mMinimized) {
            mExpandedRect.x += delta.x;
            mExpandedRect.y += delta.y;
        }
        if (surface) surface->floaterMoved(*this);
    }
    return true;
}

bool Floater::endPointerInteraction(const PointerEvent&) {
    const FloaterInteraction interaction = mInteraction;
    const bool handled = interaction != FloaterInteraction::Idle;
    mInteraction = FloaterInteraction::Idle;
    mDetachRequested = false;
    if (Surface* surface = attachedSurface()) {
        if (interaction == FloaterInteraction::Move) surface->floaterMoveEnded(*this);
        else if (interaction == FloaterInteraction::Resize) surface->floaterResized(*this, true);
    }
    return handled;
}

void Floater::onChildrenCleared() {
    mHeader.set(nullptr);
    mCustomHeader.set(nullptr);
    mContent.set(nullptr);
    mHeaderIcon.set(nullptr);
    mHeaderTitle.set(nullptr);
    mCloseButton.set(nullptr);
    mCloseButtonIcon.set(nullptr);
    mMinimizeButton.set(nullptr);
    mMinimizeButtonIcon.set(nullptr);
    mCustomHeaderClaimed = false;
    mInteraction = FloaterInteraction::Idle;
    mDetachRequested = false;
    mMinimized = false;
    setState(WidgetState::Minimized, false);
    detail::instantiateCompositeParts(*this, detail::floaterContract());
    configureCompositeParts();
}
} // namespace rdui
