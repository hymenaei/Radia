/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <iterator>
#include <optional>
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "elements/floater.h"
#include "elements/panel.h"
#include "layout/engine.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "surface/surfaceinternal.h"

namespace radia::ui {
FloaterElement& Surface::mountFloater(std::unique_ptr<FloaterElement> floater, SurfaceLayer layer) {
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    ElementRef<FloaterElement> mountedRef(floater.get());
    mount(std::move(floater), layer);
    FloaterElement* mounted = mountedRef.get();
    llassert_always(mounted && mounted->parentElement() == nullptr && mountedRoot(mounted) == mounted);
    mFloaters.emplace_back(mounted);
    constrainFloater(*mounted);
    mounted = mountedRef.get();
    llassert_always(mounted && mounted->parentElement() == nullptr && mountedRoot(mounted) == mounted);
    return *mounted;
}

FloaterElement& Surface::mountFloater(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    FloaterElement* floater = root ? dynamic_cast<FloaterElement*>(root) : nullptr;
    llassert_always(floater);
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    mount(*floater, layer);
    mFloaters.emplace_back(floater);
    constrainFloater(*floater);
    return *floater;
}

std::unique_ptr<FloaterElement> Surface::replaceFloater(FloaterElement& current, std::unique_ptr<FloaterElement> replacement) {
    if (!replacement || !managesFloater(current)) return nullptr;

    const std::optional<SurfaceLayer> layer = layerOf(&current);
    if (!layer) return nullptr;
    RootList& layerRoots = roots(*layer);
    auto found = std::find(layerRoots.begin(), layerRoots.end(), &current);
    if (found == layerRoots.end()) return nullptr;

    const auto managed = std::find_if(mFloaters.begin(), mFloaters.end(), [&current](const auto& floater) { return floater == &current; });
    if (managed == mFloaters.end()) return nullptr;

    clearInteractionState();
    std::unique_ptr<Element> retired = takeOwnedRoot(current);
    if (!retired) return nullptr;
    Element* replacementRoot = replacement.get();
    replacement->setSurface(this);
    mOwnedRoots.emplace_back(std::move(replacement));
    *found = replacementRoot;
    invalidateOrderingCache();
    *managed = static_cast<FloaterElement*>(replacementRoot);
    requestLayout();
    refreshHover();

    retired->setSurface(nullptr);
    return std::unique_ptr<FloaterElement>(static_cast<FloaterElement*>(retired.release()));
}

bool Surface::replaceFloater(FloaterElement& current, FloaterElement& replacement) {
    if (&current == &replacement || replacement.parentElement() || replacement.surface() || !managesFloater(current)) return false;

    const std::optional<SurfaceLayer> layer = layerOf(&current);
    if (!layer) return false;
    RootList& layerRoots = roots(*layer);
    const auto found = std::find(layerRoots.begin(), layerRoots.end(), &current);
    if (found == layerRoots.end()) return false;

    const auto managed = std::find_if(mFloaters.begin(), mFloaters.end(), [&current](const auto& floater) { return floater == &current; });
    if (managed == mFloaters.end()) return false;

    clearInteractionState();
    replacement.setSurface(this);
    *found = &replacement;
    current.setSurface(nullptr);
    *managed = &replacement;
    invalidateOrderingCache();
    requestLayout();
    refreshHover();
    return true;
}

std::unique_ptr<FloaterElement> Surface::unmountFloater(FloaterElement& floater) {
    std::unique_ptr<Element> element = unmount(floater);
    return std::unique_ptr<FloaterElement>(static_cast<FloaterElement*>(element.release()));
}

bool Surface::unmountBorrowedFloater(FloaterElement& floater) {
    return unmountBorrowed(floater);
}

bool Surface::ownsFloater(const FloaterElement& floater) const {
    return managesFloater(floater);
}

bool Surface::raise(Element& element) {
    for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index)
        if (raiseWithinLayer(element, static_cast<SurfaceLayer>(index))) return true;
    return false;
}

void Surface::constrainFloater(FloaterElement& floater) {
    if (!managesFloater(floater)) return;
    floater.setMovementBounds(mViewport);
    floater.clampToMovementBounds();
}

