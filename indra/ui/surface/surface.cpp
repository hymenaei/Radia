/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "surface/surface.h"
#include <algorithm>
#include <optional>
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "elements/floater.h"
#include "layout/engine.h"
#include "layout/engineinternal.h"
#include "layout/primitives.h"
#include "render/paintcontext.h"
#include "style/stylepass.h"
#include "surface/surfaceinternal.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
void applyOpacity(Style& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity;
    style.backgroundColor.a *= opacity;
    style.borderColor.a *= opacity;
    style.color.a *= opacity;
    style.iconStrokeColor.a *= opacity;
    style.outline.color.a *= opacity;
    if (!style.scrollbarColor.automatic) {
        style.scrollbarColor.thumb.a *= opacity;
        style.scrollbarColor.track.a *= opacity;
    }
    for (BoxShadow& shadow : style.shadows) shadow.color.a *= opacity;
    if (style.backgroundGradient)
        for (GradientStop& stop : style.backgroundGradient->stops) stop.color.a *= opacity;
    if (style.borderGradient)
        for (GradientStop& stop : style.borderGradient->stops) stop.color.a *= opacity;
    style.opacity = 1.f;
}

void applyDirection(Style& style, LayoutDirection direction) {
    style.direction = direction;
    if (style.textAlign == TextAlign::Start) style.textAlign = direction == LayoutDirection::RightToLeft ? TextAlign::Right : TextAlign::Left;
    else if (style.textAlign == TextAlign::End) style.textAlign = direction == LayoutDirection::RightToLeft ? TextAlign::Left : TextAlign::Right;
}

bool hasBorderRadius(const BorderRadii& radii) {
    const auto hasRadius = [](const BorderRadius& radius) {
        return radius.horizontal.pixels != 0.f || radius.horizontal.percent != 0.f || radius.vertical.pixels != 0.f || radius.vertical.percent != 0.f;
    };
    return hasRadius(radii.topLeft) || hasRadius(radii.topRight) || hasRadius(radii.bottomRight) || hasRadius(radii.bottomLeft);
}

const Element* scrollbarClipOwner(const Element& element, const Style& style) {
    if (style.borderWidth.any() || hasBorderRadius(style.borderRadius)) return &element;
    for (const Element* ancestor = element.parentElement(); ancestor; ancestor = ancestor->parentElement())
        if (dynamic_cast<const FloaterElement*>(ancestor)) return ancestor;
    return nullptr;
}
} // namespace

Surface::ElementSnapshot Surface::snapshot(Element& element) const {
    ElementSnapshot result;
    result.lifetime.set(&element);
    result.element = &element;
    result.surface = element.mSurface;
    result.parent = element.parentElement();
    result.layoutRevision = element.mLayoutInvalidationRevision;
    result.styleRevision = element.mStyleRevision;
    return result;
}

bool Surface::snapshotValid(const ElementSnapshot& snapshot) const {
    const Element* element = snapshot.lifetime.get();
    return element
        && element->mSurface == snapshot.surface
        && element->parentElement() == snapshot.parent
        && element->mLayoutInvalidationRevision == snapshot.layoutRevision;
}

bool Surface::snapshotChildValid(const ElementSnapshot& snapshot, const Element& parent) const {
    return snapshotValid(snapshot) && snapshot.parent == &parent;
}

Surface::Surface() : mTextMetrics(fixedTextMetrics()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {}

Surface::Surface(const StyleSheet& styleSheet)
    : mStyleSheet(&styleSheet), mTextMetrics(fixedTextMetrics()), mObservedStyleGeneration(styleSheet.generation()),
      mObservedTextMetricsGeneration(mTextMetrics.generation()) {}

Surface::Surface(const System& system, const TextMetrics& textMetrics)
    : mStyleSheet(&system.styleSheet()), mSystem(&system), mTextMetrics(textMetrics), mObservedStyleGeneration(system.generation()),
      mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    mScrollLayoutOptions.nativeAppearance = &system.nativeAppearance();
    mNativeAppearanceRevision = system.nativeAppearance().revision();
    system.registerSurface(*this);
}

