/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>
#include "dom/element.h"
#include "event.h"
#include "nativeappearance.h"
#include "style/stylesheet.h"

namespace radia::ui {
class HTMLFloaterElement;
class Document;
class System;
class PaintContext;
class Surface;
class StylePass;
class TextMetrics;

class SurfaceFloaterDelegate {
public:
    virtual ~SurfaceFloaterDelegate() = default;
    virtual void floaterClosed(Surface&, HTMLFloaterElement&) {}
    virtual void floaterMinimizedChanged(Surface&, HTMLFloaterElement&) {}
    virtual void floaterMoveEnded(Surface&, HTMLFloaterElement&) {}
    virtual void floaterResizeEnded(Surface&, HTMLFloaterElement&) {}
};

enum class SurfaceLayer : uint8_t { Base, Floater, Popup, Tooltip, Drag, Modal };

class Surface {
    friend class System;
    friend class Element;
    friend class HTMLFloaterElement;
    friend class detail::NodeMutation;

public:
    Surface();
    explicit Surface(const StyleSheet& styleSheet);
    ~Surface();

    void setViewport(float width, float height);
    void setScrollLayoutOptions(ScrollLayoutOptions options);
    void setFloaterDelegate(SurfaceFloaterDelegate* delegate) { mFloaterDelegate = delegate; }
    Element& mount(std::unique_ptr<Element> element, SurfaceLayer layer = SurfaceLayer::Base);
    Element& mount(Element& element, SurfaceLayer layer = SurfaceLayer::Base);
    Element& mount(Document& document, SurfaceLayer layer = SurfaceLayer::Base);
    HTMLFloaterElement& mountFloater(std::unique_ptr<HTMLFloaterElement> floater, SurfaceLayer layer = SurfaceLayer::Floater);
    HTMLFloaterElement& mountFloater(Document& document, SurfaceLayer layer = SurfaceLayer::Floater);
    std::unique_ptr<HTMLFloaterElement> replaceFloater(HTMLFloaterElement& current, std::unique_ptr<HTMLFloaterElement> replacement);
    bool replaceFloater(HTMLFloaterElement& current, HTMLFloaterElement& replacement);
    std::unique_ptr<Element> unmount(Element& element);
    std::unique_ptr<HTMLFloaterElement> unmountFloater(HTMLFloaterElement& floater);
    bool unmountBorrowed(Element& element);
    bool unmountBorrowedFloater(HTMLFloaterElement& floater);
    bool ownsFloater(const HTMLFloaterElement& floater) const;
    bool hasVisibleFloater() const;
    void clearLayer(SurfaceLayer layer);
    bool raise(Element& element);
    void placeFloater(HTMLFloaterElement& floater, const Rect& rect);
    Vec2 preferredFloaterSize(const HTMLFloaterElement& floater) const;
    Vec2 minimumFloaterSize(const HTMLFloaterElement& floater) const;
    std::optional<Rect> initialFloaterRect(const HTMLFloaterElement& floater) const;
    std::optional<Rect> prepareFloater(HTMLFloaterElement& floater) const;
    void updateLayout();
    void paint(PaintContext& context, float scale = 1.f, Vec2 pixelOrigin = {});
    void clearInteractionState();
    void clearFocus() { setFocused(nullptr, false); }

    bool pointerMove(const PointerEvent& event);
    void pointerLeave();
    bool pointerDown(const PointerEvent& event);
    bool pointerUp(const PointerEvent& event);
    bool scroll(const WheelEvent& event);
    bool keyDown(const KeyEvent& event);
    bool keyUp(const KeyEvent& event);
    bool charInput(unsigned int codepoint);
    void refreshHover();
    void advanceScrollbarInteraction(float deltaSeconds);

    bool hasFocus() const { return mFocused != nullptr; }
    bool hasPointerCapture() const { return mCaptured != nullptr || mScrollbarCapture.has_value(); }
    bool needsPaint() const { return mPaintDirty; }
    const TextMetrics& textMetrics() const { return mTextMetrics; }
    const NativeAppearance& nativeAppearance() const;
    LayoutDirection layoutDirection() const;
    CursorStyle cursor() const;
    float width() const { return mViewport.w; }
    float height() const { return mViewport.h; }

private:
    struct ElementSnapshot;
    struct PendingScrollNotification {
        Element* element = nullptr;
        std::weak_ptr<char> lifetime;
        std::shared_ptr<char> mountLifetime;
    };

    Surface(const System& system, const TextMetrics& textMetrics);
    const StyleSheet& styleSheet() const { return *mStyleSheet; }
    void setHovered(Element* node);
    void requestLayout();
    void requestPaint();
    void requestHitTestRefresh();
    void queueScrollNotification(Element& element);
    void flushScrollNotifications();
    void dispatchScrollNotification(Element& element);
    StylePass& stylePass() const;
    void invalidateStyleCache();
    void invalidateOrderingCache();
    void didPaint(std::uint64_t paintedGeneration);
    void setFocused(Element* node, bool focusVisible);
    void validateFocus();
    bool isRootedInSurface(const Element* node) const;
    bool isEnabledInTree(const Element* node) const;
    bool isFocusableInTree(const Element* node) const;
    ElementSnapshot snapshot(Element& element) const;
    bool snapshotValid(const ElementSnapshot& snapshot) const;
    bool snapshotChildValid(const ElementSnapshot& snapshot, const Element& parent) const;
    void clearKeyboardPress();
    void refreshHoverState();
    void updatePressedState();
    void elementBecameUnavailable(Element& element);
    bool moveFocus(bool backwards);
    bool routeEvent(Event& event);
    void paintElement(const Element& element, PaintContext& context, float scale, float inheritedOpacity, StylePass& styles) const;
    using RootList = std::vector<Element*>;
    static constexpr std::size_t kSurfaceLayerCount = static_cast<std::size_t>(SurfaceLayer::Modal) + 1;

