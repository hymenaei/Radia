#include "linden_common.h"
#include "rduisurface.h"
#include "rdfloater.h"
#include "rduilayout.h"
#include "rduifloaterresize.h"
#include "rduipaintcontext.h"
#include "rduisystem.h"
#include "rduitextmetrics.h"
#include <algorithm>
#include <iterator>
#include <vector>

namespace rdui
{
    namespace
    {
        class SurfaceRoot final : public Widget
        {
            public:
                explicit SurfaceRoot(const char* element = "surface-root", bool blocks_pointer = false) : Widget(element), mBlocksPointer(blocks_pointer) {}
                bool defaultPointerEvents() const override { return mBlocksPointer; }
                void paint(PaintContext&, const Style&, float) const override {}

            private:
                bool mBlocksPointer = false;
        };

        Rect intersectRects(const Rect& lhs, const Rect& rhs)
        {
            const float left = std::max(lhs.left(), rhs.left());
            const float right = std::min(lhs.right(), rhs.right());
            const float bottom = std::max(lhs.bottom(), rhs.bottom());
            const float top = std::min(lhs.top(), rhs.top());
            return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
        }

        bool acceptsPointerEvents(const Widget& widget, const StyleSheet& style_sheet)
        {
            const PointerEvents policy = resolveWidgetStyle(style_sheet, widget).pointer_events;
            if (policy == PointerEvents::Auto) return true;
            if (policy == PointerEvents::None) return false;
            return widget.pointerEvents();
        }

        Widget* hitTest(Widget& node, const Vec2& point, const StyleSheet& style_sheet, const Rect& inherited_clip)
        {
            if (node.visibility() != Visibility::Visible || !inherited_clip.contains(point)) return nullptr;
            const Style style = resolveWidgetStyle(style_sheet, node);
            const Rect child_clip = style.overflow == Overflow::Hidden ? intersectRects(inherited_clip, node.rect())
                                  : inherited_clip;
            for (auto child = node.children().rbegin(); child != node.children().rend(); ++child)
            {
                if (Widget* hit = hitTest(**child, point, style_sheet, child_clip)) return hit;
            }
            return node.rect().contains(point) && acceptsPointerEvents(node, style_sheet) ? &node : nullptr;
        }

        void collectFocusable(Widget& node, std::vector<Widget*>& result)
        {
            if (node.visibility() != Visibility::Visible || node.disabled()) return;
            if (node.focusable()) result.push_back(&node);
            for (const auto& child : node.children()) collectFocusable(*child, result);
        }

        Style withOpacity(Style style, float inherited_opacity)
        {
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

        Style withDirection(Style style, LayoutDirection direction)
        {
            if (style.text_align == TextAlign::Start)
                style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Right : TextAlign::Left;
            else if (style.text_align == TextAlign::End)
                style.text_align = direction == LayoutDirection::RightToLeft ? TextAlign::Left : TextAlign::Right;
            return style;
        }

    }

    Surface::Surface() : mRoot(std::make_unique<SurfaceRoot>()), mTextMetrics(fixedTextMetrics())
    {
        initializeLayerRoots();
        mRoot->setSurface(this);
    }

    Surface::Surface(const StyleSheet& style_sheet)
                   : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&style_sheet), mTextMetrics(fixedTextMetrics()),
                     mObservedStyleGeneration(style_sheet.generation())
    {
        initializeLayerRoots();
        mRoot->setSurface(this);
    }

    Surface::Surface(const System& system, const TextMetrics& text_metrics)
                   : mRoot(std::make_unique<SurfaceRoot>()), mStyleSheet(&system.styleSheet()), mSystem(&system),
                     mTextMetrics(text_metrics), mObservedStyleGeneration(system.generation())
    {
        system.registerSurface(*this);
        initializeLayerRoots();
        mRoot->setSurface(this);
    }

    Surface::~Surface()
    {
        if (mRoot) mRoot->setSurface(nullptr);
        for (auto& root : mLayerRoots)
            if (root) root->setSurface(nullptr);
        if (mSystem) mSystem->unregisterSurface(*this);
    }

    LayoutDirection Surface::layoutDirection() const
    {
        return mSystem ? mSystem->layoutDirection() : LayoutDirection::LeftToRight;
    }

