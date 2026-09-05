/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "surface/surface.h"
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
#include "render/paintcontext.h"
#include "style/stylepass.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::MountEpoch;
using detail::NodeRef;

namespace {
void applyDirection(ComputedStyle& style, LayoutDirection direction) {
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

bool isDetachedOwnedRoot(const Element& element) {
    return !element.parentNode() && !ElementInternalAccess::isMounted(element);
}

bool isDetachedBorrowedRoot(const Element& element) {
    return !element.parentElement() && (!element.parentNode() || element.parentNode()->asDocument()) && !ElementInternalAccess::isMounted(element);
}

const Element* scrollbarClipOwner(const Element& element, const ComputedStyle& style) {
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
    mScrollLayoutOptions.nativeMetrics = system.nativeAppearance().layoutMetrics();
    mNativeAppearanceRevision = mScrollLayoutOptions.nativeMetrics.revision;
    system.registerSurface(*this);
}

Surface::Mount::Mount(std::unique_ptr<Element> root, SurfaceLayer layer, HTMLFloaterElement* floater)
    : root(root.get()), ownedRoot(std::move(root)), layer(layer), ownership(Ownership::Owned), floater(floater) {}

Surface::Mount::Mount(Element& root, SurfaceLayer layer, HTMLFloaterElement* floater)
    : root(&root), layer(layer), ownership(Ownership::Borrowed), floater(floater) {}

Surface::Mount::~Mount() = default;

Surface::~Surface() {
    mLifetime.reset();

    std::vector<ElementRef<Element>> mountedRoots;
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts) {
            if (!mount) continue;
            detachBindings(*mount);
            if (!mount->root) continue;
            mountedRoots.emplace_back(mount->root);
            mount->lifetime.reset();
            mount->root = nullptr;
            mount->floater = nullptr;
        }

    clearInteractionState();
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->surface() == this) root->setSurface(nullptr);

    for (MountList& layerMounts : mMounts) layerMounts.clear();
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

Element& Surface::mount(std::unique_ptr<Element> element, SurfaceLayer layer) {
    llassert_always(element);
    HTMLFloaterElement* floater = dynamic_cast<HTMLFloaterElement*>(element.get());
    return installMount(std::make_unique<Mount>(std::move(element), layer, floater));
}

Element& Surface::mount(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    llassert_always(root && root->parentNode() == &document && !root->surface());

    MountPtr mount = std::make_unique<Mount>(*root, layer, dynamic_cast<HTMLFloaterElement*>(root));
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    const std::weak_ptr<char> rootLifetime = ElementInternalAccess::lifetime(*root);
    const std::weak_ptr<char> mountLifetime = mount->lifetime;
    document.addDestructionObserver([this, root, surfaceLifetime, rootLifetime, mountLifetime] {
        if (surfaceLifetime.expired() || rootLifetime.expired() || mountLifetime.expired()) return;
        (void)unmountBorrowed(*root);
    });

    return installMount(std::move(mount));
}

Element& Surface::mount(Element& element, SurfaceLayer layer) {
    return installMount(std::make_unique<Mount>(element, layer, dynamic_cast<HTMLFloaterElement*>(&element)));
}

Element& Surface::installMount(MountPtr mount) {
    llassert_always(mount
                    && mount->root
                    && (mount->ownership == Mount::Ownership::Owned ? isDetachedOwnedRoot(*mount->root) : isDetachedBorrowedRoot(*mount->root)));
    if (mount->layer == SurfaceLayer::Modal) clearInteractionState();

    Element* mounted = mount->root;
    ElementRef<Element> mountedRef(mounted);
    mounts(mount->layer).emplace_back(std::move(mount));
    mounted->setSurface(this);
    Element* current = mountedRef.get();
    llassert_always(current && current->parentElement() == nullptr && current->surface() == this);
    invalidateOrderingCache();
    requestLayout();
    return *current;
}

Surface::MountList& Surface::mounts(SurfaceLayer layer) {
    return mMounts[static_cast<std::size_t>(layer)];
}

const Surface::MountList& Surface::mounts(SurfaceLayer layer) const {
    return mMounts[static_cast<std::size_t>(layer)];
}

Surface::Mount* Surface::findMount(Element* element) noexcept {
    if (!element) return nullptr;
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts)
            if (mount && mount->root == element) return mount.get();
    return nullptr;
}

bool Surface::attachBinding(Element& root, Binding& binding) {
    Element* mounted = mountedRoot(&root);
    Mount* mount = findMount(mounted);
    if (!mount) return false;
    if (std::find(mount->bindings.begin(), mount->bindings.end(), &binding) == mount->bindings.end()) mount->bindings.push_back(&binding);
    binding.mAttachedSurface = this;
    return true;
}

void Surface::detachBinding(Binding& binding) noexcept {
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts)
            if (mount) mount->bindings.erase(std::remove(mount->bindings.begin(), mount->bindings.end(), &binding), mount->bindings.end());
    if (binding.mAttachedSurface == this) binding.mAttachedSurface = nullptr;
}

