/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "Binder.h"
#include "Document.h"
#include "ElementInternal.h"
#include "HTMLElement.h"
#include "HTMLFloaterElement.h"
#include "HTMLName.h"
#include "LayoutEngine.h"
#include "LayoutGeometry.h"
#include "LayoutPrimitives.h"
#include "PaintContext.h"
#include "StylePass.h"
#include "Surface.h"
#include "System.h"
#include "Text.h"
#include "TextMeasurer.h"

namespace Core {
using detail::ElementInternalAccess;
using detail::MountEpoch;
using detail::NodeRef;

Layout::Direction Surface::layoutDirection() const { return mSystem ? mSystem->layoutDirection() : Layout::Direction::LeftToRight; }

void Surface::generationChanged(const CSS::StyleSheet& styleSheet) {
    if (mStylePass && mStylePass->active())
        mPendingStyleSheet = &styleSheet;
    else {
        mStyleSheet = &styleSheet;
        mPendingStyleSheet = nullptr;
        mStylePass.reset();
    }
    invalidateStyleCache();
    mObservedStyleGeneration = 0;
    requestLayout();
    requestPaint();
}

void Surface::setViewport(float width, float height) {
    const bool changed = mViewport.w != width || mViewport.h != height;
    mViewport = Layout::Rect(0.f, 0.f, width, height);
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root && mount->floater)
                constrainFloater(*mount->floater);
    if (changed)
        requestLayout();
}

void Surface::setScrollLayoutOptions(Layout::ScrollLayoutOptions options) {
    syncNativeAppearance();
    if (mSystem)
        options.nativeMetrics = mSystem->nativeAppearance().layoutMetrics();
    const bool optionsChanged =
        mScrollLayoutOptions.scrollbarMode != options.scrollbarMode || mScrollLayoutOptions.nativeMetrics != options.nativeMetrics;
    if (!optionsChanged)
        return;
    mScrollLayoutOptions = options;
    requestLayout();
}

void Surface::setColorSchemeContext(Style::ColorSchemeContext context) {
    if (mColorSchemeContext == context)
        return;
    mColorSchemeContext = context;
    requestLayout();
    requestPaint();
}

void Surface::nativeAppearanceChanged() {
    if (!mSystem)
        return;
    mScrollLayoutOptions.nativeMetrics = mSystem->nativeAppearance().layoutMetrics();
    mNativeAppearanceRevision = mScrollLayoutOptions.nativeMetrics.revision;
    requestLayout();
    requestPaint();
}

void Surface::syncNativeAppearance() {
    if (!mSystem)
        return;
    const NativeLayoutMetrics metrics = mSystem->nativeAppearance().layoutMetrics();
    if (mNativeAppearanceRevision != metrics.revision || mScrollLayoutOptions.nativeMetrics != metrics)
        nativeAppearanceChanged();
}

const NativeAppearance& Surface::effectiveNativeAppearance() const {
    return mSystem ? mSystem->nativeAppearance() : defaultNativeAppearance();
}

const NativeAppearance& Surface::nativeAppearance() const { return effectiveNativeAppearance(); }

NativeScrollbarMetrics Surface::scrollbarMetrics(ScrollbarMode mode) const {
    return mScrollLayoutOptions.nativeMetrics.scrollbarMetrics(mode);
}

ScrollGeometry Surface::scrollbarGeometry(const Element& element, const Style::ComputedStyle& style) const {
    const ScrollbarMode mode = mScrollLayoutOptions.scrollbarMode;
    const NativeScrollbarMetrics metrics = scrollbarMetrics(mode);
    const float widthScale = style.scrollbarWidth() == Style::ScrollbarWidth::Thin ? .5f : 1.f;
    const bool enabled = style.scrollbarWidth() != Style::ScrollbarWidth::NoneValue;
    const auto visible = [enabled](Style::Overflow overflow, float maximum) {
        return enabled && (overflow == Style::Overflow::Scroll || (overflow == Style::Overflow::Auto && maximum > 0.f));
    };

    ScrollGeometryInput input {};
    input.scrollport = ElementInternalAccess::scrollport(element);
    input.mode = mode;
    input.direction = layoutDirection();
    input.thickness = metrics.thickness * widthScale;
    input.arrowLength = metrics.arrowLength * widthScale;
    input.minimumThumbLength = metrics.minimumThumbLength;
    input.thumbPadding = metrics.thumbPadding * widthScale;
    input.horizontal = {element.scrollLeft(), element.scrollWidth(), element.clientWidth(),
        visible(style.overflowX(), element.scrollMetrics().maxScrollLeft)};
    input.vertical = {element.scrollTop(), element.scrollHeight(), element.clientHeight(),
        visible(style.overflowY(), element.scrollMetrics().maxScrollTop)};
    return makeScrollGeometry(input);
}