Surface::~Surface() {
    for (RootList& layerRoots : mRoots)
        for (auto& root : layerRoots)
            if (root) root->setSurface(nullptr);
    if (mSystem) mSystem->unregisterSurface(*this);
}

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
    for (auto floater = mFloaters.begin(); floater != mFloaters.end();)
        if (FloaterElement* managed = *floater) {
            constrainFloater(*managed);
            ++floater;
        } else floater = mFloaters.erase(floater);
    if (changed) requestLayout();
}

void Surface::setScrollLayoutOptions(ScrollLayoutOptions options) {
    syncNativeAppearance();
    if (mSystem) options.nativeAppearance = &mSystem->nativeAppearance();
    const bool optionsChanged =
        mScrollLayoutOptions.scrollbarMode != options.scrollbarMode || mScrollLayoutOptions.nativeAppearance != options.nativeAppearance;
    if (!optionsChanged) return;
    mScrollLayoutOptions = options;
    requestLayout();
}

void Surface::nativeAppearanceChanged() {
    if (!mSystem) return;
    mScrollLayoutOptions.nativeAppearance = &mSystem->nativeAppearance();
    mNativeAppearanceRevision = mSystem->nativeAppearance().revision();
    requestLayout();
    requestPaint();
}

void Surface::syncNativeAppearance() {
    if (mSystem && mNativeAppearanceRevision != mSystem->nativeAppearance().revision()) nativeAppearanceChanged();
}

const NativeAppearance& Surface::effectiveNativeAppearance() const {
    return mScrollLayoutOptions.nativeAppearance ? *mScrollLayoutOptions.nativeAppearance : defaultNativeAppearance();
}

const NativeAppearance& Surface::nativeAppearance() const {
    return effectiveNativeAppearance();
}

NativeScrollbarMetrics Surface::scrollbarMetrics(ScrollbarMode mode) const {
    return effectiveNativeAppearance().scrollbarMetrics(mode);
}