    RootList& roots(SurfaceLayer layer);
    const RootList& roots(SurfaceLayer layer) const;
    std::unique_ptr<Element> takeOwnedRoot(Element& element);
    bool detachRoot(Element& element);
    const Element* mountedRoot(const Element* element) const;
    std::optional<SurfaceLayer> layerOf(const Element* element) const;
    bool isSurfaceRoot(const Element* element) const;
    bool hasActiveModal() const;
    Element* hitTestAt(const Vec2& point);
    Element* hitTestNode(Element& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const;
    HTMLFloaterElement* resizeFloaterAt(const Vec2& point, std::uint8_t& edges) const;
    void updateResizeCursor(const Vec2& point);
    bool raiseWithinLayer(Element& element, SurfaceLayer layer);
    void constrainFloater(HTMLFloaterElement& floater);
    bool updateLayoutIfNeeded();
    bool managesFloater(const HTMLFloaterElement& floater) const;
    void floaterClosed(HTMLFloaterElement& floater);
    void floaterMinimizedChanged(HTMLFloaterElement& floater);
    void floaterMoveEnded(HTMLFloaterElement& floater);
    void floaterResizeEnded(HTMLFloaterElement& floater);
    void generationChanged(const StyleSheet& styleSheet);
    void localeChanged();
    void keybindingsChanged();
    void nativeAppearanceChanged();

    struct ScrollbarTarget {
        Element* element = nullptr;
        ScrollGeometry geometry;
        ScrollbarHit hit;
    };

    struct ScrollbarInteraction : ScrollbarTarget {
        float grabOffset = 0.f;
        float repeatElapsed = 0.f;
        bool repeatStarted = false;
    };

    ScrollGeometry scrollbarGeometry(const Element& element, const Style& style) const;
    std::optional<ScrollbarTarget> hitTestScrollbarAt(const Vec2& point);
    std::optional<ScrollbarTarget> hitTestScrollbarNode(Element& node, const Vec2& point, const Rect& inheritedClip, StylePass& styles) const;
    bool scrollFocusedElement(const KeyEvent& event, Element& focused);
    void setScrollbarHover(std::optional<ScrollbarTarget> target);
    bool scrollbarTargetMatches(const ScrollbarTarget& target, const Element& element, ScrollbarAxis axis, ScrollbarPart part) const;
    const NativeAppearance& effectiveNativeAppearance() const;
    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const;
    void syncNativeAppearance();
    bool updateScrollbarInteraction(const Vec2& point);
    bool beginScrollbarInteraction(const ScrollbarTarget& target, const Vec2& point);

    std::array<RootList, kSurfaceLayerCount> mRoots;
    std::vector<std::unique_ptr<Element>> mOwnedRoots;
    std::vector<HTMLFloaterElement*> mFloaters;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    StyleSheet mDefaultStyleSheet;
    mutable const StyleSheet* mStyleSheet = &mDefaultStyleSheet;
    mutable const StyleSheet* mPendingStyleSheet = nullptr;
    const System* mSystem = nullptr;
    SurfaceFloaterDelegate* mFloaterDelegate = nullptr;
    const TextMetrics& mTextMetrics;
    mutable std::unique_ptr<StylePass> mStylePass;
    ScrollLayoutOptions mScrollLayoutOptions;
    Rect mViewport;
    Element* mHovered = nullptr;
    Element* mPressed = nullptr;
    Element* mFocused = nullptr;
    Element* mCaptured = nullptr;
    Element* mKeyPressed = nullptr;
    Vec2 mPointerPosition;
    int mPressedKey = 0;
    uint8_t mPressedClickCount = 0;
    bool mPointerPositionKnown = false;
    bool mHitTestDirty = false;
    bool mTabKeyHandled = false;
    bool mDispatchingScrollNotifications = false;
    bool mLayoutDirty = true;
    bool mPaintDirty = true;
    std::uint64_t mPaintRequestGeneration = 0;
    CursorStyle mResizeCursor = CursorStyle::Auto;
    std::uint64_t mNativeAppearanceRevision = 1;
    std::optional<ScrollbarTarget> mScrollbarHover;
    std::optional<ScrollbarInteraction> mScrollbarCapture;
    std::uint64_t mObservedStyleGeneration = 0;
    std::uint64_t mObservedTextMetricsGeneration = 0;
    LayoutDirection mObservedLayoutDirection = LayoutDirection::LeftToRight;
    std::vector<PendingScrollNotification> mPendingScrollNotifications;
};
} // namespace radia::ui
