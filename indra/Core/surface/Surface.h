/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>
#include "ComputedStyle.h"
#include "Element.h"
#include "Event.h"
#include "NativeAppearance.h"
#include "ScrollGeometry.h"
#include "ScrollLayoutOptions.h"
#include "StyleSheet.h"

namespace Core {
class HTMLFloaterElement;
class Document;
class System;
class PaintContext;
class Surface;
class Binding;
class TextMeasurer;
namespace Style {
class Pass;
}
template<typename ElementT> class ElementRef;

class SurfaceFloaterDelegate {
public:
    virtual ~SurfaceFloaterDelegate() = default;
    virtual void floaterClosed(Surface&, HTMLFloaterElement&) {}
    virtual void floaterMinimizedChanged(Surface&, HTMLFloaterElement&) {}
    virtual void floaterMoveEnded(Surface&, HTMLFloaterElement&) {}
    virtual void floaterResizeEnded(Surface&, HTMLFloaterElement&) {}
};

enum class SurfaceLayer : uint8_t {
    Base,
    Floater,
    Popup,
    Tooltip,
    Drag,
    Modal
};

class Surface {
    friend class System;
    friend class Element;
    friend class Layout::Engine;
    friend class HTMLFloaterElement;
    friend class Binding;
    friend class detail::NodeMutation;

public:
    Surface();
    explicit Surface(const CSS::StyleSheet& styleSheet);
    ~Surface();

    void setViewport(float width, float height);
    void setScrollLayoutOptions(Layout::ScrollLayoutOptions options);
    void setColorSchemeContext(Style::ColorSchemeContext context);
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
    void placeFloater(HTMLFloaterElement& floater, const Layout::Rect& rect);
    Layout::Vec2 preferredFloaterSize(HTMLFloaterElement& floater);
    Layout::Vec2 minimumFloaterSize(HTMLFloaterElement& floater);
    std::optional<Layout::Rect> initialFloaterRect(HTMLFloaterElement& floater);
    std::optional<Layout::Rect> prepareFloater(HTMLFloaterElement& floater);
    void updateLayout();
    void paint(PaintContext& context, float scale = 1.f, Layout::Vec2 pixelOrigin = {});
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
    const TextMeasurer& textMetrics() const { return mTextMeasurer; }
    NativeLayoutMetrics nativeLayoutMetrics() const { return mScrollLayoutOptions.nativeMetrics; }
    const NativeAppearance& nativeAppearance() const;
    Layout::Direction layoutDirection() const;
    Style::CursorStyle cursor() const;
    Style::CursorValue cursorValue() const;
    std::optional<Style::CursorStyle> pointerCursor() const;
    std::optional<Style::CursorValue> pointerCursorValue() const;
    float width() const { return mViewport.w; }
    float height() const { return mViewport.h; }

private:
    using ElementObservation = detail::ElementVisit<Element>;
    using ConstElementObservation = detail::ElementVisit<const Element>;
    using MountEpoch = detail::MountEpoch;

    struct PendingScrollNotification {
        Element* element = nullptr;
        std::weak_ptr<char> lifetime;
        MountEpoch mountEpoch;
    };

    Surface(const System& system, const TextMeasurer& textMetrics);
    const CSS::StyleSheet& styleSheet() const { return *mStyleSheet; }
    void setHovered(Element* node);
    void requestLayout();
    void requestPaint();
    void requestHitTestRefresh();
    void queueScrollNotification(Element& element);
    void flushScrollNotifications();
    void dispatchScrollNotification(Element& element);
    Style::Pass& stylePass() const;
    void invalidateStyleCache();
    void invalidateOrderingCache();
    void didPaint(std::uint64_t paintedGeneration);
    void setFocused(Element* node, bool focusVisible);
    void validateFocus();
    bool isRootedInSurface(const Element* node) const;
    bool isEnabledInTree(const Element* node) const;
    bool isFocusableInTree(const Element* node) const;
    ElementObservation observe(Element& element) const;
    ConstElementObservation observe(const Element& element) const;
    void clearKeyboardPress();
    void refreshHoverState();
    void updatePressedState();
    void elementBecameUnavailable(Element& element);
    void elementOwnerDestroyed(Element& element);
    bool moveFocus(bool backwards);
    bool routeEvent(Event& event);
    void collectFocusable(Element& node, std::vector<ElementRef<Element>>& result, Style::Pass& styles) const;
    void collectFixedPositionedElements(Element& node, std::vector<Element*>& result, Style::Pass& styles) const;
    void paintElement(Element& element, PaintContext& context, float scale, float inheritedOpacity, Style::Pass& styles,
        Layout::Vec2 paintTranslation) const;
    static constexpr std::size_t kSurfaceLayerCount = static_cast<std::size_t>(SurfaceLayer::Modal) + 1;

    struct Mount {
        enum class Ownership : uint8_t {
            Owned,
            Borrowed
        };

        Mount(std::unique_ptr<Element> root, SurfaceLayer layer, HTMLFloaterElement* floater);
        Mount(Element& root, SurfaceLayer layer, HTMLFloaterElement* floater);
        ~Mount();