    void Surface::generationChanged(const StyleSheet& style_sheet)
    {
        mStyleSheet = &style_sheet;
        mObservedStyleGeneration = 0;
        requestLayout();
        requestPaint();
    }

    bool Surface::routeEvent(RoutedEvent& event)
    {
        std::vector<Widget*> route;
        for (Widget* current = &event.target(); current; current = current->parent())
        {
            route.push_back(current);
            if (isSurfaceRoot(current)) break;
        }
        if (route.empty() || !isSurfaceRoot(route.back())) return false;

        event.mPhase = EventPhase::Capture;
        for (auto current = route.rbegin(); current != route.rend() - 1; ++current)
        {
            event.mCurrentTarget = *current;
            (*current)->onEvent(event);
            if (event.propagationStopped())
            {
                event.mCurrentTarget = nullptr;
                return event.handled() || event.defaultPrevented();
            }
        }

        event.mPhase = EventPhase::Target;
        event.mCurrentTarget = route.front();
        route.front()->onEvent(event);
        if (!event.propagationStopped())
        {
            event.mPhase = EventPhase::Bubble;
            for (auto current = std::next(route.begin()); current != route.end(); ++current)
            {
                event.mCurrentTarget = *current;
                (*current)->onEvent(event);
                if (event.propagationStopped()) break;
            }
        }
        event.mCurrentTarget = nullptr;
        return event.handled() || event.defaultPrevented();
    }

    void Surface::setViewport(float width, float height)
    {
        mViewport = Rect(0.f, 0.f, width, height);
        mRoot->setRect(mViewport);
        for (auto& root : mLayerRoots) root->setRect(mViewport);
        for (auto floater = mFloaters.begin(); floater != mFloaters.end();)
        {
            if (Floater* managed = floater->get())
            {
                constrainFloater(*managed);
                ++floater;
            }
            else floater = mFloaters.erase(floater);
        }
    }

    Widget& Surface::mount(std::unique_ptr<Widget> widget, SurfaceLayer layer)
    {
        if (layer == SurfaceLayer::Modal) clearInteractionState();
        Widget* mounted = widget.get();
        layerRoot(layer).addChild(std::move(widget));
        return *mounted;
    }

    Floater& Surface::mountFloater(std::unique_ptr<Floater> floater, SurfaceLayer layer)
    {
        if (layer != SurfaceLayer::Floater && layer != SurfaceLayer::Modal) layer = SurfaceLayer::Floater;
        Floater* mounted = floater.get();
        mount(std::move(floater), layer);
        mFloaters.emplace_back(mounted);
        constrainFloater(*mounted);
        return *mounted;
    }

