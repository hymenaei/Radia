/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <iterator>
#include <optional>
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "style/stylepass.h"
#include "surface/surface.h"

namespace radia::ui {
using detail::ElementInternalAccess;

HTMLFloaterElement& Surface::mountFloater(std::unique_ptr<HTMLFloaterElement> floater, SurfaceLayer layer) {
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    ElementRef<HTMLFloaterElement> mountedRef(floater.get());
    mount(std::move(floater), layer);
    HTMLFloaterElement* mounted = mountedRef.get();
    llassert_always(mounted && mounted->parentElement() == nullptr && mountedRoot(mounted) == mounted);
    constrainFloater(*mounted);
    mounted = mountedRef.get();
    llassert_always(mounted && mounted->parentElement() == nullptr && mountedRoot(mounted) == mounted);
    return *mounted;
}

HTMLFloaterElement& Surface::mountFloater(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    HTMLFloaterElement* floater = root ? dynamic_cast<HTMLFloaterElement*>(root) : nullptr;
    llassert_always(floater);
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    mount(document, layer);
    constrainFloater(*floater);
    return *floater;
}

std::unique_ptr<HTMLFloaterElement> Surface::replaceFloater(HTMLFloaterElement& current, std::unique_ptr<HTMLFloaterElement> replacement) {
    if (!replacement || !managesFloater(current)) return nullptr;
    if (replacement->parentNode() || ElementInternalAccess::isMounted(*replacement)) return nullptr;

    const ElementRef<HTMLFloaterElement> currentRef(&current);
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    clearInteractionState();
    if (surfaceLifetime.expired()) return nullptr;
    HTMLFloaterElement* currentElement = currentRef.get();
    if (!currentElement || !managesFloater(*currentElement)) return nullptr;

    Mount* currentMount = findMount(currentElement);
    if (!currentMount || currentMount->ownership != Mount::Ownership::Owned || currentMount->floater != currentElement) return nullptr;
    MountList& layerMounts = mounts(currentMount->layer);
    const auto found = std::find_if(layerMounts.begin(), layerMounts.end(),
                                    [currentElement](const MountPtr& mount) { return mount && mount->root == currentElement; });
    if (found == layerMounts.end()) return nullptr;

    const SurfaceLayer layer = currentMount->layer;
    MountPtr retired = std::move(*found);
    detachBindings(*retired);
    ElementRef<HTMLFloaterElement> retiredRef(currentElement);
    retired->lifetime.reset();
    retired->root = nullptr;
    retired->floater = nullptr;
    Element* replacementRoot = replacement.get();
    MountPtr replacementMount = std::make_unique<Mount>(std::move(replacement), layer, static_cast<HTMLFloaterElement*>(replacementRoot));
    ElementRef<HTMLFloaterElement> replacementRef(static_cast<HTMLFloaterElement*>(replacementRoot));
    *found = std::move(replacementMount);
    if (Element* oldRoot = retiredRef.get(); oldRoot && oldRoot->surface() == this) oldRoot->setSurface(nullptr);
    if (HTMLFloaterElement* installed = replacementRef.get()) installed->setSurface(this);
    invalidateOrderingCache();
    requestLayout();
    refreshHover();

    return std::unique_ptr<HTMLFloaterElement>(static_cast<HTMLFloaterElement*>(retired->ownedRoot.release()));
}

bool Surface::replaceFloater(HTMLFloaterElement& current, HTMLFloaterElement& replacement) {
    if (&current == &replacement
        || replacement.parentElement()
        || (replacement.parentNode() && !replacement.parentNode()->asDocument())
        || ElementInternalAccess::isMounted(replacement)
        || !managesFloater(current))
        return false;

    const ElementRef<HTMLFloaterElement> currentRef(&current);
    const ElementRef<HTMLFloaterElement> replacementRef(&replacement);
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    clearInteractionState();
    if (surfaceLifetime.expired()) return false;
    HTMLFloaterElement* currentElement = currentRef.get();
    HTMLFloaterElement* replacementElement = replacementRef.get();
    if (!currentElement || !replacementElement || !managesFloater(*currentElement)) return false;
    if (replacementElement->parentElement()
        || (replacementElement->parentNode() && !replacementElement->parentNode()->asDocument())
        || ElementInternalAccess::isMounted(*replacementElement))
        return false;

    Mount* currentMount = findMount(currentElement);
    if (!currentMount || currentMount->ownership != Mount::Ownership::Borrowed || currentMount->floater != currentElement) return false;
    MountList& layerMounts = mounts(currentMount->layer);
    const auto found = std::find_if(layerMounts.begin(), layerMounts.end(),
                                    [currentElement](const MountPtr& mount) { return mount && mount->root == currentElement; });
    if (found == layerMounts.end()) return false;

    const SurfaceLayer layer = currentMount->layer;
    MountPtr retired = std::move(*found);
    detachBindings(*retired);
    ElementRef<HTMLFloaterElement> retiredRef(currentElement);
    retired->lifetime.reset();
    retired->root = nullptr;
    retired->floater = nullptr;
    *found = std::make_unique<Mount>(*replacementElement, layer, replacementElement);
    if (Element* oldRoot = retiredRef.get(); oldRoot && oldRoot->surface() == this) oldRoot->setSurface(nullptr);
    if (HTMLFloaterElement* installed = replacementRef.get()) installed->setSurface(this);
    invalidateOrderingCache();
    requestLayout();
    refreshHover();
    return replacementRef.get() != nullptr;
}

std::unique_ptr<HTMLFloaterElement> Surface::unmountFloater(HTMLFloaterElement& floater) {
    std::unique_ptr<Element> element = unmount(floater);
    return std::unique_ptr<HTMLFloaterElement>(static_cast<HTMLFloaterElement*>(element.release()));
}

bool Surface::unmountBorrowedFloater(HTMLFloaterElement& floater) {
    return unmountBorrowed(floater);
}

bool Surface::ownsFloater(const HTMLFloaterElement& floater) const {
    return managesFloater(floater);
}

bool Surface::raise(Element& element) {
    for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index)
        if (raiseWithinLayer(element, static_cast<SurfaceLayer>(index))) return true;
    return false;
}

