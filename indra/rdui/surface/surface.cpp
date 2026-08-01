/**
 * @file surface.cpp
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
#include "surface/surface.h"
#include <algorithm>
#include "layout/engine.h"
#include "render/paintcontext.h"
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

Style withOpacity(Style style, float inherited_opacity) {
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
    return style;
}

Style withDirection(Style style, LayoutDirection direction) {
    style.direction = direction;
    if (style.text_align == TextAlign::Start) style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Right : TextAlign::Left;
    else if (style.text_align == TextAlign::End) style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Left : TextAlign::Right;
    return style;
}
} // namespace

Surface::Surface() : mRoot(std::make_unique<SurfaceRoot>()), mTextMetrics(fixedTextMetrics()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const StyleSheet& style_sheet)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&style_sheet), mTextMetrics(fixedTextMetrics()),
      mObservedStyleGeneration(style_sheet.generation()) {
    initializeLayerRoots();
    mRoot->setSurface(this);
}

Surface::Surface(const System& system, const TextMetrics& text_metrics)
    : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&system.styleSheet()), mSystem(&system), mTextMetrics(text_metrics),
      mObservedStyleGeneration(system.generation()) {
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
    mStyleSheet = &style_sheet;
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
    Widget* mounted = widget.get();
    layerRoot(layer).addChild(std::move(widget));
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
        widget.onLocaleChanged(*mSystem);
        for (const auto& child : widget.children()) self(self, *child);
    };
    refresh(refresh, *mRoot);
    for (const auto& root : mLayerRoots) refresh(refresh, *root);
    requestLayout();
}

void Surface::keybindingsChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Widget& widget) -> void {
        widget.onKeybindingsChanged(*mSystem);
        for (const auto& child : widget.children()) self(self, *child);
    };
    refresh(refresh, *mRoot);
    for (const auto& root : mLayerRoots) refresh(refresh, *root);
}

void Surface::requestLayout() {
    mLayoutDirty = true;
    mPaintDirty = true;
}

void Surface::requestPaint() {
    mPaintDirty = true;
}

void Surface::didPaint() {
    mPaintDirty = false;
}

void Surface::updateLayout() {
    const std::uint64_t generation = mSystem ? mSystem->generation() : mStyleSheet->generation();
    if (generation != mObservedStyleGeneration) {
        mObservedStyleGeneration = generation;
        mRoot->invalidateStyleTree();
        for (auto& root : mLayerRoots) root->invalidateStyleTree();
    }
    if (!mLayoutDirty || !mRoot) return;
    mLayoutDirty = false;
    layoutTree(*mRoot, *mStyleSheet, mTextMetrics, layoutDirection());
    for (std::size_t index = 0; index < mLayerRoots.size(); ++index) {
        Widget& root = *mLayerRoots[index];
        const SurfaceLayer layer = static_cast<SurfaceLayer>(index + 1);
        if (layer == SurfaceLayer::Floater || layer == SurfaceLayer::Modal)
            for (const auto& child : root.children()) layoutTree(*child, *mStyleSheet, mTextMetrics, layoutDirection());
        else layoutTree(root, *mStyleSheet, mTextMetrics, layoutDirection());
    }
    refreshHover();
}

void Surface::paint(PaintContext& context, float scale) {
    updateLayout();
    context.beginFrame();
    context.pushClip(mViewport, scale);
    paintWidget(*mRoot, context, scale, 1.f);
    for (const auto& root : mLayerRoots) paintWidget(*root, context, scale, 1.f);
    context.popClip();
    context.endFrame();
    didPaint();
}

void Surface::paintWidget(const Widget& widget, PaintContext& context, float scale, float inherited_opacity) const {
    if (widget.visibility() != Visibility::Visible) return;
    const Style unresolved = resolveWidgetStyle(*mStyleSheet, widget);
    const float child_opacity = inherited_opacity * unresolved.opacity;
    const Style painted = withDirection(withOpacity(unresolved, inherited_opacity), layoutDirection());
    const bool clips_x = unresolved.overflow_x == Overflow::Hidden;
    const bool clips_y = unresolved.overflow_y == Overflow::Hidden;
    const bool clips_children = clips_x || clips_y;
    const ClipAxes clip_axes = (clips_x ? ClipAxes::X : ClipAxes::NoAxes) | (clips_y ? ClipAxes::Y : ClipAxes::NoAxes);
    if (!painted.effects.empty()) context.beginEffects(widget.paintBounds(), painted, scale);
    if (clips_children) context.pushClip(widget.rect(), scale, clip_axes);
    widget.paint(context, painted, scale);
    for (const auto& child : widget.children()) paintWidget(*child, context, scale, child_opacity);
    if (clips_children) context.popClip();
    if (!painted.effects.empty()) context.endEffects();
}
} // namespace rdui
