/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "binding/binder.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "html/element.h"
#include "html/elementnames.h"
#include "html/floater.h"
#include "layout/engine.h"
#include "layout/primitives.h"
#include "paint/paintcontext.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::MountEpoch;
using detail::NodeRef;

namespace {} // namespace

LayoutDirection Surface::layoutDirection() const {
    return mSystem ? mSystem->layoutDirection() : LayoutDirection::LeftToRight;
}

void Surface::generationChanged(const StyleSheet& styleSheet) {
    if (mStylePass && mStylePass->active()) mPendingStyleSheet = &styleSheet;
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
    mViewport = Rect(0.f, 0.f, width, height);
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root && mount->floater) constrainFloater(*mount->floater);
    if (changed) requestLayout();
}

void Surface::setScrollLayoutOptions(ScrollLayoutOptions options) {
    syncNativeAppearance();
    if (mSystem) options.nativeMetrics = mSystem->nativeAppearance().layoutMetrics();
    const bool optionsChanged =
        mScrollLayoutOptions.scrollbarMode != options.scrollbarMode || mScrollLayoutOptions.nativeMetrics != options.nativeMetrics;
    if (!optionsChanged) return;
    mScrollLayoutOptions = options;
    requestLayout();
}

void Surface::nativeAppearanceChanged() {
    if (!mSystem) return;
    mScrollLayoutOptions.nativeMetrics = mSystem->nativeAppearance().layoutMetrics();
    mNativeAppearanceRevision = mScrollLayoutOptions.nativeMetrics.revision;
    requestLayout();
    requestPaint();
}

void Surface::syncNativeAppearance() {
    if (!mSystem) return;
    const NativeLayoutMetrics metrics = mSystem->nativeAppearance().layoutMetrics();
    if (mNativeAppearanceRevision != metrics.revision || mScrollLayoutOptions.nativeMetrics != metrics) nativeAppearanceChanged();
}

const NativeAppearance& Surface::effectiveNativeAppearance() const {
    return mSystem ? mSystem->nativeAppearance() : defaultNativeAppearance();
}

const NativeAppearance& Surface::nativeAppearance() const {
    return effectiveNativeAppearance();
}

NativeScrollbarMetrics Surface::scrollbarMetrics(ScrollbarMode mode) const {
    return mScrollLayoutOptions.nativeMetrics.scrollbarMetrics(mode);
}

ScrollGeometry Surface::scrollbarGeometry(const Element& element, const ComputedStyle& style) const {
    const ScrollbarMode mode = style.scrollbarModeSet ? style.scrollbarMode : mScrollLayoutOptions.scrollbarMode;
    const NativeScrollbarMetrics metrics = scrollbarMetrics(mode);
    const float widthScale = style.scrollbarWidth == ScrollbarWidth::Thin ? .5f : 1.f;
    const bool enabled = style.scrollbarWidth != ScrollbarWidth::NoneValue;
    const auto visible = [enabled](Overflow overflow, float maximum) {
        return enabled && (overflow == Overflow::Scroll || (overflow == Overflow::Auto && maximum > 0.f));
    };

    ScrollGeometryInput input{};
    input.scrollport = ElementInternalAccess::scrollport(element);
    input.mode = mode;
    input.direction = layoutDirection();
    input.thickness = metrics.thickness * widthScale;
    input.arrowLength = metrics.arrowLength * widthScale;
    input.minimumThumbLength = metrics.minimumThumbLength;
    input.thumbPadding = metrics.thumbPadding * widthScale;
    input.horizontal = {element.scrollLeft(), element.scrollWidth(), element.clientWidth(),
                        visible(style.overflowX, element.scrollMetrics().maxScrollLeft)};
    input.vertical = {element.scrollTop(), element.scrollHeight(), element.clientHeight(),
                      visible(style.overflowY, element.scrollMetrics().maxScrollTop)};
    return makeScrollGeometry(input);
}

bool Surface::scrollbarTargetMatches(const ScrollbarTarget& target, const Element& element, ScrollbarAxis axis, ScrollbarPart part) const {
    return target.element == &element
        && target.hit.axis == axis
        && (part == ScrollbarPart::NoneValue || target.hit.part == part)
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
    if (element.mSurface != this) return;
    const MountEpoch mountEpoch = ElementInternalAccess::mountEpoch(element);
    const auto duplicate = std::find_if(mPendingScrollNotifications.begin(), mPendingScrollNotifications.end(),
                                        [&](const auto& pending) { return pending.element == &element && pending.mountEpoch == mountEpoch; });
    if (duplicate != mPendingScrollNotifications.end()) return;
    mPendingScrollNotifications.push_back({&element, ElementInternalAccess::lifetime(element), mountEpoch});
}

void Surface::dispatchScrollNotification(Element& element) {
    Event event(kScrollEvent, element);
    event.setCancelable(false);
    routeEvent(event);
}

void Surface::flushScrollNotifications() {
    if (mDispatchingScrollNotifications) return;
    mDispatchingScrollNotifications = true;
    while (!mPendingScrollNotifications.empty()) {
        std::vector<PendingScrollNotification> pending;
        pending.swap(mPendingScrollNotifications);
        for (const PendingScrollNotification& notification : pending) {
            if (!notification.element || notification.lifetime.expired()) continue;
            Element* element = notification.element;
            if (element->mSurface != this || ElementInternalAccess::mountEpoch(*element) != notification.mountEpoch || !isRootedInSurface(element))
                continue;
            dispatchScrollNotification(*element);
        }
    }
    mDispatchingScrollNotifications = false;
}