void Surface::constrainFloater(HTMLFloaterElement& floater) {
    if (!managesFloater(floater)) return;
    floater.setMovementBounds(mViewport);
    floater.clampToMovementBounds();
}

void Surface::placeFloater(HTMLFloaterElement& floater, const Rect& rect) {
    if (!managesFloater(floater)) return;
    const ElementObservation floaterObservation = observe(floater);
    Rect placed = rect;
    if (floater.resizeable()) {
        const Vec2 minimum = minimumFloaterSize(floater);
        if (!floaterObservation.layoutValid() || !floaterObservation.styleValid() || !isRootedInSurface(floaterObservation.get())) return;
        placed.w = std::min(mViewport.w, std::max(placed.w, minimum.x));
        placed.h = std::min(mViewport.h, std::max(placed.h, minimum.y));
    }
    floater.setRect(placed);
    constrainFloater(floater);
}

Vec2 Surface::preferredFloaterSize(HTMLFloaterElement& floater) {
    const ElementObservation floaterObservation = observe(floater);
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const ComputedStyle& style = styles.style(floater);
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return {};
    const Vec2 measured = LayoutEngine::measure(floater, *mStyleSheet, mTextMetrics);
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return {};
    const auto resolve = [](const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
        const float result = value.resolve(fallback, reference);
        return minimum ? std::max(result, minimum->resolve(result)) : result;
    };
    return {resolve(style.width, style.minWidth, measured.x, mViewport.w), resolve(style.height, style.minHeight, measured.y, mViewport.h)};
}