    std::unique_ptr<Widget> Surface::unmount(Widget& widget)
    {
        for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index)
        {
            Widget& root = layerRoot(static_cast<SurfaceLayer>(index));
            auto found = std::find_if(root.mChildren.begin(), root.mChildren.end(),
                                      [&widget](const auto& child) { return child.get() == &widget; });
            if (found == root.mChildren.end()) continue;

            clearInteractionState();
            mFloaters.erase(std::remove_if(mFloaters.begin(), mFloaters.end(),
                                           [&widget](const auto& floater) { return floater.get() == &widget; }), mFloaters.end());
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

    std::unique_ptr<Floater> Surface::unmountFloater(Floater& floater)
    {
        std::unique_ptr<Widget> widget = unmount(floater);
        return std::unique_ptr<Floater>(static_cast<Floater*>(widget.release()));
    }

    void Surface::clearLayer(SurfaceLayer layer)
    {
        if (layer == SurfaceLayer::Floater || layer == SurfaceLayer::Modal)
        {
            Widget* root = &layerRoot(layer);
            mFloaters.erase(std::remove_if(mFloaters.begin(), mFloaters.end(),
                [root](const auto& floater)
                {
                    return !floater || floater->parent() == root;
                }), mFloaters.end());
        }
        layerRoot(layer).clearChildren();
        refreshHover();
    }

    bool Surface::raise(Widget& widget)
    {
        for (std::size_t index = 0; index <= static_cast<std::size_t>(SurfaceLayer::Modal); ++index)
            if (raiseWithinLayer(widget, static_cast<SurfaceLayer>(index))) return true;
        return false;
    }

    void Surface::constrainFloater(Floater& floater)
    {
        if (!managesFloater(floater)) return;
        floater.setMovementBounds(mViewport);
        floater.clampToMovementBounds();
    }

    void Surface::placeFloater(Floater& floater, const Rect& rect)
    {
        if (!managesFloater(floater)) return;
        Rect placed = rect;
        if (floater.canResize())
        {
            const Vec2 minimum = minimumFloaterSize(floater);
            placed.w = std::min(mViewport.w, std::max(placed.w, minimum.x));
            placed.h = std::min(mViewport.h, std::max(placed.h, minimum.y));
        }
        floater.setRect(placed);
        constrainFloater(floater);
    }

    Vec2 Surface::preferredFloaterSize(const Floater& floater) const
    {
        const Style style = resolveWidgetStyle(*mStyleSheet, floater);
        const Vec2 measured = measureWidget(floater, *mStyleSheet, mTextMetrics);
        const auto resolve = [](const Dimension& value, const std::optional<Length>& minimum, float fallback)
        {
            const float result = value.resolve(fallback);
            return minimum ? std::max(result, minimum->pixels) : result;
        };
        return {resolve(style.width, style.min_width, measured.x),
                resolve(style.height, style.min_height, measured.y)};
    }

    Rect Surface::initialFloaterRect(const Floater& floater, float margin) const
    {
        const Style style = resolveWidgetStyle(*mStyleSheet, floater);
        const Vec2 size = preferredFloaterSize(floater);
        const float x = style.left ? style.left->pixels
                      : style.right ? mViewport.w - style.right->pixels - size.x
                      : margin;
        const float y = style.top ? mViewport.h - style.top->pixels - size.y
                      : style.bottom ? style.bottom->pixels
                      : std::max(margin, mViewport.h - size.y - margin);
        return {x, y, size.x, size.y};
    }

    Rect Surface::prepareFloater(Floater& floater, float margin) const
    {
        const Rect authored = initialFloaterRect(floater, margin);
        floater.setOriginalSize({authored.w, authored.h});
        return authored;
    }

    void Surface::initializeLayerRoots()
    {
        for (std::size_t index = 0; index < mLayerRoots.size(); ++index)
        {
            const SurfaceLayer layer = static_cast<SurfaceLayer>(index + 1);
            mLayerRoots[index] = std::make_unique<SurfaceRoot>("surface-layer", layer == SurfaceLayer::Modal);
            mLayerRoots[index]->setRect(mViewport);
            mLayerRoots[index]->setSurface(this);
        }
    }

    Widget& Surface::layerRoot(SurfaceLayer layer)
    {
        if (layer == SurfaceLayer::Content) return *mRoot;
        return *mLayerRoots[static_cast<std::size_t>(layer) - 1];
    }

    const Widget& Surface::layerRoot(SurfaceLayer layer) const
    {
        if (layer == SurfaceLayer::Content) return *mRoot;
        return *mLayerRoots[static_cast<std::size_t>(layer) - 1];
    }

    bool Surface::isSurfaceRoot(const Widget* widget) const
    {
        if (!widget) return false;
        if (widget == mRoot.get()) return true;
        return std::any_of(mLayerRoots.begin(), mLayerRoots.end(), [widget](const auto& root) { return root.get() == widget; });
    }

    bool Surface::hasActiveModal() const
    {
        const Widget& modal = layerRoot(SurfaceLayer::Modal);
        return std::any_of(modal.children().begin(), modal.children().end(),
                           [](const auto& child) { return child->visibility() == Visibility::Visible; });
    }

    Widget* Surface::hitTestAt(const Vec2& point)
    {
        if (!mViewport.contains(point)) return nullptr;
        if (hasActiveModal()) return hitTest(layerRoot(SurfaceLayer::Modal), point, *mStyleSheet, mViewport);

        for (std::size_t index = static_cast<std::size_t>(SurfaceLayer::Modal);
             index > static_cast<std::size_t>(SurfaceLayer::Content); --index)
        {
            const SurfaceLayer layer = static_cast<SurfaceLayer>(index);
            if (layer == SurfaceLayer::Tooltip || layer == SurfaceLayer::Drag || layer == SurfaceLayer::Modal) continue;
            if (Widget* hit = hitTest(layerRoot(layer), point, *mStyleSheet, mViewport)) return hit;
        }
        return hitTest(*mRoot, point, *mStyleSheet, mViewport);
    }

    bool Surface::raiseWithinLayer(Widget& widget, SurfaceLayer layer)
    {
        Widget& root = layerRoot(layer);
        Widget* direct_child = &widget;
        while (direct_child->parent() && direct_child->parent() != &root) direct_child = direct_child->parent();
        if (direct_child->parent() != &root) return false;

        auto found = std::find_if(root.mChildren.begin(), root.mChildren.end(), [direct_child](const auto& child) { return child.get() == direct_child; });
        if (found == root.mChildren.end()) return false;
        if (std::next(found) != root.mChildren.end())
        {
            std::rotate(found, std::next(found), root.mChildren.end());
            requestPaint();
            refreshHover();
        }
        return true;
    }

    bool Surface::managesFloater(const Floater& floater) const
    {
        return (floater.parent() == &layerRoot(SurfaceLayer::Floater)
                || floater.parent() == &layerRoot(SurfaceLayer::Modal))
            && std::any_of(mFloaters.begin(), mFloaters.end(), [&floater](const auto& managed) { return managed.get() == &floater; });
    }

    bool Surface::canDetachFloater(const Floater& floater) const
    {
        return managesFloater(floater) && mFloaterDelegate && mFloaterDelegate->canDetachFloater(*this, floater);
    }

    void Surface::floaterClosed(Floater& floater)
    {
        if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterClosed(*this, floater);
    }

    void Surface::floaterMinimizedChanged(Floater& floater, bool minimized)
    {
        if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMinimizedChanged(*this, floater, minimized);
    }

    void Surface::floaterMoved(Floater& floater)
    {
        if (managesFloater(floater) && mFloaterDelegate) mFloaterDelegate->floaterMoved(*this, floater);
    }

    void Surface::floaterDetachRequested(Floater& floater, const Vec2& desired, const Vec2& drag_offset)
    {
        if (canDetachFloater(floater)) mFloaterDelegate->floaterDetachRequested(*this, floater, desired, drag_offset);
    }

    void Surface::floaterResized(Floater& floater, bool complete)
    {
        if (!managesFloater(floater)) return;
        requestLayout();
        if (mFloaterDelegate) mFloaterDelegate->floaterResized(*this, floater, complete);
    }

    void Surface::localeChanged()
    {
        if (!mSystem) return;
        const auto refresh = [this](auto&& self, Widget& widget) -> void
        {
            widget.onLocaleChanged(*mSystem);
            for (const auto& child : widget.children()) self(self, *child);
        };
        refresh(refresh, *mRoot);
        for (const auto& root : mLayerRoots) refresh(refresh, *root);
        requestLayout();
    }

    void Surface::requestLayout()
    {
        mLayoutDirty = true;
        mPaintDirty = true;
    }

    void Surface::requestPaint()
    {
        mPaintDirty = true;
    }

    void Surface::didPaint()
    {
        mPaintDirty = false;
    }

    void Surface::updateLayout()
    {
        const std::uint64_t generation = mSystem ? mSystem->generation() : mStyleSheet->generation();
        if (generation != mObservedStyleGeneration)
        {
            mObservedStyleGeneration = generation;
            mRoot->invalidateStyleTree();
            for (auto& root : mLayerRoots) root->invalidateStyleTree();
        }
        if (!mLayoutDirty || !mRoot) return;
        mLayoutDirty = false;
        layoutTree(*mRoot, *mStyleSheet, mTextMetrics, layoutDirection());
        for (std::size_t index = 0; index < mLayerRoots.size(); ++index)
        {
            Widget& root = *mLayerRoots[index];
            const SurfaceLayer layer = static_cast<SurfaceLayer>(index + 1);
            if (layer == SurfaceLayer::Floater || layer == SurfaceLayer::Modal)
            {
                // Surface placement owns a managed Floater's outer rectangle.
                // Layout only its internals so an early zero-sized viewport
                // cannot manufacture and then persist a new position.
                for (const auto& child : root.children())
                    layoutTree(*child, *mStyleSheet, mTextMetrics, layoutDirection());
            }
            else layoutTree(root, *mStyleSheet, mTextMetrics, layoutDirection());
        }
        refreshHover();
    }

    void Surface::paint(PaintContext& context, float scale)
    {
        updateLayout();
        context.beginFrame();
        context.pushClip(mViewport, scale);
        paintWidget(*mRoot, context, scale, 1.f);
        for (const auto& root : mLayerRoots) paintWidget(*root, context, scale, 1.f);
        context.popClip();
        context.endFrame();
        didPaint();
    }

    void Surface::paintWidget(const Widget& widget, PaintContext& context, float scale, float inherited_opacity) const
    {
        if (widget.visibility() != Visibility::Visible) return;
        const Style unresolved = resolveWidgetStyle(*mStyleSheet, widget);
        const float child_opacity = inherited_opacity * unresolved.opacity;
        const Style painted = withDirection(withOpacity(unresolved, inherited_opacity), layoutDirection());
        const bool clips_children = unresolved.overflow == Overflow::Hidden;
        if (!painted.effects.empty()) context.beginEffects(widget.rect(), painted, scale);
        if (clips_children) context.pushClip(widget.rect(), scale);
        widget.paint(context, painted, scale);
        for (const auto& child : widget.children()) paintWidget(*child, context, scale, child_opacity);
        if (clips_children) context.popClip();
        if (!painted.effects.empty()) context.endEffects();
    }

    void Surface::clearInteractionState()
    {
        if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, false);
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
        clearKeyboardPress();
        if (Widget* focused = mFocused.get())
        {
            focused->setState(WidgetState::Focused, false);
            focused->setState(WidgetState::FocusVisible, false);
        }
        if (Widget* captured = mCaptured.get()) captured->endPointerInteraction({mPointerPosition});
        mHovered.set(nullptr);
        mPressed.set(nullptr);
        mFocused.set(nullptr);
        mCaptured.set(nullptr);
        mResizeCursor = CursorStyle::Auto;
        resetLongClick();
        mPressedClickCount = 0;
        mTabKeyHandled = false;
    }

