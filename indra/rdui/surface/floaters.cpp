/**
 * @file floaters.cpp
 * @brief
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
#include "surface/surface.h"
#include "widgets/floater.h"
#include "widgets/panel.h"

namespace rdui {
Floater& Surface::mountFloater(std::unique_ptr<Floater> floater, SurfaceLayer layer) {
    if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
    Floater* mounted = floater.get();
    mount(std::move(floater), layer);
    mFloaters.emplace_back(mounted);
    constrainFloater(*mounted);
    return *mounted;
}

std::unique_ptr<Floater> Surface::unmountFloater(Floater& floater) {
    std::unique_ptr<Widget> widget = unmount(floater);
    return std::unique_ptr<Floater>(static_cast<Floater*>(widget.release()));
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
    Rect placed = rect;
    if (floater.canResize()) {
        const Vec2 minimum = minimumFloaterSize(floater);
        placed.w = std::min(mViewport.w, std::max(placed.w, minimum.x));
        placed.h = std::min(mViewport.h, std::max(placed.h, minimum.y));
    }
    floater.setRect(placed);
    constrainFloater(floater);
}

Vec2 Surface::preferredFloaterSize(const Floater& floater) const {
    const Style style = resolveWidgetStyle(*mStyleSheet, floater);
    const Vec2 measured = measureWidget(floater, *mStyleSheet, mTextMetrics);
    const auto resolve = [](const Dimension& value, const std::optional<Length>& minimum, float fallback) {
        const float result = value.resolve(fallback);
        return minimum ? std::max(result, minimum->pixels) : result;
    };
    return {resolve(style.width, style.min_width, measured.x), resolve(style.height, style.min_height, measured.y)};
}

Rect Surface::initialFloaterRect(const Floater& floater) const {
    const Style style = resolveWidgetStyle(*mStyleSheet, floater);
    const Vec2 size = preferredFloaterSize(floater);
    const float x = style.left ? style.left->pixels
        : style.right          ? mViewport.w - style.right->pixels - size.x
                               : std::max(0.f, (mViewport.w - size.x) * .5f);
    const float y = style.top ? mViewport.h - style.top->pixels - size.y
        : style.bottom        ? style.bottom->pixels
                              : std::max(0.f, (mViewport.h - size.y) * .5f);
    return {x, y, size.x, size.y};
}

Rect Surface::prepareFloater(Floater& floater) const {
    const Rect authored = initialFloaterRect(floater);
    const Vec2 content_size = floater.content() ? measureWidget(*floater.content(), *mStyleSheet, mTextMetrics) : Vec2{};
    floater.setAuthoredSize({authored.w, authored.h}, content_size);
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

void Surface::floaterDetachRequested(Floater& floater, const Vec2& desired, const Vec2& drag_offset) {
    if (canDetachFloater(floater)) mFloaterDelegate->floaterDetachRequested(*this, floater, desired, drag_offset);
}

void Surface::floaterResized(Floater& floater, bool complete) {
    if (!managesFloater(floater)) return;
    requestLayout();
    if (mFloaterDelegate) mFloaterDelegate->floaterResized(*this, floater, complete);
}
} // namespace rdui
