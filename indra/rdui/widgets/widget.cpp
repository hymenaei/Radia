/**
 * @file widget.cpp
 * @brief Defines the retained Widget base type, state, ownership, and invalidation.
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
#include "widgets/widget.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace rdui {
Widget::Widget(const char* element) : mElement(element) {}
Widget::~Widget() = default;

std::uint64_t Widget::styleContextRevision() const {
    return mStyleRevision;
}

Widget& Widget::setId(std::string id) {
    mId = std::move(id);
    invalidateStyleTree();
    return *this;
}

Widget& Widget::addClass(std::string class_name) {
    mClasses.insert(std::move(class_name));
    invalidateStyleTree();
    return *this;
}

Widget& Widget::setStyleElement(std::string style_element) {
    mStyleElement = std::move(style_element);
    invalidateStyleTree();
    return *this;
}

Widget& Widget::setPart(std::string part) {
    mPart = std::move(part);
    invalidateStyleTree();
    return *this;
}

Widget& Widget::setRect(const Rect& rect) {
    const bool changed = !mRectExplicit || mRect.x != rect.x || mRect.y != rect.y || mRect.w != rect.w || mRect.h != rect.h;
    if (!changed) return *this;
    mRect = rect;
    mRectExplicit = true;
    invalidateMeasure();
    return *this;
}

Widget& Widget::setPointerEvents(bool pointer_events) {
    if (mPointerEvents == pointer_events) return *this;
    mPointerEvents = pointer_events;
    if (mSurface) mSurface->requestHitTestRefresh();
    return *this;
}

Widget& Widget::setDisabled(bool disabled) {
    const bool changed = disabled != this->disabled();
    setState(WidgetState::Disabled, disabled);
    if (changed && mSurface) {
        mSurface->requestHitTestRefresh();
        if (disabled) mSurface->widgetBecameUnavailable(*this);
    }
    return *this;
}

Widget& Widget::setVisibility(Visibility visibility) {
    if (visibility == mVisibility) return *this;

    const bool layout_participation_changed = (visibility == Visibility::Collapsed) != (mVisibility == Visibility::Collapsed);
    mVisibility = visibility;
    if (layout_participation_changed) {
        ++mChildSnapshotRevision;
        if (mParent) ++mParent->mChildSnapshotRevision;
    }
    if (layout_participation_changed && mSurface) mSurface->invalidateOrderingCache();
    if (mSurface) mSurface->requestHitTestRefresh();
    if (layout_participation_changed) invalidateMeasure();
    else invalidatePaint();
    if (visibility != Visibility::Visible && mSurface) mSurface->widgetBecameUnavailable(*this);
    return *this;
}

Widget& Widget::setOnActivate(std::function<void(Widget&)> callback) {
    mOnActivate = std::move(callback);
    return *this;
}

Widget& Widget::setAction(ActionEventKind kind, std::string action) {
    mActions[kind].name = std::move(action);
    return *this;
}

Widget& Widget::setLongClickDelay(std::chrono::milliseconds delay) {
    mLongClickDelay = delay;
    return *this;
}

Widget& Widget::setIdScopeRoot(bool scope_root) {
    mIdScopeRoot = scope_root;
    return *this;
}

const std::string& Widget::action(ActionEventKind kind) const {
    static const std::string empty;
    const auto found = mActions.find(kind);
    return found == mActions.end() ? empty : found->second.name;
}

void Widget::bindAction(ActionEventKind kind, const std::shared_ptr<detail::ActionHandler>& handler) {
    mActions[kind].handler = handler;
}

void Widget::emitAction(const ActionEvent& event) {
    const auto found = mActions.find(event.kind);
    if (found == mActions.end()) return;
    if (const auto handler = found->second.handler.lock()) handler->invoke(event);
}

Widget& Widget::addChild(std::unique_ptr<Widget> child) {
    Widget* added = child.get();
    child->mParent = this;
    child->setSurface(mSurface);
    mChildren.push_back(std::move(child));
    ++mChildSnapshotRevision;
    if (mSurface) mSurface->invalidateOrderingCache();
    onChildAdded(*added);
    invalidateMeasure();
    return *this;
}

Widget& Widget::prependChild(std::unique_ptr<Widget> child) {
    Widget* added = child.get();
    child->mParent = this;
    child->setSurface(mSurface);
    mChildren.insert(mChildren.begin(), std::move(child));
    ++mChildSnapshotRevision;
    if (mSurface) mSurface->invalidateOrderingCache();
    onChildAdded(*added);
    invalidateMeasure();
    return *this;
}

void Widget::clearChildren() {
    if (mSurface) mSurface->invalidateOrderingCache();
    Surface* surface = mSurface;
    std::vector<std::unique_ptr<Widget>> children = std::move(mChildren);
    ++mChildSnapshotRevision;
    for (auto& child : children) {
        child->mParent = nullptr;
        child->setSurface(nullptr);
        if (surface) surface->widgetBecameUnavailable(*child);
    }
    onChildrenCleared();
    invalidateMeasure();
}

void Widget::setSurface(Surface* surface) {
    if (mSurface == surface) return;
    if (mSurface) mSurface->invalidateStyleCache();
    mLayoutCache = {};
    mSurface = surface;
    const Widget* expected_parent = mParent;
    const WidgetRef<Widget> self(this);
    std::vector<WidgetRef<Widget>> children;
    children.reserve(mChildren.size());
    for (const auto& child : mChildren) children.emplace_back(child.get());
    if (const System* system = attachedSystem()) onLocaleChanged(*system);
    Widget* current = self.get();
    if (!current || current->mSurface != surface || current->mParent != expected_parent) return;
    for (const WidgetRef<Widget>& child_ref : children)
        if (Widget* child = child_ref.get(); child && child->parent() == current) {
            child->setSurface(surface);
            current = self.get();
            if (!current || current->mSurface != surface || current->mParent != expected_parent) return;
        }
    if (current->mSurface) {
        current->mSurface->invalidateStyleCache();
        current->mSurface->requestLayout();
    }
}

const System* Widget::attachedSystem() const {
    return mSurface ? mSurface->mSystem : nullptr;
}

const TextMetrics& Widget::attachedTextMetrics() const {
    return mSurface ? mSurface->textMetrics() : fixedTextMetrics();
}

void Widget::invalidateMeasure() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kArrangeInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidateText() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kTextInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidateArrange() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    if (mParent) mParent->invalidateArrange();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidateArrangeTree() {
    const auto invalidate = [](auto&& self, Widget& widget) -> void {
        ++widget.mLayoutInvalidationRevision;
        widget.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
        for (auto& child : widget.mChildren) self(self, *child);
    };
    invalidate(invalidate, *this);
    if (mParent) mParent->invalidateArrange();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidateTextTree() {
    const auto invalidate = [](auto&& self, Widget& widget) -> void {
        ++widget.mLayoutInvalidationRevision;
        widget.mInvalidationReasons.add(kTextInvalidationReasons);
        for (auto& child : widget.mChildren) self(self, *child);
    };
    invalidate(invalidate, *this);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidateStyleTree(bool layout_affecting, bool descendants) {
    const auto invalidate = [layout_affecting](auto&& self, Widget& widget, bool propagate) -> void {
        ++widget.mStyleRevision;
        if (layout_affecting) ++widget.mLayoutInvalidationRevision;
        widget.mInvalidationReasons.add(layout_affecting ? kLayoutStyleInvalidationReasons : kPaintStyleInvalidationReasons);
        if (propagate)
            for (auto& child : widget.mChildren) self(self, *child, true);
    };
    invalidate(invalidate, *this, descendants);
    if (layout_affecting) {
        ++mChildSnapshotRevision;
        if (mParent) ++mParent->mChildSnapshotRevision;
        if (mSurface) mSurface->invalidateOrderingCache();
    }
    if (!layout_affecting) {
        if (mSurface) mSurface->requestPaint();
        return;
    }
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Widget::invalidatePaint() {
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
    if (mSurface) mSurface->requestPaint();
}

void Widget::clearPaintInvalidationTree() {
    mInvalidationReasons.remove(LayoutInvalidationReason::Paint);
    for (auto& child : mChildren) child->clearPaintInvalidationTree();
}

const StyleSheet* Widget::attachedStyleSheet() const {
    return mSurface ? &mSurface->styleSheet() : nullptr;
}

void Widget::setState(WidgetState state, bool enabled) {
    if (has_state(mStates, state) == enabled) return;
    set_state(mStates, state, enabled);
    const StyleSheet* style_sheet = attachedStyleSheet();
    const bool layout_affecting = !style_sheet || style_sheet->stateAffectsLayout(*this, state);
    const bool descendants = !style_sheet || style_sheet->stateAffectsDescendants(*this, state);
    invalidateStyleTree(layout_affecting, descendants);
    if (style_sheet && style_sheet->stateAffectsHitTesting(*this, state) && mSurface) mSurface->requestHitTestRefresh();
}

void Widget::activate() {
    if (disabled()) return;
    WidgetRef<Widget> self(this);
    onActivate();
    Widget* current = self.get();
    if (!current) return;
    if (current->mOnActivate) current->mOnActivate(*current);
    current = self.get();
    if (current) current->emitAction(ClickActionEvent(*current));
}

void Widget::activateFromLabel() {
    for (const Widget* current = this; current; current = current->parent())
        if (current->disabled() || current->visibility() != Visibility::Visible) return;
    onLabelActivate();
}

void Widget::dispatchMouseAction(ActionEventKind kind, const PointerEvent& event) {
    if (disabled()) return;
    emitAction(MouseActionEvent(*this, kind, event));
}

void Widget::dispatchLongClickAction(std::chrono::milliseconds held_for) {
    if (disabled()) return;
    emitAction(LongClickActionEvent(*this, held_for));
}

void Widget::translate(const Vec2& delta) {
    translateSubtree(delta);
    if (mSurface) mSurface->requestHitTestRefresh();
}

void Widget::translateSubtree(const Vec2& delta) {
    mRect.x += delta.x;
    mRect.y += delta.y;
    for (auto& child : mChildren) child->translateSubtree(delta);
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
}

void Widget::translateChild(Widget& child, const Vec2& delta) {
    llassert_always(child.parent() == this);
    child.translate(delta);
}

Vec2 Widget::intrinsicSize(const StyleSheet&, const Style&, const TextMetrics&, const IntrinsicSizeConstraints&) const {
    return {};
}

void Widget::paint(PaintContext& context, const Style& style, float) const {
    context.paintBox(rect(), style);
}

bool Widget::defaultKeyDown(const KeyEvent& event) {
    if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
    setState(WidgetState::Active, true);
    return true;
}

bool Widget::defaultKeyUp(const KeyEvent& event) {
    if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
    setState(WidgetState::Active, false);
    activate();
    return true;
}

bool Widget::defaultCharacterInput(unsigned int) {
    return false;
}

bool Widget::defaultScroll(const ScrollEvent&) {
    return false;
}

bool Widget::beginPointerInteraction(const PointerEvent&) {
    return false;
}

bool Widget::updatePointerInteraction(const PointerEvent&) {
    return false;
}

bool Widget::endPointerInteraction(const PointerEvent&) {
    return false;
}
} // namespace rdui