bool Surface::scrollbarTargetMatches(const ScrollbarTarget& target, const Element& element, ScrollbarAxis axis, ScrollbarPart part) const {
    return target.element == &element && target.hit.axis == axis && (part == ScrollbarPart::NoneValue || target.hit.part == part)
        && isRootedInSurface(&element);
}

void Surface::requestLayout() {
    mLayoutDirty = true;
    mPaintDirty = true;
    ++mPaintRequestGeneration;
}

void Surface::requestPaint() {
    mPaintDirty = true;
    ++mPaintRequestGeneration;
}

void Surface::requestHitTestRefresh() {
    mHitTestDirty = true;
    requestPaint();
}

void Surface::queueScrollNotification(Element& element) {
    if (element.mSurface != this)
        return;
    const MountEpoch mountEpoch = ElementInternalAccess::mountEpoch(element);
    const auto duplicate = std::find_if(mPendingScrollNotifications.begin(), mPendingScrollNotifications.end(), [&](const auto& pending) {
        return pending.element == &element && pending.mountEpoch == mountEpoch;
    });
    if (duplicate != mPendingScrollNotifications.end())
        return;
    mPendingScrollNotifications.push_back({&element, ElementInternalAccess::lifetime(element), mountEpoch});
}

void Surface::dispatchScrollNotification(Element& element) {
    Event event(kScrollEvent, element);
    event.setCancelable(false);
    routeEvent(event);
}

void Surface::flushScrollNotifications() {
    if (mDispatchingScrollNotifications)
        return;
    mDispatchingScrollNotifications = true;
    while (!mPendingScrollNotifications.empty()) {
        std::vector<PendingScrollNotification> pending;
        pending.swap(mPendingScrollNotifications);
        for (const PendingScrollNotification& notification : pending) {
            if (!notification.element || notification.lifetime.expired())
                continue;
            Element* element = notification.element;
            if (element->mSurface != this || ElementInternalAccess::mountEpoch(*element) != notification.mountEpoch
                || !isRootedInSurface(element))
                continue;
            dispatchScrollNotification(*element);
        }
    }
    mDispatchingScrollNotifications = false;
}

void Surface::invalidateStyleCache() {
    if (mStylePass)
        mStylePass->invalidate();
}

void Surface::invalidateOrderingCache() {
    if (mStylePass)
        mStylePass->invalidateOrdering();
}

bool Surface::hasVisibleFloater() const {
    Style::Pass& styles = stylePass();
    const Style::Pass::TraversalScope traversal = styles.enterTraversal();
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts) {
            HTMLFloaterElement* floater = mount ? mount->floater : nullptr;
            if (!floater || !mount->root || mount->root != floater
                || (mount->layer != SurfaceLayer::Floater && mount->layer != SurfaceLayer::Modal) || !isRootedInSurface(floater))
                continue;
            if (floater->isVisible(styles.style(*floater)))
                return true;
        }
    return false;
}

Style::Pass& Surface::stylePass() const {
    if (mPendingStyleSheet && (!mStylePass || !mStylePass->active())) {
        mStyleSheet = mPendingStyleSheet;
        mPendingStyleSheet = nullptr;
        mStylePass.reset();
    }
    const Layout::Direction direction = layoutDirection();
    const NativeLayoutMetrics metrics = mScrollLayoutOptions.nativeMetrics;
    const bool mismatched = !mStylePass || !mStylePass->matches(*mStyleSheet, mTextMeasurer, direction, metrics, mColorSchemeContext);
    if (mismatched && (!mStylePass || !mStylePass->active()))
        mStylePass = std::make_unique<Style::Pass>(*mStyleSheet, mTextMeasurer, direction, metrics, mColorSchemeContext);
    return *mStylePass;
}

