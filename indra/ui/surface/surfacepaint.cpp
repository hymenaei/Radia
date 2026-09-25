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
const Element* scrollbarClipOwner(const Element& element, const ComputedStyle& style) {
    if (style.borderWidth.any() || hasBorderRadius(style.borderRadius)) return &element;
    for (const Element* ancestor = element.parentElement(); ancestor; ancestor = ancestor->parentElement())
        if (dynamic_cast<const HTMLFloaterElement*>(ancestor)) return ancestor;
    return nullptr;
}

NativeScrollbarAxisGeometry projectScrollbarAxis(const ScrollbarAxisGeometry& geometry) {
    return {geometry.axis,       geometry.bounds,   geometry.track,   geometry.thumb,
            geometry.startArrow, geometry.endArrow, geometry.visible, geometry.reversed};
}

NativeScrollbarPaintGeometry projectScrollbarGeometry(const ScrollGeometry& geometry) {
    return {projectScrollbarAxis(geometry.horizontal), projectScrollbarAxis(geometry.vertical), geometry.corner, geometry.hasCorner};
}
} // namespace

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
            if (mount && mount->root) paintElement(*mount->root, context, scale, 1.f, styles, {});
    context.popClip();
    context.endFrame();
    didPaint(paintedGeneration);
}

void Surface::paintElement(const Element& element, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles,
                           Vec2 paintTranslation) const {
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
        && (painted->backgroundColor.a > 0.f || painted->backgroundGradient.has_value() || !painted->backgroundLayers.empty());
    const bool clipsX = unresolved.overflowX != Overflow::Visible;
    const bool clipsY = unresolved.overflowY != Overflow::Visible;
    const bool clipsChildren = clipsX || clipsY;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    const std::optional<BackgroundPaintContext> previousBackgroundContext = context.backgroundPaintContext();
    BackgroundPaintContext backgroundContext;
    backgroundContext.localScrollTranslation = scrollContentTranslation(layoutDirection(), {current->scrollLeft(), current->scrollTop()});
    backgroundContext.paintTranslation = paintTranslation;
    backgroundContext.viewport = mViewport;
    backgroundContext.scrollport = ElementInternalAccess::scrollport(*current);
    context.setBackgroundPaintContext(backgroundContext);
    if (!painted->effects.empty() || !painted->maskLayers.empty()) context.beginEffects(current->paintBounds(), *painted, scale);
    if (paintsBodyCanvasBackground) {
        ComputedStyle canvasBackground;
        canvasBackground.backgroundColor = painted->backgroundColor;
        canvasBackground.backgroundGradient = painted->backgroundGradient;
        canvasBackground.backgroundLayers = painted->backgroundLayers;
        context.paintBox(mViewport, canvasBackground);

        ComputedStyle bodyStyle = *painted;
        bodyStyle.backgroundColor = Color(0.f, 0.f, 0.f, 0.f);
        bodyStyle.backgroundGradient.reset();
        bodyStyle.backgroundLayers.clear();
        current->paint(context, bodyStyle, scale);
    } else current->paint(context, *painted, scale);
    context.setBackgroundPaintContext(previousBackgroundContext);
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
        const Vec2 contentTranslation =
            clipsChildren ? scrollContentTranslation(layoutDirection(), {current->scrollLeft(), current->scrollTop()}) : Vec2{};
        if (clipsChildren) {
            context.pushClip(ElementInternalAccess::scrollport(*current), scale, clipAxes);
            context.pushTranslation(contentTranslation);
        }
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
                text->paint(context, *painted, parent->styleSheet(), *parent);
                continue;
            }
            const Element* child = childNode->asElement();
            if (child && child->parentElement() == observation.get() && isRootedInSurface(child))
                paintElement(*child, context, scale, childOpacity, styles, paintTranslation + contentTranslation);
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
            request.geometry = projectScrollbarGeometry(geometry);
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
    if (!painted->effects.empty() || !painted->maskLayers.empty()) context.endEffects();
}
} // namespace radia::ui
