/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "surface/surface.h"
#include <algorithm>
#include <optional>
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "html/element.h"
#include "html/elementnames.h"
#include "html/floater.h"
#include "layout/engine.h"
#include "layout/engineinternal.h"
#include "layout/primitives.h"
#include "render/paintcontext.h"
#include "style/stylepass.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::MountEpoch;
using detail::NodeRef;

namespace {
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
        if (dynamic_cast<const HTMLFloaterElement*>(ancestor)) return ancestor;
    return nullptr;
}
} // namespace

Surface::ElementObservation Surface::observe(Element& element) const {
    return ElementObservation(element);
}
Surface::ConstElementObservation Surface::observe(const Element& element) const {
    return ConstElementObservation(element);
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
    mLifetime.reset();
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
        if (HTMLFloaterElement* managed = *floater) {
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

Element& Surface::mount(std::unique_ptr<Element> element, SurfaceLayer layer) {
    if (layer == SurfaceLayer::Modal) clearInteractionState();
    llassert_always(element && !element->parentNode() && !element->surface());
    Element* mounted = element.get();
    mOwnedRoots.emplace_back(std::move(element));
    roots(layer).push_back(mounted);
    mounted->setSurface(this);
    llassert_always(mounted && mounted->parentElement() == nullptr && mounted->surface() == this);
    invalidateOrderingCache();
    requestLayout();
    return *mounted;
}

Element& Surface::mount(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    llassert_always(root && root->parentNode() == &document && !root->surface());

    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    const std::weak_ptr<char> rootLifetime = ElementInternalAccess::lifetime(*root);
    document.addDestructionObserver([this, root, surfaceLifetime, rootLifetime] {
        if (surfaceLifetime.expired() || rootLifetime.expired()) return;
        (void)unmountBorrowed(*root);
    });

    if (layer == SurfaceLayer::Modal) clearInteractionState();
    roots(layer).push_back(root);
    root->setSurface(this);
    invalidateOrderingCache();
    requestLayout();
    return *root;
}

Element& Surface::mount(Element& element, SurfaceLayer layer) {
    if (layer == SurfaceLayer::Modal) clearInteractionState();
    llassert_always(!element.parentNode() && !element.surface());
    roots(layer).push_back(&element);
    element.setSurface(this);
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
        if (auto* htmlElement = dynamic_cast<HTMLElement*>(&element)) htmlElement->onKeybindingsChanged(*mSystem);
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
    const MountEpoch mountEpoch = ElementInternalAccess::mountEpoch(element);
    const auto duplicate = std::find_if(mPendingScrollNotifications.begin(), mPendingScrollNotifications.end(),
                                        [&](const auto& pending) { return pending.element == &element && pending.mountEpoch == mountEpoch; });
    if (duplicate != mPendingScrollNotifications.end()) return;
    mPendingScrollNotifications.push_back({&element, ElementInternalAccess::lifetime(element), mountEpoch});
}

void Surface::dispatchScrollNotification(Element& element) {
    Event event(kScrollEvent, element);
    event.setCancelable(false);
    event.setPhase(EventPhase::Target);
    event.setCurrentTarget(&element);
    const Element::EventListenerSnapshot listeners = element.eventListenerSnapshot();
    element.dispatchListeners(event, true, listeners);
    if (!event.immediatePropagationStopped()) element.dispatchListeners(event, false, listeners);
    event.setCurrentTarget(nullptr);
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
    return std::any_of(mFloaters.begin(), mFloaters.end(), [this, &styles](HTMLFloaterElement* floater) {
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
    const ConstElementObservation observation = observe(element);
    const Style& unresolved = styles.style(element);
    const Element* current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid()) return;
    if (!current->isVisible(unresolved)) return;
    styles.styleGeneratedPseudoElements(*current, unresolved);
    current = observation.get();
    if (!current || !observation.layoutValid() || !observation.styleValid()) return;
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
    const bool paintsBodyCanvasBackground = current->elementName() == kBodyTag.localName
        && !observation.parent
        && (painted->backgroundColor.a > 0.f || painted->backgroundGradient.has_value());
    const bool clipsX = unresolved.overflowX != Overflow::Visible;
    const bool clipsY = unresolved.overflowY != Overflow::Visible;
    const bool clipsChildren = clipsX || clipsY;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    if (!painted->effects.empty()) context.beginEffects(current->paintBounds(), *painted, scale);
    if (paintsBodyCanvasBackground) {
        Style canvasBackground;
        canvasBackground.backgroundColor = painted->backgroundColor;
        canvasBackground.backgroundGradient = painted->backgroundGradient;
        context.paintBox(mViewport, canvasBackground);

        Style bodyStyle = *painted;
        bodyStyle.backgroundColor = Color(0.f, 0.f, 0.f, 0.f);
        bodyStyle.backgroundGradient.reset();
        current->paint(context, bodyStyle, scale);
    } else current->paint(context, *painted, scale);
    const auto isParentStillValid = [&] {
        const Element* currentElement = observation.get();
        return currentElement
            && observation.layoutValid()
            && observation.styleValid()
            && currentElement->isVisible(unresolved)
            && isRootedInSurface(currentElement)
            && (currentElement->parentElement() == observation.parent || (!observation.parent && isSurfaceRoot(currentElement)));
    };
    if (isParentStillValid()) {
        current = observation.get();
        if (clipsChildren) {
            context.pushClip(ElementInternalAccess::scrollport(*current), scale, clipAxes);
            context.pushTranslation(scrollContentTranslation(layoutDirection(), {current->scrollLeft(), current->scrollTop()}));
        }
        Style textStyle;
        inheritStyle(textStyle, *painted);
        textStyle.textOverflow = painted->textOverflow;
        textStyle.overflowX = painted->overflowX;
        textStyle.display = DisplayMode::Inline;
        textStyle.displaySet = true;
        textStyle.padding = {};
        textStyle.margin = {};
        std::vector<NodeRef> children;
        children.reserve(current->mChildren.size());
        for (const auto& childNode : current->mChildren) children.emplace_back(childNode.get());
        for (const NodeRef& childRef : children) {
            if (!isParentStillValid()) break;
            const Node* childNode = childRef.get();
            if (!childNode) continue;
            if (const Text* text = childNode->asText()) {
                const Element* parent = observation.get();
                if (!parent) break;
                text->paint(context, textStyle, parent->styleSheet(), *parent);
                continue;
            }
            const Element* child = childNode->asElement();
            if (child && child->parentElement() == observation.get() && isRootedInSurface(child))
                paintElement(*child, context, scale, childOpacity, styles);
        }
        if (clipsChildren) {
            context.popTranslation();
            context.popClip();
        }
    }
    if (isParentStillValid()) {
        current = observation.get();
        const ScrollGeometry geometry = scrollbarGeometry(*current, *painted);
        if (geometry.horizontal.visible || geometry.vertical.visible || geometry.hasCorner) {
            NativeScrollbarPaintRequest request;
            request.geometry = geometry;
            request.colors = painted->scrollbarColor;
            request.mode = painted->scrollbarModeSet ? painted->scrollbarMode : mScrollLayoutOptions.scrollbarMode;
            request.metrics = scrollbarMetrics(request.mode);
            request.direction = painted->direction;
            request.scale = scale;
            request.appearanceRevision = effectiveNativeAppearance().revision();
            if (const Element* clipOwner = scrollbarClipOwner(*current, *painted)) {
                const Style* clipStyle = clipOwner == current ? painted : &styles.style(*clipOwner);
                request.clip.enabled = true;
                request.clip.borderBox = clipOwner->rect();
                request.clip.borderRadius = clipStyle->borderRadius;
                request.clip.borderWidth = clipStyle->borderWidth;
            }
            if (mScrollbarHover) {
                if (scrollbarTargetMatches(*mScrollbarHover, *current, ScrollbarAxis::Horizontal, ScrollbarPart::NoneValue))
                    request.horizontal.hoveredPart = mScrollbarHover->hit.part;
                if (scrollbarTargetMatches(*mScrollbarHover, *current, ScrollbarAxis::Vertical, ScrollbarPart::NoneValue))
                    request.vertical.hoveredPart = mScrollbarHover->hit.part;
            }
            if (mScrollbarCapture) {
                if (scrollbarTargetMatches(*mScrollbarCapture, *current, ScrollbarAxis::Horizontal, ScrollbarPart::NoneValue))
                    request.horizontal.pressedPart = mScrollbarCapture->hit.part;
                if (scrollbarTargetMatches(*mScrollbarCapture, *current, ScrollbarAxis::Vertical, ScrollbarPart::NoneValue))
                    request.vertical.pressedPart = mScrollbarCapture->hit.part;
            }
            request.horizontal.disabled = geometry.horizontal.visible && geometry.horizontal.maxScrollOffset <= 0.f;
            request.vertical.disabled = geometry.vertical.visible && geometry.vertical.maxScrollOffset <= 0.f;
            context.pushClip(current->rect(), scale);
            context.paintNativeScrollbar(request);
            context.popClip();
        }
    }
    if (!painted->effects.empty()) context.endEffects();
}
} // namespace radia::ui