std::optional<Rect> Surface::initialFloaterRect(HTMLFloaterElement& floater) {
    const ElementObservation floaterObservation = observe(floater);
    const Vec2 size = preferredFloaterSize(floater);
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return std::nullopt;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const ComputedStyle& style = styles.style(floater);
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return std::nullopt;
    const float x = style.left ? style.left->resolve(mViewport.w)
        : style.right          ? mViewport.w - style.right->resolve(mViewport.w) - size.x
                               : std::max(0.f, (mViewport.w - size.x) * .5f);
    const float y = style.top ? mViewport.h - style.top->resolve(mViewport.h) - size.y
        : style.bottom        ? style.bottom->resolve(mViewport.h)
                              : std::max(0.f, (mViewport.h - size.y) * .5f);
    return Rect{x, y, size.x, size.y};
}

std::optional<Rect> Surface::prepareFloater(HTMLFloaterElement& floater) {
    const ElementObservation floaterObservation = observe(floater);
    const std::optional<Rect> authored = initialFloaterRect(floater);
    if (!authored || !floaterObservation.layoutValid() || !floaterObservation.styleValid()) return std::nullopt;
    HTMLFloaterElement* currentFloater = dynamic_cast<HTMLFloaterElement*>(floaterObservation.get());
    if (!currentFloater) return std::nullopt;
    Element* body = currentFloater->body();
    const ElementRef<Element> bodyRef(body);
    body = bodyRef.get();
    if (body && body->parentElement() != currentFloater) return std::nullopt;
    const ElementObservation bodyObservation = body ? observe(*body) : ElementObservation{};
    if (body && (!bodyObservation.layoutValid() || !bodyObservation.styleValid() || !bodyObservation.attachedTo(*currentFloater)))
        return std::nullopt;
    const Vec2 bodySize = body ? LayoutEngine::measure(*body, *mStyleSheet, mTextMetrics) : Vec2{};
    body = bodyRef.get();
    if (!floaterObservation.layoutValid()
        || !floaterObservation.styleValid()
        || (body
            && (!bodyObservation.layoutValid()
                || !bodyObservation.styleValid()
                || !bodyObservation.attachedTo(*currentFloater)
                || body->parentElement() != currentFloater
                || currentFloater->body() != body)))
        return std::nullopt;
    currentFloater->setAuthoredSize({authored->w, authored->h}, bodySize);
    return authored;
}

bool Surface::raiseWithinLayer(Element& element, SurfaceLayer layer) {
    const Element* directRoot = mountedRoot(&element);
    if (!directRoot || layerOf(directRoot) != layer) return false;
    MountList& layerMounts = mounts(layer);
    const auto found =
        std::find_if(layerMounts.begin(), layerMounts.end(), [directRoot](const MountPtr& mount) { return mount && mount->root == directRoot; });
    if (found == layerMounts.end()) return false;
    if (std::next(found) != layerMounts.end()) {
        std::rotate(found, std::next(found), layerMounts.end());
        invalidateOrderingCache();
        requestPaint();
        refreshHover();
    }
    return true;
}

bool Surface::managesFloater(const HTMLFloaterElement& floater) const {
    const Mount* mount = findMount(&floater);
    return mount
        && mount->root == &floater
        && mount->floater == &floater
        && (mount->layer == SurfaceLayer::Floater || mount->layer == SurfaceLayer::Modal);
}

void Surface::floaterClosed(HTMLFloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterClosed(*this, floater);
}

void Surface::floaterMinimizedChanged(HTMLFloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMinimizedChanged(*this, floater);
}

void Surface::floaterMoveEnded(HTMLFloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMoveEnded(*this, floater);
}

void Surface::floaterResizeEnded(HTMLFloaterElement& floater) {
    if (!managesFloater(floater)) return;
    if (mFloaterDelegate) mFloaterDelegate->floaterResizeEnded(*this, floater);
}
} // namespace radia::ui