void Surface::didPaint(std::uint64_t paintedGeneration) {
    if (mPaintRequestGeneration != paintedGeneration || mLayoutDirty) {
        mPaintDirty = true;
        return;
    }
    mPaintDirty = false;
    for (MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root)
                mount->root->clearPaintInvalidationTree();
}

void Surface::updateLayout() {
    if (mDispatchingScrollNotifications)
        return;
    for (;;) {
        const bool layoutChanged = updateLayoutIfNeeded();
        if ((layoutChanged || mHitTestDirty) && mPointerPositionKnown)
            refreshHoverState();
        flushScrollNotifications();
        if (!mLayoutDirty && mPendingScrollNotifications.empty())
            break;
    }
}

bool Surface::updateLayoutIfNeeded() {
    syncNativeAppearance();
    const std::uint64_t styleGeneration = mSystem ? mSystem->generation() : mStyleSheet->generation();
    if (styleGeneration != mObservedStyleGeneration) {
        mObservedStyleGeneration = styleGeneration;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root)
                    mount->root->invalidateStyleTree();
    }
    const std::uint64_t textMetricsGeneration = mTextMeasurer.generation();
    if (textMetricsGeneration != mObservedTextMeasurerGeneration) {
        mObservedTextMeasurerGeneration = textMetricsGeneration;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root)
                    mount->root->invalidateTextTree();
    }
    const Layout::Direction direction = layoutDirection();
    if (direction != mObservedLayoutDirection) {
        mObservedLayoutDirection = direction;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root)
                    mount->root->invalidateArrangeTree();
    }
    if (!mLayoutDirty)
        return false;
    mLayoutDirty = false;
    Style::Pass& styles = stylePass();
    const Style::Pass::TraversalScope traversal = styles.enterTraversal();
    const auto layoutRoot = [&](Element& root) {
        const bool authoredRect = root.mRectExplicit;
        Layout::Engine::layout(root, styles, mScrollLayoutOptions);
        if (authoredRect)
            return;

        const Style::ComputedStyle& rootStyle = styles.style(root);
        if (!root.isDisplayed(rootStyle))
            return;
        const bool bodyRoot = root.elementName() == HTMLTagName(HTMLTag::Body);
        const Layout::Vec2 desired = root.desiredSize();
        const float width = rootStyle.width().isAuto()
            ? ((rootStyle.display() == Style::Display::Inline || rootStyle.display() == Style::Display::InlineBlock
                   || rootStyle.display() == Style::Display::InlineFlex || rootStyle.display() == Style::Display::InlineGrid)
                      ? desired.x
                      : bodyRoot ? std::max(0.f, mViewport.w - Layout::horizontalMargin(rootStyle.margin()))
                                 : mViewport.w)
            : rootStyle.width().resolve(desired.x, mViewport.w);
        const float height = rootStyle.height().isAuto()
            ? (bodyRoot ? std::max(0.f, mViewport.h - Layout::verticalMargin(rootStyle.margin())) : desired.y)
            : rootStyle.height().resolve(desired.y, mViewport.h);
        const auto& left = rootStyle.left();
        const auto& right = rootStyle.right();
        const auto& top = rootStyle.top();
        const auto& bottom = rootStyle.bottom();
        const float x = left ? left->resolve(mViewport.w)
            : right          ? mViewport.w - right->resolve(mViewport.w) - width
                             : rootStyle.margin().left.fixedPixels();
        const float y = top ? mViewport.h - top->resolve(mViewport.h) - height
            : bottom        ? bottom->resolve(mViewport.h)
                            : mViewport.h - height - rootStyle.margin().top.fixedPixels();
        Layout::detail::setArrangedRect(root, {x, y, width, height});
        Layout::Engine::layout(root, styles, mScrollLayoutOptions);
    };
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root)
                layoutRoot(*mount->root);
    return true;
}
} // namespace Core