    CursorStyle Surface::cursor() const
    {
        if (mResizeCursor != CursorStyle::Auto) return mResizeCursor;
        const Widget* widget = mCaptured ? mCaptured.get() : mHovered.get();
        if (!widget) return CursorStyle::Default;
        const CursorStyle cursor = resolveWidgetStyle(*mStyleSheet, *widget).cursor;
        return cursor == CursorStyle::Auto ? CursorStyle::Default : cursor;
    }

    bool Surface::pointerMove(const PointerEvent& event)
    {
        mPointerPosition = event.position;
        mPointerPositionKnown = true;
        if (Widget* captured = mCaptured.get())
        {
            RoutedPointerEvent routed(EventKind::PointerMove, *captured, event);
            const bool routed_handled = routeEvent(routed);
            const bool handled = !routed.defaultPrevented() && captured->updatePointerInteraction(event);
            captured->dispatchMouseAction(ActionEventKind::MouseMove, event);
            return routed_handled || handled;
        }
        updateResizeCursor(event.position);
        refreshHover();
        if (!mLongClickFired && mLongClickTarget && mHovered.get() != mLongClickTarget.get()) resetLongClick();
        if (Widget* hovered = mHovered.get())
        {
            if (isEnabledInTree(hovered))
            {
                RoutedPointerEvent routed(EventKind::PointerMove, *hovered, event);
                routeEvent(routed);
                hovered->dispatchMouseAction(ActionEventKind::MouseMove, event);
            }
        }
        return mHovered.get() != nullptr || mPressed.get() != nullptr;
    }