ScrollGeometry Surface::scrollbarGeometry(const Element& element, const Style& style) const {
    const ScrollbarMode mode = style.scrollbarModeSet ? style.scrollbarMode : mScrollLayoutOptions.scrollbarMode;
    const NativeScrollbarMetrics metrics = scrollbarMetrics(mode);
    const float widthScale = style.scrollbarWidth == ScrollbarWidth::Thin ? .5f : 1.f;
    const bool enabled = style.scrollbarWidth != ScrollbarWidth::NoneValue;
    const auto visible = [enabled](Overflow overflow, float maximum) {
        return enabled && (overflow == Overflow::Scroll || (overflow == Overflow::Auto && maximum > 0.f));
    };

    ScrollGeometryInput input{};
    input.scrollport = detail::ElementInternalAccess::scrollport(element);
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

Element& Surface::mount(std::unique_ptr<Element> element, SurfaceLayer layer) {
    if (layer == SurfaceLayer::Modal) clearInteractionState();
    llassert_always(element && !element->parentElement());
    Element* mounted = element.get();
    element->setSurface(this);
    mOwnedRoots.emplace_back(std::move(element));
    roots(layer).push_back(mounted);
    llassert_always(mounted && mounted->parentElement() == nullptr && mounted->surface() == this);
    invalidateOrderingCache();
    requestLayout();
    return *mounted;
}

Element& Surface::mount(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    llassert_always(root);
    return mount(*root, layer);
}

Element& Surface::mount(Element& element, SurfaceLayer layer) {
    if (layer == SurfaceLayer::Modal) clearInteractionState();
    llassert_always(!element.parentElement() && !element.surface());
    element.setSurface(this);
    roots(layer).push_back(&element);
    invalidateOrderingCache();
    requestLayout();
    return element;
}

std::unique_ptr<Element> Surface::takeOwnedRoot(Element& element) {
    const auto found = std::find_if(mOwnedRoots.begin(), mOwnedRoots.end(), [&element](const auto& root) { return root.get() == &element; });
    if (found == mOwnedRoots.end()) return nullptr;
    std::unique_ptr<Element> result = std::move(*found);
    mOwnedRoots.erase(found);
    return result;
}

bool Surface::detachRoot(Element& element) {
    for (RootList& layerRoots : mRoots) {
        auto found = std::find(layerRoots.begin(), layerRoots.end(), &element);
        if (found == layerRoots.end()) continue;

        clearInteractionState();
        mFloaters.erase(std::remove_if(mFloaters.begin(), mFloaters.end(), [&element](const auto& floater) { return floater == &element; }),
                        mFloaters.end());
        layerRoots.erase(found);
        element.setSurface(nullptr);
        invalidateOrderingCache();
        requestLayout();
        refreshHover();
        return true;
    }
    return false;
}

std::unique_ptr<Element> Surface::unmount(Element& element) {
    if (!detachRoot(element)) return nullptr;
    return takeOwnedRoot(element);
}

bool Surface::unmountBorrowed(Element& element) {
    if (std::any_of(mOwnedRoots.begin(), mOwnedRoots.end(), [&element](const auto& root) { return root.get() == &element; })) return false;
    return detachRoot(element);
}

void Surface::clearLayer(SurfaceLayer layer) {
    RootList& layerRoots = roots(layer);
    for (auto current = layerRoots.begin(); current != layerRoots.end();) {
        Element* root = *current;
        if (root) {
            mFloaters.erase(std::remove(mFloaters.begin(), mFloaters.end(), root), mFloaters.end());
            root->setSurface(nullptr);
            elementBecameUnavailable(*root);
            (void)takeOwnedRoot(*root);
        }
        current = layerRoots.erase(current);
    }
    mFloaters.erase(
        std::remove_if(mFloaters.begin(), mFloaters.end(), [this, layer](const auto& floater) { return !floater || layerOf(floater) == layer; }),
        mFloaters.end());
    invalidateOrderingCache();
    requestLayout();
    refreshHover();
}

Surface::RootList& Surface::roots(SurfaceLayer layer) {
    return mRoots[static_cast<std::size_t>(layer)];
}

const Surface::RootList& Surface::roots(SurfaceLayer layer) const {
    return mRoots[static_cast<std::size_t>(layer)];
}

const Element* Surface::mountedRoot(const Element* element) const {
    if (!element) return nullptr;
    const Element* root = element;
    while (root->parentElement()) root = root->parentElement();
    return isSurfaceRoot(root) ? root : nullptr;
}

std::optional<SurfaceLayer> Surface::layerOf(const Element* element) const {
    const Element* root = mountedRoot(element);
    if (!root) return std::nullopt;
    for (std::size_t index = 0; index < kSurfaceLayerCount; ++index) {
        const auto layer = static_cast<SurfaceLayer>(index);
        const RootList& layerRoots = roots(layer);
        if (std::any_of(layerRoots.begin(), layerRoots.end(), [root](const auto* candidate) { return candidate == root; })) return layer;
    }
    return std::nullopt;
}

bool Surface::isSurfaceRoot(const Element* element) const {
    if (!element || element->parentElement()) return false;
    for (const RootList& layerRoots : mRoots)
        if (std::any_of(layerRoots.begin(), layerRoots.end(), [element](const auto* root) { return root == element; })) return true;
    return false;
}

void Surface::localeChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Element& element) -> void {
        const ElementRef<Element> lifetime(&element);
        const Surface* surface = element.mSurface;
        const Element* parent = element.mParent;
        std::vector<ElementRef<Element>> children;
        const ElementList authoredChildren = element.children();
        children.reserve(authoredChildren.size());
        for (Element* child : authoredChildren) children.emplace_back(child);
        element.onLocaleChanged(*mSystem);
        Element* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const ElementRef<Element>& childRef : children)
            if (Element* child = childRef.get(); child && child->parentElement() == current) self(self, *child);
    };
    for (const RootList& layerRoots : mRoots)
        for (const auto& root : layerRoots) refresh(refresh, *root);
    requestLayout();
}

