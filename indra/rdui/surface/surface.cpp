/**
 * @file surface.cpp
 * @brief Implements retained Surface layout, input, and paint lifecycle.
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

namespace rdui {
namespace {
class SurfaceRoot final : public Widget {
public:
    explicit SurfaceRoot(const char* element = "surface-root", bool blocks_pointer = false) : Widget(element), mBlocksPointer(blocks_pointer) {}
    bool defaultPointerEvents() const override { return mBlocksPointer; }
    void paint(PaintContext&, const Style&, float) const override {}

private:
    bool mBlocksPointer = false;
};

void applyOpacity(Style& style, float inherited_opacity) {
    const float opacity = inherited_opacity * style.opacity;
    style.background_color.a *= opacity;
    style.border_color.a *= opacity;
    style.text_color.a *= opacity;
    style.icon_stroke_color.a *= opacity;
    style.outline.color.a *= opacity;
    for (BoxShadow& shadow : style.shadows) shadow.color.a *= opacity;
    if (style.background_gradient)
        for (GradientStop& stop : style.background_gradient->stops) stop.color.a *= opacity;
    if (style.border_gradient)
        for (GradientStop& stop : style.border_gradient->stops) stop.color.a *= opacity;
    style.opacity = 1.f;
}

void applyDirection(Style& style, LayoutDirection direction) {
    style.direction = direction;
    if (style.text_align == TextAlign::Start) style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Right : TextAlign::Left;
    else if (style.text_align == TextAlign::End) style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Left : TextAlign::Right;
}
} // namespace

Surface::WidgetSnapshot Surface::snapshot(Widget& widget) const {
    return WidgetSnapshot(widget);
}

bool Surface::snapshotValid(const WidgetSnapshot& state) const {
    return state.valid();
}

bool Surface::snapshotChildValid(const WidgetSnapshot& state, const Widget& parent) const {
    return state.validChildOf(parent);
}

Surface::Surface()
    : mRoot(std::make_unique<SurfaceRoot>()), mTextMetrics(fixedTextMetrics()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const StyleSheet& style_sheet)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&style_sheet), mTextMetrics(fixedTextMetrics()),
      mObservedStyleGeneration(style_sheet.generation()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const System& system, const TextMetrics& text_metrics)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&system.styleSheet()), mSystem(&system), mTextMetrics(text_metrics),
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

void Surface::generationChanged(const StyleSheet& style_sheet) {
    if (mStylePass && mStylePass->active()) mPendingStyleSheet = &style_sheet;
    else {
        mStyleSheet = &style_sheet;
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
    WidgetRef<Widget> mounted_ref(widget.get());
    Widget& root = layerRoot(layer);
    root.addChild(std::move(widget));
    Widget* mounted = mounted_ref.get();
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
        for (const WidgetRef<Widget>& child_ref : children)
            if (Widget* child = child_ref.get(); child && child->parent() == current) self(self, *child);
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
        for (const WidgetRef<Widget>& child_ref : children)
            if (Widget* child = child_ref.get(); child && child->parent() == current) self(self, *child);
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

void Surface::didPaint(std::uint64_t painted_generation) {
    if (mPaintRequestGeneration != painted_generation || mLayoutDirty) {
        mPaintDirty = true;
        return;
    }
    mPaintDirty = false;
    mRoot->clearPaintInvalidationTree();
    for (auto& root : mLayerRoots) root->clearPaintInvalidationTree();
}

void Surface::updateLayout() {
    const bool layout_changed = updateLayoutIfNeeded();
    if ((layout_changed || mHitTestDirty) && mPointerPositionKnown) refreshHoverState();
}

bool Surface::updateLayoutIfNeeded() {
    const std::uint64_t generation = mSystem ? mSystem->generation() : mStyleSheet->generation();
    if (generation != mObservedStyleGeneration) {
        mObservedStyleGeneration = generation;
        mRoot->invalidateStyleTree();
        for (auto& root : mLayerRoots) root->invalidateStyleTree();
    }
    const std::uint64_t text_metrics_generation = mTextMetrics.generation();
    if (text_metrics_generation != mObservedTextMetricsGeneration) {
        mObservedTextMetricsGeneration = text_metrics_generation;
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
            for (const WidgetRef<Widget>& child_ref : children) {
                if (Widget* child = child_ref.get(); child && child->parent() == &root)
                    layoutTreeUsingStylePass(*child, styles, direction);
            }
        } else {
            layoutTreeUsingStylePass(root, styles, direction);
        }
    }
    return true;
}

void Surface::paint(PaintContext& context, float scale) {
    const bool layout_changed = updateLayoutIfNeeded();
    if ((layout_changed || mHitTestDirty) && mPointerPositionKnown) refreshHoverState();
    const std::uint64_t painted_generation = mPaintRequestGeneration;
    StylePass& styles = stylePass();
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    context.beginFrame();
    context.pushClip(mViewport, scale);
    paintWidget(*mRoot, context, scale, 1.f, styles);
    for (const auto& root : mLayerRoots) paintWidget(*root, context, scale, 1.f, styles);
    context.popClip();
    context.endFrame();
    didPaint(painted_generation);
}

void Surface::paintWidget(const Widget& widget, PaintContext& context, float scale, float inherited_opacity, StylePass& styles) const {
    if (widget.visibility() != Visibility::Visible) return;
    const WidgetRef<const Widget> lifetime(&widget);
    const Surface* original_surface = widget.attachedSurface();
    const Widget* original_parent = widget.parent();
    const std::uint64_t original_style_revision = widget.mStyleRevision;
    const std::uint64_t original_layout_revision = widget.mLayoutInvalidationRevision;
    const Style& unresolved = styles.style(widget);
    const Widget* styled_widget = lifetime.get();
    if (!styled_widget
        || styled_widget->attachedSurface() != original_surface
        || styled_widget->parent() != original_parent
        || styled_widget->mStyleRevision != original_style_revision
        || styled_widget->mLayoutInvalidationRevision != original_layout_revision)
        return;
    const float child_opacity = inherited_opacity * unresolved.opacity;
    const LayoutDirection direction = layoutDirection();
    const bool needs_opacity = inherited_opacity != 1.f || unresolved.opacity != 1.f;
    const bool needs_direction =
        unresolved.direction != direction || unresolved.text_align == TextAlign::Start || unresolved.text_align == TextAlign::End;
    std::optional<Style> painted_storage;
    const Style* painted = &unresolved;
    if (needs_opacity || needs_direction) {
        painted_storage.emplace(unresolved);
        if (needs_opacity) applyOpacity(*painted_storage, inherited_opacity);
        if (needs_direction) applyDirection(*painted_storage, direction);
        painted = &*painted_storage;
    }
    const bool clips_x = unresolved.overflow_x == Overflow::Hidden;
    const bool clips_y = unresolved.overflow_y == Overflow::Hidden;
    const bool clips_children = clips_x || clips_y;
    const ClipAxes clip_axes = (clips_x ? ClipAxes::X : ClipAxes::NoAxes) | (clips_y ? ClipAxes::Y : ClipAxes::NoAxes);
    if (!painted->effects.empty()) context.beginEffects(widget.paintBounds(), *painted, scale);
    if (clips_children) context.pushClip(widget.rect(), scale, clip_axes);
    widget.paint(context, *painted, scale);
    const auto parent_still_valid = [&] {
        const Widget* current_widget = lifetime.get();
        return current_widget
            && current_widget->attachedSurface() == original_surface
            && current_widget->mStyleRevision == original_style_revision
            && current_widget->mLayoutInvalidationRevision == original_layout_revision
            && current_widget->visibility() == Visibility::Visible
            && isRootedInSurface(current_widget)
            && (current_widget->parent() == original_parent || (!original_parent && isSurfaceRoot(current_widget)));
    };
    if (parent_still_valid()) {
        const StylePass::ChildSnapshot children = sourceChildren(widget, styles);
        for (const WidgetRef<Widget>& child_ref : *children) {
            if (!parent_still_valid()) break;
            if (Widget* child = child_ref.get(); child && child->parent() == &widget && isRootedInSurface(child))
                paintWidget(*child, context, scale, child_opacity, styles);
        }
    }
    if (clips_children) context.popClip();
    if (!painted->effects.empty()) context.endEffects();
}
} // namespace rdui
