/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "layout/engine.h"
#include "style/stylepass.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"

namespace radia::ui {
using detail::resizeCursor;
using detail::ResizeEdges;
using detail::resizeEdgesAt;

namespace {
bool blocksPointerEvents(const HTMLFloaterElement& floater, const ComputedStyle& style) {
    const PointerEvents policy = style.pointerEvents;
    if (policy == PointerEvents::Auto) return true;
    if (policy == PointerEvents::PassThrough) return false;
    return floater.pointerEvents();
}
} // namespace

Vec2 Surface::minimumFloaterSize(HTMLFloaterElement& floater) {
    const ElementObservation floaterObservation = observe(floater);
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return {};
    HTMLFloaterElement* currentFloater = dynamic_cast<HTMLFloaterElement*>(floaterObservation.get());
    if (!currentFloater) return {};
    const ComputedStyle floaterStyle = styles.style(*currentFloater);
    if (!floaterObservation.layoutValid() || !floaterObservation.styleValid()) return {};
    const Vec2 authoredSize = currentFloater->authoredSize();
    Vec2 minimum{floaterStyle.minWidth ? floaterStyle.minWidth->resolve(authoredSize.x) : 0.f,
                 floaterStyle.minHeight ? floaterStyle.minHeight->resolve(authoredSize.y) : 0.f};

    if (Element* head = currentFloater->head()) {
        const ElementRef<Element> headRef(head);
        head = headRef.get();
        if (!head || head->parentElement() != currentFloater) return {};
        const ElementObservation headObservation = observe(*head);
        if (!headObservation.layoutValid() || !headObservation.styleValid() || !headObservation.attachedTo(*currentFloater)) return {};
        const Vec2 measured = LayoutEngine::measure(*head, *mStyleSheet, mTextMetrics);
        head = headRef.get();
        if (!floaterObservation.layoutValid()
            || !floaterObservation.styleValid()
            || !head
            || !headObservation.layoutValid()
            || !headObservation.styleValid()
            || !headObservation.attachedTo(*currentFloater)
            || head->parentElement() != currentFloater
            || currentFloater->head() != head)
            return {};
        const ComputedStyle& headStyle = styles.style(*head);
        if (!floaterObservation.layoutValid()
            || !floaterObservation.styleValid()
            || !headObservation.layoutValid()
            || !headObservation.styleValid()
            || !headObservation.attachedTo(*currentFloater)
            || head->parentElement() != currentFloater
            || currentFloater->head() != head)
            return {};
        minimum.x = std::max(minimum.x, measured.x + headStyle.margin.horizontal() + floaterStyle.padding.horizontal());
        minimum.y = std::max(minimum.y, measured.y + headStyle.margin.vertical() + floaterStyle.padding.vertical());
    }
    return minimum;
}

HTMLFloaterElement* Surface::resizeFloaterAt(const Vec2& point, std::uint8_t& edges) const {
    edges = 0;
    if (!mViewport.contains(point)) return nullptr;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    const auto findInLayer = [&](SurfaceLayer layer) -> HTMLFloaterElement* {
        const MountList& layerMounts = mounts(layer);
        for (auto child = layerMounts.rbegin(); child != layerMounts.rend(); ++child) {
            if (!*child || !(*child)->root) continue;
            auto* floater = dynamic_cast<HTMLFloaterElement*>((*child)->root);
            if (!floater || floater->closed()) continue;
            const ElementObservation floaterObservation = observe(*floater);
            const ComputedStyle& floaterStyle = styles.style(*floater);
            if (!floaterObservation.layoutValid()
                || !floaterObservation.styleValid()
                || !isRootedInSurface(floaterObservation.get())
                || !floater->isVisible(floaterStyle))
                continue;
            const bool floaterBlocksPointerEvents = blocksPointerEvents(*floater, floaterStyle);
            if (!floaterObservation.layoutValid() || !floaterObservation.styleValid() || !isRootedInSurface(floaterObservation.get())) continue;
            floater = dynamic_cast<HTMLFloaterElement*>(floaterObservation.get());
            if (!floater || floater->closed() || !floater->isVisible(floaterStyle)) continue;
            if (!floaterBlocksPointerEvents) {
                const bool descendantHit = hitTestNode(*floater, point, mViewport, styles) != nullptr;
                floater = dynamic_cast<HTMLFloaterElement*>(floaterObservation.get());
                if (!floaterObservation.layoutValid()
                    || !floaterObservation.styleValid()
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
            const ResizeEdges hit = resizeEdgesAt(floater->rect(), point);
            if (hit == ResizeEdges::NoEdges) return nullptr;
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
        if (auto* floater = dynamic_cast<HTMLFloaterElement*>(captured);
            floater && floater->mInteraction == HTMLFloaterElement::FloaterInteraction::Resize)
            return;
    }
    std::uint8_t edges = 0;
    resizeFloaterAt(point, edges);
    mResizeCursor = resizeCursor(static_cast<ResizeEdges>(edges));
}
} // namespace radia::ui