void Surface::invalidateStyleCache() {
    if (mStylePass) mStylePass->invalidate();
}

void Surface::invalidateOrderingCache() {
    if (mStylePass) mStylePass->invalidateOrdering();
}

bool Surface::hasVisibleFloater() const {
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts) {
            HTMLFloaterElement* floater = mount ? mount->floater : nullptr;
            if (!floater
                || !mount->root
                || mount->root != floater
                || (mount->layer != SurfaceLayer::Floater && mount->layer != SurfaceLayer::Modal)
                || !isRootedInSurface(floater))
                continue;
            if (floater->isVisible(styles.style(*floater))) return true;
        }
    return false;
}

StylePass& Surface::stylePass() const {
    if (mPendingStyleSheet && (!mStylePass || !mStylePass->active())) {
        mStyleSheet = mPendingStyleSheet;
        mPendingStyleSheet = nullptr;
        mStylePass.reset();
    }
    const LayoutDirection direction = layoutDirection();
    const NativeLayoutMetrics metrics = mScrollLayoutOptions.nativeMetrics;
    const bool mismatched = !mStylePass || !mStylePass->matches(*mStyleSheet, mTextMetrics, direction, metrics);
    if (mismatched && (!mStylePass || !mStylePass->active()))
        mStylePass = std::make_unique<StylePass>(*mStyleSheet, mTextMetrics, direction, metrics);
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
            if (mount && mount->root) mount->root->clearPaintInvalidationTree();
}

void Surface::updateLayout() {
    if (mDispatchingScrollNotifications) return;
    for (;;) {
        const bool layoutChanged = updateLayoutIfNeeded();
        if ((layoutChanged || mHitTestDirty) && mPointerPositionKnown) refreshHoverState();
        flushScrollNotifications();
        if (!mLayoutDirty && mPendingScrollNotifications.empty()) break;
    }
}

bool Surface::updateLayoutIfNeeded() {
    syncNativeAppearance();
    const std::uint64_t styleGeneration = mSystem ? mSystem->generation() : mStyleSheet->generation();
    if (styleGeneration != mObservedStyleGeneration) {
        mObservedStyleGeneration = styleGeneration;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root) mount->root->invalidateStyleTree();
    }
    const std::uint64_t textMetricsGeneration = mTextMetrics.generation();
    if (textMetricsGeneration != mObservedTextMetricsGeneration) {
        mObservedTextMetricsGeneration = textMetricsGeneration;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root) mount->root->invalidateTextTree();
    }
    const LayoutDirection direction = layoutDirection();
    if (direction != mObservedLayoutDirection) {
        mObservedLayoutDirection = direction;
        for (MountList& layerMounts : mMounts)
            for (const MountPtr& mount : layerMounts)
                if (mount && mount->root) mount->root->invalidateArrangeTree();
    }
    if (!mLayoutDirty) return false;
    mLayoutDirty = false;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto layoutRoot = [&](Element& root) {
        const bool authoredRect = root.mRectExplicit;
        LayoutEngine::layout(root, styles, mScrollLayoutOptions);
        if (authoredRect) return;

        const ComputedStyle& rootStyle = styles.style(root);
        if (!root.isDisplayed(rootStyle)) return;
        const bool bodyRoot = root.elementName() == kBodyTag.localName;
        const Vec2 desired = root.desiredSize();
        const float width = rootStyle.width.isAuto() ? ((rootStyle.display == DisplayMode::Inline
                                                         || rootStyle.display == DisplayMode::InlineBlock
                                                         || rootStyle.display == DisplayMode::InlineFlex
                                                         || rootStyle.display == DisplayMode::InlineGrid)
                                                            ? desired.x
                                                            : bodyRoot ? std::max(0.f, mViewport.w - rootStyle.margin.horizontal())
                                                                       : mViewport.w)
                                                     : rootStyle.width.resolve(desired.x, mViewport.w);
        const float height = rootStyle.height.isAuto() ? (bodyRoot ? std::max(0.f, mViewport.h - rootStyle.margin.vertical()) : desired.y)
                                                       : rootStyle.height.resolve(desired.y, mViewport.h);
        const float x = rootStyle.left ? rootStyle.left->resolve(mViewport.w)
            : rootStyle.right          ? mViewport.w - rootStyle.right->resolve(mViewport.w) - width
                                       : rootStyle.margin.left.fixedPixels();
        const float y = rootStyle.top ? mViewport.h - rootStyle.top->resolve(mViewport.h) - height
            : rootStyle.bottom        ? rootStyle.bottom->resolve(mViewport.h)
                                      : mViewport.h - height - rootStyle.margin.top.fixedPixels();
        layout_detail::setArrangedRect(root, {x, y, width, height});
        LayoutEngine::layout(root, styles, mScrollLayoutOptions);
    };
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) layoutRoot(*mount->root);
    return true;
}
} // namespace radia::ui