    void Surface::pointerLeave()
    {
        mPointerPositionKnown = false;
        if (!mCaptured) mResizeCursor = CursorStyle::Auto;
        setHovered(nullptr);
        updatePressedState();
        if (!mLongClickFired) resetLongClick();
    }

    bool Surface::pointerDown(const PointerEvent& event)
    {
        updateLayout();
        mPointerPosition = event.position;
        mPointerPositionKnown = true;
        std::uint8_t resize_edges = 0;
        Floater* resize_floater = event.button == PointerButton::Left
            ? resizeFloaterAt(event.position, resize_edges) : nullptr;
        if (resize_floater)
        {
            const SurfaceLayer layer = resize_floater->parent() == &layerRoot(SurfaceLayer::Modal)
                ? SurfaceLayer::Modal : SurfaceLayer::Floater;
            raiseWithinLayer(*resize_floater, layer);
            resetLongClick();
            mPressedClickCount = 0;
            clearKeyboardPress();
            if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
            mPressed.set(nullptr);
            const bool native = mFloaterDelegate
                && mFloaterDelegate->beginNativeFloaterResize(*this, *resize_floater);
            const std::optional<Rect> bounds = native ? std::nullopt : std::optional<Rect>(mViewport);
            if (resize_floater->beginResizeInteraction(event, resize_edges,
                                                       minimumFloaterSize(*resize_floater), bounds))
            {
                mCaptured.set(resize_floater);
                setHovered(resize_floater);
                setFocused(nullptr, false);
                mResizeCursor = detail::resizeCursor(static_cast<detail::ResizeEdges>(resize_edges));
                return true;
            }
        }
        Widget* hit = hitTestAt(event.position);
        if (hit) raiseWithinLayer(*hit, SurfaceLayer::Floater);
        setHovered(hit);
        bool default_prevented = false;
        if (isEnabledInTree(hit))
        {
            RoutedPointerEvent routed(EventKind::PointerDown, *hit, event);
            routeEvent(routed);
            default_prevented = routed.defaultPrevented();
        }
        if (event.button != PointerButton::Left)
        {
            if (isEnabledInTree(hit)) hit->dispatchMouseAction(ActionEventKind::MouseDown, event);
            return hit != nullptr;
        }
        resetLongClick();
        mPressedClickCount = 0;
        clearKeyboardPress();
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
        mPressed.set(nullptr);
        const bool enabled_control = isEnabledInTree(hit);
        for (Widget* candidate = enabled_control && !default_prevented ? hit : nullptr; candidate; candidate = candidate->parent())
        {
            if (!candidate->beginPointerInteraction(event)) continue;
            mCaptured.set(candidate);
            setFocused(nullptr, false);
            candidate->dispatchMouseAction(ActionEventKind::MouseDown, event);
            return true;
        }
        setFocused(enabled_control && !default_prevented && hit->focusable() ? hit : nullptr, false);
        mPressed.set(enabled_control && !default_prevented ? hit : nullptr);
        mPressedClickCount = mPressed ? event.clickCount : 0;
        updatePressedState();
        if (enabled_control && !default_prevented && !hit->action(ActionEventKind::LongClick).empty()) mLongClickTarget.set(hit);
        if (enabled_control) hit->dispatchMouseAction(ActionEventKind::MouseDown, event);
        return hit != nullptr;
    }