void Surface::keybindingsChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Element& element) -> void {
        const ElementRef<Element> lifetime(&element);
        const Surface* surface = element.mSurface;
        const Element* parent = element.mParent;
        std::vector<ElementRef<Element>> children;
        const ElementList authoredChildren = element.children();
        children.reserve(authoredChildren.size());
        for (Element* child : authoredChildren) children.emplace_back(child);
        element.onKeybindingsChanged(*mSystem);
        Element* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const ElementRef<Element>& childRef : children)
            if (Element* child = childRef.get(); child && child->parentElement() == current) self(self, *child);
    };
    for (const RootList& layerRoots : mRoots)
        for (const auto& root : layerRoots) refresh(refresh, *root);
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
    const std::shared_ptr<char> mountLifetime = detail::ElementInternalAccess::mountLifetime(element);
    const auto duplicate = std::find_if(mPendingScrollNotifications.begin(), mPendingScrollNotifications.end(),
                                        [&](const auto& pending) { return pending.element == &element && pending.mountLifetime == mountLifetime; });
    if (duplicate != mPendingScrollNotifications.end()) return;
    mPendingScrollNotifications.push_back({&element, detail::ElementInternalAccess::lifetime(element), mountLifetime});
}

void Surface::dispatchScrollNotification(Element& element) {
    Event event(kScrollEvent, element);
    event.setCancelable(false);
    event.setPhase(EventPhase::Target);
    event.setCurrentTarget(&element);
    element.dispatchListeners(event, true);
    if (!event.immediatePropagationStopped()) element.dispatchListeners(event, false);
    event.setCurrentTarget(nullptr);
}

