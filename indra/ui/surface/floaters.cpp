/**
 * @file floaters.cpp
 * @brief Manages Surface floater placement, ordering, and interaction geometry.
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
#include <algorithm>
#include <iterator>
#include <optional>
#include "layout/engine.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "widgets/floater.h"
#include "widgets/panel.h"

namespace radia::ui {
Floater& Surface::mountFloater(std::unique_ptr<Floater> floater, SurfaceLayer layer) {
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    WidgetRef<Floater> mounted_ref(floater.get());
    mount(std::move(floater), layer);
    Floater* mounted = mounted_ref.get();
    llassert_always(mounted && mounted->parent() == &layerRoot(layer));
    mFloaters.emplace_back(mounted);
    constrainFloater(*mounted);
    mounted = mounted_ref.get();
    llassert_always(mounted && mounted->parent() == &layerRoot(layer));
    return *mounted;
}

std::unique_ptr<Floater> Surface::replaceFloater(Floater& current, std::unique_ptr<Floater> replacement) {
    if (!replacement || !managesFloater(current)) return nullptr;

    Widget* parent = current.parent();
    if (!parent) return nullptr;
    auto found = std::find_if(parent->mChildren.begin(), parent->mChildren.end(), [&current](const auto& child) { return child.get() == &current; });
    if (found == parent->mChildren.end()) return nullptr;

    const auto managed = std::find_if(mFloaters.begin(), mFloaters.end(), [&current](const auto& floater) { return floater.get() == &current; });
    if (managed == mFloaters.end()) return nullptr;

    clearInteractionState();
    std::unique_ptr<Widget> retired = std::move(*found);
    replacement->mParent = parent;
    replacement->setSurface(this);
    *found = std::move(replacement);
    ++parent->mChildSnapshotRevision;
    invalidateOrderingCache();
    managed->set(static_cast<Floater*>(found->get()));
    requestLayout();
    refreshHover();

    retired->mParent = nullptr;
    retired->setSurface(nullptr);
    return std::unique_ptr<Floater>(static_cast<Floater*>(retired.release()));
}

std::unique_ptr<Floater> Surface::unmountFloater(Floater& floater) {
    std::unique_ptr<Widget> widget = unmount(floater);
    return std::unique_ptr<Floater>(static_cast<Floater*>(widget.release()));
}

bool Surface::ownsFloater(const Floater& floater) const {
    return managesFloater(floater);
}

bool Surface::raise(Widget& widget) {
    for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index)
        if (raiseWithinLayer(widget, static_cast<SurfaceLayer>(index))) return true;
    return false;
}

void Surface::constrainFloater(Floater& floater) {
    if (!managesFloater(floater)) return;
    floater.setMovementBounds(mViewport);
    floater.clampToMovementBounds();
}

void Surface::placeFloater(Floater& floater, const Rect& rect) {
    if (!managesFloater(floater)) return;
    const WidgetSnapshot floaterSnapshot = snapshot(floater);
    Rect placed = rect;
    if (floater.canResize()) {
        const Vec2 minimum = minimumFloaterSize(floater);
        if (!snapshotValid(floaterSnapshot) || !isRootedInSurface(floaterSnapshot.lifetime.get())) return;
        placed.w = std::min(mViewport.w, std::max(placed.w, minimum.x));
        placed.h = std::min(mViewport.h, std::max(placed.h, minimum.y));
    }
    floater.setRect(placed);
    constrainFloater(floater);
}

Vec2 Surface::preferredFloaterSize(const Floater& floater) const {
    const WidgetSnapshot floaterSnapshot = snapshot(const_cast<Floater&>(floater));
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style& style = styles.style(floater);
    if (!snapshotValid(floaterSnapshot)) return {};
    const Vec2 measured = measureWidget(floater, *mStyleSheet, mTextMetrics);
    if (!snapshotValid(floaterSnapshot)) return {};
    const auto resolve = [](const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference) {
        const float result = value.resolve(fallback, reference);
        return minimum ? std::max(result, minimum->resolve(result)) : result;
    };
    return {resolve(style.width, style.minWidth, measured.x, mViewport.w), resolve(style.height, style.minHeight, measured.y, mViewport.h)};
}

std::optional<Rect> Surface::initialFloaterRect(const Floater& floater) const {
    const WidgetSnapshot floaterSnapshot = snapshot(const_cast<Floater&>(floater));
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

std::optional<Rect> Surface::prepareFloater(Floater& floater) const {
    const WidgetSnapshot floaterSnapshot = snapshot(floater);
    const std::optional<Rect> authored = initialFloaterRect(floater);
    if (!authored || !snapshotValid(floaterSnapshot)) return std::nullopt;
    Widget* content = floater.content();
    const WidgetSnapshot contentSnapshot = content ? snapshot(*content) : WidgetSnapshot{};
    const Vec2 content_size = content ? measureWidget(*content, *mStyleSheet, mTextMetrics) : Vec2{};
    if (!snapshotValid(floaterSnapshot) || (content && !snapshotChildValid(contentSnapshot, floater))) return std::nullopt;
    floater.setAuthoredSize({authored->w, authored->h}, content_size);
    return authored;
}

bool Surface::raiseWithinLayer(Widget& widget, SurfaceLayer layer) {
    Widget& root = layerRoot(layer);
    Widget* direct_child = &widget;
    while (direct_child->parent() && direct_child->parent() != &root) direct_child = direct_child->parent();
    if (direct_child->parent() != &root) return false;

    auto found =
        std::find_if(root.mChildren.begin(), root.mChildren.end(), [direct_child](const auto& child) { return child.get() == direct_child; });
    if (found == root.mChildren.end()) return false;
    if (std::next(found) != root.mChildren.end()) {
        std::rotate(found, std::next(found), root.mChildren.end());
        ++root.mChildSnapshotRevision;
        invalidateOrderingCache();
        requestPaint();
        refreshHover();
    }
    return true;
}

bool Surface::managesFloater(const Floater& floater) const {
    return (floater.parent() == &layerRoot(SurfaceLayer::Floater) || floater.parent() == &layerRoot(SurfaceLayer::Modal))
        && std::any_of(mFloaters.begin(), mFloaters.end(), [&floater](const auto& managed) { return managed.get() == &floater; });
}

bool Surface::canDetachFloater(const Floater& floater) const {
    return managesFloater(floater) && mFloaterDelegate && mFloaterDelegate->canDetachFloater(*this, floater);
}

void Surface::floaterClosed(Floater& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterClosed(*this, floater);
}

void Surface::floaterMinimizedChanged(Floater& floater, bool minimized) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMinimizedChanged(*this, floater, minimized);
}

void Surface::floaterMoved(Floater& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMoved(*this, floater);
}

void Surface::floaterMoveEnded(Floater& floater) {
    if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMoveEnded(*this, floater);
}

void Surface::floaterDetachRequested(Floater& floater, const Vec2& desiredPosition, const Vec2& dragOffset) {
    if (canDetachFloater(floater)) mFloaterDelegate->floaterDetachRequested(*this, floater, desiredPosition, dragOffset);
}

void Surface::floaterResized(Floater& floater, bool complete) {
    if (!managesFloater(floater)) return;
    requestLayout();
    if (mFloaterDelegate) mFloaterDelegate->floaterResized(*this, floater, complete);
}
} // namespace radia::ui