void Surface::replaceBinding(Binding& current, Binding& replacement) noexcept {
    bool replaced = false;
    for (MountList& layerMounts : mMounts) {
        for (MountPtr& mount : layerMounts) {
            if (!mount) continue;
            for (Binding*& binding : mount->bindings) {
                if (binding != &current) continue;
                binding = &replacement;
                replaced = true;
            }
        }
    }
    current.mAttachedSurface = nullptr;
    replacement.mAttachedSurface = replaced ? this : nullptr;
}

void Surface::detachBindings(Mount& mount) noexcept {
    std::vector<Binding*> bindings;
    bindings.swap(mount.bindings);
    for (Binding* binding : bindings) {
        if (!binding || binding->mAttachedSurface != this) continue;
        binding->mAttachedSurface = nullptr;
        binding->deactivate();
    }
}

const Surface::Mount* Surface::findMount(const Element* element) const noexcept {
    if (!element) return nullptr;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root == element) return mount.get();
    return nullptr;
}

Surface::MountPtr Surface::detachMount(Element& element) {
    for (MountList& layerMounts : mMounts) {
        const auto found =
            std::find_if(layerMounts.begin(), layerMounts.end(), [&element](const MountPtr& mount) { return mount && mount->root == &element; });
        if (found == layerMounts.end()) continue;

        MountPtr detached = std::move(*found);
        layerMounts.erase(found);
        detachBindings(*detached);
        ElementRef<Element> rootRef(detached->root);
        detached->lifetime.reset();
        detached->root = nullptr;
        detached->floater = nullptr;
        clearInteractionState();
        if (Element* root = rootRef.get(); root && root->surface() == this) root->setSurface(nullptr);
        invalidateOrderingCache();
        requestLayout();
        refreshHover();
        return detached;
    }
    return nullptr;
}

void Surface::elementOwnerDestroyed(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Borrowed) return;
    (void)detachMount(element);
}

std::unique_ptr<Element> Surface::unmount(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Owned) return nullptr;
    MountPtr detached = detachMount(element);
    if (!detached) return nullptr;
    return std::move(detached->ownedRoot);
}

bool Surface::unmountBorrowed(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Borrowed) return false;
    return static_cast<bool>(detachMount(element));
}

void Surface::clearLayer(SurfaceLayer layer) {
    MountList& layerMounts = mounts(layer);
    while (!layerMounts.empty()) {
        Element* root = layerMounts.front() ? layerMounts.front()->root : nullptr;
        if (!root) {
            layerMounts.erase(layerMounts.begin());
            continue;
        }
        (void)detachMount(*root);
    }
    invalidateOrderingCache();
    requestLayout();
    refreshHover();
}

Element* Surface::mountedRoot(Element* element) {
    if (!element) return nullptr;
    Element* root = element;
    while (root->parentElement()) root = root->parentElement();
    return isSurfaceRoot(root) ? root : nullptr;
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
    const Mount* mount = findMount(root);
    return mount ? std::optional<SurfaceLayer>(mount->layer) : std::nullopt;
}

bool Surface::isSurfaceRoot(const Element* element) const {
    return element && !element->parentElement() && findMount(element);
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
    std::vector<ElementRef<Element>> mountedRoots;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) mountedRoots.emplace_back(mount->root);
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->mSurface == this && isSurfaceRoot(root)) refresh(refresh, *root);
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
    std::vector<ElementRef<Element>> mountedRoots;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) mountedRoots.emplace_back(mount->root);
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->mSurface == this && isSurfaceRoot(root)) refresh(refresh, *root);
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
    const ElementRef<Element> target(&element);
    Event event(kScrollEvent, element);
    event.setCancelable(false);
    event.setPhase(EventPhase::Target);
    event.setCurrentTarget(&element);
    element.dispatchListeners(event, true);
    if (target && !event.immediatePropagationStopped()) element.dispatchListeners(event, false);
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
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) paintElement(*mount->root, context, scale, 1.f, styles);
    context.popClip();
    context.endFrame();
    didPaint(paintedGeneration);
}

void Surface::paintElement(const Element& element, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles) const {
    const ConstElementObservation observation = observe(element);
    const ComputedStyle& unresolved = styles.style(element);
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
    std::optional<ComputedStyle> paintedStorage;
    const ComputedStyle* painted = &unresolved;
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
        ComputedStyle canvasBackground;
        canvasBackground.backgroundColor = painted->backgroundColor;
        canvasBackground.backgroundGradient = painted->backgroundGradient;
        context.paintBox(mViewport, canvasBackground);

        ComputedStyle bodyStyle = *painted;
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
        const ComputedStyle textStyle = Text::styleForParent(*painted);
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
                const ComputedStyle* clipStyle = clipOwner == current ? painted : &styles.style(*clipOwner);
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
