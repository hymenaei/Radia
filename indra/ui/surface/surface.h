/**
 * @file surface.h
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

#ifndef RD_SURFACE_H
#define RD_SURFACE_H

#include <array>
#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>
#include "event.h"
#include "style/stylesheet.h"
#include "widgets/widget.h"

namespace radia::ui {
class Floater;
class System;
class PaintContext;
class Surface;
class StylePass;
class TextMetrics;

class SurfaceFloaterDelegate {
public:
    virtual ~SurfaceFloaterDelegate() = default;
    virtual void floaterClosed(Surface&, Floater&) {}
    virtual void floaterMinimizedChanged(Surface&, Floater&) {}
    virtual void floaterMoveEnded(Surface&, Floater&) {}
    virtual void floaterResizeEnded(Surface&, Floater&) {}
};

enum class SurfaceLayer : uint8_t { Content, Floater, Popup, Tooltip, Drag, Modal };

class Surface {
    friend class System;
    friend class Widget;
    friend class Floater;

public:
    Surface();
    explicit Surface(const StyleSheet& styleSheet);
    ~Surface();

    Widget& root() { return *mRoot; }
    const Widget& root() const { return *mRoot; }

    void setViewport(float width, float height);
    void setFloaterDelegate(SurfaceFloaterDelegate* delegate) { mFloaterDelegate = delegate; }
    Widget& mount(std::unique_ptr<Widget> widget, SurfaceLayer layer = SurfaceLayer::Content);
    Floater& mountFloater(std::unique_ptr<Floater> floater, SurfaceLayer layer = SurfaceLayer::Floater);
    std::unique_ptr<Floater> replaceFloater(Floater& current, std::unique_ptr<Floater> replacement);
    std::unique_ptr<Widget> unmount(Widget& widget);
    std::unique_ptr<Floater> unmountFloater(Floater& floater);
    bool ownsFloater(const Floater& floater) const;
    bool hasVisibleFloater() const;
    void clearLayer(SurfaceLayer layer);
    bool raise(Widget& widget);
    void placeFloater(Floater& floater, const Rect& rect);
    Vec2 preferredFloaterSize(const Floater& floater) const;
    Vec2 minimumFloaterSize(const Floater& floater) const;
    std::optional<Rect> initialFloaterRect(const Floater& floater) const;
    std::optional<Rect> prepareFloater(Floater& floater) const;
    void updateLayout();
    void paint(PaintContext& context, float scale = 1.f);
    void clearInteractionState();
    void clearFocus() { setFocused(nullptr, false); }

    bool pointerMove(const PointerEvent& event);
    void pointerLeave();
    bool pointerDown(const PointerEvent& event);
    bool pointerUp(const PointerEvent& event);
    bool scroll(const ScrollEvent& event);
    bool keyDown(const KeyEvent& event);
    bool keyUp(const KeyEvent& event);
    bool charInput(unsigned int codepoint);
    void refreshHover();
    void update(std::chrono::milliseconds elapsed);

    bool hasFocus() const { return mFocused.get() != nullptr; }
    bool hasPointerCapture() const { return mCaptured.get() != nullptr; }
    bool needsPaint() const { return mPaintDirty; }
    const TextMetrics& textMetrics() const { return mTextMetrics; }
    LayoutDirection layoutDirection() const;
    CursorStyle cursor() const;
    float width() const { return mViewport.w; }
    float height() const { return mViewport.h; }

private:
    using WidgetSnapshot = WidgetVisit;

    Surface(const System& system, const TextMetrics& textMetrics);
    const StyleSheet& styleSheet() const { return *mStyleSheet; }
    void setHovered(Widget* node);
    void requestLayout();
    void requestPaint();
    void requestHitTestRefresh();
    StylePass& stylePass() const;
    void invalidateStyleCache();
    void invalidateOrderingCache();
    void didPaint(std::uint64_t paintedGeneration);
    void setFocused(Widget* node, bool focusVisible);
    void validateFocus();
    bool isRootedInSurface(const Widget* node) const;
    bool isEnabledInTree(const Widget* node) const;
    bool isFocusableInTree(const Widget* node) const;
    WidgetSnapshot snapshot(Widget& widget) const;
    bool snapshotValid(const WidgetSnapshot& snapshot) const;
    bool snapshotChildValid(const WidgetSnapshot& snapshot, const Widget& parent) const;
    void clearKeyboardPress();
    void refreshHoverState();
    void updatePressedState();
    void widgetBecameUnavailable(Widget& widget);
    void resetLongClick();
    std::chrono::milliseconds defaultLongClickDelay() const;
    bool moveFocus(bool backwards);
    bool routeEvent(RoutedEvent& event);
    void paintWidget(const Widget& widget, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles) const;
    void initializeLayerRoots();
    Widget& layerRoot(SurfaceLayer layer);
    const Widget& layerRoot(SurfaceLayer layer) const;
    bool isSurfaceRoot(const Widget* widget) const;
    bool hasActiveModal() const;
    Widget* hitTestAt(const Vec2& point);
    Widget* hitTestNode(Widget& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const;
    Floater* resizeFloaterAt(const Vec2& point, std::uint8_t& edges) const;
    void updateResizeCursor(const Vec2& point);
    bool raiseWithinLayer(Widget& widget, SurfaceLayer layer);
    void constrainFloater(Floater& floater);
    bool updateLayoutIfNeeded();
    bool managesFloater(const Floater& floater) const;
    void floaterClosed(Floater& floater);
    void floaterMinimizedChanged(Floater& floater);
    void floaterMoveEnded(Floater& floater);
    void floaterResizeEnded(Floater& floater);
    void generationChanged(const StyleSheet& styleSheet);
    void localeChanged();
    void keybindingsChanged();

    std::unique_ptr<Widget> mRoot;
    std::array<std::unique_ptr<Widget>, static_cast<std::size_t>(SurfaceLayer::Modal)> mLayerRoots;
    std::vector<WidgetRef<Floater>> mFloaters;
    StyleSheet mDefaultStyleSheet;
    mutable const StyleSheet* mStyleSheet = &mDefaultStyleSheet;
    mutable const StyleSheet* mPendingStyleSheet = nullptr;
    const System* mSystem = nullptr;
    SurfaceFloaterDelegate* mFloaterDelegate = nullptr;
    const TextMetrics& mTextMetrics;
    mutable std::unique_ptr<StylePass> mStylePass;
    Rect mViewport;
    WidgetRef<Widget> mHovered;
    WidgetRef<Widget> mPressed;
    WidgetRef<Widget> mFocused;
    WidgetRef<Widget> mCaptured;
    WidgetRef<Widget> mKeyPressed;
    WidgetRef<Widget> mLongClickTarget;
    std::chrono::milliseconds mLongClickElapsed{0};
    Vec2 mPointerPosition;
    int mPressedKey = 0;
    uint8_t mPressedClickCount = 0;
    bool mPointerPositionKnown = false;
    bool mHitTestDirty = false;
    bool mTabKeyHandled = false;
    bool mLongClickFired = false;
    bool mLayoutDirty = true;
    bool mPaintDirty = true;
    std::uint64_t mPaintRequestGeneration = 0;
    CursorStyle mResizeCursor = CursorStyle::Auto;
    std::uint64_t mObservedStyleGeneration = 0;
    std::uint64_t mObservedTextMetricsGeneration = 0;
    LayoutDirection mObservedLayoutDirection = LayoutDirection::LeftToRight;
};
} // namespace radia::ui
#endif // RD_SURFACE_H