void Surface::placeFloater(FloaterElement& floater, const Rect& rect) {
    if (!managesFloater(floater)) return;
    const ElementSnapshot floaterSnapshot = snapshot(floater);
    Rect placed = rect;
    if (floater.resizeable()) {
        const Vec2 minimum = minimumFloaterSize(floater);
        if (!snapshotValid(floaterSnapshot) || !isRootedInSurface(floaterSnapshot.lifetime.get())) return;
        placed.w = std::min(mViewport.w, std::max(placed.w, minimum.x));
        placed.h = std::min(mViewport.h, std::max(placed.h, minimum.y));
    }
    floater.setRect(placed);
    constrainFloater(floater);
}

Vec2 Surface::preferredFloaterSize(const FloaterElement& floater) const {
    const ElementSnapshot floaterSnapshot = snapshot(const_cast<FloaterElement&>(floater));
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style& style = styles.style(floater);
    if (!snapshotValid(floaterSnapshot)) return {};
    const Vec2 measured = measureElement(floater, *mStyleSheet, mTextMetrics);
    if (!snapshotValid(floaterSnapshot)) return {};
    const auto resolve = [](const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
        const float result = value.resolve(fallback, reference);
        return minimum ? std::max(result, minimum->resolve(result)) : result;
    };
    return {resolve(style.width, style.minWidth, measured.x, mViewport.w), resolve(style.height, style.minHeight, measured.y, mViewport.h)};
}

std::optional<Rect> Surface::initialFloaterRect(const FloaterElement& floater) const {
    const ElementSnapshot floaterSnapshot = snapshot(const_cast<FloaterElement&>(floater));
    const Vec2 size = preferredFloaterSize(floater);
    if (!snapshotValid(floaterSnapshot)) return std::nullopt;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style& style = styles.style(floater);
    if (!snapshotValid(floaterSnapshot)) return std::nullopt;
    const float x = style.left ? style.left->resolve(mViewport.w)
        : style.right          ? mViewport.w - style.right->resolve(mViewport.w) - size.x
                               : std::max(0.f, (mViewport.w - size.x) * .5f);
    const float y = style.top ? mViewport.h - style.top->resolve(mViewport.h) - size.y
        : style.bottom        ? style.bottom->resolve(mViewport.h)
                              : std::max(0.f, (mViewport.h - size.y) * .5f);
    return Rect{x, y, size.x, size.y};
}

std::optional<Rect> Surface::prepareFloater(FloaterElement& floater) const {
    const ElementSnapshot floaterSnapshot = snapshot(floater);
    const std::optional<Rect> authored = initialFloaterRect(floater);
    if (!authored || !snapshotValid(floaterSnapshot)) return std::nullopt;
    Element* body = floater.body();
    const ElementSnapshot bodySnapshot = body ? snapshot(*body) : ElementSnapshot{};
    const Vec2 bodySize = body ? measureElement(*body, *mStyleSheet, mTextMetrics) : Vec2{};
    if (!snapshotValid(floaterSnapshot) || (body && !snapshotChildValid(bodySnapshot, floater))) return std::nullopt;
    floater.setAuthoredSize({authored->w, authored->h}, bodySize);
    return authored;
}

bool Surface::raiseWithinLayer(Element& element, SurfaceLayer layer) {
    const Element* directRoot = mountedRoot(&element);
    if (!directRoot || layerOf(directRoot) != layer) return false;
    RootList& layerRoots = roots(layer);
    auto found = std::find(layerRoots.begin(), layerRoots.end(), directRoot);
    if (found == layerRoots.end()) return false;
    if (std::next(found) != layerRoots.end()) {
        std::rotate(found, std::next(found), layerRoots.end());
        invalidateOrderingCache();
        requestPaint();
        refreshHover();
    }
    return true;
}

bool Surface::managesFloater(const FloaterElement& floater) const {
    const std::optional<SurfaceLayer> layer = layerOf(&floater);
    return layer
        && (*layer == SurfaceLayer::Floater || *layer == SurfaceLayer::Modal)
        && mountedRoot(&floater) == &floater
        && std::any_of(mFloaters.begin(), mFloaters.end(), [&floater](const auto& managed) { return managed == &floater; });
}

void Surface::floaterClosed(FloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterClosed(*this, floater);
}

void Surface::floaterMinimizedChanged(FloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMinimizedChanged(*this, floater);
}

void Surface::floaterMoveEnded(FloaterElement& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMoveEnded(*this, floater);
}

void Surface::floaterResizeEnded(FloaterElement& floater) {
    if (!managesFloater(floater)) return;
    if (mFloaterDelegate) mFloaterDelegate->floaterResizeEnded(*this, floater);
}
} // namespace radia::ui