    bool Surface::pointerUp(const PointerEvent& event)
    {
        updateLayout();
        mPointerPosition = event.position;
        mPointerPositionKnown = true;
        if (event.button != PointerButton::Left)
        {
            WidgetRef<Widget> hit(hitTestAt(event.position));
            const bool had_hit = !!hit;
            if (isEnabledInTree(hit.get()))
            {
                RoutedPointerEvent routed(EventKind::PointerUp, *hit, event);
                routeEvent(routed);
                hit->dispatchMouseAction(ActionEventKind::MouseUp, event);
                if (hit && event.button == PointerButton::Right) hit->dispatchMouseAction(ActionEventKind::ContextMenu, event);
            }
            return had_hit;
        }
        if (Widget* captured = mCaptured.get())
        {
            resetLongClick();
            mPressedClickCount = 0;
            mCaptured.set(nullptr);
            RoutedPointerEvent routed(EventKind::PointerUp, *captured, event);
            const bool routed_handled = routeEvent(routed);
            const bool handled = !routed.defaultPrevented() && captured->endPointerInteraction(event);
            captured->dispatchMouseAction(ActionEventKind::MouseUp, event);
            refreshHover();
            return routed_handled || handled;
        }
        WidgetRef<Widget> released(mPressed.get());
        WidgetRef<Widget> hit(hitTestAt(event.position));
        bool default_prevented = false;
        if (Widget* target = released ? released.get() : hit.get())
        {
            RoutedPointerEvent routed(EventKind::PointerUp, *target, event);
            routeEvent(routed);
            default_prevented = routed.defaultPrevented();
        }
        const bool suppress_click = mLongClickFired;
        const uint8_t click_count = mPressedClickCount;
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, false);
        mPressed.set(nullptr);
        setHovered(hit.get());
        if (released && isEnabledInTree(released.get())) released->dispatchMouseAction(ActionEventKind::MouseUp, event);
        resetLongClick();
        mPressedClickCount = 0;
        const bool clicked = !suppress_click && released && released.get() == hit.get() && !default_prevented;
        if (clicked)
        {
            released->activate();
            if (released && click_count >= 2)
            {
                PointerEvent double_click = event;
                double_click.clickCount = click_count;
                released->dispatchMouseAction(ActionEventKind::DoubleClick, double_click);
            }
            refreshHover();
            return true;
        }
        return released || hit;
    }

    bool Surface::scroll(const ScrollEvent& event)
    {
        updateLayout();
        mPointerPosition = event.position;
        mPointerPositionKnown = true;
        Widget* hit = hitTestAt(event.position);
        bool routed_handled = false;
        bool default_prevented = false;
        if (isEnabledInTree(hit))
        {
            RoutedScrollEvent routed(*hit, event);
            routed_handled = routeEvent(routed);
            default_prevented = routed.defaultPrevented();
        }
        for (Widget* candidate = default_prevented ? nullptr : hit; candidate; candidate = candidate->parent())
        {
            if (candidate->defaultScroll(event)) return true;
        }
        return routed_handled || hit != nullptr;
    }

    bool Surface::keyDown(const KeyEvent& event)
    {
        validateFocus();
        if (event.key == KEY_TAB && (event.modifiers & ~MODIFIER_SHIFT) == 0)
        {
            mTabKeyHandled = moveFocus((event.modifiers & MODIFIER_SHIFT) != 0);
            return mTabKeyHandled;
        }
        Widget* focused = mFocused.get();
        if (!focused) return false;
        RoutedKeyEvent routed(EventKind::KeyDown, *focused, event);
        const bool routed_handled = routeEvent(routed);
        if (routed.defaultPrevented()) return routed_handled;
        if (isActivationKey(event.key))
        {
            if (mKeyPressed.get() && (mKeyPressed.get() != focused || mPressedKey != event.key)) clearKeyboardPress();
            if (!focused->defaultKeyDown(event)) return routed_handled;
            mKeyPressed.set(focused);
            mPressedKey = event.key;
            return true;
        }
        return focused->defaultKeyDown(event) || routed_handled;
    }

    bool Surface::keyUp(const KeyEvent& event)
    {
        if (event.key == KEY_TAB)
        {
            const bool handled = mTabKeyHandled;
            mTabKeyHandled = false;
            return handled;
        }
        validateFocus();
        Widget* focused = mFocused.get();
        if (!focused) return false;
        RoutedKeyEvent routed(EventKind::KeyUp, *focused, event);
        const bool routed_handled = routeEvent(routed);
        if (routed.defaultPrevented())
        {
            if (isActivationKey(event.key)) clearKeyboardPress();
            return routed_handled;
        }
        if (isActivationKey(event.key))
        {
            if (mKeyPressed.get() != focused || mPressedKey != event.key) return false;
            mKeyPressed.set(nullptr);
            mPressedKey = 0;
        }
        const bool handled = focused->defaultKeyUp(event);
        if (handled) refreshHover();
        return handled || routed_handled;
    }

    bool Surface::charInput(unsigned int codepoint)
    {
        validateFocus();
        Widget* focused = mFocused.get();
        if (!focused) return false;
        RoutedCharacterEvent routed(*focused, codepoint);
        const bool routed_handled = routeEvent(routed);
        return routed_handled || (!routed.defaultPrevented() && focused->defaultCharacterInput(codepoint));
    }

    void Surface::refreshHover()
    {
        updateLayout();
        if (!mPointerPositionKnown || !mRoot) return;
        updateResizeCursor(mPointerPosition);
        setHovered(hitTestAt(mPointerPosition));
        updatePressedState();
    }

    void Surface::update(std::chrono::milliseconds elapsed)
    {
        if (elapsed.count() <= 0 || mLongClickFired) return;
        WidgetRef<Widget> target(mLongClickTarget.get());
        if (!target || mPressed.get() != target.get() || mHovered.get() != target.get() || !isEnabledInTree(target.get()))
        {
            resetLongClick();
            return;
        }
        mLongClickElapsed += elapsed;
        const std::chrono::milliseconds delay = target->longClickDelay().value_or(defaultLongClickDelay());
        if (mLongClickElapsed < delay) return;
        mLongClickFired = true;
        target->dispatchLongClickAction(mLongClickElapsed);
    }

    void Surface::setHovered(Widget* node)
    {
        if (mHovered.get() == node) return;
        if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, false);
        mHovered.set(node);
        if (Widget* hovered = mHovered.get()) hovered->setState(WidgetState::Hovered, true);
    }

    void Surface::setFocused(Widget* node, bool focus_visible)
    {
        if (mFocused.get() == node)
        {
            if (node) node->setState(WidgetState::FocusVisible, focus_visible);
            return;
        }
        if (Widget* focused = mFocused.get())
        {
            clearKeyboardPress();
            focused->setState(WidgetState::Focused, false);
            focused->setState(WidgetState::FocusVisible, false);
        }
        mFocused.set(node);
        if (Widget* focused = mFocused.get())
        {
            focused->setState(WidgetState::Focused, true);
            focused->setState(WidgetState::FocusVisible, focus_visible);
        }
    }

    bool Surface::isEnabledInTree(const Widget* node) const
    {
        if (!node) return false;
        for (const Widget* current = node; current; current = current->parent())
        {
            if (current->visibility() != Visibility::Visible || current->disabled()) return false;
            if (isSurfaceRoot(current)) return true;
        }
        return false;
    }

    bool Surface::isFocusableInTree(const Widget* node) const
    {
        return node && node->focusable() && isEnabledInTree(node);
    }

    void Surface::validateFocus()
    {
        Widget* focused = mFocused.get();
        if (focused && hasActiveModal())
        {
            const Widget* current = focused;
            const Widget* modal_root = &layerRoot(SurfaceLayer::Modal);
            while (current && current != modal_root) current = current->parent();
            if (current != modal_root)
            {
                setFocused(nullptr, false);
                return;
            }
        }
        if (focused && !isFocusableInTree(focused)) setFocused(nullptr, false);
    }

    void Surface::clearKeyboardPress()
    {
        if (Widget* pressed = mKeyPressed.get()) pressed->setState(WidgetState::Active, false);
        mKeyPressed.set(nullptr);
        mPressedKey = 0;
    }

    void Surface::updatePressedState()
    {
        if (Widget* pressed = mPressed.get()) pressed->setState(WidgetState::Active, mHovered.get() == pressed);
    }

    void Surface::widgetBecameUnavailable(Widget&)
    {
        if (Widget* captured = mCaptured.get(); captured && !isEnabledInTree(captured))
        {
            mCaptured.set(nullptr);
            captured->endPointerInteraction({mPointerPosition});
            mResizeCursor = CursorStyle::Auto;
        }
        if (Widget* pressed = mPressed.get(); pressed && !isEnabledInTree(pressed))
        {
            pressed->setState(WidgetState::Active, false);
            mPressed.set(nullptr);
        }
        if (Widget* hovered = mHovered.get(); hovered && !isEnabledInTree(hovered)) setHovered(nullptr);
        if (Widget* key_pressed = mKeyPressed.get(); key_pressed && !isEnabledInTree(key_pressed)) clearKeyboardPress();
        if (Widget* target = mLongClickTarget.get(); target && !isEnabledInTree(target)) resetLongClick();
        validateFocus();
    }

    void Surface::resetLongClick()
    {
        mLongClickTarget.set(nullptr);
        mLongClickElapsed = std::chrono::milliseconds(0);
        mLongClickFired = false;
    }

    std::chrono::milliseconds Surface::defaultLongClickDelay() const
    {
        return mSystem ? mSystem->longClickDelay() : std::chrono::milliseconds(500);
    }

    bool Surface::moveFocus(bool backwards)
    {
        if (!mRoot) return false;
        std::vector<Widget*> focusable;
        if (hasActiveModal()) collectFocusable(layerRoot(SurfaceLayer::Modal), focusable);
        else
        {
            collectFocusable(*mRoot, focusable);
            collectFocusable(layerRoot(SurfaceLayer::Floater), focusable);
            collectFocusable(layerRoot(SurfaceLayer::Popup), focusable);
        }
        if (focusable.empty()) return false;

        const auto current = std::find(focusable.begin(), focusable.end(), mFocused.get());
        Widget* next = nullptr;
        if (current == focusable.end()) next = backwards ? focusable.back() : focusable.front();
        else if (backwards) next = current == focusable.begin() ? focusable.back() : *(current - 1);
        else next = std::next(current) == focusable.end() ? focusable.front() : *std::next(current);
        setFocused(next, true);
        return true;
    }

}
