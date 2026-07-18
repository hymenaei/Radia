#include "linden_common.h"
#include "rduiwidget.h"
#include "rduipaintcontext.h"
#include "rduisurface.h"
#include "rduitext.h"
#include "rduistyle.h"
#include "rduisystem.h"

namespace rdui
{
    Widget::Widget(const char* element) : mElement(element) {}
    Widget::~Widget() = default;

    Widget& Widget::setId(std::string id)
    {
        mId = std::move(id);
        invalidateStyleTree();
        return *this;
    }

    Widget& Widget::addClass(std::string class_name)
    {
        mClasses.insert(std::move(class_name));
        invalidateStyleTree();
        return *this;
    }

    Widget& Widget::setStyleElement(std::string style_element)
    {
        mStyleElement = std::move(style_element);
        invalidateStyleTree();
        return *this;
    }

    Widget& Widget::setPart(std::string part)
    {
        mPart = std::move(part);
        invalidateStyleTree();
        return *this;
    }

    Widget& Widget::setRect(const Rect& rect)
    {
        mRect = rect;
        mRectExplicit = true;
        invalidateArrange();
        return *this;
    }

    Widget& Widget::setPointerEvents(bool pointer_events)
    {
        mPointerEvents = pointer_events;
        return *this;
    }

    Widget& Widget::setDisabled(bool disabled)
    {
        const bool changed = disabled != this->disabled();
        setState(WidgetState::Disabled, disabled);
        if (changed && disabled && mSurface) mSurface->widgetBecameUnavailable(*this);
        return *this;
    }

    Widget& Widget::setVisibility(Visibility visibility)
    {
        if (visibility == mVisibility) return *this;

        const bool layout_participation_changed = (visibility == Visibility::Collapsed)
                                               != (mVisibility == Visibility::Collapsed);
        mVisibility = visibility;
        if (layout_participation_changed) invalidateMeasure();
        else invalidatePaint();
        if (visibility != Visibility::Visible && mSurface) mSurface->widgetBecameUnavailable(*this);
        return *this;
    }

    Widget& Widget::setOnActivate(std::function<void(Widget&)> callback)
    {
        mOnActivate = std::move(callback);
        return *this;
    }

    Widget& Widget::setAction(ActionEventKind kind, std::string action)
    {
        mActions[kind].name = std::move(action);
        return *this;
    }

    Widget& Widget::setLongClickDelay(std::chrono::milliseconds delay)
    {
        mLongClickDelay = delay;
        return *this;
    }

    Widget& Widget::setIdScopeRoot(bool scope_root)
    {
        mIdScopeRoot = scope_root;
        return *this;
    }

    const std::string& Widget::action(ActionEventKind kind) const
    {
        static const std::string empty;
        const auto found = mActions.find(kind);
        return found == mActions.end() ? empty : found->second.name;
    }

    void Widget::bindAction(ActionEventKind kind, const std::shared_ptr<detail::ActionHandler>& handler)
    {
        mActions[kind].handler = handler;
    }

    void Widget::emitAction(const ActionEvent& event)
    {
        const auto found = mActions.find(event.kind);
        if (found == mActions.end()) return;
        if (const auto handler = found->second.handler.lock()) handler->invoke(event);
    }

    Widget& Widget::addChild(std::unique_ptr<Widget> child)
    {
        Widget* added = child.get();
        child->mParent = this;
        child->setSurface(mSurface);
        mChildren.push_back(std::move(child));
        onChildAdded(*added);
        invalidateMeasure();
        return *this;
    }

    Widget& Widget::prependChild(std::unique_ptr<Widget> child)
    {
        Widget* added = child.get();
        child->mParent = this;
        child->setSurface(mSurface);
        mChildren.insert(mChildren.begin(), std::move(child));
        onChildAdded(*added);
        invalidateMeasure();
        return *this;
    }

