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
#include "layout/engine.h"
#include "widgets/widget.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
namespace detail {
Widget* findWidgetInScope(Widget& widget, std::string_view id) {
    if (widget.id() == id) return &widget;
    for (const auto& child : widget.children()) {
        if (child->id() == id) return child.get();
        if (child->idScopeRoot()) continue;
        if (Widget* found = findWidgetInScope(*child, id)) return found;
    }
    return nullptr;
}

void indexWidgetsInScope(Widget& widget, std::map<std::string, Widget*>& index) {
    if (!widget.id().empty()) index.emplace(widget.id(), &widget);
    for (const auto& child : widget.children()) {
        if (child->idScopeRoot()) {
            if (!child->id().empty()) index.emplace(child->id(), child.get());
        } else {
            indexWidgetsInScope(*child, index);
        }
    }
}
} // namespace detail

Widget::Widget(const char* elementName) : mElementName(elementName) {}
Widget::~Widget() = default;

std::uint64_t Widget::styleContextRevision() const {
    return mStyleRevision;
}

Widget& Widget::setId(std::string id) {
    mId = std::move(id);
    invalidateStyleTree();
    return *this;
}

Widget& Widget::addClass(std::string className) {
    mClasses.insert(std::move(className));
    invalidateStyleTree();
    return *this;
}

