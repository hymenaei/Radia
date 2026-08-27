/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include "elements/floater.h"
#include "layout/engine.h"
#include "style/stylepass.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "surface/surfaceinternal.h"

namespace radia::ui {
namespace {
bool blocksPointerEvents(const FloaterElement& floater, const Style& style) {
    const PointerEvents policy = style.pointerEvents;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return floater.pointerEvents();
}
} // namespace

Vec2 Surface::minimumFloaterSize(const FloaterElement& floater) const {
    const Vec2 authoredSize = floater.authoredSize();
    const ElementSnapshot floaterSnapshot = snapshot(const_cast<FloaterElement&>(floater));
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const Style& floaterStyle = styles.style(floater);
    if (!snapshotValid(floaterSnapshot)) return {};
    Vec2 minimum{floaterStyle.minWidth ? floaterStyle.minWidth->resolve(authoredSize.x) : 0.f,
                 floaterStyle.minHeight ? floaterStyle.minHeight->resolve(authoredSize.y) : 0.f};

    if (const Element* head = floater.head()) {
        const ElementSnapshot headSnapshot = snapshot(*const_cast<Element*>(head));
        const Vec2 measured = measureElement(*head, *mStyleSheet, mTextMetrics);
        if (!snapshotValid(floaterSnapshot) || !snapshotChildValid(headSnapshot, floater)) return {};
        const Style& headStyle = styles.style(*head);
        if (!snapshotValid(floaterSnapshot) || !snapshotChildValid(headSnapshot, floater)) return {};
        minimum.x = std::max(minimum.x, measured.x + headStyle.margin.horizontal() + floaterStyle.padding.horizontal());
        minimum.y = std::max(minimum.y, measured.y + headStyle.margin.vertical() + floaterStyle.padding.vertical());
    }
    return minimum;
}

FloaterElement* Surface::resizeFloaterAt(const Vec2& point, std::uint8_t& edges) const {
    edges = 0;
    if (!mViewport.contains(point)) return nullptr;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto findInLayer = [&](SurfaceLayer layer) -> FloaterElement* {
        const RootList& layerRoots = roots(layer);
        for (auto child = layerRoots.rbegin(); child != layerRoots.rend(); ++child) {
            auto* floater = dynamic_cast<FloaterElement*>(*child);
            if (!floater || floater->closed()) continue;
            const ElementSnapshot floaterSnapshot = snapshot(*floater);
            const Style& floaterStyle = styles.style(*floater);
            if (!snapshotValid(floaterSnapshot) || !isRootedInSurface(floaterSnapshot.lifetime.get()) || !floater->isVisible(floaterStyle)) continue;
            const bool floaterBlocksPointerEvents = blocksPointerEvents(*floater, floaterStyle);
            if (!snapshotValid(floaterSnapshot) || !isRootedInSurface(floaterSnapshot.lifetime.get())) continue;
            floater = dynamic_cast<FloaterElement*>(floaterSnapshot.lifetime.get());
            if (!floater || floater->closed() || !floater->isVisible(floaterStyle)) continue;
            if (!floaterBlocksPointerEvents) {
                const bool descendantHit = hitTestNode(*floater, point, mViewport, styles) != nullptr;
                floater = dynamic_cast<FloaterElement*>(floaterSnapshot.lifetime.get());
                if (!snapshotValid(floaterSnapshot)
                    || !floater
                    || !isRootedInSurface(floater)
                    || !floater->isVisible(floaterStyle)
                    || floater->closed())
                    continue;
                if (descendantHit) return nullptr;
                if (!floater->rect().contains(point)) continue;
                continue;
            }
            if (!floater->rect().contains(point)) continue;
            if (!floater->resizeable() || floater->minimized()) return nullptr;
            const detail::ResizeEdges hit = detail::resizeEdgesAt(floater->rect(), point);
            if (hit == detail::ResizeEdges::NoEdges) return nullptr;
            edges = static_cast<std::uint8_t>(hit);
            return floater;
        }
        return nullptr;
    };

    if (hasActiveModal()) return findInLayer(SurfaceLayer::Modal);
    return findInLayer(SurfaceLayer::Floater);
}

void Surface::updateResizeCursor(const Vec2& point) {
    if (Element* captured = mCaptured) {
        if (auto* floater = dynamic_cast<FloaterElement*>(captured); floater && floater->mInteraction == FloaterElement::FloaterInteraction::Resize)
            return;
    }
    std::uint8_t edges = 0;
    resizeFloaterAt(point, edges);
    mResizeCursor = detail::resizeCursor(static_cast<detail::ResizeEdges>(edges));
}
} // namespace radia::ui
