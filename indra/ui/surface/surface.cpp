/**
 * @file surface.cpp
 * @brief Owns the retained Surface layout, input, invalidation, and painting lifecycle.
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
#include "surface/surface.h"
#include <algorithm>
#include <optional>
#include "layout/engine.h"
#include "layout/engineinternal.h"
#include "render/paintcontext.h"
#include "style/stylepass.h"
#include "surface/ordering.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/floater.h"

namespace radia::ui {
namespace {
class SurfaceRoot final : public Widget {
public:
    explicit SurfaceRoot(const char* elementName = "surface-root", bool blocksPointer = false) : Widget(elementName), mBlocksPointer(blocksPointer) {}
    bool defaultPointerEvents() const override { return mBlocksPointer; }
    void paint(PaintContext&, const Style&, float) const override {}

private:
    bool mBlocksPointer = false;
};

void applyOpacity(Style& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity;
    style.backgroundColor.a *= opacity;
    style.borderColor.a *= opacity;
    style.textColor.a *= opacity;
    style.iconStrokeColor.a *= opacity;
    style.outline.color.a *= opacity;
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
} // namespace

Surface::WidgetSnapshot Surface::snapshot(Widget& widget) const {
    return WidgetSnapshot(widget);
}

bool Surface::snapshotValid(const WidgetSnapshot& snapshot) const {
    return snapshot.valid();
}

bool Surface::snapshotChildValid(const WidgetSnapshot& snapshot, const Widget& parent) const {
    return snapshot.validChildOf(parent);
}

Surface::Surface()
    : mRoot(std::make_unique<SurfaceRoot>()), mTextMetrics(fixedTextMetrics()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const StyleSheet& styleSheet)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&styleSheet), mTextMetrics(fixedTextMetrics()),
      mObservedStyleGeneration(styleSheet.generation()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const System& system, const TextMetrics& textMetrics)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&system.styleSheet()), mSystem(&system), mTextMetrics(textMetrics),
      mObservedStyleGeneration(system.generation()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    system.registerSurface(*this);
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::~Surface() {
    if (mRoot) mRoot->setSurface(nullptr);
    for (auto& root : mLayerRoots)
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
    mViewport = Rect(0.f, 0.f, width, height);
    mRoot->setRect(mViewport);
    for (auto& root : mLayerRoots) root->setRect(mViewport);
    for (auto floater = mFloaters.begin(); floater != mFloaters.end();)
        if (Floater* managed = floater->get()) {
            constrainFloater(*managed);
            ++floater;
        } else floater = mFloaters.erase(floater);
}

Widget& Surface::mount(std::unique_ptr<Widget> widget, SurfaceLayer layer) {
    if (layer == SurfaceLayer::Modal) clearInteractionState();
    WidgetRef<Widget> mountedRef(widget.get());
    Widget& root = layerRoot(layer);
    root.addChild(std::move(widget));
    Widget* mounted = mountedRef.get();
    llassert_always(mounted && mounted->parent() == &root);
    return *mounted;
}

std::unique_ptr<Widget> Surface::unmount(Widget& widget) {
    for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index) {
        Widget& root = layerRoot(static_cast<SurfaceLayer>(index));
        auto found = std::find_if(root.mChildren.begin(), root.mChildren.end(), [&widget](const auto& child) { return child.get() == &widget; });
        if (found == root.mChildren.end()) continue;

        clearInteractionState();
        mFloaters.erase(std::remove_if(mFloaters.begin(), mFloaters.end(), [&widget](const auto& floater) { return floater.get() == &widget; }),
                        mFloaters.end());
        std::unique_ptr<Widget> result = std::move(*found);
        root.mChildren.erase(found);
        ++root.mChildSnapshotRevision;
        invalidateOrderingCache();
        result->setSurface(nullptr);
        result->mParent = nullptr;
        requestLayout();
        refreshHover();
        return result;
    }
    return nullptr;
}

void Surface::clearLayer(SurfaceLayer layer) {
    if (layer == SurfaceLayer::Floater || layer == SurfaceLayer::Modal) {
        Widget* root = &layerRoot(layer);
        mFloaters.erase(
            std::remove_if(mFloaters.begin(), mFloaters.end(), [root](const auto& floater) { return !floater || floater->parent() == root; }),
            mFloaters.end());
    }
    layerRoot(layer).clearChildren();
    refreshHover();
}

void Surface::initializeLayerRoots() {
    for (std::size_t index = 0; index < mLayerRoots.size(); ++index) {
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index + 1);
        mLayerRoots[index] = std::make_unique<SurfaceRoot>("surface-layer", layer == SurfaceLayer::Modal);
        mLayerRoots[index]->setRect(mViewport);
        mLayerRoots[index]->setSurface(this);
    }
}

Widget& Surface::layerRoot(SurfaceLayer layer) {
    if (layer == SurfaceLayer::Content) return *mRoot;
    return *mLayerRoots[static_cast<std::size_t>(layer) - 1];
}

const Widget& Surface::layerRoot(SurfaceLayer layer) const {
    if (layer == SurfaceLayer::Content) return *mRoot;
    return *mLayerRoots[static_cast<std::size_t>(layer) - 1];
}

bool Surface::isSurfaceRoot(const Widget* widget) const {
    if (!widget) return false;
    if (widget == mRoot.get()) return true;
    return std::any_of(mLayerRoots.begin(), mLayerRoots.end(), [widget](const auto& root) { return root.get() == widget; });
}

void Surface::localeChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Widget& widget) -> void {
        const WidgetRef<Widget> lifetime(&widget);
        const Surface* surface = widget.mSurface;
        const Widget* parent = widget.mParent;
        std::vector<WidgetRef<Widget>> children;
        children.reserve(widget.children().size());
        for (const auto& child : widget.children()) children.emplace_back(child.get());
        widget.onLocaleChanged(*mSystem);
        Widget* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const WidgetRef<Widget>& childRef : children)
            if (Widget* child = childRef.get(); child && child->parent() == current) self(self, *child);
    };
    refresh(refresh, *mRoot);
    for (const auto& root : mLayerRoots) refresh(refresh, *root);
    requestLayout();
}

void Surface::keybindingsChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Widget& widget) -> void {
        const WidgetRef<Widget> lifetime(&widget);
        const Surface* surface = widget.mSurface;
        const Widget* parent = widget.mParent;
        std::vector<WidgetRef<Widget>> children;
        children.reserve(widget.children().size());
        for (const auto& child : widget.children()) children.emplace_back(child.get());
        widget.onKeybindingsChanged(*mSystem);
        Widget* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const WidgetRef<Widget>& childRef : children)
            if (Widget* child = childRef.get(); child && child->parent() == current) self(self, *child);
    };
    refresh(refresh, *mRoot);
    for (const auto& root : mLayerRoots) refresh(refresh, *root);
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

void Surface::invalidateStyleCache() {
    if (mStylePass) mStylePass->invalidate();
}

void Surface::invalidateOrderingCache() {
    if (mStylePass) mStylePass->invalidateOrdering();
}

bool Surface::hasVisibleFloater() const {
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    return std::any_of(mFloaters.begin(), mFloaters.end(), [this, &styles](const WidgetRef<Floater>& floaterRef) {
        Floater* floater = floaterRef.get();
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
    const bool mismatched = !mStylePass || !mStylePass->matches(*mStyleSheet, mTextMetrics);
    if (mismatched && (!mStylePass || !mStylePass->active())) mStylePass = std::make_unique<StylePass>(*mStyleSheet, mTextMetrics);
    return *mStylePass;
}

void Surface::didPaint(std::uint64_t paintedGeneration) {
    if (mPaintRequestGeneration != paintedGeneration || mLayoutDirty) {
        mPaintDirty = true;
        return;
    }
    mPaintDirty = false;
    mRoot->clearPaintInvalidationTree();
    for (auto& root : mLayerRoots) root->clearPaintInvalidationTree();
}

void Surface::updateLayout() {
    const bool layoutChanged = updateLayoutIfNeeded();
    if ((layoutChanged || mHitTestDirty) && mPointerPositionKnown) refreshHoverState();
}

bool Surface::updateLayoutIfNeeded() {
    const std::uint64_t styleGeneration = mSystem ? mSystem->generation() : mStyleSheet->generation();
    if (styleGeneration != mObservedStyleGeneration) {
        mObservedStyleGeneration = styleGeneration;
        mRoot->invalidateStyleTree();
        for (auto& root : mLayerRoots) root->invalidateStyleTree();
    }
    const std::uint64_t textMetricsGeneration = mTextMetrics.generation();
    if (textMetricsGeneration != mObservedTextMetricsGeneration) {
        mObservedTextMetricsGeneration = textMetricsGeneration;
        mRoot->invalidateTextTree();
        for (auto& root : mLayerRoots) root->invalidateTextTree();
    }
    const LayoutDirection direction = layoutDirection();
    if (direction != mObservedLayoutDirection) {
        mObservedLayoutDirection = direction;
        mRoot->invalidateArrangeTree();
        for (auto& root : mLayerRoots) root->invalidateArrangeTree();
    }
    if (!mLayoutDirty || !mRoot) return false;
    mLayoutDirty = false;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    layoutTreeUsingStylePass(*mRoot, styles, direction);
    for (std::size_t index = 0; index < mLayerRoots.size(); ++index) {
        Widget& root = *mLayerRoots[index];
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index + 1);
        if (layer == SurfaceLayer::Floater || layer == SurfaceLayer::Modal) {
            std::vector<WidgetRef<Widget>> children;
            children.reserve(root.children().size());
            for (const auto& child : root.children()) children.emplace_back(child.get());
            for (const WidgetRef<Widget>& childRef : children)
                if (Widget* child = childRef.get(); child && child->parent() == &root) layoutTreeUsingStylePass(*child, styles, direction);
        } else {
            layoutTreeUsingStylePass(root, styles, direction);
        }
    }
    return true;
}

void Surface::paint(PaintContext& context, float scale) {
    const bool layoutChanged = updateLayoutIfNeeded();
    if ((layoutChanged || mHitTestDirty) && mPointerPositionKnown) refreshHoverState();
    const std::uint64_t paintedGeneration = mPaintRequestGeneration;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    context.beginFrame();
    context.pushClip(mViewport, scale);
    paintWidget(*mRoot, context, scale, 1.f, styles);
    for (const auto& root : mLayerRoots) paintWidget(*root, context, scale, 1.f, styles);
    context.popClip();
    context.endFrame();
    didPaint(paintedGeneration);
}

void Surface::paintWidget(const Widget& widget, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles) const {
    const WidgetRef<const Widget> lifetime(&widget);
    const Surface* originalSurface = widget.attachedSurface();
    const Widget* originalParent = widget.parent();
    const std::uint64_t originalStyleRevision = widget.mStyleRevision;
    const std::uint64_t originalLayoutRevision = widget.mLayoutInvalidationRevision;
    const Style& unresolved = styles.style(widget);
    const Widget* styledWidget = lifetime.get();
    if (!styledWidget
        || styledWidget->attachedSurface() != originalSurface
        || styledWidget->parent() != originalParent
        || styledWidget->mStyleRevision != originalStyleRevision
        || styledWidget->mLayoutInvalidationRevision != originalLayoutRevision)
        return;
    if (!widget.isVisible(unresolved)) return;
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
    const bool clipsX = unresolved.overflowX == Overflow::Hidden;
    const bool clipsY = unresolved.overflowY == Overflow::Hidden;
    const bool clipsChildren = clipsX || clipsY;
    const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
    if (!painted->effects.empty()) context.beginEffects(widget.paintBounds(), *painted, scale);
    if (clipsChildren) context.pushClip(widget.rect(), scale, clipAxes);
    widget.paint(context, *painted, scale);
    const auto isParentStillValid = [&] {
        const Widget* currentWidget = lifetime.get();
        return currentWidget
            && currentWidget->attachedSurface() == originalSurface
            && currentWidget->mStyleRevision == originalStyleRevision
            && currentWidget->mLayoutInvalidationRevision == originalLayoutRevision
            && currentWidget->isVisible(unresolved)
            && isRootedInSurface(currentWidget)
            && (currentWidget->parent() == originalParent || (!originalParent && isSurfaceRoot(currentWidget)));
    };
    if (isParentStillValid()) {
        const StylePass::ChildSnapshot children = sourceChildren(widget, styles);
        for (const WidgetRef<Widget>& childRef : *children) {
            if (!isParentStillValid()) break;
            if (Widget* child = childRef.get(); child && child->parent() == &widget && isRootedInSurface(child))
                paintWidget(*child, context, scale, childOpacity, styles);
        }
    }
    if (clipsChildren) context.popClip();
    if (!painted->effects.empty()) context.endEffects();
}
} // namespace radia::ui