void Surface::flushScrollNotifications() {
    if (mDispatchingScrollNotifications) return;
    mDispatchingScrollNotifications = true;
    while (!mPendingScrollNotifications.empty()) {
        std::vector<PendingScrollNotification> pending;
        pending.swap(mPendingScrollNotifications);
        for (const PendingScrollNotification& notification : pending) {
            if (!notification.element || notification.lifetime.expired() || notification.mountLifetime == nullptr) continue;
            Element* element = notification.element;
            if (element->mSurface != this
                || detail::ElementInternalAccess::mountLifetime(*element) != notification.mountLifetime
                || !isRootedInSurface(element))
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
    return std::any_of(mFloaters.begin(), mFloaters.end(), [this, &styles](FloaterElement* floater) {
        if (!floater || !isRootedInSurface(floater)) return false;
        return floater->isVisible(styles.style(*floater));
    });
}

StylePass& Surface::stylePass() const {
    if (mPendingStyleSheet && (!mStylePass || !mStylePass->active())) {
        mStyleSheet = mPendingStyleSheet;
        mPendingStyleSheet = nullptr;
        mStylePass.reset();
    }
    const LayoutDirection direction = layoutDirection();
    const NativeAppearance& appearance = effectiveNativeAppearance();
    const bool mismatched = !mStylePass || !mStylePass->matches(*mStyleSheet, mTextMetrics, direction, &appearance);
    if (mismatched && (!mStylePass || !mStylePass->active()))
        mStylePass = std::make_unique<StylePass>(*mStyleSheet, mTextMetrics, direction, &appearance);
    return *mStylePass;
}

void Surface::didPaint(std::uint64_t paintedGeneration) {
    if (mPaintRequestGeneration != paintedGeneration || mLayoutDirty) {
        mPaintDirty = true;
        return;
    }
    mPaintDirty = false;
    for (RootList& layerRoots : mRoots)
        for (auto& root : layerRoots) root->clearPaintInvalidationTree();
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
        for (RootList& layerRoots : mRoots)
            for (auto& root : layerRoots) root->invalidateStyleTree();
    }
    const std::uint64_t textMetricsGeneration = mTextMetrics.generation();
    if (textMetricsGeneration != mObservedTextMetricsGeneration) {
        mObservedTextMetricsGeneration = textMetricsGeneration;
        for (RootList& layerRoots : mRoots)
            for (auto& root : layerRoots) root->invalidateTextTree();
    }
    const LayoutDirection direction = layoutDirection();
    if (direction != mObservedLayoutDirection) {
        mObservedLayoutDirection = direction;
        for (RootList& layerRoots : mRoots)
            for (auto& root : layerRoots) root->invalidateArrangeTree();
    }
    if (!mLayoutDirty) return false;
    mLayoutDirty = false;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto layoutRoot = [&](Element& root) {
        const bool authoredRect = root.mRectExplicit;
        layoutTreeUsingStylePass(root, styles, mScrollLayoutOptions);
        if (authoredRect) return;

        const Style& rootStyle = styles.style(root);
        if (!root.isDisplayed(rootStyle)) return;
        const bool bodyRoot = root.elementName() == "body";
        const Vec2 desired = root.desiredSize();
        const float width = rootStyle.width.isAuto() ? ((rootStyle.display == DisplayMode::Inline
                                                         || rootStyle.display == DisplayMode::InlineBlock
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
        layoutTreeUsingStylePass(root, styles, mScrollLayoutOptions);
    };
    for (const RootList& layerRoots : mRoots)
        for (const auto& root : layerRoots) layoutRoot(*root);
    return true;
}

void Surface::paint(PaintContext& context, float scale, Vec2 pixelOrigin) {
    updateLayout();
    const std::uint64_t paintedGeneration = mPaintRequestGeneration;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    PaintTarget target;
    target.bounds = mViewport;
    target.pixelOrigin = pixelOrigin;
    target.scale = scale;
    target.nativeAppearance = &effectiveNativeAppearance();
    target.clipAA = AAIntent::Coverage;
    context.beginFrame(target);
    context.pushClip(mViewport, scale);
    for (const RootList& layerRoots : mRoots)
        for (const auto& root : layerRoots) paintElement(*root, context, scale, 1.f, styles);
    context.popClip();
    context.endFrame();
    didPaint(paintedGeneration);
}

void Surface::paintElement(const Element& element, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles) const {
    const ElementRef<const Element> lifetime(&element);
    const Surface* originalSurface = element.surface();
    const Element* originalParent = element.parentElement();
    const std::uint64_t originalStyleRevision = element.mStyleRevision;
    const std::uint64_t originalLayoutRevision = element.mLayoutInvalidationRevision;
    const Style& unresolved = styles.style(element);
    const Element* styledElement = lifetime.get();
    if (!styledElement
        || styledElement->surface() != originalSurface
        || styledElement->parentElement() != originalParent
        || styledElement->mStyleRevision != originalStyleRevision
        || styledElement->mLayoutInvalidationRevision != originalLayoutRevision)
        return;
    if (!element.isVisible(unresolved)) return;
    const float childOpacity = inheritedOpacity * unresolved.opacity;
    const LayoutDirection direction = layoutDirection();
    const bool needsOpacity = inheritedOpacity != 1.f || unresolved.opacity != 1.f;
    const bool needsDirection =
        unresolved.direction != direction || unresolved.textAlign == TextAlign::Start || unresolved.textAlign == TextAlign::End;
    std::optional<Style> paintedStorage;
    const Style* painted = &unresolved;
    if (needsOpacity || needsDirection) {
        paintedStorage.emplace(unresolved);
        if (needsOpacity) applyOpacity(*paintedStorage, inheritedOpacity);
        if (needsDirection) applyDirection(*paintedStorage, direction);
        painted = &*paintedStorage;
    }
    const bool paintsBodyCanvasBackground = element.elementName() == "body"
        && !originalParent
        && (painted->backgroundColor.a > 0.f || painted->backgroundGradient.has_value());
    const bool clipsX = unresolved.overflowX != Overflow::Visible;
    const bool clipsY = unresolved.overflowY != Overflow::Visible;
    const bool clipsChildren = clipsX || clipsY;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    if (!painted->effects.empty()) context.beginEffects(element.paintBounds(), *painted, scale);
    if (paintsBodyCanvasBackground) {
        Style canvasBackground;
        canvasBackground.backgroundColor = painted->backgroundColor;
        canvasBackground.backgroundGradient = painted->backgroundGradient;
        context.paintBox(mViewport, canvasBackground);

        Style bodyStyle = *painted;
        bodyStyle.backgroundColor = Color(0.f, 0.f, 0.f, 0.f);
        bodyStyle.backgroundGradient.reset();
        element.paint(context, bodyStyle, scale);
    } else element.paint(context, *painted, scale);
    const auto isParentStillValid = [&] {
        const Element* currentElement = lifetime.get();
        return currentElement
            && currentElement->surface() == originalSurface
            && currentElement->mStyleRevision == originalStyleRevision
            && currentElement->mLayoutInvalidationRevision == originalLayoutRevision
            && currentElement->isVisible(unresolved)
            && isRootedInSurface(currentElement)
            && (currentElement->parentElement() == originalParent || (!originalParent && isSurfaceRoot(currentElement)));
    };
    if (isParentStillValid()) {
        if (clipsChildren) {
            context.pushClip(detail::ElementInternalAccess::scrollport(element), scale, clipAxes);
            context.pushTranslation(scrollContentTranslation(layoutDirection(), {element.scrollLeft(), element.scrollTop()}));
        }
        Style textStyle;
        inheritStyle(textStyle, *painted);
        textStyle.textOverflow = painted->textOverflow;
        textStyle.overflowX = painted->overflowX;
        textStyle.display = DisplayMode::Inline;
        textStyle.displaySet = true;
        textStyle.padding = {};
        textStyle.margin = {};
        for (const auto& childNode : element.mChildren) {
            if (!isParentStillValid()) break;
            if (const Text* text = childNode->asText()) {
                text->paint(context, textStyle, element.styleSheet(), element);
                continue;
            }
            const Element* child = childNode->asElement();
            if (child && child->parentElement() == &element && isRootedInSurface(child) && element.shouldPaintChild(*child, *painted))
                paintElement(*child, context, scale, childOpacity, styles);
        }
        if (clipsChildren) {
            context.popTranslation();
            context.popClip();
        }
    }
    if (isParentStillValid()) {
        const ScrollGeometry geometry = scrollbarGeometry(element, *painted);
        if (geometry.horizontal.visible || geometry.vertical.visible || geometry.hasCorner) {
            NativeScrollbarPaintRequest request;
            request.geometry = geometry;
            request.colors = painted->scrollbarColor;
            request.mode = painted->scrollbarModeSet ? painted->scrollbarMode : mScrollLayoutOptions.scrollbarMode;
            request.metrics = scrollbarMetrics(request.mode);
            request.direction = painted->direction;
            request.scale = scale;
            request.appearanceRevision = effectiveNativeAppearance().revision();
            if (const Element* clipOwner = scrollbarClipOwner(element, *painted)) {
                const Style* clipStyle = clipOwner == &element ? painted : &styles.style(*clipOwner);
                request.clip.enabled = true;
                request.clip.borderBox = clipOwner->rect();
                request.clip.borderRadius = clipStyle->borderRadius;
                request.clip.borderWidth = clipStyle->borderWidth;
            }
            if (mScrollbarHover) {
                if (scrollbarTargetMatches(*mScrollbarHover, element, ScrollbarAxis::Horizontal, ScrollbarPart::NoneValue))
                    request.horizontal.hoveredPart = mScrollbarHover->hit.part;
                if (scrollbarTargetMatches(*mScrollbarHover, element, ScrollbarAxis::Vertical, ScrollbarPart::NoneValue))
                    request.vertical.hoveredPart = mScrollbarHover->hit.part;
            }
            if (mScrollbarCapture) {
                if (scrollbarTargetMatches(*mScrollbarCapture, element, ScrollbarAxis::Horizontal, ScrollbarPart::NoneValue))
                    request.horizontal.pressedPart = mScrollbarCapture->hit.part;
                if (scrollbarTargetMatches(*mScrollbarCapture, element, ScrollbarAxis::Vertical, ScrollbarPart::NoneValue))
                    request.vertical.pressedPart = mScrollbarCapture->hit.part;
            }
            request.horizontal.disabled = geometry.horizontal.visible && geometry.horizontal.maxScrollOffset <= 0.f;
            request.vertical.disabled = geometry.vertical.visible && geometry.vertical.maxScrollOffset <= 0.f;
            context.pushClip(element.rect(), scale);
            context.paintNativeScrollbar(request);
            context.popClip();
        }
    }
    if (!painted->effects.empty()) context.endEffects();
}
} // namespace radia::ui
