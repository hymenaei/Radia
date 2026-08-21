/**
 * @file widget.h
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

#ifndef RD_WIDGETS_WIDGET_H
#define RD_WIDGETS_WIDGET_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>
#include "event.h"
#include "eventcall.h"
#include "text/source.h"
#include "types.h"
#include "widgetevent.h"

namespace radia::ui {
enum class LayoutInvalidationReason : uint8_t {
    NoInvalidation = 0,
    Measure = 1 << 0,
    Arrange = 1 << 1,
    Style = 1 << 2,
    Text = 1 << 3,
    Paint = 1 << 4
};

class InvalidationFlags {
public:
    constexpr InvalidationFlags() = default;
    constexpr InvalidationFlags(LayoutInvalidationReason reason) : mBits(static_cast<uint8_t>(reason)) {}
    constexpr explicit InvalidationFlags(uint8_t bits) : mBits(bits) {}

    constexpr bool intersects(InvalidationFlags other) const { return (mBits & other.mBits) != 0; }
    constexpr bool contains(LayoutInvalidationReason reason) const { return (mBits & static_cast<uint8_t>(reason)) != 0; }
    constexpr uint8_t value() const { return mBits; }

    constexpr void add(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits | other.mBits); }
    constexpr void remove(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits & static_cast<uint8_t>(~other.mBits)); }

private:
    uint8_t mBits = 0;
};

struct IntrinsicSizeConstraints {
    std::optional<float> width;
    std::optional<float> height;

    IntrinsicSizeConstraints() = default;
    IntrinsicSizeConstraints(std::optional<float> constrainedWidth, std::optional<float> constrainedHeight)
        : width(constrainedWidth), height(constrainedHeight) {}
};

inline constexpr InvalidationFlags layoutInvalidationMask(LayoutInvalidationReason reason) {
    return InvalidationFlags(reason);
}

inline constexpr InvalidationFlags layoutInvalidationMask(InvalidationFlags flags) {
    return flags;
}

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left) | static_cast<uint8_t>(right));
}

inline constexpr InvalidationFlags operator|(InvalidationFlags left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left.value() | static_cast<uint8_t>(right)));
}

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, InvalidationFlags right) {
    return right | left;
}

inline constexpr InvalidationFlags kMeasureInvalidationReasons =
    layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Style | LayoutInvalidationReason::Text);
inline constexpr InvalidationFlags kArrangeInvalidationReasons = kMeasureInvalidationReasons | LayoutInvalidationReason::Arrange;
inline constexpr InvalidationFlags kTextInvalidationReasons =
    layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Text);
inline constexpr InvalidationFlags kPaintStyleInvalidationReasons = layoutInvalidationMask(LayoutInvalidationReason::Paint);
inline constexpr InvalidationFlags kLayoutStyleInvalidationReasons = kArrangeInvalidationReasons | LayoutInvalidationReason::Paint;

class Widget;
class Field;
class Floater;
namespace detail { template<typename WidgetT> class WidgetVisit; }
using WidgetVisit = detail::WidgetVisit<Widget>;
using ConstWidgetVisit = detail::WidgetVisit<const Widget>;
class TreeTraversalCache;
class LayoutEngine;
class PaintContext;
class System;
class Surface;
struct Style;
class StyleSheet;
class TextMetrics;
enum class VerticalAlign;

struct LayoutContextKey {
    const StyleSheet* styleSheet = nullptr;
    const TextMetrics* textMetrics = nullptr;
    std::uint64_t styleGeneration = 0;
    std::uint64_t textMetricsGeneration = 0;

    constexpr bool operator==(const LayoutContextKey& other) const {
        return styleSheet == other.styleSheet
            && textMetrics == other.textMetrics
            && styleGeneration == other.styleGeneration
            && textMetricsGeneration == other.textMetricsGeneration;
    }
};

namespace detail { class WidgetCompilerAccess; }
namespace layout_detail {
class WidgetLayoutAccess;
struct ChildLayout;
Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment);
void setArrangedRect(Widget& node, const Rect& rect);
}

template<typename WidgetT> class WidgetRef {
public:
    WidgetRef() = default;
    WidgetRef(WidgetT* widget) { set(widget); }

    WidgetT* get() const { return mLifetime.expired() ? nullptr : mWidget; }
    WidgetT* operator->() const { return get(); }
    WidgetT& operator*() const { return *get(); }
    explicit operator bool() const { return get() != nullptr; }

    void set(WidgetT* widget) {
        mWidget = widget;
        mLifetime = widget ? widget->lifetime() : std::weak_ptr<char>();
    }

private:
    WidgetT* mWidget = nullptr;
    std::weak_ptr<char> mLifetime;
};

class Widget {
    template<typename> friend class WidgetRef;
    template<typename> friend class detail::WidgetVisit;
    friend class TreeTraversalCache;
    friend class Binder;
    friend class Field;
    friend class Floater;
    friend class LayoutEngine;
    friend class StylePass;
    friend class Surface;
    friend Rect layout_detail::positionedRect(const layout_detail::ChildLayout&, const Rect&, VerticalAlign);
    friend void layout_detail::setArrangedRect(Widget&, const Rect&);
    friend class layout_detail::WidgetLayoutAccess;
    friend class detail::WidgetCompilerAccess;
    friend Style resolveWidgetStyle(const StyleSheet& styleSheet, const Widget& node);

public:
    virtual ~Widget();

    Widget& setId(std::string id);
    Widget& addClass(std::string className);
    Widget& setRect(const Rect& rect);
    Widget& setPointerEvents(bool pointerEvents);
    Widget& setDisabled(bool disabled);
    Widget& setVisibility(Visibility visibility);
    Widget& setHidden(bool hidden);
    virtual bool setTextContent(TextSource content);
    virtual bool setCheckedValue(bool checked);
    virtual std::optional<bool> checkedValue() const;
    Widget& setOnActivate(std::function<void(Widget&)> callback);
    Widget& setEventCall(WidgetEventKind kind, EventCall call);
    Widget& setLongClickDelay(std::chrono::milliseconds delay);

    virtual Widget& addChild(std::unique_ptr<Widget> child);
    virtual Widget& prependChild(std::unique_ptr<Widget> child);
    virtual void clearChildren();

    const std::string& elementName() const { return mElementName; }
    const std::string& styleElement() const { return mStyleElement.empty() ? mElementName : mStyleElement; }
    const std::string& part() const { return mPart; }
    const std::string& id() const { return mId; }
    const std::set<std::string>& classes() const { return mClasses; }
    const Rect& rect() const { return mRect; }
    const Vec2& desiredSize() const { return mDesiredSize; }
    Widget* parent() { return mParent; }
    const Widget* parent() const { return mParent; }
    const std::vector<std::unique_ptr<Widget>>& children() const { return mChildren; }
    uint8_t states() const { return mStates; }
    std::uint64_t styleContextRevision() const;
    bool pointerEvents() const { return mPointerEvents.value_or(defaultPointerEvents()); }
    Visibility visibility() const { return mVisibilityOverride.value_or(Visibility::Visible); }
    bool isDisplayed(const Style& style) const;
    bool isVisible(const Style& style) const;
    bool disabled() const { return radia::ui::hasState(mStates, WidgetState::Disabled); }
    bool idScopeRoot() const { return mIdScopeRoot; }
    bool flowBreakBefore() const { return mFlowBreakBefore; }
    const EventCall* eventCall(WidgetEventKind kind) const;
    const std::optional<std::chrono::milliseconds>& longClickDelay() const { return mLongClickDelay; }

    bool hasState(WidgetState state) const { return radia::ui::hasState(mStates, state); }
    void activate();
    void activateFromLabel();

    virtual Vec2 intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                               const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const;
    virtual bool defaultPointerEvents() const { return false; }
    virtual bool focusable() const { return false; }
    virtual void paint(PaintContext& context, const Style& style, float scale) const;

protected:
    explicit Widget(const char* elementName);
    virtual void onEvent(RoutedEvent&) {}
    virtual bool defaultKeyDown(const KeyEvent& event);
    virtual bool defaultKeyUp(const KeyEvent& event);
    virtual bool defaultCharacterInput(unsigned int codepoint);
    virtual bool defaultScroll(const ScrollEvent& event);
    virtual bool beginPointerInteraction(const PointerEvent& event);
    virtual bool updatePointerInteraction(const PointerEvent& event);
    virtual bool endPointerInteraction(const PointerEvent& event);
    virtual void constrainResolvedStyle(Style& style) const {}
    void emitEvent(const WidgetEvent& event);
    void translate(const Vec2& delta);
    void invalidateMeasure();
    void invalidateArrange();
    void invalidateText();
    void invalidatePaint();
    const StyleSheet* attachedStyleSheet() const;
    const System* attachedSystem() const;
    Surface* attachedSurface() const { return mSurface; }
    const TextMetrics& attachedTextMetrics() const;
    virtual void onActivate() {}
    virtual void onLabelActivate() { activate(); }
    virtual void onChildAdded(Widget&) {}
    virtual void onChildrenCleared() {}
    virtual void onLocaleChanged(const System&) {}
    virtual bool onKeybindingsChanged(const System&) { return false; }
    virtual void onArranged(const Style&) {}
    virtual Rect paintBounds() const { return mRect; }
    virtual bool hasLayoutGapBetween(const Widget&, const Widget&) const { return true; }
    virtual float layoutOverlapBetween(const Widget&, const Widget&, const Style&) const { return 0.f; }
    void translateChild(Widget& child, const Vec2& delta);
    void setState(WidgetState state, bool enabled);
    Widget& setDisplayNone(bool displayNone);

private:
    struct EventSlot {
        std::optional<EventCall> call;
        std::weak_ptr<detail::EventHandler> handler;
    };

    void bindEventHandler(WidgetEventKind kind, const std::shared_ptr<detail::EventHandler>& handler);
    void dispatchMouseEvent(WidgetEventKind kind, const PointerEvent& event);
    void dispatchLongClickEvent(std::chrono::milliseconds heldFor);
    void translateSubtree(const Vec2& delta);
    void invalidateArrangeTree();
    void invalidateTextTree();
    void invalidateStyleTree(bool layoutAffecting = true, bool propagateToDescendants = true);
    void clearPaintInvalidationTree();
    void setSurface(Surface* surface);
    Widget& setStyleElement(std::string styleElement);
    Widget& setPart(std::string part);
    Widget& setIdScopeRoot(bool scopeRoot);
    std::weak_ptr<char> lifetime() const { return mLifetime; }

    struct LayoutCache {
        Vec2 measuredSize;
        Vec2 intrinsicSize;
        float measuredWidth = 0.f;
        float measuredHeight = 0.f;
        LayoutContextKey layoutContext;
        LayoutDirection direction = LayoutDirection::LeftToRight;
        bool measuredWidthSet = false;
        bool measuredHeightSet = false;
        bool measuredRectExplicit = false;
        bool measuredRectConstraintSet = false;
        float measuredRectWidth = 0.f;
        float measuredRectHeight = 0.f;
        bool measureValid = false;
        bool intrinsicValid = false;
        bool arrangeValid = false;
    };

    std::string mElementName;
    std::string mStyleElement;
    std::string mPart;
    std::string mId;
    std::set<std::string> mClasses;
    Rect mRect;
    Vec2 mDesiredSize;
    std::vector<std::unique_ptr<Widget>> mChildren;
    std::uint64_t mChildSnapshotRevision = 1;
    std::function<void(Widget&)> mOnActivate;
    std::map<WidgetEventKind, EventSlot> mEventSlots;
    std::optional<std::chrono::milliseconds> mLongClickDelay;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    Widget* mParent = nullptr;
    Surface* mSurface = nullptr;
    uint8_t mStates = 0;
    std::optional<bool> mPointerEvents;
    std::optional<Visibility> mVisibilityOverride;
    std::optional<bool> mDisplayNoneOverride;
    bool mIdScopeRoot = false;
    bool mFlowBreakBefore = false;
    bool mRectExplicit = false;
    std::uint64_t mStyleRevision = 1;
    std::uint64_t mLayoutInvalidationRevision = 0;
    InvalidationFlags mInvalidationReasons =
        layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Style);
    LayoutCache mLayoutCache;
};

namespace detail {
Widget* findWidgetInScope(Widget& widget, std::string_view id);
void indexWidgetsInScope(Widget& widget, std::map<std::string, Widget*>& index);

template<typename WidgetT> class WidgetVisit {
public:
    static_assert(std::is_same_v<WidgetT, radia::ui::Widget> || std::is_same_v<WidgetT, const radia::ui::Widget>);

    WidgetVisit() = default;
    explicit WidgetVisit(WidgetT& widget)
        : lifetime(&widget), surface(widget.mSurface), parent(widget.mParent), layoutRevision(widget.mLayoutInvalidationRevision),
          styleRevision(widget.mStyleRevision) {}

    WidgetT* get() const { return lifetime.get(); }
    bool valid() const {
        const WidgetT* widget = lifetime.get();
        return widget && widget->mSurface == surface && widget->mParent == parent && widget->mLayoutInvalidationRevision == layoutRevision;
    }
    bool validChildOf(const Widget& expectedParent) const {
        const WidgetT* widget = lifetime.get();
        return valid() && widget->mParent == &expectedParent;
    }
    bool styleValid() const {
        const WidgetT* widget = lifetime.get();
        return valid() && widget->mStyleRevision == styleRevision;
    }

    WidgetRef<WidgetT> lifetime;
    const Surface* surface = nullptr;
    const Widget* parent = nullptr;
    std::uint64_t layoutRevision = 0;
    std::uint64_t styleRevision = 0;
};
} // namespace detail
} // namespace radia::ui
#endif // RD_WIDGETS_WIDGET_H