        Element* root = nullptr;
        std::unique_ptr<Element> ownedRoot;
        SurfaceLayer layer;
        Ownership ownership;
        HTMLFloaterElement* floater = nullptr;
        std::shared_ptr<char> lifetime = std::make_shared<char>(0);
        std::vector<Binding*> bindings;
    };

    using MountPtr = std::unique_ptr<Mount>;
    using MountList = std::vector<MountPtr>;

    MountList& mounts(SurfaceLayer layer);
    const MountList& mounts(SurfaceLayer layer) const;
    Mount* findMount(Element* element) noexcept;
    const Mount* findMount(const Element* element) const noexcept;
    Element& installMount(MountPtr mount);
    MountPtr detachMount(Element& element);
    bool attachBinding(Element& root, Binding& binding);
    void detachBinding(Binding& binding) noexcept;
    void replaceBinding(Binding& current, Binding& replacement) noexcept;
    void detachBindings(Mount& mount) noexcept;
    Element* mountedRoot(Element* element);
    const Element* mountedRoot(const Element* element) const;
    const Layout::ScrollLayoutOptions& scrollLayoutOptions() const { return mScrollLayoutOptions; }
    std::optional<SurfaceLayer> layerOf(const Element* element) const;
    bool isSurfaceRoot(const Element* element) const;
    bool hasActiveModal() const;
    Element* hitTestAt(const Layout::Vec2& point);
    static bool acceptsPointerEvents(const Element& element, const Style::ComputedStyle& style);
    Element* hitTestNode(Element& node, const Layout::Vec2& point, const Layout::Rect& inheritedClip, Style::Pass& styles) const;
    HTMLFloaterElement* resizeFloaterAt(const Layout::Vec2& point, std::uint8_t& edges) const;
    void updateResizeCursor(const Layout::Vec2& point);
    bool raiseWithinLayer(Element& element, SurfaceLayer layer);
    void constrainFloater(HTMLFloaterElement& floater);
    bool updateLayoutIfNeeded();
    bool managesFloater(const HTMLFloaterElement& floater) const;
    void floaterClosed(HTMLFloaterElement& floater);
    void floaterMinimizedChanged(HTMLFloaterElement& floater);
    void floaterMoveEnded(HTMLFloaterElement& floater);
    void floaterResizeEnded(HTMLFloaterElement& floater);
    void generationChanged(const CSS::StyleSheet& styleSheet);
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

    ScrollGeometry scrollbarGeometry(const Element& element, const Style::ComputedStyle& style) const;
    std::optional<ScrollbarTarget> hitTestScrollbarAt(const Layout::Vec2& point);
    std::optional<ScrollbarTarget> hitTestScrollbarNode(Element& node, const Layout::Vec2& point, const Layout::Rect& inheritedClip,
        Style::Pass& styles) const;
    bool scrollFocusedElement(const KeyEvent& event, Element& focused);
    void setScrollbarHover(std::optional<ScrollbarTarget> target);
    bool scrollbarTargetMatches(const ScrollbarTarget& target, const Element& element, ScrollbarAxis axis, ScrollbarPart part) const;
    const NativeAppearance& effectiveNativeAppearance() const;
    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const;
    void syncNativeAppearance();
    bool updateScrollbarInteraction(const Layout::Vec2& point);
    bool beginScrollbarInteraction(const ScrollbarTarget& target, const Layout::Vec2& point);

    std::array<MountList, kSurfaceLayerCount> mMounts;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    CSS::StyleSheet mDefaultStyleSheet;
    mutable const CSS::StyleSheet* mStyleSheet = &mDefaultStyleSheet;
    mutable const CSS::StyleSheet* mPendingStyleSheet = nullptr;
    const System* mSystem = nullptr;
    SurfaceFloaterDelegate* mFloaterDelegate = nullptr;
    const TextMeasurer& mTextMeasurer;
    mutable std::unique_ptr<Style::Pass> mStylePass;
    Layout::ScrollLayoutOptions mScrollLayoutOptions;
    Style::ColorSchemeContext mColorSchemeContext {Style::ColorSchemeMode::Dark};
    Layout::Rect mViewport;
    Element* mHovered = nullptr;
    Element* mPressed = nullptr;
    Element* mFocused = nullptr;
    Element* mCaptured = nullptr;
    Element* mKeyPressed = nullptr;
    Layout::Vec2 mPointerPosition;
    int mPressedKey = 0;
    uint8_t mPressedClickCount = 0;
    bool mPointerPositionKnown = false;
    bool mSuppressMouseEventsUntilPointerUp = false;
    bool mHitTestDirty = false;
    bool mTabKeyHandled = false;
    bool mDispatchingScrollNotifications = false;
    bool mLayoutDirty = true;
    bool mPaintDirty = true;
    std::uint64_t mPaintRequestGeneration = 0;
    Style::CursorStyle mResizeCursor = Style::CursorStyle::Auto;
    std::uint64_t mNativeAppearanceRevision = 1;
    std::optional<ScrollbarTarget> mScrollbarHover;
    std::optional<ScrollbarInteraction> mScrollbarCapture;
    std::uint64_t mObservedStyleGeneration = 0;
    std::uint64_t mObservedTextMeasurerGeneration = 0;
    Layout::Direction mObservedLayoutDirection = Layout::Direction::LeftToRight;
    std::vector<PendingScrollNotification> mPendingScrollNotifications;
};
} // namespace Core