Widget& Widget::setStyleElement(std::string styleElement) {
    mStyleElement = std::move(styleElement);
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

Widget& Widget::setPointerEvents(bool pointerEvents) {
    if (mPointerEvents == pointerEvents) return *this;
    mPointerEvents = pointerEvents;
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
    if (mVisibilityOverride && *mVisibilityOverride == visibility) return *this;
    mVisibilityOverride = visibility;
    if (mSurface) mSurface->requestHitTestRefresh();
    invalidatePaint();
    if (visibility != Visibility::Visible && mSurface) mSurface->widgetBecameUnavailable(*this);
    return *this;
}

Widget& Widget::setHidden(bool hidden) {
    return setVisibility(hidden ? Visibility::Hidden : Visibility::Visible);
}

bool Widget::isDisplayed(const Style& style) const {
    return style.display != DisplayMode::NoneValue && !mDisplayNoneOverride.value_or(false);
}

bool Widget::isVisible(const Style& style) const {
    return isDisplayed(style) && mVisibilityOverride.value_or(style.visibility) == Visibility::Visible;
}

Widget& Widget::setDisplayNone(bool displayNone) {
    if (displayNone) {
        if (mDisplayNoneOverride && *mDisplayNoneOverride) return *this;
        mDisplayNoneOverride = true;
    } else {
        if (!mDisplayNoneOverride) return *this;
        mDisplayNoneOverride.reset();
    }
    ++mChildSnapshotRevision;
    if (mParent) ++mParent->mChildSnapshotRevision;
    if (mSurface) {
        mSurface->invalidateOrderingCache();
        mSurface->requestHitTestRefresh();
    }
    invalidateMeasure();
    if (displayNone && mSurface) mSurface->widgetBecameUnavailable(*this);
    return *this;
}

bool Widget::setTextContent(TextSource) {
    return false;
}

bool Widget::setCheckedValue(bool) {
    return false;
}

std::optional<bool> Widget::checkedValue() const {
    return std::nullopt;
}

Widget& Widget::setOnActivate(std::function<void(Widget&)> callback) {
    mOnActivate = std::move(callback);
    return *this;
}

Widget& Widget::setEventCall(WidgetEventKind kind, EventCall call) {
    mEventSlots[kind].call = std::move(call);
    return *this;
}

Widget& Widget::setLongClickDelay(std::chrono::milliseconds delay) {
    mLongClickDelay = delay;
    return *this;
}

Widget& Widget::setIdScopeRoot(bool scopeRoot) {
    mIdScopeRoot = scopeRoot;
    return *this;
}

const EventCall* Widget::eventCall(WidgetEventKind kind) const {
    const auto found = mEventSlots.find(kind);
    return found == mEventSlots.end() || !found->second.call ? nullptr : &*found->second.call;
}

void Widget::bindEventHandler(WidgetEventKind kind, const std::shared_ptr<detail::EventHandler>& handler) {
    mEventSlots[kind].handler = handler;
}

void Widget::emitEvent(const WidgetEvent& event) {
    const auto found = mEventSlots.find(event.kind);
    if (found == mEventSlots.end()) return;
    if (const auto handler = found->second.handler.lock()) {
        if (found->second.call) handler->invoke(event, *found->second.call);
    }
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
    const Widget* expectedParent = mParent;
    const WidgetRef<Widget> self(this);
    std::vector<WidgetRef<Widget>> children;
    children.reserve(mChildren.size());
    for (const auto& child : mChildren) children.emplace_back(child.get());
    if (const System* system = this->system()) onLocaleChanged(*system);
    Widget* current = self.get();
    if (!current || current->mSurface != surface || current->mParent != expectedParent) return;
    for (const WidgetRef<Widget>& childRef : children)
        if (Widget* child = childRef.get(); child && child->parent() == current) {
            child->setSurface(surface);
            current = self.get();
            if (!current || current->mSurface != surface || current->mParent != expectedParent) return;
        }
    if (current->mSurface) {
        current->mSurface->invalidateStyleCache();
        current->mSurface->requestLayout();
    }
}

const System* Widget::system() const {
    return mSurface ? mSurface->mSystem : nullptr;
}

const TextMetrics& Widget::textMetrics() const {
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

void Widget::invalidateStyleTree(bool layoutAffecting, bool propagateToDescendants) {
    const auto invalidate = [layoutAffecting](auto&& self, Widget& widget, bool propagate) -> void {
        ++widget.mStyleRevision;
        if (layoutAffecting) ++widget.mLayoutInvalidationRevision;
        widget.mInvalidationReasons.add(layoutAffecting ? kLayoutStyleInvalidationReasons : kPaintStyleInvalidationReasons);
        if (propagate)
            for (auto& child : widget.mChildren) self(self, *child, true);
    };
    invalidate(invalidate, *this, propagateToDescendants);
    if (layoutAffecting) {
        ++mChildSnapshotRevision;
        if (mParent) ++mParent->mChildSnapshotRevision;
        if (mSurface) mSurface->invalidateOrderingCache();
    }
    if (!layoutAffecting) {
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

const StyleSheet* Widget::styleSheet() const {
    return mSurface ? &mSurface->styleSheet() : nullptr;
}

void Widget::setState(WidgetState state, bool enabled) {
    if (radia::ui::hasState(mStates, state) == enabled) return;
    radia::ui::setState(mStates, state, enabled);
    const StyleSheet* styleSheet = this->styleSheet();
    const bool layoutAffecting = !styleSheet || styleSheet->stateAffectsLayout(*this, state);
    const bool propagateToDescendants = !styleSheet || styleSheet->stateAffectsDescendants(*this, state);
    invalidateStyleTree(layoutAffecting, propagateToDescendants);
    if (styleSheet && styleSheet->stateAffectsHitTesting(*this, state) && mSurface) mSurface->requestHitTestRefresh();
}

void Widget::activate() {
    if (disabled()) return;
    WidgetRef<Widget> self(this);
    onActivate();
    Widget* current = self.get();
    if (!current) return;
    if (current->mOnActivate) current->mOnActivate(*current);
    current = self.get();
    if (current) current->emitEvent(ClickEvent(*current));
}

void Widget::activateFromLabel() {
    const StyleSheet* styleSheet = this->styleSheet();
    for (const Widget* current = this; current; current = current->parent()) {
        if (current->disabled()) return;
        if (styleSheet) {
            if (!current->isVisible(resolveWidgetStyle(*styleSheet, *current))) return;
        } else if (!current->isVisible(Style{})) {
            return;
        }
    }
    onLabelActivate();
}

void Widget::dispatchMouseEvent(WidgetEventKind kind, const PointerEvent& event) {
    if (disabled()) return;
    emitEvent(MouseWidgetEvent(*this, kind, event));
}

void Widget::dispatchLongClickEvent(std::chrono::milliseconds heldFor) {
    if (disabled()) return;
    emitEvent(LongClickEvent(*this, heldFor));
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
} // namespace radia::ui