    void Widget::clearChildren()
    {
        for (auto& child : mChildren)
        {
            child->mParent = nullptr;
            child->setSurface(nullptr);
        }
        mChildren.clear();
        onChildrenCleared();
        invalidateMeasure();
    }

    void Widget::setSurface(Surface* surface)
    {
        if (mSurface == surface) return;
        mSurface = surface;
        if (const System* system = attachedSystem()) onLocaleChanged(*system);
        for (auto& child : mChildren) child->setSurface(surface);
        if (mSurface)
        {
            mSurface->requestLayout();
        }
    }

    const System* Widget::attachedSystem() const
    {
        return mSurface ? mSurface->mSystem : nullptr;
    }

    const TextMetrics& Widget::attachedTextMetrics() const
    {
        return mSurface ? mSurface->textMetrics() : fixedTextMetrics();
    }

    void Widget::invalidateMeasure()
    {
        mMeasureDirty = true;
        mArrangeDirty = true;
        if (mParent) mParent->invalidateMeasure();
        else if (mSurface) mSurface->requestLayout();
    }

    void Widget::invalidateArrange()
    {
        mArrangeDirty = true;
        if (mSurface) mSurface->requestLayout();
    }

    void Widget::invalidateStyleTree()
    {
        const auto invalidate = [](auto&& self, Widget& widget) -> void
        {
            widget.mMeasureDirty = true;
            widget.mArrangeDirty = true;
            for (auto& child : widget.mChildren) self(self, *child);
        };
        invalidate(invalidate, *this);
        // Descendant selectors can change the whole subtree, while this
        // Widget's new measured size can change every ancestor's layout.
        if (mParent) mParent->invalidateMeasure();
        else if (mSurface) mSurface->requestLayout();
    }

    void Widget::invalidatePaint()
    {
        if (mSurface) mSurface->requestPaint();
    }

    const StyleSheet* Widget::attachedStyleSheet() const
    {
        return mSurface ? &mSurface->styleSheet() : nullptr;
    }

    void Widget::setState(WidgetState state, bool enabled)
    {
        if (has_state(mStates, state) == enabled) return;
        set_state(mStates, state, enabled);
        invalidateStyleTree();
    }

    void Widget::activate()
    {
        if (disabled()) return;
        onActivate();
        if (mOnActivate) mOnActivate(*this);
        emitAction(ClickActionEvent(*this));
    }

    void Widget::dispatchMouseAction(ActionEventKind kind, const PointerEvent& event)
    {
        if (disabled()) return;
        emitAction(MouseActionEvent(*this, kind, event));
    }

    void Widget::dispatchLongClickAction(std::chrono::milliseconds held_for)
    {
        if (disabled()) return;
        emitAction(LongClickActionEvent(*this, held_for));
    }

    void Widget::translate(const Vec2& delta)
    {
        mRect.x += delta.x;
        mRect.y += delta.y;
        for (auto& child : mChildren) child->translate(delta);
        invalidatePaint();
    }

    Vec2 Widget::intrinsicSize(const StyleSheet&, const Style&, const TextMetrics&) const
    {
        return {};
    }

    void Widget::paint(PaintContext& context, const Style& style, float) const
    {
        context.paintBox(rect(), style);
    }

    bool Widget::defaultKeyDown(const KeyEvent& event)
    {
        if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
        setState(WidgetState::Active, true);
        return true;
    }

    bool Widget::defaultKeyUp(const KeyEvent& event)
    {
        if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
        setState(WidgetState::Active, false);
        activate();
        return true;
    }

    bool Widget::defaultCharacterInput(unsigned int)
    {
        return false;
    }

    bool Widget::defaultScroll(const ScrollEvent&)
    {
        return false;
    }

    bool Widget::beginPointerInteraction(const PointerEvent&)
    {
        return false;
    }

    bool Widget::updatePointerInteraction(const PointerEvent&)
    {
        return false;
    }

    bool Widget::endPointerInteraction(const PointerEvent&)
    {
        return false;
    }
}
